//! Installable plugin packages. Payload generations are immutable; one small atomic record selects
//! the active generation. Archive paths never become ambient filesystem capabilities.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use photocraft_format::zip::ZipReader;
use serde::{Deserialize, Serialize};

use crate::{EngineError, Result};

mod store;
pub(crate) use store::StoreGuard;

pub const MAX_PACKAGE_BYTES: usize = 512 << 20;
const MAX_EXPANDED: usize = 1024 << 20;
const MAX_FILE: usize = 256 << 20;
const MAX_FILES: usize = 512;
const MAX_RECORD: usize = 512 << 10;
const RECORD_SUFFIX: &str = ".installation.json";
const OWNERSHIP: &str = ".photocraft-install.json";
pub use photocraft_plugin_api::{FilterPayload as FilterInfo, NativePayload as NativeInfo, PackageV2 as PackageInfo};

/// Held by the installation and every job using it. Updates/uninstall require the last lease.
#[derive(Clone)]
pub struct InstalledNative {
    pub worker: PathBuf,
    pub package: PackageInfo,
    pub(crate) generation: Option<ManagedGeneration>,
}

#[derive(Clone)]
pub(crate) struct ManagedGeneration {
    root: PathBuf,
    id: String,
    hash: String,
    activation: u64,
}

impl InstalledNative {
    /// Keep the selected payload alive through execution or a result's commit. Memory-only
    /// fixtures have no managed store; installed native workers always carry this identity.
    pub(crate) fn lease(&self) -> Result<Option<StoreGuard>> {
        let Some(generation) = &self.generation else { return Ok(None) };
        let guard = StoreGuard::read(&generation.root)?;
        let record = read_record(&generation.root, &generation.id)?;
        if !record.is_some_and(|record| record.hash == generation.hash && record.package.as_ref() == Some(&self.package))
            || activation(&generation.root, &generation.id)? != generation.activation
            || checked_path(&generation.root, &format!("{}.disabled", generation.id))?.exists()
        {
            return Err(error(
                "This native plugin was disabled, removed or updated in another PhotoCraft instance. Restart this PhotoCraft instance to reload plugins, then run the action again.",
            ));
        }
        Ok(Some(guard))
    }

    pub(crate) fn reactivated(&self, activation: u64) -> Self {
        let mut native = self.clone();
        if let Some(generation) = &mut native.generation {
            generation.activation = activation;
        }
        native
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRecord {
    path: String,
    size: u64,
    hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    format_version: u32,
    id: String,
    hash: String,
    package: Option<PackageInfo>,
    files: Vec<FileRecord>,
}

pub(crate) struct Package {
    pub info: PackageInfo,
    pub module: Vec<u8>,
    pub storage: crate::extension_cmds::StorageSize,
    pub filters: BTreeMap<String, Arc<photocraft_plugins::Plugin>>,
    files: Vec<FileRecord>,
}

pub(crate) struct Loaded {
    pub module: Vec<u8>,
    pub native: Option<Arc<InstalledNative>>,
    pub package: Option<PackageInfo>,
    pub filters: BTreeMap<String, Arc<photocraft_plugins::Plugin>>,
    pub storage: crate::extension_cmds::StorageSize,
}

pub(crate) struct InstalledPayload {
    pub native: Option<Arc<InstalledNative>>,
    pub enabled: bool,
    pub warning: Option<String>,
}

fn error(message: impl std::fmt::Display) -> EngineError {
    EngineError::Other(format!("Plugin package: {message}"))
}

/// Only validated archive members count. Installation records and allocation overhead are
/// deliberately excluded; model metadata and weights under runtime/models/ are a subset.
fn storage_size(files: &[FileRecord]) -> Result<crate::extension_cmds::StorageSize> {
    let mut storage = crate::extension_cmds::StorageSize::default();
    for file in files {
        storage.installed_bytes = storage.installed_bytes.checked_add(file.size).ok_or_else(|| error("installation size overflow"))?;
        if file.path.to_ascii_lowercase().starts_with("runtime/models/") {
            storage.model_bytes = storage.model_bytes.checked_add(file.size).ok_or_else(|| error("model size overflow"))?;
        }
    }
    Ok(storage)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

fn hash_valid(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn safe_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 240 || name.contains(['\\', ':']) || name.starts_with('/') {
        return Err(error("invalid package path"));
    }
    for part in name.split('/') {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$")
            || stem.strip_prefix("COM").is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
            || stem.strip_prefix("LPT").is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"));
        if part.is_empty()
            || matches!(part, "." | "..")
            || part.ends_with(['.', ' '])
            || part.len() > 128
            || reserved
            || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(error(format!("unsafe package path {name:?}")));
        }
    }
    if name == OWNERSHIP {
        return Err(error("reserved installation metadata path"));
    }
    Ok(())
}

pub(crate) fn host_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "windows-x86_64",
        ("windows", "aarch64") => "windows-aarch64",
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        ("macos", "x86_64") => "macos-x86_64",
        ("macos", "aarch64") => "macos-aarch64",
        _ => "unsupported",
    }
}

