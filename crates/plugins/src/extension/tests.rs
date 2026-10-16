use super::*;
use serde_json::json;

fn manifest() -> Value {
    json!({"apiVersion":1, "id":"org.example.panel", "name":"Example", "version":"1.0.0",
        "permissions":{"commands":["image.adjustments.invert"]},
        "panel":{"fields":[{"id":"amount", "label":"Amount", "type":"int", "min":1, "max":100, "default":50}],
            "buttons":[{"id":"apply", "label":"Apply"}]}})
}

fn wasm(manifest: &Value, response: &str, body: Option<&str>) -> Vec<u8> {
    wasm_for_version(manifest, response, body, photocraft_plugin_api::PANEL_API_VERSION as i32)
}

fn wasm_for_version(manifest: &Value, response: &str, body: Option<&str>, version: i32) -> Vec<u8> {
    let manifest = manifest.to_string();
    let quote = |s: &str| s.bytes().map(|b| format!("\\{b:02x}")).collect::<String>();
    let handler = body.map(str::to_owned).unwrap_or_else(|| format!("i64.const {}", ((response.len() as u64) << 32) | 65536));
    wat::parse_str(format!(
        r#"(module
        (memory (export "memory") 4 512)
        (data (i32.const 32) "{}") (data (i32.const 65536) "{}")
        (func (export "pce_api_version") (result i32) i32.const {})
        (func (export "pce_manifest") (result i64) i64.const {})
        (func (export "pce_alloc") (param i32) (result i32) i32.const 196608)
        (func (export "pce_handle") (param i32 i32) (result i64) {})
    )"#,
        quote(&manifest),
        quote(response),
        version,
        ((manifest.len() as u64) << 32) | 32,
        handler
    ))
    .unwrap()
}

