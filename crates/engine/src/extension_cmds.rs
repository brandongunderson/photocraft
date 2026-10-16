//! Installable, sandboxed extensions with native panels. Extension state belongs to a session;
//! the desktop attaches a persistent store. All edits dispatch through the regular engine gate.
use std::collections::BTreeMap;
use std::sync::Arc;

use photocraft_plugins::extension::{Extension, MAX_JSON_BYTES, MAX_MODULE_BYTES, Manifest};
use serde_json::{Value, json};

use crate::{EngineError, Result, Session, commands::CommandSpec};

/// Validated payload bytes, cached at installation or startup rather than scanning the store
/// while drawing the plugin manager. Model assets are included in the installed total.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageSize {
    pub installed_bytes: u64,
    pub model_bytes: u64,
}

impl StorageSize {
    fn for_module(bytes: &[u8]) -> Self {
        Self { installed_bytes: bytes.len() as u64, model_bytes: 0 }
    }
}

pub struct Installed {
    pub extension: Arc<Extension>,
    pub enabled: bool,
    pub state: Value,
    pub storage: StorageSize,
    /// Package-owned filters never enter the process-wide standalone filter registry.
    pub filters: BTreeMap<String, Arc<photocraft_plugins::Plugin>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub package: Option<crate::extension_package::PackageInfo>,
    #[cfg(not(target_arch = "wasm32"))]
    pub native: Option<Arc<crate::extension_package::InstalledNative>>,
}

#[derive(Default)]
pub struct ExtensionHost {
    pub(crate) platform: crate::plugin_host::Platform,
    pub installed: BTreeMap<String, Installed>,
    /// Bounded image comparisons are dropped when their installation changes.
    pub(crate) previews: crate::plugin_preview::PreviewStore,
    #[cfg(not(target_arch = "wasm32"))]
    pub folder: Option<std::path::PathBuf>,
    pub load_errors: Vec<String>,
    pub revision: u64,
    /// Revocable tokens for document-independent native tasks. Disabling an installation
    /// invalidates an earlier result even if the same installation is enabled again.
    pub(crate) task_epochs: BTreeMap<String, Arc<()>>,
}

fn error(e: impl std::fmt::Display) -> EngineError {
    EngineError::Other(e.to_string())
}

/// Deliberately explicit: adding a command here is an API/security review. No file access,
/// preferences, automation, action playback, extension management or indirect command execution.
pub const COMMANDS: &[&str] = &[
    "edit.undo",
    "edit.redo",
    "edit.fill",
    "edit.stroke",
    "edit.clear",
    "image.imageSize",
    "plugin.processImage",
    "plugin.runTask",
    "image.canvasSize",
    "image.crop",
    "image.trim",
    "image.duplicate",
    "image.adjustments.brightnessContrast",
    "image.adjustments.levels",
    "image.adjustments.curves",
    "image.adjustments.exposure",
    "image.adjustments.vibrance",
    "image.adjustments.hueSaturation",
    "image.adjustments.colorBalance",
    "image.adjustments.blackWhite",
    "image.adjustments.photoFilter",
    "image.adjustments.channelMixer",
    "image.adjustments.invert",
    "image.adjustments.posterize",
    "image.adjustments.threshold",
    "image.adjustments.desaturate",
    "image.adjustments.equalize",
    "layer.new.layer",
    "layer.duplicate",
    "layer.delete",
    "layer.select",
    "layer.setProps",
    "layer.flattenImage",
    "layer.mergeLayers",
    "layer.mergeVisible",
    "select.all",
    "select.deselect",
    "select.inverse",
    "select.modify.expand",
    "select.modify.contract",
    "select.modify.feather",
    "filter.blur.gaussianBlur",
    "filter.blur.motionBlur",
    "filter.noise.addNoise",
    "filter.sharpen.unsharpMask",
    "filter.other.highPass",
];

pub(crate) type Completion = (String, Arc<Extension>, Value);

