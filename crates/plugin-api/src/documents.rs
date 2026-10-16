//! Serialized document broker contracts. Handles never expose editor object layouts.
use serde::{Deserialize, Serialize};

use crate::{ApiError, BrokerRequest, ErrorCode, Result, ScopedHandle};

pub const LAYERS_READ: &str = "documents.layers.read";
pub const LAYERS_EDIT: &str = "documents.layers.edit";

fn request<T: Serialize>(id: &str, method: &str, params: &T) -> Result<BrokerRequest> {
    if !crate::event_identifier(id) {
        return Err(ApiError::new(ErrorCode::InvalidJson, "invalid request ID"));
    }
    let params = serde_json::to_value(params).map_err(|e| ApiError::new(ErrorCode::InvalidJson, e.to_string()))?;
    let params = params.as_object().cloned().ok_or_else(|| ApiError::new(ErrorCode::InvalidJson, "broker parameters must be an object"))?;
    Ok(BrokerRequest { id: id.into(), method: method.into(), params })
}

fn handle(value: &ScopedHandle) -> Result<()> {
    if value.0.is_empty() || value.0.len() > 192 {
        return Err(ApiError::new(ErrorCode::InvalidHandle, "invalid handle representation"));
    }
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayersRead {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<ScopedHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ScopedHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

impl LayersRead {
    pub fn request(&self, id: &str) -> Result<BrokerRequest> {
        if self.limit.is_some_and(|n| !(1..=128).contains(&n)) {
            return Err(ApiError::new(ErrorCode::LimitExceeded, "layer page needs 1–128 rows"));
        }
        for value in self.document.iter().chain(self.cursor.iter()) {
            handle(value)?;
        }
        request(id, LAYERS_READ, self)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayersEdit {
    pub document: ScopedHandle,
    pub expected_revision: u64,
    pub edits: Vec<LayerEdit>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct LayerEdit {
    pub layer: ScopedHandle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
}

impl LayersEdit {
    pub fn request(&self, id: &str) -> Result<BrokerRequest> {
        handle(&self.document)?;
        if self.edits.is_empty() || self.edits.len() > 128 {
            return Err(ApiError::new(ErrorCode::LimitExceeded, "edit batch needs 1–128 operations"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for edit in &self.edits {
            handle(&edit.layer)?;
            if !seen.insert(&edit.layer.0) {
                return Err(ApiError::new(ErrorCode::InvalidHandle, "layer handles must be distinct"));
            }
            if edit.name.is_none() && edit.visible.is_none() {
                return Err(ApiError::new(ErrorCode::InvalidJson, "layer edit needs a name or visibility"));
            }
            if edit.name.as_ref().is_some_and(|name| name.trim().is_empty() || name.chars().count() > 256 || name.chars().any(char::is_control)) {
                return Err(ApiError::new(ErrorCode::InvalidJson, "layer names need 1–256 printable characters"));
            }
        }
        request(id, LAYERS_EDIT, self)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct LayersSnapshot {
    pub document: Option<ScopedHandle>,
    pub revision: Option<u64>,
    pub name: String,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub depth: Option<u32>,
    pub layers: Vec<LayerSnapshot>,
    pub total_layers: usize,
    pub next_cursor: Option<ScopedHandle>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LayerSnapshot {
    pub handle: ScopedHandle,
    pub name: String,
    pub visible: bool,
    pub selected: bool,
    pub kind: String,
    pub depth: usize,
    pub locked: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LayersEdited {
    pub edited: usize,
    pub revision: Option<u64>,
}
