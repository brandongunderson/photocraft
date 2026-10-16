//! Current API command effects: validate grants before dispatching editor commands or jobs.
use std::sync::Arc;

use photocraft_plugins::extension::{Extension, Response};
use serde_json::{Value, json};

use super::{Principal, authorize, validate_principal};
use crate::{EngineError, Result, Session};

pub(super) fn apply(s: &mut Session, owner: Principal, extension: Arc<Extension>, response: Response) -> Result<Value> {
    validate_principal(s, &owner)?;
    for call in &response.commands {
        if !extension.manifest.permissions.commands.contains(&call.command) || !crate::extension_cmds::COMMANDS.contains(&call.command.as_str()) {
            return Err(EngineError::Other(format!("extension requested ungranted command {:?}", call.command)));
        }
        authorize(s, &owner, &call.command, &call.params)?;
    }
    if let Some(call) = response.commands.iter().find(|call| call.command == "plugin.runTask") {
        if response.commands.len() != 1 {
            return Err(EngineError::Other("A native plugin task must be the only command in a plugin action".into()));
        }
        return crate::plugin_task_cmds::run(s, &call.params, Some((owner.id, extension, response.state)));
    }
    // Settle floating selections for document actions after the response is
    // decoded and all grants checked. A sole maintenance task above never touches it.
    if !response.commands.is_empty() {
        crate::float_cmds::before_command(s, "extension.invoke")?;
    }
    if let Some(call) = response.commands.iter().find(|call| call.command == "plugin.processImage") {
        if response.commands.len() != 1 {
            return Err(EngineError::Other("An image processing job must be the only command in a plugin action".into()));
        }
        return crate::plugin_image_cmds::run(s, &call.params, Some((owner.id, extension, response.state)));
    }
    let mut results = Vec::new();
    for (index, call) in response.commands.iter().enumerate() {
        authorize(s, &owner, &call.command, &call.params)?;
        let result = s.execute(&call.command, call.params.clone()).map_err(|error| {
            EngineError::Other(format!(
                "extension action stopped at step {} ({}); {} earlier steps completed; undoable edits remain in History: {error}",
                index + 1,
                call.command,
                index
            ))
        })?;
        results.push(result);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _admission = s.extensions.folder.as_deref().map(crate::extension_package::AdmissionGuard::store).transpose()?;
    validate_principal(s, &owner)?;
    if let Some(installed) = s.extensions.installed.get_mut(&owner.id) {
        installed.state = response.state;
    }
    Ok(json!({"id":owner.id,"message":response.message,"results":results}))
}
