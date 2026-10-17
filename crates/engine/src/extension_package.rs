//! Installable plugin packages. Payload generations are immutable; one small atomic record selects
//! the active generation. Archive paths never become ambient filesystem capabilities.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use photocraft_format::zip::ZipReader;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{EngineError, Result};

mod managed;
mod store;
pub(crate) use store::StoreGuard;

pub const MAX_PACKAGE_BYTES: usize = 512 << 20;
const MAX_EXPANDED: usize = 1024 << 20;
const MAX_FILE: usize = 256 << 20;
const MAX_FILES: usize = 512;
const MAX_RECORD: usize = 512 << 10;
const MAX_ROLLBACK_JOURNAL: usize = MAX_RECORD + 1024;
const RECORD_SUFFIX: &str = ".installation.json";
const OWNERSHIP: &str = ".photocraft-install.json";
pub use photocraft_plugin_api::{FilterPayload as FilterInfo, NativePayload as NativeInfo, PackageV2 as PackageInfo};

/// Shared package/runtime metadata. OS generation leases separately protect in-use payloads.
#[derive(Clone)]
pub struct InstalledNative {
    pub worker: PathBuf,
    pub package: PackageInfo,
    pub(crate) generation: Option<ManagedGeneration>,
    pub(crate) worker_v2: Option<Arc<crate::plugin_runtime::worker_v2::Supervisor>>,
}

/// An OS lease protects immutable files without holding the store publication lock.
pub(crate) struct GenerationLease {
    _guard: StoreGuard,
    path: PathBuf,
    exclusive: bool,
}

/// Retained only through a short authority check and document/state admission.
pub(crate) struct AdmissionGuard {
    _store: StoreGuard,
    _generation: Option<GenerationLease>,
}

impl AdmissionGuard {
    pub(crate) fn store(root: &Path) -> Result<Self> {
        Ok(Self { _store: StoreGuard::read(root)?, _generation: None })
    }
}

pub(crate) struct GenerationWatch {
    stop: std::sync::mpsc::SyncSender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for GenerationWatch {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone)]
pub(crate) struct ManagedGeneration {
    root: PathBuf,
    id: String,
    hash: String,
    activation: u64,
    runtime_instance: u64,
    verified: Arc<std::sync::OnceLock<()>>,
}

impl InstalledNative {
    pub(crate) fn verify_payload(&self, context: &crate::jobs::JobCtx) -> Result<()> {
        let Some(generation) = &self.generation else {
            return Ok(());
        };
        let _lease = self.lease()?;
        if generation.verified.get().is_some() {
            return context.check();
        }
        let record = {
            let _store = StoreGuard::read(&generation.root)?;
            read_record(&generation.root, &generation.id)?.ok_or_else(|| error("installation record is missing"))?
        };
        verify_closure(&generation.root, &record, context)?;
        let _ = generation.verified.set(());
        Ok(())
    }
    pub(crate) fn validate_storage(&self) -> Result<()> {
        if self.worker_v2.is_none() {
            return Ok(());
        }
        let Some(generation) = &self.generation else {
            return Ok(());
        };
        let checked = managed::paths(&generation.root, &generation.id, &generation.hash).map(|_| ());
        if checked.is_err() {
            self.stop();
        }
        checked
    }
    pub(crate) fn scheduler_owner(&self) -> String {
        self.generation.as_ref().map(generation_owner).unwrap_or_else(|| format!("fixture:{}", self.worker.display()))
    }
    pub(crate) fn watch(&self, context: &crate::jobs::JobCtx) -> Result<Option<GenerationWatch>> {
        let Some(generation) = self.generation.clone() else {
            return Ok(None);
        };
        watch_identity(generation.root, generation.id, generation.hash, generation.activation, context).map(Some)
    }
    /// Keep the selected payload alive through execution or a result's commit. Memory-only
    /// fixtures have no managed store; installed native workers always carry this identity.
    pub(crate) fn lease(&self) -> Result<Option<GenerationLease>> {
        let Some(generation) = &self.generation else { return Ok(None) };
        let guard = StoreGuard::read(&generation.root)?;
        let lease = self.checked_lease(generation)?;
        drop(guard);
        Ok(Some(lease))
    }

    pub(crate) fn admission(&self) -> Result<Option<AdmissionGuard>> {
        let Some(generation) = &self.generation else { return Ok(None) };
        let store = StoreGuard::read(&generation.root)?;
        let lease = self.checked_lease(generation)?;
        Ok(Some(AdmissionGuard { _store: store, _generation: Some(lease) }))
    }

    fn checked_lease(&self, generation: &ManagedGeneration) -> Result<GenerationLease> {
        let record = read_record(&generation.root, &generation.id)?;
        if !record.is_some_and(|record| record.hash == generation.hash && record.package.as_ref() == Some(&self.package))
            || activation(&generation.root, &generation.id)? != generation.activation
            || checked_path(&generation.root, &format!("{}.disabled", generation.id))?.exists()
        {
            return Err(error(
                "This native plugin was disabled, removed or updated in another PhotoCraft instance. Restart this PhotoCraft instance to reload plugins, then run the action again.",
            ));
        }
        generation_lease(&generation.root, &generation.id, &generation.hash, false)
    }

