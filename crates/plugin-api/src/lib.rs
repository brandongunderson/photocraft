//! Standalone, data-only contracts for the PhotoCraft Plugin API.
//!
//! No editor, UI, image or plugin implementation is a dependency of this crate.
//! Deserialization is followed by explicit validation at every trust boundary.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod authoring;
mod capabilities;
mod contributions;
mod descriptor;
mod documents;
mod events;
mod protocol;
mod worker;

pub use authoring::*;
pub use capabilities::*;
pub use contributions::*;
pub use descriptor::*;
pub use documents::*;
pub use events::*;
pub use protocol::*;
pub use worker::*;

#[cfg(feature = "schema")]
pub mod schema;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Contract metadata version. Independent of the panel, filter and package ABIs.
pub const HOST_API_VERSION: u16 = 1;
pub const PANEL_API_VERSION: u32 = 2;
pub const MAX_DESCRIPTOR_BYTES: usize = 64 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;
pub const MAX_STATE_BYTES: usize = 128 * 1024;

/// Stable machine-readable failures. Callers should match codes, not prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    InvalidJson,
    InvalidDescriptor,
    LimitExceeded,
    IncompatibleApi,
    MissingCapability,
    UnsupportedVersion,
    PermissionDenied,
    InvalidHandle,
    StaleRevision,
    NotFound,
    Conflict,
    UnsupportedOperation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[error("{message}")]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default)]
    pub details: Value,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), details: json!({}) }
    }

    pub fn at(code: ErrorCode, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), details: json!({"path":path.into()}) }
    }
}

pub type Result<T> = std::result::Result<T, ApiError>;

pub(crate) fn invalid(path: &str, message: &str) -> ApiError {
    ApiError::at(ErrorCode::InvalidDescriptor, path, message)
}

pub(crate) fn identifier(id: &str, max: usize) -> bool {
    !id.is_empty()
        && id.len() <= max
        && id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

pub(crate) fn event_identifier(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && !id.starts_with('_') && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub(crate) fn label(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= max
        && value.chars().all(|c| !c.is_control() && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
}

pub(crate) fn json_size(value: &Value, max: usize, path: &str) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| ApiError::new(ErrorCode::InvalidJson, e.to_string()))?;
    if bytes.len() > max {
        return Err(ApiError::at(ErrorCode::LimitExceeded, path, format!("JSON value exceeds {max} bytes")));
    }
    Ok(())
}
