//! Fixed exports and safe, bounded guest-owned request/response storage.
use std::io::{self, Write};
use std::sync::Mutex;

use serde_json::Value;

use crate::{MANIFEST, MAX_JSON_BYTES, handler};

#[derive(Default)]
struct Buffers {
    request: Vec<u8>,
    response: Vec<u8>,
}

impl Buffers {
    fn allocate(&mut self, size: usize) -> Option<*mut u8> {
        if size == 0 || size > MAX_JSON_BYTES {
            return None;
        }
        if size > self.request.len() {
            self.request.try_reserve_exact(size - self.request.len()).ok()?;
        }
        self.request.resize(size, 0);
        Some(self.request.as_mut_ptr())
    }

    fn handle(&mut self, pointer: *const u8, size: usize) -> Option<&[u8]> {
        // Never dereference an ABI pointer. Accept only the exact buffer we allocated.
        if pointer != self.request.as_ptr() || size == 0 || size != self.request.len() || size > MAX_JSON_BYTES {
            return None;
        }
        let bytes = self.request.get(..size)?;
        let response = match serde_json::from_slice::<Value>(bytes) {
            Ok(request) => handler::handle(&request),
            Err(_) => serde_json::json!({"state": null, "message": "Invalid event request JSON.", "commands": []}),
        };
        self.response.clear();
        serde_json::to_writer(LimitedWriter(&mut self.response), &response).ok()?;
        Some(&self.response)
    }
}

struct LimitedWriter<'a>(&'a mut Vec<u8>);

impl Write for LimitedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = self.0.len().checked_add(bytes.len()).ok_or(io::ErrorKind::InvalidData)?;
        if length > MAX_JSON_BYTES {
            return Err(io::ErrorKind::InvalidData.into());
        }
        self.0.try_reserve(bytes.len()).map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

static BUFFERS: Mutex<Buffers> = Mutex::new(Buffers { request: Vec::new(), response: Vec::new() });

fn packed(bytes: &[u8]) -> u64 {
    let Ok(pointer) = u32::try_from(bytes.as_ptr() as usize) else { return 0 };
    let Ok(length) = u32::try_from(bytes.len()) else { return 0 };
    (u64::from(length) << 32) | u64::from(pointer)
}

// Rust 2024 requires unsafe(...) for export attributes. These narrowly scoped allowances
// cover symbol names only: there are no unsafe functions, blocks, or pointer dereferences.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pce_api_version() -> u32 {
    1
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pce_manifest() -> u64 {
    packed(MANIFEST.as_bytes())
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pce_alloc(size: u32) -> *mut u8 {
    let Ok(mut buffers) = BUFFERS.lock() else { return std::ptr::null_mut() };
    buffers.allocate(size as usize).unwrap_or(std::ptr::null_mut())
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pce_handle(pointer: *const u8, size: u32) -> u64 {
    let Ok(mut buffers) = BUFFERS.lock() else { return 0 };
    buffers.handle(pointer, size as usize).map_or(0, packed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_is_bounded_and_reusable() {
        let mut buffers = Buffers::default();
        assert!(buffers.allocate(0).is_none());
        assert!(buffers.allocate(MAX_JSON_BYTES + 1).is_none());
        assert!(buffers.allocate(MAX_JSON_BYTES).is_some());
        assert_eq!(buffers.request.len(), MAX_JSON_BYTES);
        assert!(buffers.allocate(2).is_some());
        assert_eq!(buffers.request.len(), 2);
    }

    #[test]
    fn handle_rejects_foreign_pointers_and_mismatched_lengths() {
        let mut buffers = Buffers::default();
        let pointer = buffers.allocate(2).unwrap();
        assert!(buffers.handle(std::ptr::null(), 2).is_none());
        assert!(buffers.handle(pointer, 1).is_none());
        assert!(buffers.handle(pointer, MAX_JSON_BYTES + 1).is_none());
    }

    #[test]
    fn malformed_json_has_a_valid_error_response_without_commands() {
        let mut buffers = Buffers::default();
        let pointer = buffers.allocate(2).unwrap();
        buffers.request.copy_from_slice(b"{!");
        let response: Value = serde_json::from_slice(buffers.handle(pointer, 2).unwrap()).unwrap();
        assert_eq!(response["commands"], serde_json::json!([]));
        assert!(response["message"].as_str().unwrap().contains("Invalid event request JSON"));
    }

    #[test]
    fn serialization_never_exceeds_the_host_budget() {
        let mut bytes = Vec::new();
        let mut writer = LimitedWriter(&mut bytes);
        writer.write_all(&vec![b'x'; MAX_JSON_BYTES]).unwrap();
        assert!(writer.write_all(b"x").is_err());
        assert_eq!(bytes.len(), MAX_JSON_BYTES);
    }

    #[test]
    fn unencodable_response_is_rejected_instead_of_clearing_saved_state() {
        let mut request = serde_json::json!({"event": "info", "values": {}, "state": "", "document": null});
        let overhead = serde_json::to_vec(&request).unwrap().len();
        request["state"] = serde_json::json!("x".repeat(MAX_JSON_BYTES - overhead));
        let bytes = serde_json::to_vec(&request).unwrap();
        assert_eq!(bytes.len(), MAX_JSON_BYTES);
        let mut buffers = Buffers::default();
        let pointer = buffers.allocate(bytes.len()).unwrap();
        buffers.request.copy_from_slice(&bytes);
        // The full malformed state plus explanatory message would exceed 256 KiB.
        assert!(buffers.handle(pointer, bytes.len()).is_none());
    }
}
