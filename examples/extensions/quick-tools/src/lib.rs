//! A complete user-authored extension. This guest crate is separate from the host workspace.
//! Request storage stays in a safe Rust buffer; only ABI export attributes need `unsafe`.
use serde_json::{Value, json};
use std::sync::Mutex;

const MANIFEST: &str = include_str!("../manifest.json");
static REQUEST: Mutex<Vec<u8>> = Mutex::new(Vec::new());

#[unsafe(no_mangle)]
pub extern "C" fn pce_api_version() -> u32 {
    2
}

fn packed(bytes: &[u8]) -> u64 {
    ((bytes.len() as u64) << 32) | bytes.as_ptr() as u64
}

#[unsafe(no_mangle)]
pub extern "C" fn pce_manifest() -> u64 {
    packed(MANIFEST.as_bytes())
}

#[unsafe(no_mangle)]
pub extern "C" fn pce_alloc(size: u32) -> *mut u8 {
    if size > 256 * 1024 {
        return std::ptr::null_mut();
    }
    let Ok(mut request) = REQUEST.lock() else { return std::ptr::null_mut() };
    request.resize(size as usize, 0);
    request.as_mut_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn pce_handle(ptr: *const u8, len: u32) -> u64 {
    let request = REQUEST
        .lock()
        .ok()
        .and_then(|request| {
            if request.as_ptr() != ptr {
                return None;
            }
            request.get(..len as usize).and_then(|bytes| serde_json::from_slice(bytes).ok())
        })
        .unwrap_or(Value::Null);
    let response = handle(&request);
    let bytes = serde_json::to_vec(&response).unwrap_or_else(|_| b"{\"message\":\"Could not encode response\"}".to_vec());
    packed(Box::leak(bytes.into_boxed_slice()))
}

fn handle(request: &Value) -> Value {
    if request.get("apiVersion").and_then(Value::as_u64) != Some(2) {
        return json!({"message": "Unsupported PhotoCraft Plugin API request"});
    }
    if request.get("document").is_none_or(Value::is_null) {
        return json!({"message": "Open an image before using Quick Tools."});
    }
    let values = request.get("values").unwrap_or(&Value::Null);
    let call = match request.get("event").and_then(Value::as_str) {
        Some("resize") => json!({"command": "image.imageSize", "params": {
            "percent": values.get("percent").and_then(Value::as_u64).unwrap_or(200),
            "resample": values.get("resample").and_then(Value::as_str).unwrap_or("lanczos")
        }}),
        Some("newLayer") => json!({"command": "layer.new.layer", "params": {"name": "Quick Tools layer"}}),
        Some("invert") => json!({"command": "image.adjustments.invert", "params": {}}),
        _ => return json!({"message": "Unknown action"}),
    };
    let runs = request.get("state").and_then(|s| s.get("runs")).and_then(Value::as_u64).unwrap_or(0).saturating_add(1);
    json!({"commands": [call], "state": {"runs": runs}, "message": format!("Action {runs} completed. Use Edit › Undo to reverse the edit.")})
}
