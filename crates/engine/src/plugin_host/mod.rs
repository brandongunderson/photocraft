//! Versioned plugin capabilities. Identity is supplied by the host, never decoded from a guest.
mod broker;
mod command_effects;
mod events;

use std::sync::{Arc, Weak};

use photocraft_plugin_api::{ApiError, CapabilitySupport, ErrorCode, HostApi, InspectorView, KnownCapability, negotiate};
use photocraft_plugins::extension::{Extension, Manifest};
use serde_json::{Value, json};

use crate::{EngineError, Result, Session, commands::CommandSpec};

pub(crate) use broker::Platform;
pub(crate) type Authority = fn(&str, &Value) -> Result<()>;

/// Only host code constructs this identity. A serialized plugin id is never a principal.
#[derive(Clone)]
pub(crate) struct Principal {
    id: String,
    session: u64,
    extension: Weak<Extension>,
    epoch: Weak<()>,
    authorities: Vec<Authority>,
}

pub(crate) fn failure(code: ErrorCode, message: impl Into<String>) -> EngineError {
    EngineError::PluginApi(ApiError::new(code, message))
}

pub fn host_api() -> HostApi {
    HostApi { capabilities: KnownCapability::ALL.into_iter().map(|id| CapabilitySupport { id: id.into(), version: 1 }).collect(), ..HostApi::default() }
}

pub(crate) fn compatibility(manifest: &Manifest) -> Result<Value> {
    if manifest.api_version != photocraft_plugin_api::PANEL_API_VERSION {
        return Err(failure(ErrorCode::UnsupportedVersion, "Unsupported panel version; rebuild the plugin for the current PhotoCraft Plugin API"));
    }
    for contributed in &manifest.contributes.commands {
        let root = contributed.id.split('.').next().unwrap_or_default();
        let reserved = ["ui", "tool", "window", "help", "view", "file", "edit", "type", "select", "filter", "image", "layer"].contains(&root)
            || crate::commands::command_specs().iter().any(|command| command.id.split('.').next() == Some(root));
        if reserved || crate::commands::find(&contributed.id).is_some() {
            return Err(failure(ErrorCode::Conflict, format!("Command {} already exists", contributed.id)));
        }
    }
    let selected = negotiate(&manifest.capabilities, &host_api().capabilities)?;
    let has = |cap: KnownCapability| selected.available.iter().any(|offer| offer.id.as_str() == cap.as_str());
    for (needed, cap) in [
        (!manifest.permissions.commands.is_empty(), KnownCapability::CommandsInvoke),
        (!manifest.contributes.inspectors.is_empty(), KnownCapability::UiViews),
        (!manifest.contributes.subscriptions.is_empty(), KnownCapability::Events),
    ] {
        if needed && !has(cap) {
            return Err(failure(ErrorCode::PermissionDenied, format!("This contribution requires a declared {} capability", cap.as_str())));
        }
    }
    if manifest.contributes.inspectors.len() > 1 || manifest.contributes.subscriptions.len() > 1 {
        return Err(failure(ErrorCode::UnsupportedOperation, "This host supports one inspector and one document subscription per plugin"));
    }
    if manifest.contributes.subscriptions.iter().any(|subscription| !manifest.contributes.inspectors.iter().any(|view| view.event == subscription.handler)) {
        return Err(failure(ErrorCode::UnsupportedOperation, "Document subscriptions must target the inspector handler in this host"));
    }
    Ok(json!({"panelApi":2,"hostApi":host_api().version,"negotiated":selected}))
}

pub(crate) fn validate_install(s: &Session, manifest: &Manifest) -> Result<()> {
    validate_contributions(&s.extensions, manifest)
}

pub(crate) fn validate_contributions(host: &crate::extension_cmds::ExtensionHost, manifest: &Manifest) -> Result<()> {
    compatibility(manifest)?;
    for contributed in &manifest.contributes.commands {
        if host
            .installed
            .iter()
            .any(|(owner, installed)| owner != &manifest.id && installed.extension.manifest.contributes.commands.iter().any(|other| other.id == contributed.id))
        {
            return Err(failure(ErrorCode::Conflict, format!("Command {} already exists", contributed.id)));
        }
    }
    Ok(())
}

