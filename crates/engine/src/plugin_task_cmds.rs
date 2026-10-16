//! Document-independent work owned by an installed native plugin. The host supplies the
//! request/result paths; the guest may select options, never an executable or another package.
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{EngineError, Result, Session, commands::CommandSpec, extension_cmds::Completion};

const MAX_OPTIONS_BYTES: usize = 128 << 10;

fn error(message: impl std::fmt::Display) -> EngineError {
    EngineError::Other(message.to_string())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    id: Option<String>,
    #[serde(default = "empty")]
    options: Value,
}

fn empty() -> Value {
    json!({})
}

struct Budget(usize);

impl std::io::Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.checked_add(bytes.len()).filter(|n| *n <= MAX_OPTIONS_BYTES).ok_or_else(|| std::io::Error::other("options exceed 128 KiB"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn params(value: &Value) -> Result<Params> {
    let fields = value.as_object().ok_or_else(|| error("Native plugin task settings must be an object"))?;
    if fields.keys().any(|key| !matches!(key.as_str(), "id" | "options")) {
        return Err(error("Unknown native plugin task setting"));
    }
    if fields.get("id").is_some_and(|id| !id.is_null() && !id.as_str().is_some_and(photocraft_plugins::manifest::valid_id)) {
        return Err(error("`id` must be a valid installed plugin id"));
    }
    // Validate before cloning a caller-owned Value: a large malformed request must not
    // allocate a second unbounded copy while checking the serialized options budget.
    if let Some(options) = fields.get("options") {
        if !options.is_object() {
            return Err(error("Native plugin task options must be an object"));
        }
        serde_json::to_writer(Budget(0), options).map_err(|_| error("Native plugin task options exceed 128 KiB"))?;
    }
    serde_json::from_value(value.clone()).map_err(error)
}

pub(crate) fn run(s: &mut Session, value: &Value, completion: Option<Completion>) -> Result<Value> {
    let params = params(value)?;
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, params.id, params.options, completion);
        Err(error("Native plugin tasks require the desktop application"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::Arc;
        let (id, extension, state) = match completion {
            Some((id, extension, state)) => {
                if params.id.as_ref().is_some_and(|requested| requested != &id) {
                    return Err(error("A plugin may run native tasks only with its own installation"));
                }
                (id, extension, Some(state))
            }
            None => {
                let id = params.id.ok_or_else(|| error("Choose an installed plugin with `id`"))?;
                crate::extension_cmds::activate(s, &id)?;
                let installed = s.extensions.installed.get(&id).ok_or_else(|| error("The requested plugin is not installed"))?;
                (id, Arc::clone(&installed.extension), None)
            }
        };
        crate::extension_cmds::activate(s, &id)?;
        let installed = s
            .extensions
            .installed
            .get(&id)
            .filter(|p| p.enabled && Arc::ptr_eq(&p.extension, &extension))
            .ok_or_else(|| error("The task's plugin was disabled, removed or replaced"))?;
        if !extension.manifest.permissions.commands.iter().any(|grant| grant == "plugin.runTask") {
            return Err(error("This plugin has not requested the plugin.runTask permission"));
        }
        let native = installed
            .native
            .as_ref()
            .filter(|native| native.package.package_version == 2)
            .ok_or_else(|| error("This plugin does not contain a compatible native worker"))?;
        let (resources, resident) = if let Some(worker) = &native.worker_v2 {
            let declared = worker.resources();
            let resources = crate::plugin_scheduler::Resources {
                cpu_units: declared.cpu_units,
                temp_bytes: declared.temp_bytes,
                memory_bytes: 16 << 20,
                ..Default::default()
            };
            let resident = crate::plugin_scheduler::Resources {
                memory_bytes: declared.memory_bytes,
                gpu_bytes: declared.gpu_bytes,
                cache_bytes: declared.cache_bytes,
                ..Default::default()
            };
            s.plugin_scheduler.validate(resources.plus(resident))?;
            (resources, resident)
        } else {
            (
                crate::plugin_scheduler::Resources { cpu_units: 1, memory_bytes: 1 << 30, gpu_bytes: 512 << 20, temp_bytes: 128 << 20, cache_bytes: 0 },
                Default::default(),
            )
        };
        let owner = crate::extension_cmds::scheduler_owner(s, &id);
        let ticket = if resident == crate::plugin_scheduler::Resources::default() {
            s.plugin_scheduler.enqueue(&owner, crate::plugin_scheduler::Priority::Background, resources)?
        } else {
            s.plugin_scheduler.enqueue_worker(&owner, crate::plugin_scheduler::Priority::Background, resources, resident)?
        };
        let native = Arc::clone(native);
        let apply_native = Arc::clone(&native);
        let source_state = installed.state.clone();
        let epoch = Arc::clone(s.extensions.task_epochs.entry(id.clone()).or_default());
        let binding = Binding { id, extension, native: apply_native, epoch, state, source_state };
        crate::jobs::run(
            s,
            "Run plugin task",
            false,
            move |ctx| {
                let _permit = ticket.wait(ctx)?;
                let _store = native.lease()?;
                crate::plugin_runtime::task_native(&native, &params.options, ctx)
            },
            move |s, result| binding.complete(s, result),
        )
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct Binding {
    id: String,
    extension: std::sync::Arc<photocraft_plugins::extension::Extension>,
    native: std::sync::Arc<crate::extension_package::InstalledNative>,
    epoch: std::sync::Arc<()>,
    state: Option<Value>,
    source_state: Value,
}

#[cfg(not(target_arch = "wasm32"))]
impl Binding {
    fn complete(self, s: &mut Session, result: crate::plugin_runtime::TaskResult) -> Result<Value> {
        use std::sync::Arc;
        let _store = self.native.admission()?;
        self.extension.check_current().map_err(error)?;
        let installed = s
            .extensions
            .installed
            .get_mut(&self.id)
            .filter(|p| p.enabled && Arc::ptr_eq(&p.extension, &self.extension) && p.native.as_ref().is_some_and(|n| Arc::ptr_eq(n, &self.native)))
            .ok_or_else(|| error("The task's plugin was disabled, removed or replaced before completion"))?;
        if !s.extensions.task_epochs.get(&self.id).is_some_and(|current| Arc::ptr_eq(current, &self.epoch)) {
            return Err(error("This plugin task was revoked before completion"));
        }
        if let Some(state) = self.state.filter(|state| state != &self.source_state) {
            if installed.state != self.source_state {
                return Err(error("This plugin's state changed while the task ran. Its proposed state was not committed; run the task again if needed."));
            }
            installed.state = state;
        }
        Ok(json!({"id":self.id,"task":true,"message":result.message,"data":result.data}))
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "plugin.runTask",
        label: "Run Native Plugin Task",
        menu: &[],
        shortcut: None,
        params: r#"{"id":text,"options":object={}} (uses this installed plugin's own native worker; no document required)"#,
        enabled: crate::commands::always,
        run: |s, p| run(s, p, None),
        journal: false,
    }]
}

#[cfg(test)]
mod tests;
