//! Host-to-guest events for the current PhotoCraft Plugin API.
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ApiError, ErrorCode, HostApi, MAX_RESPONSE_BYTES, MAX_STATE_BYTES, NegotiatedCapabilities, Result, json_size};

/// Additional event-envelope fields may be ignored by old guests. Descriptors and effects
/// remain strict. Unknown events must be handled without inventing new host authority.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct EventRequest {
    pub api_version: u32,
    pub host: HostApi,
    #[serde(default)]
    pub compatibility: Option<Compatibility>,
    pub event: String,
    #[serde(default)]
    pub values: Value,
    #[serde(default)]
    pub state: Value,
    #[serde(default)]
    pub document: Value,
    #[serde(default)]
    pub responses: Vec<BrokerReply>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Compatibility {
    pub panel_api: u32,
    pub host_api: u16,
    pub negotiated: NegotiatedCapabilities,
}

/// Exactly one result or error, preserving a successful JSON null if a later method uses it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged, deny_unknown_fields)]
pub enum BrokerReply {
    Success { id: String, result: Value },
    Failure { id: String, error: ApiError },
}

impl BrokerReply {
    pub fn id(&self) -> &str {
        match self {
            Self::Success { id, .. } | Self::Failure { id, .. } => id,
        }
    }

    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        match self {
            Self::Success { result, .. } => serde_json::from_value(result.clone()).map_err(|e| ApiError::new(ErrorCode::InvalidJson, e.to_string())),
            Self::Failure { error, .. } => Err(error.clone()),
        }
    }
}

impl EventRequest {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(ApiError::new(ErrorCode::LimitExceeded, "event request exceeds 256 KiB"));
        }
        let request: Self = serde_json::from_slice(bytes).map_err(|e| ApiError::new(ErrorCode::InvalidJson, e.to_string()))?;
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<()> {
        if self.api_version != crate::PANEL_API_VERSION {
            return Err(ApiError::new(ErrorCode::IncompatibleApi, "guest requires the current PhotoCraft Plugin API"));
        }
        self.host.validate()?;
        if self.compatibility.as_ref().is_some_and(|selected| selected.panel_api != self.api_version || selected.host_api != self.host.version) {
            return Err(ApiError::new(ErrorCode::IncompatibleApi, "negotiated protocol versions disagree"));
        }
        if !crate::event_identifier(&self.event) || self.responses.len() > 8 {
            return Err(ApiError::new(ErrorCode::InvalidJson, "invalid event name or response count"));
        }
        json_size(&self.state, MAX_STATE_BYTES, "state")?;
        let mut ids = std::collections::BTreeSet::new();
        for response in &self.responses {
            if !crate::event_identifier(response.id()) || !ids.insert(response.id()) {
                return Err(ApiError::new(ErrorCode::InvalidJson, "invalid or duplicate broker response ID"));
            }
        }
        Ok(())
    }

    pub fn reply(&self, id: &str) -> Result<&BrokerReply> {
        self.responses.iter().find(|reply| reply.id() == id).ok_or_else(|| ApiError::new(ErrorCode::NotFound, "broker reply is missing"))
    }
}
