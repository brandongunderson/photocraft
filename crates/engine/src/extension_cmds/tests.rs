use super::*;

fn manifest() -> Value {
    json!({"apiVersion":1, "id":"org.example.panel", "name":"Example", "version":"1.0.0",
        "capabilities":{"required":[{"id":"host.commands.invoke","minVersion":1}]},
        "permissions":{"commands":["image.adjustments.invert", "layer.new.layer"]},
        "panel":{"buttons":[{"id":"apply", "label":"Apply"}]}})
}

#[test]
fn allowlist_contains_only_registered_commands() {
    let missing = COMMANDS.iter().filter(|id| crate::commands::find(id).is_none()).collect::<Vec<_>>();
    assert!(missing.is_empty(), "unregistered extension commands: {missing:?}");
}

#[test]
fn plugin_file_review_compares_versions_and_requires_explicit_replacement_intent() {
    assert_eq!(disposition(None, "1.0.0").0, "install");
    assert_eq!(disposition(Some("1.0.0"), "1.1.0").0, "update");
    assert_eq!(disposition(Some("1.1.0"), "1.0.0").0, "downgrade");
    assert_eq!(disposition(Some("1.0.0"), "1.0.0+rebuild").0, "reinstall");
    assert_eq!(disposition(Some("nightly"), "1.0.0").0, "replace");
    let mut session = Session::new();
    install(&mut session, json!({}));
    let mut candidate = manifest();
    candidate["version"] = json!("1.1.0");
    let mut input = json!({"data":b64(&wasm(&candidate, &json!({})))});
    let review = session.execute("extension.inspect", input.clone()).unwrap();
    assert_eq!(review["installation"]["action"], "update");
    input.as_object_mut().unwrap().extend(review["installation"].as_object().unwrap().clone());
    input["expectedHash"] = review["hash"].clone();
    input["intent"] = json!("install");
    assert!(session.execute("extension.install", input.clone()).is_err());
    input["intent"] = json!("update");
    session.execute("extension.enable", json!({"id":"org.example.panel","enabled":false})).unwrap();
    assert!(session.execute("extension.install", input.clone()).err().unwrap().to_string().contains("review"));
    let review = session.execute("extension.inspect", input.clone()).unwrap();
    input.as_object_mut().unwrap().extend(review["installation"].as_object().unwrap().clone());
    assert_eq!(session.execute("extension.install", input).unwrap()["action"], "update");
    assert_eq!(session.extensions.installed["org.example.panel"].extension.manifest.version, "1.1.0");
    assert!(!session.extensions.installed["org.example.panel"].enabled);
}

fn wasm(manifest: &Value, response: &Value) -> Vec<u8> {
    let manifest = manifest.to_string();
    let response = response.to_string();
    let quote = |s: &str| s.bytes().map(|b| format!("\\{b:02x}")).collect::<String>();
    wat::parse_str(format!(
        r#"(module
        (memory (export "memory") 12 512)
        (data (i32.const 32) "{}") (data (i32.const 65536) "{}")
        (func (export "pce_api_version") (result i32) i32.const 1)
        (func (export "pce_manifest") (result i64) i64.const {})
        (func (export "pce_alloc") (param i32) (result i32) i32.const 393216)
        (func (export "pce_handle") (param i32 i32) (result i64) i64.const {})
    )"#,
        quote(&manifest),
        quote(&response),
        ((manifest.len() as u64) << 32) | 32,
        ((response.len() as u64) << 32) | 65536
    ))
    .unwrap()
}

