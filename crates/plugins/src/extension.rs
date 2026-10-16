//! Native-panel extensions. Code runs in a fresh, bounded WASM instance per event; the host
//! supplies explicit JSON state and validates every requested command before executing it.
mod runtime;

use std::collections::BTreeSet;

use photocraft_plugin_api::{BrokerRequest, Capabilities, Contributions, InspectorView, UpdateSource};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::manifest::{Manifest as FilterManifest, ParamSpec};
use crate::{Area, Error, Kind, Result};

pub use runtime::Extension;

pub const MAX_MODULE_BYTES: usize = 8 << 20;
pub const MAX_JSON_BYTES: usize = 256 << 10;
/// Keep room for event, values and document metadata in the next bounded request.
pub const MAX_STATE_BYTES: usize = 128 << 10;
pub const MAX_COMMANDS: usize = 32;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub api_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updates: Option<UpdateSource>,
    pub permissions: Permissions,
    pub panel: Panel,
    #[serde(default, skip_serializing_if = "Capabilities::is_empty")]
    pub capabilities: Capabilities,
    #[serde(default, skip_serializing_if = "Contributions::is_empty")]
    pub contributes: Contributions,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Permissions {
    #[serde(default)]
    pub document_read: bool,
    #[serde(default)]
    pub commands: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Panel {
    /// Opt in to the host's native before/after image-preview layout.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preview: bool,
    #[serde(default)]
    pub fields: Vec<Field>,
    pub buttons: Vec<Button>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub id: String,
    pub label: String,
    #[serde(flatten)]
    pub spec: ParamSpec,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Button {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    #[serde(default)]
    pub state: Value,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub commands: Vec<Invocation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<BrokerRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<InspectorView>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
    pub command: String,
    pub params: Value,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Manifest(message.into())
}

fn label(text: &str) -> bool {
    !text.trim().is_empty() && text.chars().count() <= 128 && text.chars().all(|c| !c.is_control())
}

fn event_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') && !id.starts_with('_')
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > crate::manifest::MAX_MANIFEST_BYTES {
            return Err(invalid("extension manifest exceeds 64 KiB"));
        }
        let manifest: Self = serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
        if manifest.api_version != photocraft_plugin_api::PANEL_API_VERSION {
            return Err(invalid(format!(
                "unsupported marker {} for the current PhotoCraft Plugin API; expected {}",
                manifest.api_version,
                photocraft_plugin_api::PANEL_API_VERSION
            )));
        }
        photocraft_plugin_api::Descriptor::parse(bytes).map_err(|e| invalid(e.to_string()))?;
        if !crate::manifest::valid_id(&manifest.id) || manifest.id != manifest.id.to_ascii_lowercase() || manifest.id.starts_with('.') {
            return Err(invalid(
                "extension id must be lowercase ASCII, start with a letter or digit, and contain only letters, digits, dots, underscores or hyphens",
            ));
        }
        if !manifest.id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric) {
            return Err(invalid("extension id must start with a letter or digit"));
        }
        let requires_button = manifest.contributes.inspectors.is_empty();
        if manifest.panel.fields.len() > 32 || (requires_button && manifest.panel.buttons.is_empty()) || manifest.panel.buttons.len() > 16 {
            return Err(invalid("extension panels need 1–16 buttons and at most 32 fields"));
        }
        let mut ids = BTreeSet::new();
        for field in &manifest.panel.fields {
            if !event_id(&field.id) || !label(&field.label) || !ids.insert(&field.id) {
                return Err(invalid("invalid or duplicate panel field"));
            }
        }
        ids.clear();
        for button in &manifest.panel.buttons {
            if !event_id(&button.id) || !label(&button.label) || !ids.insert(&button.id) {
                return Err(invalid("invalid or duplicate panel button"));
            }
        }
        if manifest.permissions.commands.len() > 128 {
            return Err(invalid("at most 128 command permissions are allowed"));
        }
        let mut commands = BTreeSet::new();
        for id in &manifest.permissions.commands {
            if id.is_empty() || id.len() > 128 || !id.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)) || !commands.insert(id) {
                return Err(invalid("invalid or duplicate command permission"));
            }
        }
        // Reuse the filter API's validation for names, numeric ranges, choices and defaults.
        let schema = serde_json::to_vec(&manifest.schema()).map_err(|e| invalid(e.to_string()))?;
        FilterManifest::parse(&schema)?;
        Ok(manifest)
    }

    fn schema(&self) -> FilterManifest {
        FilterManifest {
            id: self.id.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            author: self.author.clone(),
            description: self.description.clone(),
            kind: Kind::Filter,
            params: self.panel.fields.iter().map(|f| (f.id.clone(), f.spec.clone())).collect(),
            overlap: 0,
            area: Area::Content,
        }
    }

    pub fn values(&self, values: &Value) -> Result<Map<String, Value>> {
        if !values.is_null() && !values.is_object() {
            return Err(Error::Params("panel values must be an object".into()));
        }
        if let Some(values) = values.as_object() {
            for key in values.keys() {
                if !self.panel.fields.iter().any(|f| &f.id == key) {
                    return Err(Error::Params(format!("unknown panel field {key:?}")));
                }
            }
        }
        self.schema().resolve_params(values)
    }
}

impl Response {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let parsed = photocraft_plugin_api::PanelResponse::parse(bytes).map_err(|e| {
            let message = match e.details.get("path").and_then(Value::as_str).filter(|path| path.len() <= 256 && path.chars().all(|c| !c.is_control())) {
                Some(path) => format!("{path}: {e}"),
                None => e.to_string(),
            };
            match e.code {
                photocraft_plugin_api::ErrorCode::LimitExceeded => Error::Limit(message),
                _ => Error::Abi(format!("extension response: {message}")),
            }
        })?;
        Ok(Self {
            state: parsed.state,
            message: parsed.message,
            commands: parsed.commands.into_iter().map(|c| Invocation { command: c.command, params: Value::Object(c.params) }).collect(),
            requests: parsed.requests,
            view: parsed.view,
        })
    }
}

#[cfg(test)]
mod tests;
