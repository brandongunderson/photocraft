//! Safe guest-owned ABI storage and typed public messages. No editor/UI dependency.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub use photocraft_plugin_api as api;
pub use serde_json;
pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");
use std::io::{self, Write};

#[cfg(feature = "authoring")]
pub mod authoring;

use api::{ApiError, ErrorCode, EventRequest, MAX_RESPONSE_BYTES, PanelResponse, Result};

/// A pure handler proposes effects; only PhotoCraft can authorize and commit them.
pub type Handler = fn(&EventRequest) -> Result<PanelResponse>;

pub struct GuestBuffers {
    request: Vec<u8>,
    response: Vec<u8>,
}

impl Default for GuestBuffers {
    fn default() -> Self {
        Self::new()
    }
}

impl GuestBuffers {
    pub const fn new() -> Self {
        Self { request: Vec::new(), response: Vec::new() }
    }

    pub fn allocate(&mut self, size: usize) -> Result<&mut [u8]> {
        if size == 0 || size > MAX_RESPONSE_BYTES {
            return Err(ApiError::new(ErrorCode::LimitExceeded, "request buffer needs 1 byte to 256 KiB"));
        }
        if size > self.request.len() {
            self.request.try_reserve_exact(size - self.request.len()).map_err(|e| ApiError::new(ErrorCode::LimitExceeded, e.to_string()))?;
        }
        self.request.resize(size, 0);
        Ok(&mut self.request)
    }

    /// Never dereference an ABI pointer. The host may only use this exact owned allocation.
    pub fn respond(&mut self, pointer: *const u8, length: usize, handler: Handler) -> Result<&[u8]> {
        if pointer != self.request.as_ptr() || length == 0 || length != self.request.len() {
            return Err(ApiError::new(ErrorCode::InvalidHandle, "foreign request buffer or mismatched length"));
        }
        let request = EventRequest::parse(&self.request)?;
        let response = match handler(&request) {
            Ok(response) => response,
            Err(error) => PanelResponse { state: request.state.clone(), message: error.message, ..PanelResponse::default() },
        };
        response.validate()?;
        self.response.clear();
        serde_json::to_writer(BoundedWriter(&mut self.response), &response).map_err(|e| ApiError::new(ErrorCode::LimitExceeded, e.to_string()))?;
        Ok(&self.response)
    }
}

struct BoundedWriter<'a>(&'a mut Vec<u8>);
impl Write for BoundedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = self.0.len().checked_add(bytes.len()).ok_or(io::ErrorKind::InvalidData)?;
        if length > MAX_RESPONSE_BYTES {
            return Err(io::ErrorKind::InvalidData.into());
        }
        self.0.try_reserve(bytes.len()).map_err(|_| io::ErrorKind::OutOfMemory)?;
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[doc(hidden)]
pub fn packed(bytes: &[u8]) -> u64 {
    let (Ok(pointer), Ok(length)) = (u32::try_from(bytes.as_ptr() as usize), u32::try_from(bytes.len())) else { return 0 };
    (u64::from(length) << 32) | u64::from(pointer)
}

/// Export the four import-free PhotoCraft Plugin API entry points. Rust 2024's narrow unsafe
/// export attributes name symbols only: generated functions contain no unsafe operations.
#[macro_export]
macro_rules! export_plugin {
    ($manifest:expr, $handler:path) => {
        static PHOTOCRAFT_GUEST_BUFFERS: ::std::sync::Mutex<$crate::GuestBuffers> = ::std::sync::Mutex::new($crate::GuestBuffers::new());
        #[allow(unsafe_code)]
        #[unsafe(export_name = "pce_api_version")]
        pub extern "C" fn photocraft_guest_api_version() -> u32 {
            $crate::api::PANEL_API_VERSION
        }
        #[allow(unsafe_code)]
        #[unsafe(export_name = "pce_manifest")]
        pub extern "C" fn photocraft_guest_manifest() -> u64 {
            $crate::packed(($manifest).as_bytes())
        }
        #[allow(unsafe_code)]
        #[unsafe(export_name = "pce_alloc")]
        pub extern "C" fn photocraft_guest_allocate(length: u32) -> *mut u8 {
            let Ok(mut buffers) = PHOTOCRAFT_GUEST_BUFFERS.lock() else { return ::std::ptr::null_mut() };
            buffers.allocate(length as usize).map_or(::std::ptr::null_mut(), |bytes| bytes.as_mut_ptr())
        }
        #[allow(unsafe_code)]
        #[unsafe(export_name = "pce_handle")]
        pub extern "C" fn photocraft_guest_handle(pointer: *const u8, length: u32) -> u64 {
            let Ok(mut buffers) = PHOTOCRAFT_GUEST_BUFFERS.lock() else { return 0 };
            buffers.respond(pointer, length as usize, $handler).map_or(0, $crate::packed)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(state: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"apiVersion":1,"host":{"version":1,"capabilities":[]},"event":"info","values":{},"state":state})).unwrap()
    }

    #[test]
    fn rejects_foreign_buffers_lengths_and_malformed_requests() {
        let mut guest = GuestBuffers::new();
        assert!(guest.allocate(0).is_err());
        assert!(guest.allocate(MAX_RESPONSE_BYTES + 1).is_err());
        let buffer = guest.allocate(2).unwrap();
        buffer.copy_from_slice(b"{!");
        let pointer = buffer.as_ptr();
        let handler: Handler = |_| Ok(PanelResponse::default());
        assert!(guest.respond(std::ptr::null(), 2, handler).is_err());
        assert!(guest.respond(pointer, 1, handler).is_err());
        assert!(guest.respond(pointer, 2, handler).is_err());
    }

    #[test]
    fn handler_errors_preserve_state_and_propose_no_effects() {
        let mut guest = GuestBuffers::new();
        let input = request(json!({"saved":7}));
        let buffer = guest.allocate(input.len()).unwrap();
        buffer.copy_from_slice(&input);
        let pointer = buffer.as_ptr();
        let response = PanelResponse::parse(guest.respond(pointer, input.len(), |_| Err(ApiError::new(ErrorCode::Conflict, "Try again"))).unwrap()).unwrap();
        assert_eq!(response.state, json!({"saved":7}));
        assert_eq!(response.message, "Try again");
        assert!(response.requests.is_empty() && response.commands.is_empty());
    }

    #[test]
    fn oversized_or_conflicting_responses_fail_closed() {
        let mut guest = GuestBuffers::new();
        let input = request(serde_json::Value::Null);
        let buffer = guest.allocate(input.len()).unwrap();
        buffer.copy_from_slice(&input);
        let pointer = buffer.as_ptr();
        assert!(guest.respond(pointer, input.len(), |_| Ok(PanelResponse { message: "x".repeat(4097), ..Default::default() })).is_err());
        assert!(guest.respond(pointer, input.len(), |_| Ok(PanelResponse { state: json!("x".repeat(129 * 1024)), ..Default::default() })).is_err());
    }

    #[test]
    fn serialized_output_is_bounded_independently_of_field_limits() {
        let mut bytes = Vec::new();
        let mut writer = BoundedWriter(&mut bytes);
        writer.write_all(&vec![b'x'; MAX_RESPONSE_BYTES]).unwrap();
        assert!(writer.write_all(b"x").is_err());
        assert_eq!(bytes.len(), MAX_RESPONSE_BYTES);
    }
}