fn b64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            out.push(if i <= chunk.len() { TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

fn install(s: &mut Session, response: Value) {
    s.execute("extension.install", json!({"data":b64(&wasm(&manifest(), &response))})).unwrap();
}

#[test]
fn standalone_storage_is_reported_for_inspection_installation_and_listing() {
    let bytes = wasm(&manifest(), &json!({"message":"ready"}));
    let params = json!({"data":b64(&bytes)});
    let expected = json!({"installedBytes": bytes.len(), "modelBytes": 0});
    let mut session = Session::new();
    let review = session.execute("extension.inspect", params.clone()).unwrap();
    assert_eq!(review["storage"], expected);
    assert_eq!(review["downloadBytes"], bytes.len());
    let installed = session.execute("extension.install", params).unwrap();
    assert_eq!(installed["storage"], expected);
    let listed = session.execute("extension.list", json!({})).unwrap();
    assert_eq!(listed["extensions"][0]["storage"], expected);
    assert_eq!(session.extensions.installed["org.example.panel"].storage.installed_bytes, bytes.len() as u64);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn standalone_storage_survives_raw_module_load_update_and_restart() {
    let folder = tempfile::tempdir().unwrap();
    let bytes = wasm(&manifest(), &json!({"message":"raw module"}));
    std::fs::write(folder.path().join("org.example.panel.pcext"), &bytes).unwrap();
    let mut session = Session::new();
    session.extensions.attach_folder(folder.path().into()).unwrap();
    assert_eq!(session.extensions.installed["org.example.panel"].storage, StorageSize::for_module(&bytes));

    let updated = wasm(&manifest(), &json!({"message":"a longer replacement message"}));
    session.execute("extension.install", json!({"data":b64(&updated),"replace":true})).unwrap();
    let expected = StorageSize::for_module(&updated);
    assert_ne!(expected.installed_bytes, bytes.len() as u64);
    assert_eq!(session.extensions.installed["org.example.panel"].storage, expected);
    let mut restarted = Session::new();
    restarted.extensions.attach_folder(folder.path().into()).unwrap();
    assert!(restarted.extensions.load_errors.is_empty());
    assert_eq!(restarted.extensions.installed["org.example.panel"].storage, expected);
    let listed = restarted.execute("extension.list", json!({})).unwrap();
    assert_eq!(listed["extensions"][0]["storage"], serde_json::to_value(expected).unwrap());
}

fn invoke(s: &mut Session) -> Result<Value> {
    s.execute("extension.invoke", json!({"id":"org.example.panel", "event":"apply", "values":{}}))
}

#[test]
fn image_processing_is_exclusive_and_cannot_bypass_command_authorization() {
    check_native_command_authorization("plugin.processImage");
}

fn check_native_command_authorization(service: &str) {
    let mut s = Session::new();
    s.execute("file.new", json!({"width":8,"height":8})).unwrap();
    let source = Arc::clone(&s.active().unwrap().doc);
    let mut m = manifest();
    m["permissions"]["commands"] = json!([service, "layer.new.layer"]);
    let calls = json!([{"command":"layer.new.layer","params":{}},{"command":service,"params":{}}]);
    let bytes = wasm(&m, &json!({"state":{"runs":1},"commands":calls}));
    s.execute("extension.install", json!({"data":b64(&bytes)})).unwrap();
    assert!(invoke(&mut s).unwrap_err().to_string().contains("only command"));
    assert!(Arc::ptr_eq(&source, &s.active().unwrap().doc));
    let bytes = wasm(&m, &json!({"state":{"runs":1},"commands":[{"command":service,"params":{}}]}));
    s.execute("extension.install", json!({"data":b64(&bytes),"replace":true})).unwrap();
    s.authorize = Some(|id, _| if matches!(id, "plugin.processImage") { Err(error("native processing denied")) } else { Ok(()) });
    assert!(invoke(&mut s).unwrap_err().to_string().contains("native processing denied"));
    assert!(s.extensions.installed["org.example.panel"].state.is_null());
    assert!(!s.has_jobs());
}

#[test]
fn installed_code_edits_document_through_engine_and_undo() {
    let mut s = Session::new();
    s.execute("file.new", json!({"width":8,"height":8,"background":"black"})).unwrap();
    install(&mut s, json!({"commands":[{"command":"image.adjustments.invert","params":{}}], "state":{"runs":1}, "message":"done"}));
    let before = s.active().unwrap().doc.clone();
    assert_eq!(invoke(&mut s).unwrap()["message"], "done");
    assert_ne!(s.active().unwrap().doc, before);
    assert_eq!(s.extensions.installed["org.example.panel"].state["runs"], 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc, before);
    assert_eq!(Session::new().extensions.installed.len(), 0);
}

#[test]
fn grants_are_checked_for_entire_response_before_any_edit() {
    let mut s = Session::new();
    s.execute("file.new", json!({"width":8,"height":8})).unwrap();
    install(&mut s, json!({"commands":[{"command":"layer.new.layer","params":{}}, {"command":"file.new","params":{}}]}));
    let before = s.active().unwrap().doc.clone();
    assert!(invoke(&mut s).unwrap_err().to_string().contains("ungranted"));
    assert_eq!(s.active().unwrap().doc, before);
    s.execute("extension.enable", json!({"id":"org.example.panel","enabled":false})).unwrap();
    assert!(invoke(&mut s).unwrap_err().to_string().contains("disabled"));
}

#[test]
fn oversized_state_is_rejected_before_edits_and_preserves_prior_state() {
    let mut s = Session::new();
    s.execute("file.new", json!({"width":8,"height":8})).unwrap();
    install(
        &mut s,
        json!({"state":"x".repeat(photocraft_plugins::extension::MAX_STATE_BYTES - 1),
            "commands":[{"command":"layer.new.layer","params":{}}]}),
    );
    let before = Arc::clone(&s.active().unwrap().doc);
    let prior = json!({"runs": 7});
    s.extensions.installed.get_mut("org.example.panel").unwrap().state = prior.clone();
    for _ in 0..2 {
        let failure = invoke(&mut s).unwrap_err().to_string();
        assert!(failure.contains("state"), "{failure}");
        assert!(!failure.contains("request size"), "the old state must remain usable");
        assert!(Arc::ptr_eq(&before, &s.active().unwrap().doc));
        assert_eq!(s.extensions.installed["org.example.panel"].state, prior);
    }
}

#[test]
fn ambient_host_commands_and_recursive_extensions_cannot_be_granted() {
    for command in ["file.open", "prefs.set", "actions.play", "extension.invoke", "plugin.run", "filter.distort.displace"] {
        let mut m = manifest();
        m["permissions"]["commands"] = json!([command]);
        let mut s = Session::new();
        assert!(s.execute("extension.install", json!({"data":b64(&wasm(&m, &json!({})))})).is_err(), "{command}");
    }
}

#[test]
fn nested_edits_honor_session_authorization() {
    let mut s = Session::new();
    install(&mut s, json!({"commands":[{"command":"layer.new.layer","params":{}}]}));
    s.authorize = Some(|id, _| if id == "layer.new.layer" { Err(error("denied by caller")) } else { Ok(()) });
    assert!(invoke(&mut s).unwrap_err().to_string().contains("denied by caller"));
}

#[test]
fn malformed_events_values_and_failed_updates_keep_old_extension() {
    let mut s = Session::new();
    install(&mut s, json!({"message":"original"}));
    assert!(s.execute("extension.install", json!({"data":"AAAA","replace":true})).is_err());
    assert_eq!(invoke(&mut s).unwrap()["message"], "original");
    assert!(s.execute("extension.invoke", json!({"id":"org.example.panel","event":"missing"})).is_err());
    assert!(s.execute("extension.invoke", json!({"id":"org.example.panel","event":"apply","values":true})).is_err());
    assert!(s.execute("extension.enable", json!({"id":"org.example.panel","enabled":"true"})).is_err());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn install_update_disable_and_remove_survive_restarts() {
    let folder = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    s.extensions.attach_folder(folder.path().into()).unwrap();
    install(&mut s, json!({"message":"one"}));
    assert!(folder.path().join("org.example.panel.installation.json").is_file());
    s.execute("extension.enable", json!({"id":"org.example.panel","enabled":false})).unwrap();
    let mut next = Session::new();
    next.extensions.attach_folder(folder.path().into()).unwrap();
    assert!(!next.extensions.installed["org.example.panel"].enabled);
    next.execute("extension.enable", json!({"id":"org.example.panel","enabled":true})).unwrap();
    next.execute("extension.install", json!({"data":b64(&wasm(&manifest(), &json!({"message":"two"}))),"replace":true})).unwrap();
    assert_eq!(invoke(&mut next).unwrap()["message"], "two");
    next.execute("extension.enable", json!({"id":"org.example.panel","enabled":false})).unwrap();
    next.execute("extension.remove", json!({"id":"org.example.panel"})).unwrap();
    assert!(!folder.path().join("org.example.panel.disabled").exists());
    let mut last = Session::new();
    last.extensions.attach_folder(folder.path().into()).unwrap();
    assert!(last.extensions.installed.is_empty());
    std::fs::write(folder.path().join("bad.pcext"), b"bad module").unwrap();
    last.extensions.attach_folder(folder.path().into()).unwrap();
    assert_eq!(last.extensions.load_errors.len(), 1);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn failed_reinstall_and_explicit_repair_preserve_disabled_installations() {
    let folder = tempfile::tempdir().unwrap();
    let mut session = Session::new();
    session.extensions.attach_folder(folder.path().into()).unwrap();
    install(&mut session, json!({"message":"original"}));
    session.execute("extension.enable", json!({"id":"org.example.panel","enabled":false})).unwrap();
    let marker = folder.path().join("org.example.panel.disabled");
    let record = folder.path().join("org.example.panel.installation.json");
    std::fs::write(&record, b"{ malformed metadata").unwrap();

    let mut restarted = Session::new();
    restarted.extensions.attach_folder(folder.path().into()).unwrap();
    assert!(!restarted.extensions.installed.contains_key("org.example.panel"));
    let data = b64(&wasm(&manifest(), &json!({"message":"repaired"})));
    assert!(restarted.execute("extension.install", json!({"data":data})).is_err());
    assert!(marker.is_file(), "a failed install must preserve the user's disabled choice");

    let repaired = restarted.execute("extension.install", json!({"data":data,"replace":true})).unwrap();
    assert_eq!(repaired["enabled"], false);
    assert!(marker.is_file());
    assert!(!restarted.extensions.installed["org.example.panel"].enabled);
    assert!(invoke(&mut restarted).unwrap_err().to_string().contains("disabled"));
    let mut verified = Session::new();
    verified.extensions.attach_folder(folder.path().into()).unwrap();
    assert!(!verified.extensions.installed["org.example.panel"].enabled);
    verified.execute("extension.enable", json!({"id":"org.example.panel","enabled":true})).unwrap();
    assert!(!marker.exists());
    assert_eq!(invoke(&mut verified).unwrap()["message"], "repaired");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn new_install_keeps_an_existing_disabled_marker_until_explicit_enable() {
    let folder = tempfile::tempdir().unwrap();
    let marker = folder.path().join("org.example.panel.disabled");
    std::fs::write(&marker, b"disabled").unwrap();
    let mut session = Session::new();
    session.extensions.attach_folder(folder.path().into()).unwrap();
    install(&mut session, json!({"message":"new installation"}));
    assert!(!session.extensions.installed["org.example.panel"].enabled);
    assert_eq!(std::fs::read(&marker).unwrap(), b"disabled");
    session.execute("extension.enable", json!({"id":"org.example.panel","enabled":true})).unwrap();
    assert!(!marker.exists());
    assert_eq!(invoke(&mut session).unwrap()["message"], "new installation");
}

#[test]
fn reviewed_content_cannot_be_swapped_at_installation() {
    let data = b64(&wasm(&manifest(), &json!({})));
    let mut s = Session::new();
    let review = s.execute("extension.inspect", json!({"data":data})).unwrap();
    assert!(s.execute("extension.install", json!({"data":data,"expectedHash":"changed"})).is_err());
    s.execute("extension.install", json!({"data":data,"expectedHash":review["hash"]})).unwrap();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "build examples/extensions/quick-tools for wasm32-unknown-unknown first"]
fn authored_rust_extension_resizes_adds_layers_and_retains_state() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/extensions/quick-tools/target/wasm32-unknown-unknown/release/photocraft_extension_quick_tools.wasm"
    );
    let mut s = Session::new();
    s.execute("extension.install", json!({"path":path})).unwrap();
    s.execute("file.new", json!({"width":12,"height":8,"background":"black"})).unwrap();
    s.execute("extension.invoke", json!({"id":"org.photocraft.quick-tools","event":"resize","values":{"percent":200,"resample":"nearest"}})).unwrap();
    assert_eq!(s.active().unwrap().doc.size.width, 24);
    assert_eq!(s.active().unwrap().doc.size.height, 16);
    s.execute("extension.invoke", json!({"id":"org.photocraft.quick-tools","event":"newLayer"})).unwrap();
    assert_eq!(s.active().unwrap().doc.layers.len(), 2);
    assert_eq!(s.extensions.installed["org.photocraft.quick-tools"].state["runs"], 2);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc.layers.len(), 1);
}