fn check_info(info: &PackageInfo) -> Result<()> {
    if info.module != "plugin.pcext" {
        return Err(error("unsupported package module (expected plugin.pcext)"));
    }
    match info.package_version {
        1 => return Err(error("Version 1 native plugin packages are no longer supported. Install an updated version 2 package from the plugin publisher.")),
        2 => {
            if info.filters.len() > 16 {
                return Err(error("a package may declare at most 16 filters"));
            }
            let mut names = BTreeSet::from(["package.json".to_owned(), "plugin.pcext".into(), "license".into(), "third-party-notices.md".into()]);
            if let Some(native) = &info.native {
                if native.service != "plugin.processImage" {
                    return Err(error("version 2 native service must be plugin.processImage"));
                }
                if !matches!(
                    native.platform.as_str(),
                    "windows-x86_64" | "windows-aarch64" | "linux-x86_64" | "linux-aarch64" | "macos-x86_64" | "macos-aarch64"
                ) || native.platform != host_platform()
                {
                    return Err(error(format!("native plugin platform {} is incompatible with {}", native.platform, host_platform())));
                }
                safe_name(&native.entry)?;
                if native.platform.starts_with("windows-") && !native.entry.to_ascii_lowercase().ends_with(".exe") {
                    return Err(error("Windows native entries must end in .exe"));
                }
                if !names.insert(native.entry.to_ascii_lowercase()) {
                    return Err(error("native entry collides with another package module"));
                }
            }
            for filter in &info.filters {
                safe_name(&filter.module)?;
                if !filter.module.ends_with(".wasm") || !names.insert(filter.module.to_ascii_lowercase()) {
                    return Err(error("filter modules must be unique .wasm paths without package entry collisions"));
                }
            }
        }
        _ => return Err(error("unsupported package version")),
    }
    Ok(())
}

fn required_files(info: &PackageInfo) -> Result<Vec<&str>> {
    let mut files = vec!["package.json", "plugin.pcext", "LICENSE", "THIRD-PARTY-NOTICES.md"];
    if let Some(native) = &info.native {
        files.push(&native.entry);
    }
    files.extend(info.filters.iter().map(|filter| filter.module.as_str()));
    Ok(files)
}

fn module_limit(info: Option<&PackageInfo>, path: &str) -> usize {
    if path == "plugin.pcext" {
        photocraft_plugins::extension::MAX_MODULE_BYTES
    } else if info.is_some_and(|info| info.filters.iter().any(|filter| filter.module == path)) {
        photocraft_plugins::Limits::default().max_module_bytes
    } else {
        MAX_FILE
    }
}

fn load_filter(info: Option<&PackageInfo>, path: &str, data: &[u8], filters: &mut BTreeMap<String, Arc<photocraft_plugins::Plugin>>) -> Result<()> {
    if info.is_none_or(|info| !info.filters.iter().any(|filter| filter.module == path)) {
        return Ok(());
    }
    let plugin = photocraft_plugins::Plugin::load(data, photocraft_plugins::Limits::default()).map_err(|e| error(format!("filter {path}: {e}")))?;
    let id = plugin.manifest().id.clone();
    if id == "native" || filters.contains_key(&id) {
        return Err(error(format!("duplicate or reserved filter id {id:?}")));
    }
    filters.insert(id, Arc::new(plugin));
    Ok(())
}

