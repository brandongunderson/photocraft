//! Host-managed state is selected by the same generation hash as immutable code.
//! Callers hold the store's exclusive mutation lock through prepare, commit and Record
//! publication. Nothing here reads or migrates a plugin's external/native-owned cache.
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Result, checked_path, error, hash_valid, is_link, safe_name, valid_id};

const MARKER: &str = ".photocraft-managed.json";
const STAGE_MARKER: &str = ".photocraft-managed-stage.json";
const MAX_PLAN_BYTES: usize = 64 << 10;
const MAX_MARKER_BYTES: u64 = 8192;
const MAX_OPERATIONS: usize = 64;
const MAX_ENTRIES: usize = 16384;
const MAX_DEPTH: usize = 8;
const MAX_JSON_DEPTH: usize = 16;
const MAX_JSON_NODES: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManagedQuotas {
    pub settings_bytes: u64,
    pub data_bytes: u64,
    pub cache_bytes: u64,
    pub secrets_reference_bytes: u64,
    pub files_per_area: usize,
}

impl Default for ManagedQuotas {
    fn default() -> Self {
        Self { settings_bytes: 1 << 20, data_bytes: 64 << 20, cache_bytes: 128 << 20, secrets_reference_bytes: 64 << 10, files_per_area: 2048 }
    }
}

impl ManagedQuotas {
    fn validate(self) -> Result<()> {
        if !(2..=4 << 20).contains(&self.settings_bytes)
            || self.data_bytes > 1024 << 20
            || self.cache_bytes > 2048 << 20
            || !(2..=1 << 20).contains(&self.secrets_reference_bytes)
            || self.files_per_area > 4096
        {
            return Err(error("managed state quotas exceed host limits"));
        }
        Ok(())
    }