#[cfg(not(target_arch = "wasm32"))]
fn validate_native_permission(manifest: &Manifest, package: Option<&crate::extension_package::PackageInfo>) -> Result<()> {
    if manifest.permissions.commands.iter().any(|id| id == "plugin.runTask") && package.is_none_or(|p| p.native.is_none()) {
        return Err(error("The plugin.runTask permission requires a version 2 package with its own native worker"));
    }
    let Some(package) = package else { return Ok(()) };
    if let Some(native) = &package.native
        && !manifest.permissions.commands.contains(&native.service)
    {
        return Err(error(format!("native package requires the {} permission", native.service)));
    }
    if !package.filters.is_empty() && !manifest.permissions.commands.iter().any(|id| id == "plugin.processImage") {
        return Err(error("A package with image filters requires the plugin.processImage permission"));
    }
    Ok(())
}

pub fn validate_permissions(manifest: &Manifest) -> Result<()> {
    crate::plugin_host::compatibility(manifest)?;
    for id in &manifest.permissions.commands {
        if !COMMANDS.contains(&id.as_str()) || crate::commands::find(id).is_none() {
            return Err(error(format!("extension command {id:?} is not supported by this host; see extension.commands")));
        }
    }
    Ok(())
}

impl ExtensionHost {
    pub fn list(&self) -> Value {
        json!({"extensions": self.installed.values().map(|p| {
            let mut item = json!({"manifest": p.extension.manifest, "enabled": p.enabled, "storage": p.storage});
            #[cfg(not(target_arch = "wasm32"))]
            { item["package"] = json!(p.package); }
            #[cfg(target_arch = "wasm32")]
            { item["package"] = Value::Null; }
            item
        }).collect::<Vec<_>>(),
            "errors": self.load_errors})
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn attach_folder(&mut self, folder: std::path::PathBuf) -> Result<()> {
        for installed in self.installed.values() {
            ensure_idle(installed)?;
        }
        let folder = crate::extension_package::root(&folder)?;
        let _store = crate::extension_package::StoreGuard::read(&folder)?;
        self.folder = Some(folder.clone());
        self.load_errors.clear();
        self.previews.clear();
        self.task_epochs.clear();
        self.installed.clear();
        self.platform = Default::default();
        let records = crate::extension_package::record_ids(&folder)?;
        let record_ids = records.iter().cloned().collect::<std::collections::BTreeSet<_>>();
        if records.len() > 4096 {
            self.load_errors.push("Only the first 4096 plugin installation records were inspected".into());
        }
        for id in records.into_iter().take(4096) {
            if self.installed.len() >= 128 {
                self.load_errors.push("Only the first 128 plugins were loaded".into());
                break;
            }
            let loaded = (|| {
                let loaded = crate::extension_package::load(&folder, &id)?;
                let extension = Extension::load(&loaded.module).map_err(error)?;
                validate_permissions(&extension.manifest)?;
                crate::plugin_host::validate_contributions(self, &extension.manifest)?;
                if extension.manifest.id != id {
                    return Err(error("plugin id does not match its installation record"));
                }
                validate_native_permission(&extension.manifest, loaded.package.as_ref())?;
                let enabled = !crate::extension_package::checked_path(&folder, &format!("{id}.disabled"))?.exists();
                self.installed.insert(
                    id.clone(),
                    Installed {
                        extension: Arc::new(extension),
                        enabled,
                        state: Value::Null,
                        storage: loaded.storage,
                        native: loaded.native,
                        package: loaded.package,
                        filters: loaded.filters,
                    },
                );
                Ok(())
            })();
            if let Err(e) = loaded {
                self.load_errors.push(format!("{id}: {e}"));
            }
        }
        let mut paths = std::fs::read_dir(&folder)
            .map_err(error)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "pcext") && !p.file_stem().and_then(|n| n.to_str()).is_some_and(|id| record_ids.contains(id)))
            .collect::<Vec<_>>();
        paths.sort();
        let available = 128usize.saturating_sub(self.installed.len());
        if paths.len() > available {
            self.load_errors.push("Only the first 128 extensions were loaded".into());
            paths.truncate(available);
        }
        for path in paths {
            let loaded = (|| {
                let bytes = read_module(&path)?;
                let extension = Extension::load(&bytes).map_err(error)?;
                validate_permissions(&extension.manifest)?;
                crate::plugin_host::validate_contributions(self, &extension.manifest)?;
                validate_native_permission(&extension.manifest, None)?;
                let id = extension.manifest.id.clone();
                if path.file_stem().and_then(|s| s.to_str()) != Some(id.as_str()) {
                    return Err(error("extension filename must match its embedded id"));
                }
                let enabled = !folder.join(format!("{id}.disabled")).exists();
                crate::extension_package::checked_path(&folder, &format!("{id}.pcext"))?;
                self.installed.insert(
                    id,
                    Installed {
                        extension: Arc::new(extension),
                        enabled,
                        state: Value::Null,
                        storage: StorageSize::for_module(&bytes),
                        native: None,
                        package: None,
                        filters: BTreeMap::new(),
                    },
                );
                Ok(())
            })();
            if let Err(e) = loaded {
                self.load_errors.push(format!("{}: {e}", path.display()));
            }
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn read_module(path: &std::path::Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(error)?;
    let meta = file.metadata().map_err(error)?;
    if !meta.is_file() || meta.len() > crate::extension_package::MAX_PACKAGE_BYTES as u64 {
        return Err(error("plugin must be a regular file of at most 512 MiB"));
    }
    let mut bytes = Vec::new();
    file.take(crate::extension_package::MAX_PACKAGE_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(error)?;
    if bytes.len() > crate::extension_package::MAX_PACKAGE_BYTES {
        return Err(error("plugin package exceeds 512 MiB"));
    }
    Ok(bytes)
}

struct Input {
    bytes: Vec<u8>,
    extension: Extension,
    #[cfg(not(target_arch = "wasm32"))]
    package: Option<crate::extension_package::Package>,
}

impl Input {
    fn storage(&self) -> StorageSize {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(package) = &self.package {
            return package.storage;
        }
        StorageSize::for_module(&self.bytes)
    }

    fn package_info(&self) -> Value {
        #[cfg(not(target_arch = "wasm32"))]
        {
            json!(self.package.as_ref().map(|package| &package.info))
        }
        #[cfg(target_arch = "wasm32")]
        {
            Value::Null
        }
    }
}

fn module(p: &Value) -> Result<Input> {
    let bytes = if let Some(data) = p.get("data") {
        let data =
            data.as_str().filter(|d| d.len() <= (MAX_MODULE_BYTES / 3 + 1) * 4).ok_or_else(|| error("extension data must be base64 of at most 8 MiB"))?;
        crate::plugin_cmds::b64_decode(data).ok_or_else(|| error("invalid extension base64"))?
    } else {
        let path = p.get("path").and_then(Value::as_str).ok_or_else(|| error("give path or base64 data"))?;
        #[cfg(not(target_arch = "wasm32"))]
        {
            read_module(std::path::Path::new(path))?
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            return Err(error("use base64 data on the web"));
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    let package = if bytes.starts_with(b"PK") {
        if p.get("data").is_some() {
            return Err(error("native packages must be installed from a local .pcplugin file"));
        }
        Some(crate::extension_package::inspect(&bytes)?)
    } else {
        None
    };
    #[cfg(not(target_arch = "wasm32"))]
    let extension = Extension::load(package.as_ref().map_or(bytes.as_slice(), |p| p.module.as_slice())).map_err(error)?;
    #[cfg(target_arch = "wasm32")]
    let extension = Extension::load(&bytes).map_err(error)?;
    validate_permissions(&extension.manifest)?;
    #[cfg(not(target_arch = "wasm32"))]
    validate_native_permission(&extension.manifest, package.as_ref().map(|package| &package.info))?;
    #[cfg(target_arch = "wasm32")]
    if extension.manifest.permissions.commands.iter().any(|id| id == "plugin.runTask") {
        return Err(error("Native plugin tasks require a desktop package"));
    }
    Ok(Input {
        bytes,
        extension,
        #[cfg(not(target_arch = "wasm32"))]
        package,
    })
}

fn install(s: &mut Session, p: &Value) -> Result<Value> {
    let input = module(p)?;
    let storage = input.storage();
    let bytes = &input.bytes;
    let extension = &input.extension;
    crate::plugin_host::validate_install(s, &extension.manifest)?;
    if let Some(expected) = p.get("expectedHash").and_then(Value::as_str)
        && blake3::hash(bytes).to_hex().as_str() != expected
    {
        return Err(error("extension changed since review; inspect it again"));
    }
    let id = extension.manifest.id.clone();
    let replacing = s.extensions.installed.contains_key(&id);
    if replacing && !p.get("replace").and_then(Value::as_bool).unwrap_or(false) {
        return Err(error("extension already installed; use replace:true to update it"));
    }
    if !replacing && s.extensions.installed.len() >= 128 {
        return Err(error("at most 128 extensions may be installed"));
    }
    let enabled = s.extensions.installed.get(&id).is_none_or(|i| i.enabled);
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(installed) = s.extensions.installed.get(&id) {
        ensure_idle(installed)?;
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(package) = &input.package {
        if package.info.native.is_some() && p.get("allowNative").and_then(Value::as_bool) != Some(true) {
            return Err(error("This package includes native code. Review it in Manage Plugins before installing (allowNative:true required)."));
        }
        if s.extensions.folder.is_none() {
            return Err(error("plugin packages require a persistent plugin folder"));
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let (native, enabled) = if let Some(folder) = &s.extensions.folder {
        let replace = p.get("replace").and_then(Value::as_bool) == Some(true);
        let installed = crate::extension_package::install(folder, &id, bytes, input.package.as_ref(), replace)?;
        // A repaired installation is usable now; keep other startup failures and any new warning.
        let startup_prefix = format!("{id}: ");
        s.extensions.load_errors.retain(|message| !message.starts_with(&startup_prefix));
        if let Some(warning) = installed.warning {
            s.extensions.load_errors.push(warning);
        }
        (installed.native, installed.enabled)
    } else {
        (None, enabled)
    };
    let result = json!({"manifest": extension.manifest, "enabled": enabled, "replaced": replacing, "storage": storage});
    s.forget_plugin_preview(&id);
    crate::plugin_host::revoke(s, &id);
    s.extensions.installed.insert(
        id,
        Installed {
            extension: Arc::new(input.extension),
            enabled,
            state: Value::Null,
            storage,
            #[cfg(not(target_arch = "wasm32"))]
            filters: input.package.as_ref().map(|package| package.filters.clone()).unwrap_or_default(),
            #[cfg(target_arch = "wasm32")]
            filters: BTreeMap::new(),
            #[cfg(not(target_arch = "wasm32"))]
            package: input.package.as_ref().map(|package| package.info.clone()),
            #[cfg(not(target_arch = "wasm32"))]
            native,
        },
    );
    s.extensions.revision = s.extensions.revision.wrapping_add(1);
    Ok(result)
}

fn id(p: &Value) -> Result<&str> {
    p.get("id").and_then(Value::as_str).ok_or_else(|| error("missing extension id"))
}

fn invoke(s: &mut Session, p: &Value) -> Result<Value> {
    let plugin = id(p)?.to_owned();
    let event = p.get("event").and_then(Value::as_str).ok_or_else(|| error("missing button event"))?;
    crate::plugin_host::invoke(s, &plugin, event, p.get("values").unwrap_or(&Value::Null))
}

fn enable(s: &mut Session, p: &Value) -> Result<Value> {
    let id = id(p)?;
    let enabled = p.get("enabled").and_then(Value::as_bool).ok_or_else(|| error("enabled must be a boolean"))?;
    if !s.extensions.installed.contains_key(id) {
        return Err(error("extension is not installed"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _store = s.extensions.folder.as_ref().map(|folder| crate::extension_package::StoreGuard::write(folder)).transpose()?;
    #[cfg(not(target_arch = "wasm32"))]
    let activation = if let Some(folder) = &s.extensions.folder {
        let marker = crate::extension_package::checked_path(folder, &format!("{id}.disabled"))?;
        let activation = crate::extension_package::revoke_activation(folder, id)?;
        if enabled {
            remove_if_exists(&marker)?;
        } else {
            photocraft_format::atomic_write(&marker, b"").map_err(error)?;
        }
        Some(activation)
    } else {
        None
    };
    if let Some(installed) = s.extensions.installed.get_mut(id) {
        installed.enabled = enabled;
        #[cfg(not(target_arch = "wasm32"))]
        if let (Some(activation), Some(native)) = (activation, &installed.native) {
            installed.native = Some(Arc::new(native.reactivated(activation)));
        }
    }
    if !enabled {
        crate::plugin_host::revoke(s, id);
        s.extensions.task_epochs.remove(id);
        s.forget_plugin_preview(id);
    }
    s.extensions.revision = s.extensions.revision.wrapping_add(1);
    Ok(json!({"id": id, "enabled": enabled}))
}

#[cfg(not(target_arch = "wasm32"))]
fn ensure_idle(installed: &Installed) -> Result<()> {
    if installed.native.as_ref().is_some_and(|native| Arc::strong_count(native) > 1) {
        return Err(error("Wait for this plugin's processing to finish or cancel it before updating or removing it"));
    }
    if installed.filters.values().any(|filter| Arc::strong_count(filter) > 1) {
        return Err(error("Wait for this plugin's processing to finish or cancel it before updating or removing it"));
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn remove_if_exists(path: &std::path::Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(error(e)),
    }
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let id = id(p)?;
    if !s.extensions.installed.contains_key(id) {
        return Err(error("extension is not installed"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(installed) = s.extensions.installed.get(id) {
        ensure_idle(installed)?;
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(folder) = &s.extensions.folder
        && let Some(warning) = crate::extension_package::remove(folder, id)?
    {
        s.extensions.load_errors.push(warning);
    }
    s.extensions.installed.remove(id);
    crate::plugin_host::revoke(s, id);
    s.extensions.task_epochs.remove(id);
    s.forget_plugin_preview(id);
    s.extensions.revision = s.extensions.revision.wrapping_add(1);
    Ok(json!({"removed": id}))
}

pub fn specs() -> Vec<CommandSpec> {
    let spec = |id, label, params, run| CommandSpec { id, label, params, run, menu: &[], shortcut: None, enabled: crate::commands::always, journal: false };
    vec![
        spec("extension.commands", "Extension Commands", "{}", |_, _| {
            Ok(
                json!({"commands": COMMANDS.iter().filter(|id| crate::commands::find(id).is_some()).collect::<Vec<_>>(), "apiVersion": 1, "maxJsonBytes": MAX_JSON_BYTES}),
            )
        }),
        spec("extension.inspect", "Inspect Extension", r#"{"path":text,"data":text}"#, |_, p| {
            let input = module(p)?;
            Ok(json!({"manifest": input.extension.manifest, "hash": blake3::hash(&input.bytes).to_hex().as_str(),
                "package": input.package_info(), "storage": input.storage(), "downloadBytes": input.bytes.len()}))
        }),
        spec("extension.install", "Install Extension", r#"{"path":text,"data":text,"replace":bool=false,"allowNative":bool=false}"#, install),
        spec("extension.list", "List Extensions", "{}", |s, _| Ok(s.extensions.list())),
        spec("extension.invoke", "Run Extension Action", r#"{"id":text,"event":text,"values":json}"#, invoke),
        spec("extension.enable", "Enable Extension", r#"{"id":text,"enabled":bool}"#, enable),
        spec("extension.remove", "Remove Extension", r#"{"id":text}"#, remove),
    ]
}

#[cfg(test)]
mod tests;