/// Fully validate and CRC-check every member before installation creates any files.
pub(crate) fn inspect(bytes: &[u8]) -> Result<Package> {
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err(error("archive exceeds 512 MiB"));
    }
    let zip = ZipReader::new(bytes).map_err(error)?;
    if zip.entries.is_empty() || zip.entries.len() > MAX_FILES {
        return Err(error("a package must contain 1–512 files"));
    }
    let mut names = BTreeSet::new();
    for entry in &zip.entries {
        safe_name(&entry.name)?;
        if !names.insert(entry.name.to_ascii_lowercase()) {
            return Err(error("duplicate or case-aliased package path"));
        }
    }
    // A file cannot also be the parent directory of another member.
    for name in &names {
        let mut prefix = String::new();
        for part in name.split('/').take(name.split('/').count().saturating_sub(1)) {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if names.contains(&prefix) {
                return Err(error("package file/directory collision"));
            }
        }
    }
    let info: PackageInfo = serde_json::from_slice(&zip.read_by_name("package.json", 64 << 10).map_err(error)?).map_err(error)?;
    check_info(&info)?;
    for required in required_files(&info)? {
        if zip.find(required).is_none() {
            return Err(error(format!("missing required bundle file {required}")));
        }
    }
    let mut remaining = MAX_EXPANDED;
    let mut files = Vec::new();
    let mut module = Vec::new();
    let mut filters = BTreeMap::new();
    for entry in &zip.entries {
        let limit = module_limit(Some(&info), &entry.name);
        let data = zip.read(entry, remaining.min(limit)).map_err(error)?;
        if data.is_empty() {
            return Err(error(format!("empty bundle file {}", entry.name)));
        }
        remaining = remaining.checked_sub(data.len()).ok_or_else(|| error("expanded package exceeds 1 GiB"))?;
        load_filter(Some(&info), &entry.name, &data, &mut filters)?;
        files.push(FileRecord { path: entry.name.clone(), size: data.len() as u64, hash: blake3::hash(&data).to_hex().to_string() });
        if entry.name == "plugin.pcext" {
            module = data;
        }
    }
    Ok(Package { info, module, storage: storage_size(&files)?, filters, files })
}

fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_type().is_symlink() || meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    meta.file_type().is_symlink()
}

/// The user-selected store root may resolve through a link; every managed descendant may not.
pub(crate) fn root(folder: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(folder).map_err(error)?;
    std::fs::canonicalize(folder).map_err(error)
}

pub(crate) fn checked_path(root: &Path, relative: &str) -> Result<PathBuf> {
    safe_name(relative)?;
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if is_link(&meta) => return Err(error("managed installation paths must not contain links or reparse points")),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(error(e)),
        }
    }
    Ok(path)
}

fn read_bounded(path: &Path, max: usize) -> Result<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path).map_err(error)?;
    if !meta.is_file() || is_link(&meta) || meta.len() > max as u64 {
        return Err(error("invalid or oversized installation file"));
    }
    let mut data = Vec::new();
    std::fs::File::open(path).map_err(error)?.take(max as u64 + 1).read_to_end(&mut data).map_err(error)?;
    if data.len() > max {
        return Err(error("installation file grew beyond its limit"));
    }
    Ok(data)
}

fn record_path(root: &Path, id: &str) -> Result<PathBuf> {
    if !valid_id(id) {
        return Err(error("invalid installed plugin id"));
    }
    checked_path(root, &format!("{id}{RECORD_SUFFIX}"))
}

fn validate_record(record: &Record, id: &str) -> Result<()> {
    if record.format_version != 1 || record.id != id || !valid_id(id) || !hash_valid(&record.hash) || record.files.is_empty() || record.files.len() > MAX_FILES
    {
        return Err(error("invalid installation record"));
    }
    if let Some(package) = &record.package {
        check_info(package)?;
    }
    let mut seen = BTreeSet::new();
    let mut size = 0u64;
    for file in &record.files {
        safe_name(&file.path)?;
        size = size.checked_add(file.size).ok_or_else(|| error("installation size overflow"))?;
        if file.size == 0
            || file.size > MAX_FILE as u64
            || size > MAX_EXPANDED as u64
            || !hash_valid(&file.hash)
            || !seen.insert(file.path.to_ascii_lowercase())
        {
            return Err(error("invalid installation file record"));
        }
    }
    if !seen.contains("plugin.pcext") {
        return Err(error("installation has no plugin module"));
    }
    if let Some(package) = &record.package
        && required_files(package)?.iter().any(|name| !record.files.iter().any(|file| file.path == *name))
    {
        return Err(error("native installation is missing required files"));
    }
    if record.package.is_none() && (record.files.len() != 1 || record.files.first().is_none_or(|f| f.path != "plugin.pcext")) {
        return Err(error("raw plugin installation contains unexpected files"));
    }
    if record.files.iter().any(|file| file.size > module_limit(record.package.as_ref(), &file.path) as u64) {
        return Err(error("installed module exceeds its size limit"));
    }
    Ok(())
}