pub(crate) fn validate_principal(s: &Session, principal: &Principal) -> Result<Arc<Extension>> {
    let platform = &s.extensions.platform;
    if platform.session != Some(principal.session)
        || !platform.epochs.get(&principal.id).is_some_and(|epoch| Weak::ptr_eq(&principal.epoch, &Arc::downgrade(epoch)))
    {
        return Err(failure(ErrorCode::InvalidHandle, "The plugin invocation was revoked"));
    }
    let extension = s
        .extensions
        .installed
        .get(&principal.id)
        .filter(|installed| installed.enabled && Weak::ptr_eq(&principal.extension, &Arc::downgrade(&installed.extension)))
        .map(|installed| Arc::clone(&installed.extension))
        .ok_or_else(|| failure(ErrorCode::InvalidHandle, "The plugin was disabled, removed or replaced"))?;
    extension.check_current().map_err(|cause| failure(ErrorCode::InvalidHandle, cause.to_string()))?;
    Ok(extension)
}

pub(crate) fn require(s: &Session, principal: &Principal, capability: KnownCapability) -> Result<()> {
    let extension = validate_principal(s, principal)?;
    let selected = negotiate(&extension.manifest.capabilities, &host_api().capabilities)?;
    if !selected.available.iter().any(|offer| offer.id.as_str() == capability.as_str()) {
        return Err(failure(ErrorCode::PermissionDenied, format!("The plugin has not requested {}", capability.as_str())));
    }
    Ok(())
}

pub(crate) fn authorize(s: &Session, principal: &Principal, command: &str, params: &Value) -> Result<()> {
    validate_principal(s, principal)?;
    if s.plugins_disabled {
        return Err(failure(ErrorCode::PermissionDenied, "Plugins are disabled for this recovery session"));
    }
    for policy in &principal.authorities {
        policy(command, params)?;
    }
    if let Some(policy) = s.authorize {
        policy(command, params)?;
    }
    Ok(())
}

pub(crate) fn principal(s: &mut Session, id: &str) -> Result<Principal> {
    crate::extension_cmds::activate(s, id)?;
    let installed =
        s.extensions.installed.get(id).filter(|p| p.enabled).ok_or_else(|| failure(ErrorCode::NotFound, "The plugin is not installed or is disabled"))?;
    let extension = Arc::downgrade(&installed.extension);
    let platform = &mut s.extensions.platform;
    let session = platform.session.ok_or_else(|| failure(ErrorCode::LimitExceeded, "Plugin session identities are exhausted"))?;
    let epoch = Arc::downgrade(platform.epochs.entry(id.into()).or_insert_with(|| Arc::new(())));
    Ok(Principal { id: id.into(), session, extension, epoch, authorities: s.authorize.into_iter().collect() })
}

pub(crate) fn revoke(s: &mut Session, id: &str) {
    s.extensions.platform.revoke(id);
    s.plugin_scheduler.cancel_plugin(&crate::extension_cmds::scheduler_owner(s, id));
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(native) = s.extensions.installed.get(id).and_then(|installed| installed.native.as_ref()) {
        native.stop();
    }
}