    pub(crate) fn reactivated(&self, activation: u64) -> Self {
        let mut native = self.clone();
        if let Some(generation) = &mut native.generation {
            generation.activation = activation;
        }
        if let Some(supervisor) = &native.worker_v2 {
            supervisor.stop();
            native.worker_v2 = Some(Arc::new(configure_supervisor(supervisor.fresh(), native.generation.as_ref())));
        }
        native
    }

    pub(crate) fn stop(&self) {
        if let Some(supervisor) = &self.worker_v2 {
            supervisor.stop();
        }
    }
}

fn generation_owner(generation: &ManagedGeneration) -> String {
    let identity = format!("{}\0{}\0{}\0{}\0{}", generation.root.display(), generation.id, generation.hash, generation.activation, generation.runtime_instance);
    format!("{}:{}", generation.id, blake3::hash(identity.as_bytes()).to_hex())
}

fn configure_supervisor(
    supervisor: crate::plugin_runtime::worker_v2::Supervisor,
    generation: Option<&ManagedGeneration>,
) -> crate::plugin_runtime::worker_v2::Supervisor {
    let Some(generation) = generation.cloned() else {
        return supervisor;
    };
    let owner = generation_owner(&generation);
    supervisor
        .with_resident_admission(Arc::new(move |resources, context| {
            crate::plugin_scheduler::PluginScheduler::default().resident(&owner, resources, context).map(|permit| Box::new(permit) as Box<dyn Send>)
        }))
        .with_liveness(Arc::new(move || generation_current(&generation)))
}

fn generation_current(generation: &ManagedGeneration) -> bool {
    identity_current(&generation.root, &generation.id, &generation.hash, generation.activation)
}

fn identity_current(root: &Path, id: &str, hash: &str, epoch: u64) -> bool {
    // Publication is nonblocking. An unrelated writer briefly owning the store is not revocation.
    let Ok(_store) = StoreGuard::read(root) else {
        return true;
    };
    read_record(root, id).is_ok_and(|record| record.is_some_and(|record| record.hash == hash))
        && activation(root, id).is_ok_and(|activation| activation == epoch)
        && checked_path(root, &format!("{id}.disabled")).is_ok_and(|path| !path.exists())
}

fn watch_identity(root: PathBuf, id: String, hash: String, epoch: u64, context: &crate::jobs::JobCtx) -> Result<GenerationWatch> {
    let context = context.clone();
    let (stop, receiver) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("plugin-revocation".into())
        .spawn(move || {
            while matches!(receiver.recv_timeout(std::time::Duration::from_millis(100)), Err(std::sync::mpsc::RecvTimeoutError::Timeout)) {
                if !identity_current(&root, &id, &hash, epoch) {
                    context.cancel();
                    break;
                }
            }
        })
        .map_err(error)?;
    Ok(GenerationWatch { stop, thread: Some(thread) })
}