fn read_record(root: &Path, id: &str) -> Result<Option<Record>> {
    let path = record_path(root, id)?;
    if !path.exists() {
        return Ok(None);
    }
    let record: Record = serde_json::from_slice(&read_bounded(&path, MAX_RECORD)?).map_err(error)?;
    validate_record(&record, id)?;
    Ok(Some(record))
}

fn activation(root: &Path, id: &str) -> Result<u64> {
    if !valid_id(id) {
        return Err(error("invalid activation id"));
    }
    let path = checked_path(root, &format!("{id}.activation.json"))?;
    if !path.exists() {
        return Ok(0); // Existing stores acquire an epoch on their first management operation.
    }
    serde_json::from_slice(&read_bounded(&path, 32)?).map_err(error)
}

/// Caller holds the exclusive store guard. Retain the small tombstone after removal so
/// reinstalling identical bytes cannot resurrect another process's obsolete result.
pub(crate) fn revoke_activation(root: &Path, id: &str) -> Result<u64> {
    let next = activation(root, id)?.checked_add(1).ok_or_else(|| error("plugin activation limit reached"))?;
    let path = checked_path(root, &format!("{id}.activation.json"))?;
    photocraft_format::atomic_write(&path, &serde_json::to_vec(&next).map_err(error)?).map_err(error)?;
    Ok(next)
}

/// Explicit replacement may repair malformed metadata, but cannot turn filesystem errors,
/// oversized records or linked paths into permission to overwrite an installation.
fn previous_record(root: &Path, id: &str, replace: bool) -> Result<(Option<Record>, Option<Vec<u8>>)> {
    let path = record_path(root, id)?;
    if !path.exists() {
        return Ok((None, None));
    }
    let bytes = read_bounded(&path, MAX_RECORD)?;
    let parsed: Result<Record> = (|| {
        let record: Record = serde_json::from_slice(&bytes).map_err(error)?;
        validate_record(&record, id)?;
        Ok(record)
    })();
    match parsed {
        Ok(record) => Ok((Some(record), None)),
        Err(cause) if !replace => {
            Err(error(format!("installed metadata for {id} is damaged; review the plugin and reinstall with replace:true to repair it: {cause}")))
        }
        Err(_) => Ok((None, Some(bytes))),
    }
}

/// Preserve damaged metadata for recovery without making it another startup installation.
/// Existing sidecars must match byte-for-byte; repair never overwrites an unrelated file.
fn preserve_corrupt_record(root: &Path, id: &str, bytes: &[u8]) -> Result<PathBuf> {
    let digest = blake3::hash(bytes).to_hex();
    let short_digest = digest.as_str().get(..16).ok_or_else(|| error("invalid metadata digest"))?;
    let path = checked_path(root, &format!("{id}.corrupt-{short_digest}.json"))?;
    let mut output = match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(output) => output,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if read_bounded(&path, MAX_RECORD)? != bytes {
                return Err(error("damaged metadata backup already exists with different contents; preserve or rename it before retrying"));
            }
            return Ok(path);
        }
        Err(e) => return Err(error(e)),
    };
    let written = output.write_all(bytes).and_then(|()| output.sync_all());
    drop(output);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&path);
        return Err(error(e));
    }
    Ok(path)
}

pub(crate) fn record_ids(root: &Path) -> Result<Vec<String>> {
    let _store = StoreGuard::read(root)?;
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(root).map_err(error)? {
        let entry = entry.map_err(error)?;
        if let Some(id) = entry.file_name().to_str().and_then(|n| n.strip_suffix(RECORD_SUFFIX)) {
            ids.push(id.to_owned());
            if ids.len() > 4096 {
                break;
            }
        }
    }
    ids.sort();
    Ok(ids)
}

fn generation(root: &Path, record: &Record) -> Result<PathBuf> {
    validate_record(record, &record.id)?;
    checked_path(root, &format!("bundles/{}/{}", record.id, record.hash))
}