    fn total(self) -> u64 {
        self.settings_bytes.saturating_add(self.data_bytes).saturating_add(self.cache_bytes).saturating_add(self.secrets_reference_bytes)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManagedUsage {
    pub settings_bytes: u64,
    pub data_bytes: u64,
    pub cache_bytes: u64,
    pub secrets_reference_bytes: u64,
    pub data_files: usize,
    pub cache_files: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManagedPaths {
    pub root: PathBuf,
    pub settings: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
    pub secrets_references: PathBuf,
    pub schema_version: u32,
    pub quotas: ManagedQuotas,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MigrationPlan {
    pub format_version: u32,
    pub from_schema: u32,
    pub to_schema: u32,
    #[serde(default)]
    pub settings: Vec<JsonEdit>,
    #[serde(default)]
    pub data: Vec<DataEdit>,
    #[serde(default)]
    pub secrets_references: Vec<ReferenceRename>,
    #[serde(default)]
    pub copy_cache: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum JsonEdit {
    Rename { from: String, to: String },
    SetDefault { key: String, value: Value },
    Remove { key: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum DataEdit {
    Move { from: String, to: String },
    Remove { path: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReferenceRename {
    pub from: String,
    pub to: String,
}

fn key_valid(key: &str) -> bool {
    !key.is_empty() && key.len() <= 128 && key.chars().all(|c| !c.is_control())
}

fn data_name(path: &str) -> Result<()> {
    safe_name(path)?;
    if path.split('/').count() > MAX_DEPTH.saturating_add(1) {
        return Err(error("managed data path exceeds the directory depth limit"));
    }
    Ok(())
}

fn json_shape(value: &Value) -> Result<()> {
    let mut pending = vec![(value, 0usize)];
    let mut count = 0usize;
    while let Some((value, depth)) = pending.pop() {
        count = count.saturating_add(1);
        if depth > MAX_JSON_DEPTH || count > MAX_JSON_NODES {
            return Err(error("managed JSON exceeds its depth or value-count limit"));
        }
        match value {
            Value::Array(values) => pending.extend(values.iter().map(|value| (value, depth.saturating_add(1)))),
            Value::Object(values) => {
                if values.keys().any(|key| !key_valid(key)) {
                    return Err(error("managed JSON contains an invalid object key"));
                }
                pending.extend(values.values().map(|value| (value, depth.saturating_add(1))));
            }
            _ => {}
        }
        if pending.len() > MAX_JSON_NODES {
            return Err(error("managed JSON exceeds its value-count limit"));
        }
    }
    Ok(())
}

impl MigrationPlan {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_PLAN_BYTES {
            return Err(error("managed migration plan exceeds 64 KiB"));
        }
        let plan: Self = serde_json::from_slice(bytes).map_err(error)?;
        plan.validate()?;
        Ok(plan)
    }

    fn validate(&self) -> Result<()> {
        let operations = self.settings.len().checked_add(self.data.len()).and_then(|n| n.checked_add(self.secrets_references.len()));
        if self.format_version != 1 || self.to_schema <= self.from_schema || operations.is_none_or(|count| count > MAX_OPERATIONS) {
            return Err(error("managed migrations require formatVersion 1, an increasing explicit schema and at most 64 operations"));
        }
        for edit in &self.settings {
            match edit {
                JsonEdit::Rename { from, to } => {
                    if !key_valid(from) || !key_valid(to) || from == to {
                        return Err(error("invalid managed settings rename"));
                    }
                }
                JsonEdit::SetDefault { key, value } => {
                    if !key_valid(key) {
                        return Err(error("invalid managed settings default key"));
                    }
                    json_shape(value)?;
                }
                JsonEdit::Remove { key } if !key_valid(key) => return Err(error("invalid managed settings removal key")),
                JsonEdit::Remove { .. } => {}
            }
        }
        for edit in &self.data {
            match edit {
                DataEdit::Move { from, to } => {
                    data_name(from)?;
                    data_name(to)?;
                    if from == to {
                        return Err(error("managed data migration cannot move a file onto itself"));
                    }
                }
                DataEdit::Remove { path } => data_name(path)?,
            }
        }
        for rename in &self.secrets_references {
            if !key_valid(&rename.from) || !key_valid(&rename.to) || rename.from == rename.to {
                return Err(error("invalid managed secret-reference rename"));
            }
        }
        let mut budget = JsonBudget { written: 0, limit: MAX_PLAN_BYTES as u64 };
        serde_json::to_writer(&mut budget, self).map_err(error)?;
        Ok(())
    }

    fn digest(&self) -> Result<String> {
        self.validate()?;
        Ok(blake3::hash(&serde_json::to_vec(self).map_err(error)?).to_hex().to_string())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Marker {
    format_version: u32,
    id: String,
    code_hash: String,
    schema_version: u32,
    source_hash: Option<String>,
    migration_hash: Option<String>,
    quotas: ManagedQuotas,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StageMarker {
    format_version: u32,
    id: String,
    code_hash: String,
    quotas: ManagedQuotas,
}

struct JsonBudget {
    written: u64,
    limit: u64,
}

impl Write for JsonBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.written =
            self.written.checked_add(bytes.len() as u64).filter(|n| *n <= self.limit).ok_or_else(|| std::io::Error::other("managed JSON quota exceeded"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn identity(id: &str, hash: &str) -> Result<()> {
    if !valid_id(id) || !hash_valid(hash) {
        return Err(error("invalid managed plugin identity or generation hash"));
    }
    Ok(())
}

fn generation(root: &Path, id: &str, hash: &str) -> Result<PathBuf> {
    identity(id, hash)?;
    checked_path(root, &format!("managed/{id}/{hash}"))
}

fn file_bytes(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path).map_err(error)?;
    if is_link(&meta) || !meta.is_file() || meta.len() > limit {
        return Err(error(format!("managed file must be regular and within quota: {}", path.display())));
    }
    let file = File::open(path).map_err(error)?;
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes).map_err(error)?;
    if bytes.len() as u64 > limit {
        return Err(error("managed file grew beyond its quota"));
    }
    Ok(bytes)
}

fn read_marker(base: &Path, id: &str, hash: &str) -> Result<Marker> {
    let marker: Marker = serde_json::from_slice(&file_bytes(&checked_path(base, MARKER)?, MAX_MARKER_BYTES)?).map_err(error)?;
    if marker.format_version != 1
        || marker.id != id
        || marker.code_hash != hash
        || marker.source_hash.as_deref().is_some_and(|value| !hash_valid(value))
        || marker.migration_hash.as_deref().is_some_and(|value| !hash_valid(value))
    {
        return Err(error("managed generation marker identity mismatch"));
    }
    marker.quotas.validate()?;
    Ok(marker)
}

struct Tree {
    files: Vec<(String, u64)>,
    directories: Vec<String>,
    bytes: u64,
}

fn tree(base: &Path, limit: u64, files_limit: usize) -> Result<Tree> {
    scan_tree(base, limit, files_limit, MAX_DEPTH)
}

fn scan_tree(base: &Path, limit: u64, files_limit: usize, max_depth: usize) -> Result<Tree> {
    let metadata = std::fs::symlink_metadata(base).map_err(error)?;
    if is_link(&metadata) || !metadata.is_dir() {
        return Err(error("managed area must be a real directory"));
    }
    let mut result = Tree { files: Vec::new(), directories: Vec::new(), bytes: 0 };
    let mut pending = vec![(base.to_path_buf(), 0usize)];
    let mut entries = 0usize;
    while let Some((directory, depth)) = pending.pop() {
        for entry in std::fs::read_dir(directory).map_err(error)? {
            let entry = entry.map_err(error)?;
            entries = entries.saturating_add(1);
            if entries > MAX_ENTRIES {
                return Err(error("managed area has too many filesystem entries"));
            }
            let relative = entry.path().strip_prefix(base).map_err(error)?.to_str().ok_or_else(|| error("managed paths must be UTF-8"))?.replace('\\', "/");
            let path = checked_path(base, &relative)?;
            let metadata = std::fs::symlink_metadata(&path).map_err(error)?;
            if is_link(&metadata) {
                return Err(error("managed state cannot contain links or reparse points"));
            }
            if metadata.is_dir() {
                if depth >= max_depth {
                    return Err(error("managed area exceeds eight directory levels"));
                }
                result.directories.push(relative);
                pending.push((path, depth.saturating_add(1)));
            } else if metadata.is_file() {
                result.bytes =
                    result.bytes.checked_add(metadata.len()).filter(|bytes| *bytes <= limit).ok_or_else(|| error("managed area byte quota exceeded"))?;
                if result.files.len() >= files_limit {
                    return Err(error("managed area file quota exceeded"));
                }
                result.files.push((relative, metadata.len()));
            } else {
                return Err(error("managed state supports only regular files and directories"));
            }
        }
    }
    result.files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

fn settings(base: &Path, quotas: ManagedQuotas) -> Result<Value> {
    let value: Value = serde_json::from_slice(&file_bytes(&checked_path(base, "settings.json")?, quotas.settings_bytes)?).map_err(error)?;
    if !value.is_object() {
        return Err(error("managed settings must be a JSON object"));
    }
    json_shape(&value)?;
    Ok(value)
}

fn references(base: &Path, quotas: ManagedQuotas) -> Result<BTreeMap<String, String>> {
    let references: BTreeMap<String, String> =
        serde_json::from_slice(&file_bytes(&checked_path(base, "secrets-refs.json")?, quotas.secrets_reference_bytes)?).map_err(error)?;
    if references.len() > 256
        || references.iter().any(|(key, reference)| {
            !key_valid(key)
                || reference.len() > 256
                || !reference
                    .strip_prefix("photocraft-secret:")
                    .is_some_and(|token| !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte)))
        })
    {
        return Err(error("managed secret references must be bounded opaque photocraft-secret: identifiers, never credential payloads"));
    }
    Ok(references)
}

fn validate(base: &Path, marker: &Marker, quotas: ManagedQuotas) -> Result<ManagedUsage> {
    quotas.validate()?;
    let _ = settings(base, quotas)?;
    let _ = references(base, quotas)?;
    let data = tree(&checked_path(base, "data")?, quotas.data_bytes, quotas.files_per_area)?;
    let cache = tree(&checked_path(base, "cache")?, quotas.cache_bytes, quotas.files_per_area)?;
    let entries = data.files.len().saturating_add(data.directories.len()).saturating_add(cache.files.len()).saturating_add(cache.directories.len());
    if entries > MAX_ENTRIES.saturating_sub(6) {
        return Err(error("managed generation has too many combined filesystem entries"));
    }
    let allowed = [MARKER, STAGE_MARKER, "settings.json", "secrets-refs.json", "data", "cache"];
    for entry in std::fs::read_dir(base).map_err(error)?.take(allowed.len().saturating_add(1)) {
        let entry = entry.map_err(error)?;
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|name| allowed.contains(name)) else {
            return Err(error("managed generation contains an unknown root entry"));
        };
        let metadata = std::fs::symlink_metadata(checked_path(base, name)?).map_err(error)?;
        if is_link(&metadata)
            || (matches!(name, "data" | "cache") && !metadata.is_dir())
            || (!matches!(name, "data" | "cache") && !metadata.is_file())
            || (matches!(name, MARKER | STAGE_MARKER) && metadata.len() > MAX_MARKER_BYTES)
        {
            return Err(error("managed generation root contains an invalid entry"));
        }
    }
    if marker.format_version != 1 {
        return Err(error("unsupported managed generation format"));
    }
    Ok(ManagedUsage {
        settings_bytes: std::fs::metadata(checked_path(base, "settings.json")?).map_err(error)?.len(),
        secrets_reference_bytes: std::fs::metadata(checked_path(base, "secrets-refs.json")?).map_err(error)?.len(),
        data_bytes: data.bytes,
        cache_bytes: cache.bytes,
        data_files: data.files.len(),
        cache_files: cache.files.len(),
    })
}

fn make_paths(base: PathBuf, schema_version: u32, quotas: ManagedQuotas) -> Result<ManagedPaths> {
    Ok(ManagedPaths {
        settings: checked_path(&base, "settings.json")?,
        data: checked_path(&base, "data")?,
        cache: checked_path(&base, "cache")?,
        secrets_references: checked_path(&base, "secrets-refs.json")?,
        root: base,
        schema_version,
        quotas,
    })
}

/// Resolving a path does not grant authority to a guest; the broker must separately bind it
/// to a host-injected principal and validate the selected generation under a store lease.
pub(crate) fn paths(root: &Path, id: &str, hash: &str) -> Result<ManagedPaths> {
    let base = generation(root, id, hash)?;
    let marker = read_marker(&base, id, hash)?;
    validate(&base, &marker, marker.quotas)?;
    make_paths(base, marker.schema_version, marker.quotas)
}

/// Discovery reads only bounded ownership metadata and the four expected path types.
/// Activation must use `paths` to validate JSON, quotas and descendant files before use.
pub(crate) fn metadata(root: &Path, id: &str, hash: &str) -> Result<ManagedPaths> {
    let base = generation(root, id, hash)?;
    let marker = read_marker(&base, id, hash)?;
    let paths = make_paths(base, marker.schema_version, marker.quotas)?;
    for (path, directory) in [(&paths.root, true), (&paths.data, true), (&paths.cache, true), (&paths.settings, false), (&paths.secrets_references, false)] {
        let metadata = std::fs::symlink_metadata(path).map_err(error)?;
        if is_link(&metadata) || (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
            return Err(error("managed metadata paths must be real directories or regular files"));
        }
    }
    Ok(paths)
}

pub(crate) fn usage(root: &Path, id: &str, hash: &str) -> Result<ManagedUsage> {
    let base = generation(root, id, hash)?;
    let marker = read_marker(&base, id, hash)?;
    validate(&base, &marker, marker.quotas)
}

/// Prepared state is not selected until the caller publishes the code's active Record.
#[must_use]
pub(crate) struct StagedManaged {
    stage: Option<tempfile::TempDir>,
    destination: PathBuf,
    marker: Marker,
}

impl StagedManaged {
    pub(crate) fn reused(&self) -> bool {
        self.stage.is_none()
    }

    pub(crate) fn commit(self) -> Result<ManagedPaths> {
        if let Some(stage) = &self.stage {
            validate(stage.path(), &self.marker, self.marker.quotas)?;
            let marker = checked_path(stage.path(), MARKER)?;
            let mut output = OpenOptions::new().write(true).create_new(true).open(&marker).map_err(error)?;
            output.write_all(&serde_json::to_vec(&self.marker).map_err(error)?).map_err(error)?;
            photocraft_format::atomic::sync_file(&output).map_err(error)?;
            drop(output);
            let tree = scan_tree(stage.path(), self.marker.quotas.total().saturating_add(2 * MAX_MARKER_BYTES), MAX_ENTRIES, MAX_DEPTH.saturating_add(1))?;
            for path in tree.directories.iter().rev() {
                sync_directory(&checked_path(stage.path(), path)?)?;
            }
            sync_directory(stage.path())?;
            // Destination was checked before staging. Never replace an existing state.
            if self.destination.exists() {
                return Err(error("managed destination appeared during migration; retry under the store mutation lock"));
            }
            std::fs::rename(stage.path(), &self.destination).map_err(error)?;
            if let Some(parent) = self.destination.parent() {
                sync_directory(parent)?;
            }
        } else {
            let current = read_marker(&self.destination, &self.marker.id, &self.marker.code_hash)?;
            validate(&self.destination, &current, self.marker.quotas)?;
        }
        make_paths(self.destination, self.marker.schema_version, self.marker.quotas)
    }
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        photocraft_format::atomic::sync_file(&File::open(path).map_err(error)?).map_err(error)
    }
    #[cfg(not(unix))]
    {
        // Windows' safe standard file API cannot open a directory for a flush.
        let _ = path;
        Ok(())
    }
}

struct CopyBudget {
    written: u64,
    fail_after: Option<u64>,
}

impl CopyBudget {
    fn write(&mut self, output: &mut File, bytes: &[u8]) -> Result<()> {
        self.written = self.written.checked_add(bytes.len() as u64).ok_or_else(|| error("managed staging size overflow"))?;
        if self.fail_after.is_some_and(|limit| self.written > limit) {
            return Err(error("simulated full disk during managed staging"));
        }
        output.write_all(bytes).map_err(error)
    }
}

fn write_json(base: &Path, name: &str, value: &impl Serialize, limit: u64, budget: &mut CopyBudget) -> Result<()> {
    let mut count = JsonBudget { written: 0, limit };
    serde_json::to_writer(&mut count, value).map_err(error)?;
    let bytes = serde_json::to_vec(value).map_err(error)?;
    let mut output = OpenOptions::new().write(true).create_new(true).open(checked_path(base, name)?).map_err(error)?;
    budget.write(&mut output, &bytes)?;
    photocraft_format::atomic::sync_file(&output).map_err(error)
}

fn copy_tree(source: &Path, destination: &Path, limit: u64, files_limit: usize, budget: &mut CopyBudget) -> Result<()> {
    let snapshot = tree(source, limit, files_limit)?;
    for directory in &snapshot.directories {
        std::fs::create_dir_all(checked_path(destination, directory)?).map_err(error)?;
    }
    for (relative, size) in snapshot.files {
        let source = checked_path(source, &relative)?;
        let target = checked_path(destination, &relative)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(error)?;
        }
        let mut input = File::open(source).map_err(error)?.take(size.saturating_add(1));
        let mut output = OpenOptions::new().write(true).create_new(true).open(target).map_err(error)?;
        let mut copied = 0u64;
        let mut chunk = [0u8; 64 << 10];
        loop {
            let length = input.read(&mut chunk).map_err(error)?;
            if length == 0 {
                break;
            }
            copied = copied.checked_add(length as u64).filter(|copied| *copied <= size).ok_or_else(|| error("managed source changed during migration"))?;
            budget.write(&mut output, chunk.get(..length).ok_or_else(|| error("invalid managed copy length"))?)?;
        }
        if copied != size {
            return Err(error("managed source changed during migration"));
        }
        photocraft_format::atomic::sync_file(&output).map_err(error)?;
    }
    Ok(())
}

fn apply(plan: &MigrationPlan, settings: &mut Map<String, Value>, references: &mut BTreeMap<String, String>, stage: &Path) -> Result<()> {
    for edit in &plan.settings {
        match edit {
            JsonEdit::Rename { from, to } => {
                if settings.contains_key(to) {
                    return Err(error("managed settings rename destination already exists"));
                }
                let value = settings.remove(from).ok_or_else(|| error("managed settings rename source is missing"))?;
                settings.insert(to.clone(), value);
            }
            JsonEdit::SetDefault { key, value } => {
                settings.entry(key.clone()).or_insert_with(|| value.clone());
            }
            JsonEdit::Remove { key } => {
                settings.remove(key);
            }
        }
    }
    for rename in &plan.secrets_references {
        if references.contains_key(&rename.to) {
            return Err(error("managed secret-reference destination already exists"));
        }
        let value = references.remove(&rename.from).ok_or_else(|| error("managed secret-reference source is missing"))?;
        references.insert(rename.to.clone(), value);
    }
    let data = checked_path(stage, "data")?;
    for edit in &plan.data {
        match edit {
            DataEdit::Move { from, to } => {
                let source = checked_path(&data, from)?;
                let destination = checked_path(&data, to)?;
                if !std::fs::symlink_metadata(&source).map_err(error)?.is_file() || destination.exists() {
                    return Err(error("managed data move requires a regular source and a fresh destination"));
                }
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent).map_err(error)?;
                }
                std::fs::rename(source, destination).map_err(error)?;
            }
            DataEdit::Remove { path } => {
                let path = checked_path(&data, path)?;
                match std::fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.is_file() && !is_link(&metadata) => std::fs::remove_file(path).map_err(error)?,
                    Ok(_) => return Err(error("managed data removal accepts only regular files")),
                    Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
                    Err(cause) => return Err(error(cause)),
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn prepare(root: &Path, id: &str, hash: &str, source_hash: Option<&str>, plan: Option<&MigrationPlan>) -> Result<StagedManaged> {
    prepare_with_quotas(root, id, hash, source_hash, plan, ManagedQuotas::default())
}

pub(crate) fn prepare_with_quotas(
    root: &Path,
    id: &str,
    hash: &str,
    source_hash: Option<&str>,
    plan: Option<&MigrationPlan>,
    quotas: ManagedQuotas,
) -> Result<StagedManaged> {
    prepare_inner(root, id, hash, source_hash, plan, quotas, None)
}

fn prepare_inner(
    root: &Path,
    id: &str,
    hash: &str,
    source_hash: Option<&str>,
    plan: Option<&MigrationPlan>,
    quotas: ManagedQuotas,
    fail_after: Option<u64>,
) -> Result<StagedManaged> {
    identity(id, hash)?;
    quotas.validate()?;
    if let Some(source) = source_hash {
        identity(id, source)?;
    }
    let migration_hash = plan.map(MigrationPlan::digest).transpose()?;
    let destination = generation(root, id, hash)?;
    if destination.exists() {
        let marker = read_marker(&destination, id, hash)?;
        validate(&destination, &marker, quotas)?;
        if let Some(plan) = plan
            && (marker.schema_version != plan.to_schema || marker.migration_hash != migration_hash)
        {
            return Err(error("existing managed generation has a different migration/schema; it will not be overwritten"));
        }
        return Ok(StagedManaged { stage: None, destination, marker });
    }
    let source = source_hash.map(|source| generation(root, id, source)).transpose()?.filter(|path| path.exists());
    let source_marker = source.as_ref().zip(source_hash).map(|(path, hash)| read_marker(path, id, hash)).transpose()?;
    if let (Some(source), Some(marker)) = (&source, &source_marker) {
        validate(source, marker, marker.quotas)?;
    }
    let source_schema = source_marker.as_ref().map_or(0, |marker| marker.schema_version);
    if plan.is_some_and(|plan| plan.from_schema != source_schema) {
        return Err(error("managed migration fromSchema does not match the source generation"));
    }
    let parent = checked_path(root, &format!("managed/{id}"))?;
    std::fs::create_dir_all(&parent).map_err(error)?;
    let _ = recover_staging(root, id)?;
    let stage = tempfile::Builder::new().prefix("staging-").tempdir_in(&parent).map_err(error)?;
    let marker = Marker {
        format_version: 1,
        id: id.into(),
        code_hash: hash.into(),
        schema_version: plan.map_or(source_schema, |plan| plan.to_schema),
        source_hash: source_hash.map(str::to_owned),
        migration_hash,
        quotas,
    };
    let mut budget = CopyBudget { written: 0, fail_after };
    write_json(stage.path(), STAGE_MARKER, &StageMarker { format_version: 1, id: id.into(), code_hash: hash.into(), quotas }, MAX_MARKER_BYTES, &mut budget)?;
    std::fs::create_dir(checked_path(stage.path(), "data")?).map_err(error)?;
    std::fs::create_dir(checked_path(stage.path(), "cache")?).map_err(error)?;
    let source_quotas = source_marker.as_ref().map_or(quotas, |marker| marker.quotas);
    let mut settings = source.as_ref().map(|source| settings(source, source_quotas)).transpose()?.unwrap_or_else(|| Value::Object(Map::new()));
    let mut references = source.as_ref().map(|source| references(source, source_quotas)).transpose()?.unwrap_or_default();
    if let Some(source) = &source {
        copy_tree(&checked_path(source, "data")?, &checked_path(stage.path(), "data")?, quotas.data_bytes, quotas.files_per_area, &mut budget)?;
        if plan.is_some_and(|plan| plan.copy_cache) {
            copy_tree(&checked_path(source, "cache")?, &checked_path(stage.path(), "cache")?, quotas.cache_bytes, quotas.files_per_area, &mut budget)?;
        }
    }
    if let Some(plan) = plan {
        apply(plan, settings.as_object_mut().ok_or_else(|| error("managed settings object is missing"))?, &mut references, stage.path())?;
    }
    write_json(stage.path(), "settings.json", &settings, quotas.settings_bytes, &mut budget)?;
    write_json(stage.path(), "secrets-refs.json", &references, quotas.secrets_reference_bytes, &mut budget)?;
    validate(stage.path(), &marker, quotas)?;
    Ok(StagedManaged { stage: Some(stage), destination, marker })
}

/// Remove only a validated, exclusively leased managed generation. No native cache paths,
/// installation payloads or sibling plugins are inspected or deleted.
pub(crate) fn remove(root: &Path, id: &str, hash: &str) -> Result<()> {
    let base = generation(root, id, hash)?;
    if !base.exists() {
        return Ok(());
    }
    let marker = read_marker(&base, id, hash)?;
    validate(&base, &marker, marker.quotas)?;
    remove_owned_tree(&base, marker.quotas.total().saturating_add(2 * MAX_MARKER_BYTES))
}

fn remove_owned_tree(base: &Path, maximum: u64) -> Result<()> {
    let inventory = scan_tree(base, maximum, MAX_ENTRIES, MAX_DEPTH.saturating_add(1))?;
    // Keep recovery ownership until all payloads and directories have been removed.
    // Interrupted staging cleanup can safely identify and retry this tree.
    let (mut markers, payloads): (Vec<_>, Vec<_>) = inventory.files.into_iter().partition(|(path, _)| path == MARKER || path == STAGE_MARKER);
    markers.sort_by_key(|(path, _)| path == STAGE_MARKER);
    for (path, _) in payloads {
        std::fs::remove_file(checked_path(base, &path)?).map_err(error)?;
    }
    let mut directories = inventory.directories;
    directories.sort_by_key(|path| std::cmp::Reverse(path.split('/').count()));
    for path in directories {
        std::fs::remove_dir(checked_path(base, &path)?).map_err(error)?;
    }
    for (path, _) in markers {
        std::fs::remove_file(checked_path(base, &path)?).map_err(error)?;
    }
    std::fs::remove_dir(base).map_err(error)
}

/// Bounded recovery of abandoned directories carrying our staging ownership marker.
/// Unknown directories and malformed markers are preserved rather than guessed/deleted.
pub(crate) fn recover_staging(root: &Path, id: &str) -> Result<usize> {
    if !valid_id(id) {
        return Err(error("invalid managed plugin id"));
    }
    let parent = checked_path(root, &format!("managed/{id}"))?;
    if !parent.exists() {
        return Ok(0);
    }
    let mut removed = 0usize;
    let mut count = 0usize;
    for entry in std::fs::read_dir(&parent).map_err(error)? {
        let entry = entry.map_err(error)?;
        count = count.saturating_add(1);
        if count > 4096 {
            return Err(error("managed generation recovery exceeds 4096 entries"));
        }
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|name| name.starts_with("staging-")) else { continue };
        let stage = checked_path(&parent, name)?;
        let metadata = std::fs::symlink_metadata(&stage).map_err(error)?;
        if !metadata.is_dir() || is_link(&metadata) {
            continue;
        }
        let parsed = (|| -> Result<StageMarker> {
            let marker = serde_json::from_slice::<StageMarker>(&file_bytes(&checked_path(&stage, STAGE_MARKER)?, MAX_MARKER_BYTES)?).map_err(error)?;
            if marker.format_version != 1 || marker.id != id || !hash_valid(&marker.code_hash) {
                return Err(error("staging ownership mismatch"));
            }
            marker.quotas.validate()?;
            Ok(marker)
        })();
        let Ok(marker) = parsed else { continue };
        remove_owned_tree(&stage, marker.quotas.total().saturating_add(2 * MAX_MARKER_BYTES))?;
        removed = removed.saturating_add(1);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "org.example.storage";
    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    fn initial(root: &Path) -> ManagedPaths {
        let paths = prepare(root, ID, A, None, None).unwrap().commit().unwrap();
        std::fs::write(&paths.settings, br#"{"old":3,"keep":true}"#).unwrap();
        std::fs::write(paths.data.join("input.bin"), b"owned payload").unwrap();
        std::fs::write(paths.cache.join("cache.bin"), b"discardable").unwrap();
        std::fs::write(&paths.secrets_references, br#"{"account":"photocraft-secret:opaque-123"}"#).unwrap();
        paths
    }

    fn migration() -> MigrationPlan {
        MigrationPlan::parse(
            br#"{"formatVersion":1,"fromSchema":0,"toSchema":1,
                "settings":[{"op":"rename","from":"old","to":"new"},{"op":"setDefault","key":"added","value":[1,2]}],
                "data":[{"op":"move","from":"input.bin","to":"v1/output.bin"}],
                "secretsReferences":[{"from":"account","to":"login"}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn complete_state_is_published_before_the_callers_active_record() {
        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join("selected-record");
        std::fs::write(&selected, A).unwrap();
        let prepared = prepare(root.path(), ID, A, None, None).unwrap();
        assert!(!prepared.reused());
        assert!(!generation(root.path(), ID, A).unwrap().exists());
        let ready = prepared.commit().unwrap();
        assert_eq!(ready.schema_version, 0);
        assert!(ready.root.join(MARKER).is_file());
        assert_eq!(std::fs::read_to_string(selected).unwrap(), A);
        assert_eq!(usage(root.path(), ID, A).unwrap().data_bytes, 0);
    }

    #[test]
    fn migration_copies_settings_data_and_opaque_refs_without_changing_the_backup() {
        let root = tempfile::tempdir().unwrap();
        let old = initial(root.path());
        let next = prepare(root.path(), ID, B, Some(A), Some(&migration())).unwrap().commit().unwrap();
        assert_eq!(next.schema_version, 1);
        assert_eq!(settings(&next.root, next.quotas).unwrap(), json!({"new":3,"keep":true,"added":[1,2]}));
        assert_eq!(std::fs::read(next.data.join("v1/output.bin")).unwrap(), b"owned payload");
        assert!(!next.data.join("input.bin").exists());
        assert!(std::fs::read_dir(&next.cache).unwrap().next().is_none());
        assert_eq!(references(&next.root, next.quotas).unwrap().get("login").unwrap(), "photocraft-secret:opaque-123");
        assert_eq!(settings(&old.root, old.quotas).unwrap(), json!({"old":3,"keep":true}));
        assert!(old.data.join("input.bin").is_file());
        assert!(old.cache.join("cache.bin").is_file());
        assert_eq!(paths(root.path(), ID, A).unwrap().schema_version, 0, "rollback selects the old state without a reverse migration");
    }

    #[test]
    fn identical_reinstall_and_a_b_a_preserve_existing_states() {
        let root = tempfile::tempdir().unwrap();
        let old = initial(root.path());
        prepare(root.path(), ID, B, Some(A), Some(&migration())).unwrap().commit().unwrap();
        std::fs::write(&old.settings, br#"{"userChange":9}"#).unwrap();
        let rollback = prepare(root.path(), ID, A, Some(B), None).unwrap();
        assert!(rollback.reused());
        rollback.commit().unwrap();
        assert_eq!(settings(&old.root, old.quotas).unwrap(), json!({"userChange":9}));
        let repeated = prepare(root.path(), ID, B, Some(A), Some(&migration())).unwrap();
        assert!(repeated.reused(), "the migration is not rerun against new A state");
        repeated.commit().unwrap();
        assert_eq!(paths(root.path(), ID, B).unwrap().schema_version, 1);
    }

    #[test]
    fn legacy_missing_managed_state_is_empty_but_corrupt_existing_state_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let ready = prepare(root.path(), ID, B, Some(A), None).unwrap().commit().unwrap();
        assert_eq!(ready.schema_version, 0);
        assert_eq!(settings(&ready.root, ready.quotas).unwrap(), json!({}));
        let corrupt = generation(root.path(), ID, A).unwrap();
        std::fs::create_dir_all(&corrupt).unwrap();
        std::fs::write(corrupt.join("settings.json"), b"{}").unwrap();
        assert!(prepare(root.path(), ID, C, Some(A), None).is_err());
        assert!(!generation(root.path(), ID, C).unwrap().exists());
    }

    #[test]
    fn mismatched_schema_and_unsupported_operations_leave_the_source_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let old = initial(root.path());
        let mut plan = migration();
        plan.from_schema = 2;
        plan.to_schema = 3;
        assert!(prepare(root.path(), ID, B, Some(A), Some(&plan)).is_err());
        assert!(MigrationPlan::parse(br#"{"formatVersion":1,"fromSchema":0,"toSchema":1,"run":"worker.exe"}"#).is_err());
        assert!(MigrationPlan::parse(br#"{"formatVersion":1,"fromSchema":0,"toSchema":1,"data":[{"op":"move","from":"../outside","to":"x"}]}"#).is_err());
        assert_eq!(settings(&old.root, old.quotas).unwrap(), json!({"old":3,"keep":true}));
    }

    #[test]
    fn late_migration_failure_commits_nothing() {
        let root = tempfile::tempdir().unwrap();
        let old = initial(root.path());
        let mut plan = migration();
        plan.settings.push(JsonEdit::Rename { from: "missing".into(), to: "failure".into() });
        assert!(prepare(root.path(), ID, B, Some(A), Some(&plan)).is_err());
        assert_eq!(settings(&old.root, old.quotas).unwrap(), json!({"old":3,"keep":true}));
        assert!(!generation(root.path(), ID, B).unwrap().exists());
        assert_eq!(recover_staging(root.path(), ID).unwrap(), 0, "normal failure drops its own staging directory");
    }

    #[test]
    fn simulated_full_disk_and_quota_failure_preserve_old_working_state() {
        let root = tempfile::tempdir().unwrap();
        let old = initial(root.path());
        let selected = root.path().join("selected-record");
        std::fs::write(&selected, A).unwrap();
        let failure = prepare_inner(root.path(), ID, B, Some(A), Some(&migration()), ManagedQuotas::default(), Some(250)).err().unwrap().to_string();
        assert!(failure.contains("simulated full disk"), "{failure}");
        let quotas = ManagedQuotas { data_bytes: 1, ..ManagedQuotas::default() };
        assert!(prepare_with_quotas(root.path(), ID, B, Some(A), None, quotas).is_err());
        assert_eq!(std::fs::read_to_string(&selected).unwrap(), A);
        assert_eq!(std::fs::read(old.data.join("input.bin")).unwrap(), b"owned payload");
        assert!(!generation(root.path(), ID, B).unwrap().exists());
    }

    #[test]
    fn interruption_is_ignored_recovered_and_retried_without_touching_unknown_dirs() {
        let root = tempfile::tempdir().unwrap();
        let old = initial(root.path());
        let prepared = prepare(root.path(), ID, B, Some(A), Some(&migration())).unwrap();
        let abandoned = prepared.stage.as_ref().unwrap().path().to_path_buf();
        std::mem::forget(prepared);
        let unknown = abandoned.parent().unwrap().join("staging-unknown");
        std::fs::create_dir(&unknown).unwrap();
        std::fs::write(unknown.join("do-not-delete"), b"unknown owner").unwrap();
        let retry = prepare(root.path(), ID, B, Some(A), Some(&migration())).unwrap();
        assert!(!abandoned.exists());
        retry.commit().unwrap();
        assert!(unknown.join("do-not-delete").is_file());
        assert!(old.data.join("input.bin").is_file());
    }

    #[test]
    fn repeated_ready_access_performs_no_write_or_migration() {
        let root = tempfile::tempdir().unwrap();
        let old = initial(root.path());
        let before = std::fs::read(old.root.join(MARKER)).unwrap();
        let guarded = [old.root.join(MARKER), old.root.join(STAGE_MARKER), old.settings.clone(), old.secrets_references.clone()]
            .into_iter()
            .map(|path| {
                let original = std::fs::metadata(&path).unwrap().permissions();
                let mut readonly = original.clone();
                readonly.set_readonly(true);
                std::fs::set_permissions(&path, readonly).unwrap();
                (path, original)
            })
            .collect::<Vec<_>>();
        let result = prepare(root.path(), ID, A, Some(A), None).and_then(|ready| {
            assert!(ready.reused());
            ready.commit()
        });
        for (path, original) in guarded {
            std::fs::set_permissions(path, original).unwrap();
        }
        result.unwrap();
        assert_eq!(std::fs::read(old.root.join(MARKER)).unwrap(), before);
        assert_eq!(settings(&old.root, old.quotas).unwrap(), json!({"old":3,"keep":true}));
    }

    #[test]
    fn data_migration_paths_are_bounded_before_creating_directories() {
        let mut plan = migration();
        plan.data = vec![DataEdit::Move { from: "input.bin".into(), to: format!("{}output.bin", "nested/".repeat(MAX_DEPTH.saturating_add(1))) }];
        assert!(plan.validate().is_err());
        plan.data = vec![DataEdit::Remove { path: "../outside".into() }];
        assert!(plan.validate().is_err());
        let root = tempfile::tempdir().unwrap();
        let old = initial(root.path());
        let ready = prepare(root.path(), ID, A, Some(A), None).unwrap();
        assert!(ready.reused());
        ready.commit().unwrap();
        assert_eq!(settings(&old.root, old.quotas).unwrap(), json!({"old":3,"keep":true}));
    }

    #[test]
    fn discovery_reads_metadata_without_loading_settings_or_descendants() {
        let root = tempfile::tempdir().unwrap();
        let ready = initial(root.path());
        std::fs::write(&ready.settings, b"not valid JSON").unwrap();
        std::fs::write(ready.data.join("unopened.bin"), b"contents are not read during discovery").unwrap();
        let metadata = metadata(root.path(), ID, A).unwrap();
        assert_eq!(metadata.root, ready.root);
        assert_eq!(metadata.quotas, ManagedQuotas::default());
        assert!(paths(root.path(), ID, A).is_err(), "activation still validates settings");
        std::fs::remove_file(&ready.settings).unwrap();
        std::fs::create_dir(&ready.settings).unwrap();
        assert!(super::metadata(root.path(), ID, A).is_err(), "discovery rejects invalid root path types");
    }

    #[test]
    fn cache_copy_is_explicit_and_separate_quotas_are_enforced() {
        let root = tempfile::tempdir().unwrap();
        initial(root.path());
        let mut plan = migration();
        plan.copy_cache = true;
        let ready = prepare(root.path(), ID, B, Some(A), Some(&plan)).unwrap().commit().unwrap();
        assert_eq!(std::fs::read(ready.cache.join("cache.bin")).unwrap(), b"discardable");
        let usage = usage(root.path(), ID, B).unwrap();
        assert_eq!(usage.data_bytes, 13);
        assert_eq!(usage.cache_bytes, 11);
        assert_eq!(usage.data_files, 1);
        assert_eq!(usage.cache_files, 1);
        assert!(usage.settings_bytes > 0 && usage.secrets_reference_bytes > 0);
        let cold_cache = ManagedQuotas { cache_bytes: 0, ..ManagedQuotas::default() };
        let next = prepare_with_quotas(root.path(), ID, C, Some(B), None, cold_cache).unwrap().commit().unwrap();
        assert!(std::fs::read_dir(next.cache).unwrap().next().is_none());
    }

    #[test]
    fn literal_credentials_and_excessive_json_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let ready = initial(root.path());
        std::fs::write(&ready.secrets_references, br#"{"account":"literal-password"}"#).unwrap();
        assert!(paths(root.path(), ID, A).is_err());
        let mut value = json!(0);
        for _ in 0..18 {
            value = json!([value]);
        }
        assert!(json_shape(&value).is_err());
    }

    #[test]
    fn explicit_removal_is_scoped_and_preserves_legacy_external_cache() {
        let root = tempfile::tempdir().unwrap();
        let old = initial(root.path());
        let sibling = prepare(root.path(), "org.example.neighbor", A, None, None).unwrap().commit().unwrap();
        let external = root.path().join("legacy-native-cache");
        std::fs::create_dir(&external).unwrap();
        std::fs::write(external.join("user-model.bin"), b"legacy").unwrap();
        remove(root.path(), ID, A).unwrap();
        assert!(!old.root.exists());
        assert!(sibling.root.is_dir());
        assert_eq!(std::fs::read(external.join("user-model.bin")).unwrap(), b"legacy");
        assert!(remove(root.path(), "../outside", A).is_err());
    }
}