pub(crate) fn bind(
    root: &Path,
    id: &str,
    expected_hash: &str,
    extension: photocraft_plugins::extension::Extension,
) -> Result<photocraft_plugins::extension::Extension> {
    let _store = StoreGuard::read(root)?;
    let record = read_record(root, id)?.ok_or_else(|| error("installation record is missing"))?;
    if record.hash != expected_hash || record.descriptor.as_ref().is_some_and(|descriptor| descriptor != &extension.manifest) {
        return Err(error("Plugin installation changed while it was being loaded. Restart this PhotoCraft instance to reload plugins."));
    }
    let root = root.to_path_buf();
    let epoch = activation(&root, id)?;
    Ok(extension.with_authority(move || {
        let check = || -> Result<()> {
            let _store = StoreGuard::read(&root)?;
            if read_record(&root, &record.id)?.as_ref() != Some(&record)
                || activation(&root, &record.id)? != epoch
                || checked_path(&root, &format!("{}.disabled", record.id))?.exists()
            {
                return Err(error("Plugin installation was disabled, removed or updated. Restart this PhotoCraft instance to reload plugins."));
            }
            Ok(())
        };
        check().map_err(|cause| photocraft_plugins::Error::Failed(cause.to_string()))
    }))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRecord {
    path: String,
    size: u64,
    hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    format_version: u32,
    id: String,
    hash: String,
    package: Option<PackageInfo>,
    files: Vec<FileRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    descriptor: Option<photocraft_plugins::extension::Manifest>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RollbackJournal {
    format_version: u32,
    id: String,
    target_hash: String,
    previous: Option<Record>,
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

pub(crate) struct DeferredPayload {
    root: PathBuf,
    record: Record,
    activation: u64,
    loaded: std::sync::OnceLock<Arc<Loaded>>,
    gate: std::sync::Mutex<()>,
}

impl DeferredPayload {
    pub(crate) fn watch(&self, context: &crate::jobs::JobCtx) -> Result<GenerationWatch> {
        watch_identity(self.root.clone(), self.record.id.clone(), self.record.hash.clone(), self.activation, context)
    }

    fn current_lease(&self) -> Result<GenerationLease> {
        let _store = StoreGuard::read(&self.root)?;
        if read_record(&self.root, &self.record.id)?.as_ref() != Some(&self.record) || activation(&self.root, &self.record.id)? != self.activation {
            return Err(error("Plugin installation changed. Restart this PhotoCraft instance to reload plugins."));
        }
        generation_lease(&self.root, &self.record.id, &self.record.hash, false)
    }

    pub(crate) fn panel(&self) -> Result<Vec<u8>> {
        let _lease = self.current_lease()?;
        let file = self.record.files.iter().find(|file| file.path == "plugin.pcext").ok_or_else(|| error("plugin module is missing"))?;
        let directory = generation(&self.root, &self.record)?;
        let owned: Record = serde_json::from_slice(&read_bounded(&directory.join(OWNERSHIP), MAX_RECORD)?).map_err(error)?;
        if owned != self.record {
            return Err(error("installation ownership record does not match"));
        }
        let bytes = read_bounded(&checked_path(&directory, &file.path)?, file.size as usize)?;
        if bytes.len() as u64 != file.size || blake3::hash(&bytes).to_hex().as_str() != file.hash {
            return Err(error("installed panel module changed"));
        }
        Ok(bytes)
    }

    /// Called on a scheduled worker: discovery and panel events never hash models or compile filters.
    pub(crate) fn activate(&self, context: &crate::jobs::JobCtx) -> Result<Arc<Loaded>> {
        let _guard = self.gate.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _lease = self.current_lease()?;
        if let Some(loaded) = self.loaded.get() {
            return Ok(Arc::clone(loaded));
        }
        verify_closure(&self.root, &self.record, context)?;
        let directory = generation(&self.root, &self.record)?;
        let mut filters = BTreeMap::new();
        for file in &self.record.files {
            context.check()?;
            if self.record.package.as_ref().is_some_and(|package| package.filters.iter().any(|filter| filter.module == file.path)) {
                let bytes = read_bounded(&checked_path(&directory, &file.path)?, file.size as usize)?;
                if blake3::hash(&bytes).to_hex().as_str() != file.hash {
                    return Err(error("installed filter changed during activation"));
                }
                load_filter(self.record.package.as_ref(), &file.path, &bytes, &mut filters)?;
            }
        }
        let loaded =
            Arc::new(Loaded { module: Vec::new(), native: None, package: self.record.package.clone(), filters, storage: storage_size(&self.record.files)? });
        let _ = self.loaded.set(Arc::clone(&loaded));
        Ok(loaded)
    }
}

fn verify_closure(root: &Path, record: &Record, context: &crate::jobs::JobCtx) -> Result<()> {
    let directory = generation(root, record)?;
    let owned: Record = serde_json::from_slice(&read_bounded(&directory.join(OWNERSHIP), MAX_RECORD)?).map_err(error)?;
    if &owned != record {
        return Err(error("installation ownership record does not match"));
    }
    let total = record.files.iter().fold(0_u64, |total, file| total.saturating_add(file.size));
    let mut checked = 0_u64;
    let mut buffer = [0_u8; 64 << 10];
    for file in &record.files {
        context.check()?;
        let path = checked_path(&directory, &file.path)?;
        let metadata = std::fs::symlink_metadata(&path).map_err(error)?;
        if !metadata.is_file() || is_link(&metadata) || metadata.len() != file.size {
            return Err(error(format!("installed file changed: {}", file.path)));
        }
        let input = std::fs::File::open(&path).map_err(error)?;
        let metadata = input.metadata().map_err(error)?;
        if !metadata.is_file() || is_link(&metadata) || metadata.len() != file.size {
            return Err(error(format!("installed file changed: {}", file.path)));
        }
        let mut input = input.take(file.size.saturating_add(1));
        let mut hash = blake3::Hasher::new();
        let mut size = 0_u64;
        loop {
            context.check()?;
            let count = input.read(&mut buffer).map_err(error)?;
            if count == 0 {
                break;
            }
            size = size.saturating_add(count as u64);
            hash.update(buffer.get(..count).ok_or_else(|| error("invalid plugin verification read"))?);
        }
        if size != file.size || hash.finalize().to_hex().as_str() != file.hash {
            return Err(error(format!("installed file changed: {}", file.path)));
        }
        checked = checked.saturating_add(size);
        context.progress(0.05 * (checked as f32 / total.max(1) as f32), "Verifying plugin files");
    }
    Ok(())
}

pub(crate) struct Discovered {
    pub extension: Arc<photocraft_plugins::extension::Extension>,
    pub deferred: Option<Arc<DeferredPayload>>,
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

fn generation_lease(root: &Path, id: &str, hash: &str, exclusive: bool) -> Result<GenerationLease> {
    if !valid_id(id) || !hash_valid(hash) {
        return Err(error("invalid generation lease identity"));
    }
    let name = format!("{id}.{hash}.lease");
    Ok(GenerationLease { _guard: StoreGuard::named(root, &name, exclusive)?, path: checked_path(root, &name)?, exclusive })
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
        2 => {
            if info.filters.len() > 16 {
                return Err(error("a package may declare at most 16 filters"));
            }
            let mut names = BTreeSet::from(["package.json".to_owned(), "plugin.pcext".into(), "license".into(), "third-party-notices.md".into()]);
            if let Some(native) = &info.native {
                if native.service != "plugin.processImage" {
                    return Err(error("native service must be plugin.processImage"));
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
    if path == "runtime/worker-v2.json" {
        photocraft_plugin_api::MAX_WORKER_DESCRIPTOR_BYTES
    } else if path == "runtime/managed-v1.json" {
        64 << 10
    } else if path == "plugin.pcext" {
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
        if entry.name == "runtime/worker-v2.json" {
            if info.native.is_none() {
                return Err(error("worker protocol 2 requires a package-owned native executable"));
            }
            photocraft_plugin_api::WorkerV2Descriptor::parse(&data).map_err(error)?;
        }
        if entry.name == "runtime/managed-v1.json" {
            managed::MigrationPlan::parse(&data)?;
        }
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
    if let Some(descriptor) = &record.descriptor
        && descriptor.id != id
    {
        return Err(error("installed descriptor identity mismatch"));
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
    load_generation_inner(root, record, true)
}

fn load_generation_inner(root: &Path, record: &Record, with_managed_state: bool) -> Result<Loaded> {
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
    let native = if with_managed_state {
        if record.files.iter().any(|file| file.path == "runtime/worker-v2.json") {
            managed::paths(root, &record.id, &record.hash)?;
        }
        native_handle(root, record, activation)?
    } else {
        None
    };
    Ok(Loaded { module, native, package: record.package.clone(), filters, storage: storage_size(&record.files)? })
}

fn native_handle(root: &Path, record: &Record, activation: u64) -> Result<Option<Arc<InstalledNative>>> {
    let Some(package) = &record.package else {
        return Ok(None);
    };
    let Some(native) = &package.native else {
        return Ok(None);
    };
    let dir = generation(root, record)?;
    let worker = checked_path(&dir, &native.entry)?;
    static NEXT_RUNTIME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let mut current = NEXT_RUNTIME.load(std::sync::atomic::Ordering::Relaxed);
    let runtime_instance = loop {
        let next = current.checked_add(1).ok_or_else(|| error("private plugin runtime identities are exhausted"))?;
        match NEXT_RUNTIME.compare_exchange_weak(current, next, std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed) {
            Ok(_) => break current,
            Err(observed) => current = observed,
        }
    };
    let verified = Arc::new(std::sync::OnceLock::new());
    let worker_v2 = record
        .files
        .iter()
        .find(|file| file.path == "runtime/worker-v2.json")
        .map(|file| {
            let bytes = read_bounded(&checked_path(&dir, &file.path)?, photocraft_plugin_api::MAX_WORKER_DESCRIPTOR_BYTES)?;
            if bytes.len() as u64 != file.size || blake3::hash(&bytes).to_hex().as_str() != file.hash {
                return Err(error("worker protocol declaration changed"));
            }
            let descriptor = photocraft_plugin_api::WorkerV2Descriptor::parse(&bytes).map_err(error)?;
            let identity = ManagedGeneration {
                root: root.into(),
                id: record.id.clone(),
                hash: record.hash.clone(),
                activation,
                runtime_instance,
                verified: Arc::clone(&verified),
            };
            let supervisor = configure_supervisor(
                crate::plugin_runtime::worker_v2::Supervisor::new(worker.clone(), record.id.clone(), record.hash.clone(), descriptor)?,
                Some(&identity),
            )
            .with_managed_storage(serde_json::to_value(managed::metadata(root, &record.id, &record.hash)?).map_err(error)?)?;
            Ok(Arc::new(supervisor))
        })
        .transpose()?;
    Ok(Some(Arc::new(InstalledNative {
        worker,
        package: package.clone(),
        generation: Some(ManagedGeneration { root: root.into(), id: record.id.clone(), hash: record.hash.clone(), activation, runtime_instance, verified }),
        worker_v2,
    })))
}

fn rollback_journal_path(root: &Path, id: &str) -> Result<PathBuf> {
    if !valid_id(id) {
        return Err(error("invalid rollback plugin id"));
    }
    checked_path(root, &format!("{id}.rollback-pending.json"))
}

fn rollback_journal(root: &Path, id: &str) -> Result<Option<RollbackJournal>> {
    let path = rollback_journal_path(root, id)?;
    if !path.exists() {
        return Ok(None);
    }
    let journal: RollbackJournal = serde_json::from_slice(&read_bounded(&path, MAX_ROLLBACK_JOURNAL)?).map_err(error)?;
    if journal.format_version != 1 || journal.id != id || !hash_valid(&journal.target_hash) {
        return Err(error("invalid rollback transaction metadata"));
    }
    if let Some(previous) = &journal.previous {
        validate_record(previous, id)?;
        if previous.hash == journal.target_hash {
            return Err(error("rollback transaction must select a different generation"));
        }
    }
    Ok(Some(journal))
}

/// The active hash makes a prepared journal authoritative only after publication.
/// Until then, a failed/crashed update leaves the previous rollback record selected.
fn rollback_record(root: &Path, id: &str, active_hash: Option<&str>) -> Result<Option<Record>> {
    if let Some(journal) = rollback_journal(root, id)?
        && Some(journal.target_hash.as_str()) == active_hash
    {
        return Ok(journal.previous);
    }
    let path = checked_path(root, &format!("{id}.rollback.json"))?;
    if !path.exists() {
        return Ok(None);
    }
    let record: Record = serde_json::from_slice(&read_bounded(&path, MAX_RECORD)?).map_err(error)?;
    validate_record(&record, id)?;
    Ok(Some(record))
}

fn finish_rollback_journal(root: &Path, id: &str, active_hash: Option<&str>) -> Result<()> {
    let Some(journal) = rollback_journal(root, id)? else {
        return Ok(());
    };
    if Some(journal.target_hash.as_str()) != active_hash {
        return Ok(());
    }
    let backup = checked_path(root, &format!("{id}.rollback.json"))?;
    if let Some(previous) = journal.previous {
        photocraft_format::atomic_write(&backup, &serde_json::to_vec(&previous).map_err(error)?).map_err(error)?;
    } else {
        crate::extension_cmds::remove_if_exists(&backup)?;
    }
    crate::extension_cmds::remove_if_exists(&rollback_journal_path(root, id)?)
}

fn stage_rollback_journal(root: &Path, id: &str, target_hash: &str, previous: Option<&Record>) -> Result<()> {
    if !valid_id(id) || !hash_valid(target_hash) || previous.is_some_and(|previous| previous.id != id || previous.hash == target_hash) {
        return Err(error("invalid rollback transaction target or source"));
    }
    // Do not overwrite the only durable record of an earlier committed transaction.
    finish_rollback_journal(root, id, previous.map(|record| record.hash.as_str()))?;
    let journal = RollbackJournal { format_version: 1, id: id.into(), target_hash: target_hash.into(), previous: previous.cloned() };
    let bytes = serde_json::to_vec(&journal).map_err(error)?;
    if bytes.len() > MAX_ROLLBACK_JOURNAL {
        return Err(error("rollback transaction metadata exceeds its byte limit"));
    }
    photocraft_format::atomic_write(&rollback_journal_path(root, id)?, &bytes).map_err(error)
}

/// Read only host-owned metadata during discovery. Old records retain the eager adapter.
pub(crate) fn discover(root: &Path, id: &str) -> Result<Discovered> {
    let _store = StoreGuard::read(root)?;
    let record = read_record(root, id)?.ok_or_else(|| error("installation record is missing"))?;
    let Some(manifest) = record.descriptor.clone() else {
        let loaded = load_generation(root, &record)?;
        return Ok(Discovered {
            extension: Arc::new(bind(root, id, &record.hash, photocraft_plugins::extension::Extension::load(&loaded.module).map_err(error)?)?),
            deferred: None,
            native: loaded.native,
            package: loaded.package,
            filters: loaded.filters,
            storage: loaded.storage,
        });
    };
    // Ownership remains readable across host API changes; executable discovery still
    // validates the cached descriptor against the current contract before registering it.
    let manifest = photocraft_plugins::extension::Manifest::parse(&serde_json::to_vec(&manifest).map_err(error)?).map_err(error)?;
    let dir = generation(root, &record)?;
    let stored: Record = serde_json::from_slice(&read_bounded(&dir.join(OWNERSHIP), MAX_RECORD)?).map_err(error)?;
    if stored != record {
        return Err(error("installation ownership record does not match"));
    }
    let activation = activation(root, id)?;
    let deferred = Arc::new(DeferredPayload { root: root.into(), record: record.clone(), activation, loaded: Default::default(), gate: Default::default() });
    let source = Arc::clone(&deferred);
    let extension = Arc::new(bind(
        root,
        id,
        &record.hash,
        photocraft_plugins::extension::Extension::deferred(manifest, move || {
            source.panel().map_err(|cause| photocraft_plugins::Error::Failed(cause.to_string()))
        }),
    )?);
    let native = native_handle(root, &record, activation)?;
    Ok(Discovered {
        extension,
        deferred: Some(deferred),
        native,
        package: record.package.clone(),
        filters: BTreeMap::new(),
        storage: storage_size(&record.files)?,
    })
}

#[cfg(test)]
pub(crate) fn load(root: &Path, id: &str) -> Result<Loaded> {
    let _store = StoreGuard::read(root)?;
    let record = read_record(root, id)?.ok_or_else(|| error("installation record is missing"))?;
    load_generation(root, &record)
}

pub(crate) fn needs_repair(root: &Path, id: &str) -> Result<bool> {
    let _store = StoreGuard::read(root)?;
    Ok(read_record(root, id)?.is_some_and(|record| load_generation_inner(root, &record, false).is_err()))
}

fn cleanup(root: &Path, record: &Record) -> Result<()> {
    let lease = generation_lease(root, &record.id, &record.hash, true)?;
    cleanup_leased(root, record, &lease)
}

fn cleanup_leased(root: &Path, record: &Record, lease: &GenerationLease) -> Result<()> {
    if !lease.exclusive || lease.path != checked_path(root, &format!("{}.{}.lease", record.id, record.hash))? {
        return Err(error("generation cleanup requires its exclusive lease"));
    }
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewGuard {
    pub expected_store_hash: String,
    pub expected_installed_hash: String,
    pub expected_activation: u64,
}

/// Fingerprint the active pointer, including damaged metadata that an explicit repair may
/// replace. The activation tombstone also detects remove/reinstall of identical bytes.
fn review_guard_inner(root: &Path, id: &str) -> Result<ReviewGuard> {
    let record = record_path(root, id)?;
    let loose = checked_path(root, &format!("{id}.pcext"))?;
    let hash = if record.exists() {
        blake3::hash(&read_bounded(&record, MAX_RECORD)?).to_hex().to_string()
    } else if loose.exists() {
        blake3::hash(&read_bounded(&loose, photocraft_plugins::extension::MAX_MODULE_BYTES)?).to_hex().to_string()
    } else {
        String::new()
    };
    let canonical = root.canonicalize().map_err(error)?;
    Ok(ReviewGuard {
        expected_store_hash: blake3::hash(canonical.as_os_str().as_encoded_bytes()).to_hex().to_string(),
        expected_installed_hash: hash,
        expected_activation: activation(root, id)?,
    })
}

pub(crate) fn review_guard(root: &Path, id: &str) -> Result<ReviewGuard> {
    let _store = StoreGuard::read(root)?;
    review_guard_inner(root, id)
}

/// Recovery metadata is display-only; it never grants execution or contributions.
pub(crate) fn recovery_metadata(root: &Path, id: &str) -> Result<(ReviewGuard, Option<photocraft_plugins::extension::Manifest>)> {
    let _store = StoreGuard::read(root)?;
    let guard = review_guard_inner(root, id)?;
    let manifest = read_record(root, id).ok().flatten().and_then(|record| record.descriptor);
    Ok((guard, manifest))
}

pub(crate) fn review_guard_for(root: &Path, id: &str, expected: Option<&photocraft_plugins::extension::Manifest>) -> Result<ReviewGuard> {
    let _store = StoreGuard::read(root)?;
    // A different process may have published a newer descriptor since this session loaded it.
    // Damaged metadata is still reviewable for explicit repair, fingerprinted as raw bytes.
    if let Ok(Some(record)) = read_record(root, id)
        && (expected.is_none() || record.descriptor.as_ref().zip(expected).is_some_and(|(actual, expected)| actual != expected))
    {
        return Err(error("The installed plugin changed in another PhotoCraft instance. Restart to reload plugins before updating."));
    }
    review_guard_inner(root, id)
}

pub(crate) fn install_checked(
    root: &Path,
    id: &str,
    bytes: &[u8],
    package: Option<&Package>,
    replace: bool,
    guard: Option<&ReviewGuard>,
) -> Result<InstalledPayload> {
    install_guarded_with_publication(root, id, bytes, package, replace, guard, |path, bytes| photocraft_format::atomic_write(path, bytes).map_err(error))
}

#[cfg(test)]
pub(crate) fn install(root: &Path, id: &str, bytes: &[u8], package: Option<&Package>, replace: bool) -> Result<InstalledPayload> {
    install_with_publication(root, id, bytes, package, replace, |path, bytes| photocraft_format::atomic_write(path, bytes).map_err(error))
}

#[cfg(test)]
fn install_with_publication(
    root: &Path,
    id: &str,
    bytes: &[u8],
    package: Option<&Package>,
    replace: bool,
    publish: impl FnOnce(&Path, &[u8]) -> Result<()>,
) -> Result<InstalledPayload> {
    install_guarded_with_publication(root, id, bytes, package, replace, None, publish)
}

fn install_guarded_with_publication(
    root: &Path,
    id: &str,
    bytes: &[u8],
    package: Option<&Package>,
    replace: bool,
    guard: Option<&ReviewGuard>,
    publish: impl FnOnce(&Path, &[u8]) -> Result<()>,
) -> Result<InstalledPayload> {
    let _store = StoreGuard::write(root)?;
    if let Some(expected) = guard
        && review_guard_inner(root, id)? != *expected
    {
        return Err(error("The installed plugin changed since review. Inspect it again."));
    }
    activation(root, id)?.checked_add(1).ok_or_else(|| error("plugin activation limit reached"))?;
    let (previous, corrupt) = previous_record(root, id, replace)?;
    let older = rollback_record(root, id, previous.as_ref().map(|record| record.hash.as_str()))?;
    if !replace && previous.as_ref().is_some_and(|record| load_generation_inner(root, record, false).is_ok()) {
        return Err(error("extension already installed; use replace:true to update it"));
    }
    let enabled = !checked_path(root, &format!("{id}.disabled"))?.exists();
    let mut record = Record {
        format_version: 1,
        id: id.into(),
        hash: blake3::hash(bytes).to_hex().to_string(),
        package: package.map(|p| p.info.clone()),
        files: package
            .map(|p| p.files.clone())
            .unwrap_or_else(|| vec![FileRecord { path: "plugin.pcext".into(), size: bytes.len() as u64, hash: blake3::hash(bytes).to_hex().to_string() }]),
        descriptor: Some(photocraft_plugins::extension::Extension::load(package.map_or(bytes, |package| package.module.as_slice())).map_err(error)?.manifest),
    };
    // A managed-data snapshot must be quiescent. Only this plugin's source generation is
    // excluded; readers of unrelated plugins do not block this update.
    let _source_lease =
        previous.as_ref().filter(|previous| previous.hash != record.hash).map(|previous| generation_lease(root, id, &previous.hash, true)).transpose()?;
    let destination_lease = generation_lease(root, id, &record.hash, true)?;
    let destination = generation(root, &record)?;
    let mut repair_descriptor = false;
    // Repairing identical payload bytes preserves their recorded ownership metadata.
    if destination.exists() {
        let marker = destination.join(OWNERSHIP);
        if let Ok(bytes) = read_bounded(&marker, MAX_RECORD)
            && let Ok(stored) = serde_json::from_slice::<Record>(&bytes)
            && stored.format_version == record.format_version
            && stored.id == record.id
            && stored.hash == record.hash
            && stored.package == record.package
            && stored.files == record.files
        {
            validate_record(&stored, id)?;
            if stored.descriptor.is_none() {
                record.descriptor = None;
            } else {
                repair_descriptor = stored.descriptor != record.descriptor;
            }
        }
    }
    let parent = destination.parent().ok_or_else(|| error("invalid generation directory"))?;
    std::fs::create_dir_all(parent).map_err(error)?;
    let record_bytes = serde_json::to_vec(&record).map_err(error)?;
    if record_bytes.len() > MAX_RECORD {
        return Err(error("installation record is too large"));
    }
    let repaired = corrupt.as_deref().map(|bytes| preserve_corrupt_record(root, id, bytes)).transpose()?;
    if repair_descriptor {
        // The exclusive generation lease and identical owned file list bind this cache
        // repair to the newly reviewed package. Full payload verification still follows.
        photocraft_format::atomic_write(&destination.join(OWNERSHIP), &record_bytes).map_err(error)?;
    }
    // Antivirus or disk damage can remove a worker after a successful installation. Reinstalling
    // the identical archive repairs its owned generation instead of trapping it in an invalid
    // cache forever. Never follow links or remove files absent from the ownership record.
    if destination.exists() && load_generation_inner(root, &record, false).is_err() {
        cleanup_leased(root, &record, &destination_lease)?;
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
    let migration = package
        .and_then(|package| package.files.iter().find(|file| file.path == "runtime/managed-v1.json"))
        .map(|file| read_bounded(&checked_path(&destination, &file.path)?, 64 << 10).and_then(|bytes| managed::MigrationPlan::parse(&bytes)))
        .transpose()?;
    let mut quotas = managed::ManagedQuotas::default();
    if record.files.iter().any(|file| file.path == "runtime/worker-v2.json") {
        let declaration = photocraft_plugin_api::WorkerV2Descriptor::parse(&read_bounded(
            &checked_path(&destination, "runtime/worker-v2.json")?,
            photocraft_plugin_api::MAX_WORKER_DESCRIPTOR_BYTES,
        )?)
        .map_err(error)?;
        quotas.cache_bytes = quotas.cache_bytes.min(declaration.resources.cache_bytes);
    }
    let staged_state = managed::prepare_with_quotas(root, id, &record.hash, previous.as_ref().map(|record| record.hash.as_str()), migration.as_ref(), quotas)?;
    let reused_state = staged_state.reused() && previous.as_ref().is_none_or(|previous| previous.hash != record.hash);
    staged_state.commit()?;
    let loaded = match load_generation(root, &record) {
        Ok(loaded) => loaded,
        Err(e) => {
            if created {
                let _ = cleanup_leased(root, &record, &destination_lease);
            }
            return Err(e);
        }
    };
    if previous.as_ref().is_none_or(|previous| previous.hash != record.hash) {
        stage_rollback_journal(root, id, &record.hash, previous.as_ref())?;
    }
    // Publish last. Any validation or extraction failure leaves the previous active record intact.
    let published = revoke_activation(root, id).and_then(|activation| {
        publish(&record_path(root, id)?, &record_bytes)?;
        Ok(activation)
    });
    let activation = match published {
        Ok(activation) => activation,
        Err(e) => {
            if created && let Err(cleanup_error) = cleanup_leased(root, &record, &destination_lease) {
                return Err(error(format!("{e}; staged payload cleanup failed: {cleanup_error}")));
            }
            return Err(e);
        }
    };
    // If finalization fails, the matching target-hash journal remains authoritative and
    // preserves rollback across restart. A later mutation finishes it before replacing it.
    let journal_warning = finish_rollback_journal(root, id, Some(&record.hash))
        .err()
        .map(|cause| format!("The plugin was installed; rollback metadata will be finalized on the next update: {cause}"));
    let warning = older
        .filter(|older| older.hash != record.hash && previous.as_ref().is_none_or(|previous| previous.hash != older.hash))
        .and_then(|older| {
            cleanup(root, &older)
                .and_then(|()| {
                    if managed::paths(root, id, &older.hash).is_ok() {
                        managed::remove(root, id, &older.hash)?;
                    }
                    Ok(())
                })
                .err()
        })
        .map(|cause| format!("An older generation is retained until it can be collected: {cause}"))
        .or(journal_warning)
        .or_else(|| {
            repaired
                .map(|path| format!("Repaired damaged plugin metadata; preserved the original at {} and retained any previous payload folders", path.display()))
        })
        .or_else(|| {
            reused_state.then(|| {
                "Selected this code generation's existing managed-data snapshot; it was preserved from an earlier install or interrupted publication.".into()
            })
        });
    Ok(InstalledPayload { native: loaded.native.map(|native| Arc::new(native.reactivated(activation))), enabled, warning })
}

/// Restore a verified prior code/state pair. No guest migration executes during rollback.
#[cfg(test)]
pub(crate) fn rollback(root: &Path, id: &str) -> Result<()> {
    rollback_checked(root, id, |_| Ok(()))
}

pub(crate) fn rollback_checked(root: &Path, id: &str, validate: impl FnOnce(&photocraft_plugins::extension::Manifest) -> Result<()>) -> Result<()> {
    let _store = StoreGuard::write(root)?;
    let current = read_record(root, id)?.ok_or_else(|| error("plugin is not installed"))?;
    let previous = rollback_record(root, id, Some(&current.hash))?.ok_or_else(|| error("no previous plugin installation is available"))?;
    if previous.hash == current.hash {
        return Err(error("no different previous plugin installation is available"));
    }
    let _lease = generation_lease(root, id, &previous.hash, true)?;
    managed::prepare(root, id, &previous.hash, None, None)?.commit()?;
    let loaded = load_generation(root, &previous)?;
    let extension = photocraft_plugins::extension::Extension::load(&loaded.module).map_err(error)?;
    validate(&extension.manifest)?;
    let bytes = serde_json::to_vec(&previous).map_err(error)?;
    stage_rollback_journal(root, id, &previous.hash, Some(&current))?;
    revoke_activation(root, id)?;
    photocraft_format::atomic_write(&record_path(root, id)?, &bytes).map_err(error)?;
    // A matching journal is already the durable next rollback choice. Finalization is
    // optional here; a full disk cannot turn a committed rollback into a false failure.
    let _ = finish_rollback_journal(root, id, Some(&previous.hash));
    Ok(())
}

pub(crate) fn managed_usage(root: &Path, id: &str) -> Result<Value> {
    let _store = StoreGuard::read(root)?;
    let current = read_record(root, id)?.ok_or_else(|| error("plugin is not installed"))?;
    let directory = checked_path(root, &format!("managed/{id}/{}", current.hash))?;
    let usage = if directory.exists() { managed::usage(root, id, &current.hash)? } else { managed::ManagedUsage::default() };
    serde_json::to_value(usage).map_err(error)
}

#[cfg(test)]
pub(crate) fn remove(root: &Path, id: &str) -> Result<Option<String>> {
    remove_with_data(root, id, false)
}

#[cfg(test)]
pub(crate) fn remove_with_data(root: &Path, id: &str, purge_data: bool) -> Result<Option<String>> {
    remove_checked(root, id, purge_data, None)
}

pub(crate) fn remove_checked(root: &Path, id: &str, purge_data: bool, expected: Option<&ReviewGuard>) -> Result<Option<String>> {
    let _store = StoreGuard::write(root)?;
    if let Some(expected) = expected
        && review_guard_inner(root, id)? != *expected
    {
        return Err(error("The installed plugin changed since it was loaded. Restart PhotoCraft before removing it."));
    }
    let (record, corrupt) = previous_record(root, id, true)?;
    if let Some(bytes) = &corrupt {
        preserve_corrupt_record(root, id, bytes)?;
    }
    let previous = rollback_record(root, id, record.as_ref().map(|record| record.hash.as_str()));
    let mut warnings = Vec::new();
    let previous = match previous {
        Ok(previous) => previous,
        Err(cause) => {
            warnings.push(format!("The previous installation could not be verified; its files were retained: {cause}"));
            None
        }
    };
    revoke_activation(root, id)?;
    // Remove a loose module before its authoritative record so it cannot reappear at restart.
    let loose_module = checked_path(root, &format!("{id}.pcext"))?;
    if loose_module.exists() {
        std::fs::remove_file(loose_module).map_err(error)?;
    }
    if let Some(record) = record {
        std::fs::remove_file(record_path(root, id)?).map_err(error)?;
        let cleanup = cleanup(root, &record).and_then(|()| if purge_data { managed::remove(root, id, &record.hash) } else { Ok(()) });
        warnings.extend(cleanup.err().map(|e| e.to_string()));
        if let Some(previous) = previous.filter(|previous| previous.hash != record.hash)
            && let Err(cause) = self::cleanup(root, &previous).and_then(|()| if purge_data { managed::remove(root, id, &previous.hash) } else { Ok(()) })
        {
            warnings.push(cause.to_string());
        }
        if let Err(cause) = crate::extension_cmds::remove_if_exists(&checked_path(root, &format!("{id}.rollback.json"))?) {
            warnings.push(cause.to_string());
        }
        if let Err(cause) = crate::extension_cmds::remove_if_exists(&rollback_journal_path(root, id)?) {
            warnings.push(cause.to_string());
        }
        if let Err(cause) = crate::extension_cmds::remove_if_exists(&checked_path(root, &format!("{id}.disabled"))?) {
            warnings.push(format!("Disabled marker could not be cleaned up: {cause}"));
        }
        return Ok((!warnings.is_empty()).then(|| warnings.join("; ")));
    }
    if corrupt.is_some() {
        std::fs::remove_file(record_path(root, id)?).map_err(error)?;
        warnings.push("Removed the damaged installation record. Its backup, unverified payload and managed data were retained.".into());
    }
    for path in [checked_path(root, &format!("{id}.rollback.json"))?, rollback_journal_path(root, id)?] {
        if let Err(cause) = crate::extension_cmds::remove_if_exists(&path) {
            warnings.push(cause.to_string());
        }
    }
    crate::extension_cmds::remove_if_exists(&checked_path(root, &format!("{id}.disabled"))?)?;
    Ok((!warnings.is_empty()).then(|| warnings.join("; ")))
}

#[cfg(test)]
mod tests;