fn load_generation(root: &Path, record: &Record) -> Result<Loaded> {
    let activation = activation(root, &record.id)?;
    let dir = generation(root, record)?;
    let marker = dir.join(OWNERSHIP);
    let stored: Record = serde_json::from_slice(&read_bounded(&marker, MAX_RECORD)?).map_err(error)?;
    if &stored != record {
        return Err(error("installation ownership record does not match"));
    }
    let mut module = Vec::new();
    let mut filters = BTreeMap::new();
    for file in &record.files {
        let path = checked_path(&dir, &file.path)?;
        let data = read_bounded(&path, file.size as usize)?;
        if data.len() as u64 != file.size || blake3::hash(&data).to_hex().as_str() != file.hash {
            return Err(error(format!("installed file changed: {}", file.path)));
        }
        load_filter(record.package.as_ref(), &file.path, &data, &mut filters)?;
        if file.path == "plugin.pcext" {
            module = data;
        }
    }
    let native = record.package.as_ref().and_then(|package| {
        package.native.as_ref().map(|native| {
            Arc::new(InstalledNative {
                worker: dir.join(&native.entry),
                package: package.clone(),
                generation: Some(ManagedGeneration { root: root.into(), id: record.id.clone(), hash: record.hash.clone(), activation }),
            })
        })
    });
    Ok(Loaded { module, native, package: record.package.clone(), filters, storage: storage_size(&record.files)? })
}

pub(crate) fn load(root: &Path, id: &str) -> Result<Loaded> {
    let _store = StoreGuard::read(root)?;
    let record = read_record(root, id)?.ok_or_else(|| error("installation record is missing"))?;
    load_generation(root, &record)
}

fn cleanup(root: &Path, record: &Record) -> Result<()> {
    let dir = generation(root, record)?;
    if !dir.exists() {
        return Ok(());
    }
    let stored: Record = serde_json::from_slice(&read_bounded(&dir.join(OWNERSHIP), MAX_RECORD)?).map_err(error)?;
    if &stored != record {
        return Err(error("refusing to remove a generation with different ownership"));
    }
    let mut directories = BTreeSet::new();
    // Preflight all paths before deleting any payload. Delete only recorded owned files.
    let files = record.files.iter().map(|f| checked_path(&dir, &f.path)).collect::<Result<Vec<_>>>()?;
    for path in files {
        let mut parent = path.parent();
        while let Some(p) = parent.filter(|p| *p != dir) {
            directories.insert(p.to_path_buf());
            parent = p.parent();
        }
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(error(e)),
        }
    }
    for path in directories.iter().rev() {
        match std::fs::remove_dir(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(error(e)),
        }
    }
    std::fs::remove_file(dir.join(OWNERSHIP)).map_err(error)?;
    std::fs::remove_dir(&dir).map_err(error)?;
    if let Some(parent) = dir.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    Ok(())
}