pub(crate) fn event_request(s: &Session, extension: &Extension, event: &str, values: Value, state: Value, responses: Value) -> Value {
    let document = if extension.manifest.permissions.document_read {
        s.active()
            .map(|doc| {
                json!({"id":doc.doc.id,"name":doc.doc.name,"width":doc.doc.size.width,"height":doc.doc.size.height,
            "depth":doc.doc.depth.bits(),"mode":format!("{:?}",doc.doc.mode),"revision":doc.revision,"activeLayer":doc.active_layer.map(|id|id.0)})
            })
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    json!({"apiVersion":2,"host":host_api(),"compatibility":compatibility(&extension.manifest).ok(),
        "event":event,"values":values,"state":state,"document":document,"responses":responses})
}

/// Runs a user-invoked panel action through the current event and capability contract.
pub(crate) fn invoke(s: &mut Session, id: &str, event: &str, values: &Value) -> Result<Value> {
    let identity = principal(s, id)?;
    authorize(s, &identity, "extension.invoke", &json!({"id":id,"event":event,"values":values}))?;
    let extension = validate_principal(s, &identity)?;
    let valid_event = extension.manifest.panel.buttons.iter().any(|button| button.id == event)
        || extension.manifest.contributes.commands.iter().any(|command| command.event == event);
    if !valid_event {
        return Err(failure(ErrorCode::NotFound, "Unknown plugin action"));
    }
    let values = Value::Object(extension.manifest.values(values).map_err(|e| EngineError::Other(e.to_string()))?);
    let state = s.extensions.installed.get(id).map(|installed| installed.state.clone()).unwrap_or(Value::Null);
    let mut request = event_request(s, &extension, event, values.clone(), state, json!([]));
    for _ in 0..4 {
        let response = extension.handle(&request).map_err(|e| EngineError::Other(e.to_string()))?;
        validate_principal(s, &identity)?;
        if response.view.is_some() {
            require(s, &identity, KnownCapability::UiViews)?;
        }
        if !response.commands.is_empty() {
            if response.view.is_some() {
                return Err(failure(ErrorCode::UnsupportedOperation, "A view update cannot be combined with command effects"));
            }
            require(s, &identity, KnownCapability::CommandsInvoke)?;
            return command_effects::apply(s, identity, extension, response);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _admission = s.extensions.folder.as_deref().map(crate::extension_package::AdmissionGuard::store).transpose()?;
        validate_principal(s, &identity)?;
        let (responses, edited) = broker::apply_requests(s, &identity, &response.requests, false)?;
        if edited || response.requests.is_empty() {
            if let Some(view) = response.view {
                require(s, &identity, KnownCapability::UiViews)?;
                s.extensions.platform.views.insert(id.into(), view);
            }
            if let Some(installed) = s.extensions.installed.get_mut(id) {
                installed.state = response.state;
            }
            return Ok(json!({"id":id,"message":response.message,"results":responses}));
        }
        request = event_request(s, &extension, "host_response", values.clone(), response.state, Value::Array(responses));
    }
    Err(failure(ErrorCode::LimitExceeded, "The plugin exceeded four host-query rounds"))
}

fn contributed(s: &Session, command: &str) -> Option<(String, photocraft_plugin_api::CommandContribution)> {
    s.extensions.installed.iter().find_map(|(id, installed)| {
        installed.extension.manifest.contributes.commands.iter().find(|item| item.id == command).map(|item| (id.clone(), item.clone()))
    })
}

pub(crate) fn dynamic_reason(s: &Session, id: &str) -> Option<String> {
    let (owner, command) = contributed(s, id)?;
    if s.plugins_disabled {
        Some("Plugins are disabled for this recovery session".into())
    } else if s.extensions.installed.get(&owner).is_none_or(|installed| !installed.enabled) {
        Some("The plugin is disabled".into())
    } else if command.needs_document && s.active().is_none() {
        Some("no document open".into())
    } else {
        None
    }
}

pub(crate) fn is_contributed(s: &Session, id: &str) -> bool {
    contributed(s, id).is_some()
}

/// Broker operations validate before touching transient state. Panel invocation defers
/// settlement to the decoded action, so document-independent tasks do not commit a move.
pub(crate) fn preserves_floating(_s: &Session, id: &str, _params: &Value) -> bool {
    matches!(
        id,
        "extension.capabilities"
            | "extension.runtime"
            | "extension.storage"
            | "extension.rollback"
            | "extension.list"
            | "extension.commands"
            | "extension.inspect"
            | "extension.install"
            | "extension.enable"
            | "extension.remove"
            | "extension.view.open"
            | "extension.view.close"
            | "extension.view.inspect"
            | "plugin.document.read"
            | "plugin.document.edit"
            | "plugin.runTask"
            | "extension.invoke"
            | "image.pluginPreview.discard"
            | "jobs.list"
            | "jobs.cancel"
    )
}

pub(crate) fn invoke_command(s: &mut Session, id: &str, params: &Value) -> Result<Value> {
    let (owner, command) = contributed(s, id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?;
    if let Some(reason) = dynamic_reason(s, id) {
        return Err(EngineError::Disabled(id.into(), reason));
    }
    let mut values = command.params;
    if let Some(params) = params.as_object() {
        for (key, value) in params {
            if key != "coalesce" {
                values.insert(key.clone(), value.clone());
            }
        }
    }
    invoke(s, &owner, &command.event, &Value::Object(values))
}

impl Session {
    pub fn has_command(&self, id: &str) -> bool {
        crate::commands::find(id).is_some() || is_contributed(self, id)
    }

    pub fn plugin_command_entries(&self) -> Vec<Value> {
        self.extensions
            .installed
            .values()
            .flat_map(|installed| {
                installed.extension.manifest.contributes.commands.iter().map(|command| {
                    let reason = dynamic_reason(self, &command.id);
                    json!({"id":command.id,"label":command.label,"menu":["Plugins",installed.extension.manifest.name],"shortcut":null,
                    "params":serde_json::to_string(&command.params).unwrap_or_default(),"enabled":reason.is_none(),"disabledReason":reason,
                    "needsDocument":command.needs_document && self.active().is_none()})
                })
            })
            .collect()
    }

    pub fn command_entries(&self) -> Vec<Value> {
        let mut rows =
            crate::commands::command_specs().iter().map(|spec| crate::commands::list_entry(self, spec, self.disabled_reason(spec.id))).collect::<Vec<_>>();
        rows.extend(self.plugin_command_entries());
        rows
    }

    pub fn plugin_inspector_view(&self, id: &str) -> Option<&InspectorView> {
        self.extensions.platform.views.get(id)
    }

    pub fn plugin_view_error(&self, id: &str) -> Option<&str> {
        self.extensions.platform.errors.get(id).map(String::as_str)
    }

    /// Mutation hooks queue metadata only. Guest code is scheduled by the event pump later.
    pub(crate) fn plugin_document_changed(&mut self) {
        self.extensions.platform.invalidate(self.authorize);
    }
}

pub fn specs() -> Vec<CommandSpec> {
    let spec = |id, label, params, run| CommandSpec { id, label, params, run, menu: &[], shortcut: None, enabled: crate::commands::always, journal: false };
    vec![
        spec("extension.capabilities", "Plugin Capabilities", "{}", |s, _| {
            Ok(json!({"host":host_api(),"panelApis":[photocraft_plugin_api::PANEL_API_VERSION],"packageFormats":[2],
            "limits":{"queryRounds":4,"requests":8,"layerPage":128,"editBatch":128,"handles":4096,"eventWorkers":4,"inspectorsPerPlugin":1,
                "installedPlugins":4096,"retainedPreviews":16,"previewBytes":512_u64<<20},
            "features":{"package3":false,"worker2":!cfg!(target_arch="wasm32"),"restrictedNative":false,"lazyDescriptors":!cfg!(target_arch="wasm32"),"managedStorage":!cfg!(target_arch="wasm32")},
            "scheduler":s.plugin_scheduler.status(),"previews":s.plugin_preview_usage()}))
        }),
        spec("extension.view.open", "Open Plugin Inspector", r#"{"id":text}"#, events::open),
        spec("extension.view.close", "Close Plugin Inspector", r#"{"id":text}"#, events::close),
        spec("extension.view.inspect", "Inspect Plugin View", r#"{"id":text}"#, |s, p| {
            let id = p.get("id").and_then(Value::as_str).ok_or_else(|| failure(ErrorCode::InvalidJson, "Missing plugin id"))?;
            Ok(json!({"view":s.plugin_inspector_view(id),"error":s.plugin_view_error(id),"pending":s.plugin_events_pending()}))
        }),
        spec("plugin.document.read", "Read Plugin Layer Snapshot", r#"{"id":text,"document":text,"cursor":text,"limit":u32}"#, |s, p| {
            broker::direct(s, p, false)
        }),
        spec("plugin.document.edit", "Apply Plugin Layer Edit Batch", r#"{"id":text,"document":text,"expectedRevision":u64,"edits":json}"#, |s, p| {
            broker::direct(s, p, true)
        }),
    ]
}

#[cfg(test)]
mod tests;