#[test]
fn manifest_validates_schema_defaults_and_grants() {
    let m = Manifest::parse(&serde_json::to_vec(&manifest()).unwrap()).unwrap();
    assert_eq!(m.values(&Value::Null).unwrap()["amount"], 50);
    assert_eq!(m.values(&json!({"amount":999})).unwrap()["amount"], 100);
    assert!(m.values(&json!({"unknown":1})).is_err());
    assert!(m.values(&json!({"amount":"bad"})).is_err());
    for id in ["../escape", "Org.example", "", "-bad", ".hidden"] {
        let mut value = manifest();
        value["id"] = json!(id);
        assert!(Manifest::parse(&serde_json::to_vec(&value).unwrap()).is_err(), "{id}");
    }
    let mut value = manifest();
    value["apiVersion"] = json!(3);
    assert!(Manifest::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value = manifest();
    value["panel"]["buttons"] = json!([]);
    assert!(Manifest::parse(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn unsupported_descriptor_markers_are_rejected() {
    for version in [0, 2, 3, u32::MAX] {
        let mut value = manifest();
        value["apiVersion"] = json!(version);
        assert!(
            matches!(Manifest::parse(&serde_json::to_vec(&value).unwrap()), Err(Error::Manifest(message)) if message.contains("current PhotoCraft Plugin API")),
            "{version}"
        );
    }
}

#[test]
fn current_responses_accept_optional_broker_fields_through_one_parser() {
    for response in [r#"{"requests":[]}"#, r#"{"view":null}"#] {
        assert!(Response::parse(response.as_bytes()).is_ok());
    }
    let current = Manifest::parse(&serde_json::to_vec(&manifest()).unwrap()).unwrap();
    let serialized = serde_json::to_value(&current).unwrap();
    assert_eq!(serialized["apiVersion"], photocraft_plugin_api::PANEL_API_VERSION);
    assert!(serialized.get("capabilities").is_none());
    assert!(serialized.get("contributes").is_none());
}

fn inspector_manifest() -> Value {
    let mut value = manifest();
    value["capabilities"] = json!({"required":[{"id":"host.document.read","minVersion":1}],"optional":[{"id":"future.optional","minVersion":1}]});
    value["contributes"] = json!({
        "inspectors":[{"id":"org.example.panel.inspector","label":"Inspector","event":"inspect"}],
        "subscriptions":[{"id":"org.example.panel.changes","event":"document.changed","handler":"inspect"}]
    });
    value["panel"]["buttons"] = json!([]);
    value
}

#[test]
fn panel_runtime_round_trips_inspector_and_broker_requests() {
    let value = inspector_manifest();
    let bytes = wasm(
        &value,
        r#"{"state":{"count":1},"view":{"title":"Layers","rows":[{"label":"Layers","value":"3"}]},"requests":[{"id":"read","method":"host.document.read","params":{}}]}"#,
        None,
    );
    let extension = Extension::load(&bytes).unwrap();
    assert_eq!(extension.manifest.api_version, photocraft_plugin_api::PANEL_API_VERSION);
    assert!(extension.manifest.panel.buttons.is_empty());
    let response = extension.handle(&json!({"event":"inspect"})).unwrap();
    assert_eq!(response.requests.len(), 1);
    assert_eq!(response.view.unwrap().rows[0].value, "3");
}

#[test]
fn exported_and_embedded_panel_versions_require_the_current_api() {
    let current = manifest();
    assert!(Extension::load(&wasm(&current, "{}", None)).is_ok());
    for version in [-1, 0, 2, 3] {
        assert!(
            matches!(Extension::load(&wasm_for_version(&current, "{}", None, version)), Err(Error::Abi(message)) if message.contains("current PhotoCraft Plugin API")),
            "{version}"
        );
    }
    let mut invalid = current;
    invalid["apiVersion"] = json!(0);
    assert!(matches!(Extension::load(&wasm(&invalid, "{}", None)), Err(Error::Manifest(message)) if message.contains("current PhotoCraft Plugin API")));
}

#[test]
fn unsupported_panel_version_is_rejected_before_running_its_manifest_export() {
    let invalid = wat::parse_str(
        r#"(module
        (memory (export "memory") 1)
        (func (export "pce_api_version") (result i32) i32.const 0)
        (func (export "pce_manifest") (result i64) unreachable))"#,
    )
    .unwrap();
    assert!(matches!(Extension::load(&invalid), Err(Error::Abi(message)) if message.contains("current PhotoCraft Plugin API")));
}

#[test]
fn panel_descriptors_and_response_budgets_are_validated_before_execution() {
    let mut value = inspector_manifest();
    value["contributes"]["inspectors"][0]["id"] = json!("another.plugin.inspector");
    assert!(Manifest::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    for response in [
        json!({"commands":[{"command":"edit.fill","params":{}}],"requests":[{"id":"read","method":"host.document.read","params":{}}]}),
        json!({"requests":[{"id":"read","method":"host.document.read","params":{}},{"id":"read","method":"host.document.read","params":{}}]}),
        json!({"requests":[{"id":"read","method":"host.document.read","params":null}]}),
        json!({"view":{"rows":[{"label":"Layers","value":"a".repeat(1025)}]}}),
        json!({"requests":(0..9).map(|i|json!({"id":format!("request{i}"),"method":"host.document.read","params":{}})).collect::<Vec<_>>()}),
    ] {
        assert!(Response::parse(&serde_json::to_vec(&response).unwrap()).is_err());
    }
}

#[test]
fn native_image_preview_is_an_optional_boolean_panel_contribution() {
    let parse = |value: &Value| Manifest::parse(&serde_json::to_vec(value).unwrap());
    let original = manifest();
    let parsed = parse(&original).unwrap();
    assert!(!parsed.panel.preview);
    assert!(serde_json::to_value(&parsed).unwrap()["panel"].get("preview").is_none());
    let mut value = original;
    value["panel"]["preview"] = json!(true);
    assert!(parse(&value).unwrap().panel.preview);
    for invalid in [json!("true"), json!(1), Value::Null, json!({})] {
        value["panel"]["preview"] = invalid;
        assert!(parse(&value).is_err());
    }
}

#[test]
fn guest_logic_returns_commands_state_and_message() {
    let bytes = wasm(&manifest(), r#"{"message":"done","state":{"runs":1},"commands":[{"command":"image.adjustments.invert","params":{}}]}"#, None);
    let extension = Extension::load(&bytes).unwrap();
    let result = extension.handle(&json!({"event":"apply"})).unwrap();
    assert_eq!(result.state["runs"], 1);
    assert_eq!(result.commands[0].command, "image.adjustments.invert");
}

#[test]
fn infinite_loops_traps_invalid_json_and_bad_pointers_are_errors() {
    for body in ["(loop $forever br $forever) i64.const 0", "unreachable", "i64.const -1"] {
        let extension = Extension::load(&wasm(&manifest(), "{}", Some(body))).unwrap();
        assert!(extension.handle(&json!({})).is_err(), "{body}");
    }
    let extension = Extension::load(&wasm(&manifest(), "not json", None)).unwrap();
    assert!(extension.handle(&json!({})).is_err());
    let extension = Extension::load(&wasm(&manifest(), "{}", None)).unwrap();
    assert!(extension.handle(&json!({"data":"x".repeat(MAX_JSON_BYTES)})).is_err());
}

#[test]
fn imports_start_loops_and_oversized_memory_are_rejected() {
    for text in [
        r#"(module (import "wasi_snapshot_preview1" "fd_write" (func)))"#,
        r#"(module (func $start (loop $forever br $forever)) (start $start))"#,
        r#"(module (memory (export "memory") 513))"#,
    ] {
        assert!(Extension::load(&wat::parse_str(text).unwrap()).is_err());
    }
}

#[test]
fn malformed_responses_are_rejected_before_host_execution() {
    for response in [r#"{"commands":[{"command":"edit.fill","params":null}]}"#, r#"{"unexpected":true}"#] {
        assert!(Response::parse(response.as_bytes()).is_err());
    }
    assert!(
        Response::parse(&serde_json::to_vec(&json!({"commands": (0..33).map(|_| json!({"command":"edit.fill","params":{}})).collect::<Vec<_>>() })).unwrap())
            .is_err()
    );
}

#[test]
fn response_state_has_headroom_for_the_next_request() {
    // A JSON string includes two quote bytes. The response can be valid and below its own
    // limit while its state would otherwise consume almost the entire next request.
    let accepted = json!({"state": "x".repeat(MAX_STATE_BYTES - 2)});
    let bytes = serde_json::to_vec(&accepted).unwrap();
    assert_eq!(serde_json::to_vec(&Response::parse(&bytes).unwrap().state).unwrap().len(), MAX_STATE_BYTES);

    let oversized = json!({"state": "x".repeat(MAX_STATE_BYTES - 1)});
    let bytes = serde_json::to_vec(&oversized).unwrap();
    assert!(bytes.len() < MAX_JSON_BYTES);
    assert!(matches!(Response::parse(&bytes), Err(Error::Limit(message)) if message.contains("state")));
}