pub(crate) fn install(root: &Path, id: &str, bytes: &[u8], package: Option<&Package>, replace: bool) -> Result<InstalledPayload> {
    let _store = StoreGuard::write(root)?;
    activation(root, id)?.checked_add(1).ok_or_else(|| error("plugin activation limit reached"))?;
    let (previous, corrupt) = previous_record(root, id, replace)?;
    if !replace && previous.as_ref().is_some_and(|record| load_generation(root, record).is_ok()) {
        return Err(error("extension already installed; use replace:true to update it"));
    }
    let enabled = !checked_path(root, &format!("{id}.disabled"))?.exists();
    let record = Record {
        format_version: 1,
        id: id.into(),
        hash: blake3::hash(bytes).to_hex().to_string(),
        package: package.map(|p| p.info.clone()),
        files: package
            .map(|p| p.files.clone())
            .unwrap_or_else(|| vec![FileRecord { path: "plugin.pcext".into(), size: bytes.len() as u64, hash: blake3::hash(bytes).to_hex().to_string() }]),
    };
    let destination = generation(root, &record)?;
    let parent = destination.parent().ok_or_else(|| error("invalid generation directory"))?;
    std::fs::create_dir_all(parent).map_err(error)?;
    let record_bytes = serde_json::to_vec(&record).map_err(error)?;
    if record_bytes.len() > MAX_RECORD {
        return Err(error("installation record is too large"));
    }
    let repaired = corrupt.as_deref().map(|bytes| preserve_corrupt_record(root, id, bytes)).transpose()?;
    // Antivirus or disk damage can remove a worker after a successful installation. Reinstalling
    // the identical archive repairs its owned generation instead of trapping it in an invalid
    // cache forever. Never follow links or remove files absent from the ownership record.
    if destination.exists() && load_generation(root, &record).is_err() {
        cleanup(root, &record)?;
        std::fs::create_dir_all(parent).map_err(error)?;
    }
    let created = !destination.exists();
    if created {
        let temp = tempfile::Builder::new().prefix("staging-").tempdir_in(parent).map_err(error)?;
        let zip = package.map(|_| ZipReader::new(bytes).map_err(error)).transpose()?;
        for file in &record.files {
            let path = checked_path(temp.path(), &file.path)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(error)?;
            }
            let data = match &zip {
                Some(zip) => zip.read_by_name(&file.path, file.size as usize).map_err(error)?,
                None => bytes.to_vec(),
            };
            if data.len() as u64 != file.size || blake3::hash(&data).to_hex().as_str() != file.hash {
                return Err(error(format!("package changed during installation: {}", file.path)));
            }
            let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(error)?;
            output.write_all(&data).map_err(error)?;
            #[cfg(unix)]
            if record.package.as_ref().and_then(|package| package.native.as_ref()).is_some_and(|native| native.entry == file.path) {
                use std::os::unix::fs::PermissionsExt;
                output.set_permissions(std::fs::Permissions::from_mode(0o700)).map_err(error)?;
            }
            output.sync_all().map_err(error)?;
        }
        let mut marker = std::fs::OpenOptions::new().write(true).create_new(true).open(temp.path().join(OWNERSHIP)).map_err(error)?;
        marker.write_all(&record_bytes).map_err(error)?;
        marker.sync_all().map_err(error)?;
        drop(marker);
        std::fs::rename(temp.path(), &destination).map_err(error)?;
    }
    let loaded = match load_generation(root, &record) {
        Ok(loaded) => loaded,
        Err(e) => {
            if created {
                let _ = cleanup(root, &record);
            }
            return Err(e);
        }
    };
    // Publish last. Any validation or extraction failure leaves the previous active record intact.
    let published = revoke_activation(root, id).and_then(|activation| {
        record_path(root, id).and_then(|path| photocraft_format::atomic_write(&path, &record_bytes).map_err(error))?;
        Ok(activation)
    });
    let activation = match published {
        Ok(activation) => activation,
        Err(e) => {
            if created && let Err(cleanup_error) = cleanup(root, &record) {
                return Err(error(format!("{e}; staged payload cleanup failed: {cleanup_error}")));
            }
            return Err(e);
        }
    };
    let warning = previous.filter(|old| old.hash != record.hash).and_then(|old| cleanup(root, &old).err()).map(|e| e.to_string()).or_else(|| {
        repaired.map(|path| format!("Repaired damaged plugin metadata; preserved the original at {} and retained any previous payload folders", path.display()))
    });
    Ok(InstalledPayload { native: loaded.native.map(|native| Arc::new(native.reactivated(activation))), enabled, warning })
}

pub(crate) fn remove(root: &Path, id: &str) -> Result<Option<String>> {
    let _store = StoreGuard::write(root)?;
    let record = read_record(root, id)?;
    revoke_activation(root, id)?;
    // Remove a legacy fallback before the authoritative record, so it cannot reappear at restart.
    let legacy = checked_path(root, &format!("{id}.pcext"))?;
    if legacy.exists() {
        std::fs::remove_file(legacy).map_err(error)?;
    }
    if let Some(record) = record {
        std::fs::remove_file(record_path(root, id)?).map_err(error)?;
        let mut warnings = cleanup(root, &record).err().map(|e| e.to_string()).into_iter().collect::<Vec<_>>();
        if let Err(cause) = crate::extension_cmds::remove_if_exists(&checked_path(root, &format!("{id}.disabled"))?) {
            warnings.push(format!("Disabled marker could not be cleaned up: {cause}"));
        }
        return Ok((!warnings.is_empty()).then(|| warnings.join("; ")));
    }
    crate::extension_cmds::remove_if_exists(&checked_path(root, &format!("{id}.disabled"))?)?;
    Ok(None)
}

#[cfg(test)]
mod tests;
