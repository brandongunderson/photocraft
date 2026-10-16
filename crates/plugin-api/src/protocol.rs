use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{ApiError, ErrorCode, InspectorView, MAX_RESPONSE_BYTES, MAX_STATE_BYTES, Result, invalid, json_size};

/// A bearer reference only. Hosts must check issuer, owner, installation and invocation scope.
/// Deserializing a handle grants no authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct ScopedHandle(#[cfg_attr(feature = "schema", schemars(length(min = 1, max = 192)))] pub String);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrokerRequest {
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 64)))]
    pub id: String,
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 96)))]
    pub method: String,
    #[serde(default)]
    pub params: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandInvocation {
    pub command: String,
    pub params: Map<String, Value>,
}

/// Bounded response for the current PhotoCraft Plugin API.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PanelResponse {
    #[serde(default)]
    pub state: Value,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(length(max = 4096)))]
    pub message: String,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(length(max = 32)))]
    pub commands: Vec<CommandInvocation>,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(length(max = 8)))]
    pub requests: Vec<BrokerRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<InspectorView>,
}

impl PanelResponse {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(ApiError::new(ErrorCode::LimitExceeded, "panel response exceeds 256 KiB"));
        }
        let response: Self = serde_json::from_slice(bytes).map_err(|e| ApiError::new(ErrorCode::InvalidJson, e.to_string()))?;
        response.validate()?;
        Ok(response)
    }

    pub fn validate(&self) -> Result<()> {
        json_size(&self.state, MAX_STATE_BYTES, "state")?;
        if self.commands.len() > 32 || self.requests.len() > 8 || self.message.len() > 4096 {
            return Err(ApiError::new(ErrorCode::LimitExceeded, "response command/request/message budget exceeded"));
        }
        if !self.commands.is_empty() && !self.requests.is_empty() {
            return Err(invalid("requests", "commands and broker requests cannot be mixed in one response"));
        }
        let mut ids = std::collections::BTreeSet::new();
        for request in &self.requests {
            if !crate::event_identifier(&request.id) || !crate::identifier(&request.method, 96) || !ids.insert(&request.id) {
                return Err(invalid("requests", "invalid or duplicate request ID, or invalid method"));
            }
            json_size(&Value::Object(request.params.clone()), 65536, "requests.params")?;
        }
        for invocation in &self.commands {
            if invocation.command.is_empty()
                || invocation.command.len() > 128
                || !invocation.command.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            {
                return Err(invalid("commands.command", "invalid command ID"));
            }
        }
        if let Some(view) = &self.view {
            view.validate()?;
        }
        Ok(())
    }
}
