use std::time::{Duration, Instant};

use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{Document, Layer};
use photocraft_geom::Size;
use photocraft_plugin_api::BrokerRequest;

use super::*;

const OWNER: &str = "org.example.platform";

fn descriptor(id: &str, capabilities: &[KnownCapability]) -> Value {
    json!({"apiVersion":2,"id":id,"name":"Neutral platform fixture","version":"0.1.0",
        "permissions":{"documentRead":true,"commands":[]},
        "panel":{"fields":[],"buttons":[{"id":"apply","label":"Apply"}]},
        "capabilities":{"required":capabilities.iter().map(|id|json!({"id":id.as_str(),"minVersion":1})).collect::<Vec<_>>()}})
}

/// This synthetic guest returns request.state when it is an object; otherwise the fixed
/// fallback reply. It lets tests prepare a response containing real, already-issued handles
/// without replacing the installed module (which would correctly revoke those handles).
/// Test payload strings contain no braces; this intentionally tiny scanner is not an SDK.
fn guest(manifest: &Value, fallback: &Value) -> Vec<u8> {
    let manifest_bytes = manifest.to_string();
    let fallback_bytes = fallback.to_string();
    let quote = |bytes: &str| bytes.bytes().map(|byte| format!("\\{byte:02x}")).collect::<String>();
    wat::parse_str(format!(
        r#"(module
          (memory (export "memory") 12 512)
          (data (i32.const 32) "{}")
          (data (i32.const 65536) "{}")
          (func (export "pce_api_version") (result i32) i32.const {})
          (func (export "pce_manifest") (result i64) i64.const {})
          (func (export "pce_alloc") (param i32) (result i32) i32.const 393216)
          (func (export "pce_handle") (param $ptr i32) (param $len i32) (result i64)
            (local $i i32) (local $start i32) (local $depth i32) (local $char i32)
            (block $fallback
              (loop $search
                local.get $i i32.const 8 i32.add local.get $len i32.gt_u br_if $fallback
                local.get $ptr local.get $i i32.add i64.load align=1
                i64.const 0x3a22657461747322 i64.eq
                (if
                  (then
                    local.get $i i32.const 8 i32.add local.tee $i local.set $start
                    local.get $ptr local.get $i i32.add i32.load8_u i32.const 123 i32.ne br_if $fallback
                    (loop $object
                      local.get $i local.get $len i32.ge_u br_if $fallback
                      local.get $ptr local.get $i i32.add i32.load8_u local.set $char
                      local.get $char i32.const 123 i32.eq
                      (if (then local.get $depth i32.const 1 i32.add local.set $depth))
                      local.get $char i32.const 125 i32.eq
                      (if (then local.get $depth i32.const 1 i32.sub local.set $depth))
                      local.get $i i32.const 1 i32.add local.set $i
                      local.get $depth i32.eqz
                      (if (then
                        local.get $i local.get $start i32.sub i64.extend_i32_u i64.const 32 i64.shl
                        local.get $ptr local.get $start i32.add i64.extend_i32_u i64.or return))
                      br $object)))
                local.get $i i32.const 1 i32.add local.set $i
                br $search))
            i64.const {}))"#,
        quote(&manifest_bytes),
        quote(&fallback_bytes),
        manifest["apiVersion"].as_u64().unwrap(),
        ((manifest_bytes.len() as u64) << 32) | 32,
        ((fallback_bytes.len() as u64) << 32) | 65536
    ))
    .unwrap()
}

fn b64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let bits = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            output.push(if i <= chunk.len() { TABLE[((bits >> (18 - 6 * i)) & 63) as usize] as char } else { '=' });
        }
    }
    output
}

pub(super) fn install(s: &mut Session, manifest: &Value, fallback: &Value, replace: bool) -> Result<Value> {
    s.execute("extension.install", json!({"data":b64(&guest(manifest,fallback)),"replace":replace}))
}

fn session_with(depth: SampleType, mode: ColorMode, lock_second: bool) -> Session {
    let mut document = Document::new("Platform test", Size::new(8, 6), mode, depth);
    let format = document.pixel_format();
    document.layers = vec![Layer::raster("First", format), Layer::raster("Second", format)];
    document.layers[1].locks.all = lock_second;
    let mut s = Session::new();
    s.add_document(document, None);
    s
}

pub(super) fn session() -> Session {
    session_with(SampleType::U16, ColorMode::Rgb, false)
}

fn editor_manifest(id: &str) -> Value {
    descriptor(id, &[KnownCapability::DocumentRead, KnownCapability::DocumentEdit])
}

fn installed_editor() -> Session {
    let mut s = session();
    install(&mut s, &editor_manifest(OWNER), &json!({}), false).unwrap();
    s
}

fn snapshot(s: &mut Session, id: &str, extra: Value) -> Value {
    let mut params = extra.as_object().unwrap().clone();
    params.insert("id".into(), json!(id));
    s.execute("plugin.document.read", Value::Object(params)).unwrap()
}

fn edit_params(snapshot: &Value, id: &str) -> Value {
    json!({"id":id,"document":snapshot["document"],"expectedRevision":snapshot["revision"],"edits":[
        {"layer":snapshot["layers"][0]["handle"],"name":"Organized First","visible":false},
        {"layer":snapshot["layers"][1]["handle"],"name":"Organized Second","visible":true}]})
}

fn broker_request(id: &str, method: &str, params: Value) -> BrokerRequest {
    BrokerRequest { id: id.into(), method: method.into(), params: params.as_object().unwrap().clone() }
}

fn assert_code<T: std::fmt::Debug>(result: Result<T>, code: ErrorCode) {
    match result {
        Err(EngineError::PluginApi(error)) => assert_eq!(error.code, code, "{error}"),
        other => panic!("expected {code:?}, got {other:?}"),
    }
}

fn read_only_authority(command: &str, _: &Value) -> Result<()> {
    if command == "plugin.document.edit" { Err(EngineError::Other("captured read-only authority".into())) } else { Ok(()) }
}

pub(super) fn deny_read(command: &str, _: &Value) -> Result<()> {
    if command == "plugin.document.read" { Err(EngineError::Other("read denied by current caller".into())) } else { Ok(()) }
}

fn deny_contributed(command: &str, _: &Value) -> Result<()> {
    if command == "org.example.platform.action" { Err(EngineError::Other("contributed command denied".into())) } else { Ok(()) }
}

#[test]
fn capability_negotiation_rejects_required_and_preserves_optional_unknown_ids() {
    let mut value = editor_manifest(OWNER);
    value["capabilities"]["required"] = json!([{"id":"future.missing","minVersion":1}]);
    let manifest = Manifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_code(compatibility(&manifest), ErrorCode::MissingCapability);
    value["capabilities"] = json!({"optional":[{"id":"future.optional","minVersion":1}]});
    let manifest = Manifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    let selected = compatibility(&manifest).unwrap();
    assert_eq!(selected["negotiated"]["unavailableOptional"], json!(["future.optional"]));
    value["capabilities"] = json!({"required":[{"id":"host.document.read","minVersion":2}]});
    let manifest = Manifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_code(compatibility(&manifest), ErrorCode::UnsupportedVersion);
}

#[test]
fn host_injected_principals_are_session_owned_and_revocable() {
    let mut first = installed_editor();
    let principal = principal(&mut first, OWNER).unwrap();
    assert!(validate_principal(&first, &principal).is_ok());
    let second = installed_editor();
    assert_code(validate_principal(&second, &principal).map(|_| ()), ErrorCode::InvalidHandle);
    first.execute("extension.enable", json!({"id":OWNER,"enabled":false})).unwrap();
    assert_code(validate_principal(&first, &principal).map(|_| ()), ErrorCode::InvalidHandle);
    first.execute("extension.enable", json!({"id":OWNER,"enabled":true})).unwrap();
    assert_code(validate_principal(&first, &principal).map(|_| ()), ErrorCode::InvalidHandle);
}

#[test]
fn handles_reject_foreign_plugins_and_sessions_even_with_matching_document_identity() {
    let mut first = installed_editor();
    let snapshot = snapshot(&mut first, OWNER, json!({}));
    let other = "org.example.other";
    install(&mut first, &editor_manifest(other), &json!({}), false).unwrap();
    assert_code(first.execute("plugin.document.read", json!({"id":other,"document":snapshot["document"]})), ErrorCode::InvalidHandle);
    let mut second = Session::new();
    second.add_document((*first.active().unwrap().doc).clone(), None);
    install(&mut second, &editor_manifest(OWNER), &json!({}), false).unwrap();
    assert_code(second.execute("plugin.document.read", json!({"id":OWNER,"document":snapshot["document"]})), ErrorCode::InvalidHandle);
}

#[test]
fn replacement_and_disable_enable_revoke_existing_handles() {
    let mut s = installed_editor();
    let old = snapshot(&mut s, OWNER, json!({}));
    install(&mut s, &editor_manifest(OWNER), &json!({"message":"new generation"}), true).unwrap();
    assert_code(s.execute("plugin.document.read", json!({"id":OWNER,"document":old["document"]})), ErrorCode::InvalidHandle);
    let old = snapshot(&mut s, OWNER, json!({}));
    s.execute("extension.enable", json!({"id":OWNER,"enabled":false})).unwrap();
    s.execute("extension.enable", json!({"id":OWNER,"enabled":true})).unwrap();
    assert_code(s.execute("plugin.document.read", json!({"id":OWNER,"document":old["document"]})), ErrorCode::InvalidHandle);
}

#[test]
fn closing_and_reopening_same_document_id_never_resurrects_old_handles() {
    let mut s = installed_editor();
    let old = snapshot(&mut s, OWNER, json!({}));
    let closed = s.close(0).unwrap();
    let id = closed.doc.id;
    s.add_document((*closed.doc).clone(), None);
    assert_eq!(s.active().unwrap().doc.id, id);
    assert_code(s.execute("plugin.document.read", json!({"id":OWNER,"document":old["document"]})), ErrorCode::InvalidHandle);
}

#[test]
fn handles_retain_captured_automation_authority_and_obey_current_authority() {
    let mut s = installed_editor();
    s.authorize = Some(read_only_authority);
    let restricted = snapshot(&mut s, OWNER, json!({}));
    s.authorize = None;
    let before = Arc::clone(&s.active().unwrap().doc);
    let error = s.execute("plugin.document.edit", edit_params(&restricted, OWNER)).unwrap_err();
    assert!(error.to_string().contains("captured read-only authority"));
    assert!(Arc::ptr_eq(&before, &s.active().unwrap().doc));
    let unrestricted = snapshot(&mut s, OWNER, json!({}));
    s.authorize = Some(deny_read);
    assert!(s.execute("plugin.document.read", json!({"id":OWNER,"document":unrestricted["document"]})).unwrap_err().to_string().contains("current caller"));
}

#[test]
fn stale_cursors_and_revisions_fail_without_editing() {
    let mut s = installed_editor();
    let old = snapshot(&mut s, OWNER, json!({"limit":1}));
    assert!(old["nextCursor"].is_string());
    s.edit("External rename", |doc, _| {
        doc.layers[0].name = "Changed".into();
        Ok(())
    })
    .unwrap();
    assert_code(s.execute("plugin.document.read", json!({"id":OWNER,"cursor":old["nextCursor"]})), ErrorCode::StaleRevision);
    let current = snapshot(&mut s, OWNER, json!({}));
    let mut edit = edit_params(&current, OWNER);
    edit["expectedRevision"] = old["revision"].clone();
    let before = Arc::clone(&s.active().unwrap().doc);
    assert_code(s.execute("plugin.document.edit", edit), ErrorCode::StaleRevision);
    assert!(Arc::ptr_eq(&before, &s.active().unwrap().doc));
}

#[test]
fn repeated_snapshots_reuse_handles_and_old_history_does_not_exhaust_the_budget() {
    let mut s = installed_editor();
    let mut prior = snapshot(&mut s, OWNER, json!({}));
    for index in 0..2000 {
        let next = snapshot(&mut s, OWNER, json!({}));
        assert_eq!(next["document"], prior["document"]);
        assert_eq!(next["layers"][0]["handle"], prior["layers"][0]["handle"]);
        if index % 50 == 0 {
            s.edit("New snapshot", |doc, _| {
                doc.layers[0].name = format!("Layer {index}");
                Ok(())
            })
            .unwrap();
            prior = snapshot(&mut s, OWNER, json!({}));
        } else {
            prior = next;
        }
    }
}

#[test]
fn forged_wrong_kind_duplicate_and_cross_document_handles_fail() {
    let mut s = installed_editor();
    let first = snapshot(&mut s, OWNER, json!({"limit":1}));
    assert_code(s.execute("plugin.document.read", json!({"id":OWNER,"document":"fabricated"})), ErrorCode::InvalidHandle);
    assert_code(s.execute("plugin.document.read", json!({"id":OWNER,"document":first["layers"][0]["handle"]})), ErrorCode::InvalidHandle);
    let full = snapshot(&mut s, OWNER, json!({}));
    let mut duplicate = edit_params(&full, OWNER);
    duplicate["edits"][1]["layer"] = full["layers"][0]["handle"].clone();
    assert_code(s.execute("plugin.document.edit", duplicate), ErrorCode::InvalidHandle);
    let mut wrong_kind = edit_params(&full, OWNER);
    wrong_kind["edits"][0]["layer"] = first["nextCursor"].clone();
    assert_code(s.execute("plugin.document.edit", wrong_kind), ErrorCode::InvalidHandle);
    s.add_document((*s.active().unwrap().doc).clone(), None);
    let second = snapshot(&mut s, OWNER, json!({}));
    let mut cross = edit_params(&second, OWNER);
    cross["edits"][0]["layer"] = full["layers"][0]["handle"].clone();
    assert_code(s.execute("plugin.document.edit", cross), ErrorCode::InvalidHandle);
}

#[test]
fn locked_later_operation_preserves_document_revision_history_and_state_atomically() {
    let mut s = session_with(SampleType::F32, ColorMode::Cmyk, true);
    install(&mut s, &editor_manifest(OWNER), &json!({}), false).unwrap();
    let snapshot = snapshot(&mut s, OWNER, json!({}));
    let before = Arc::clone(&s.active().unwrap().doc);
    let revision = s.active().unwrap().revision;
    let history = s.active().unwrap().history.past_len();
    s.extensions.installed.get_mut(OWNER).unwrap().state = json!({"runs":7});
    assert_code(s.execute("plugin.document.edit", edit_params(&snapshot, OWNER)), ErrorCode::PermissionDenied);
    assert!(Arc::ptr_eq(&before, &s.active().unwrap().doc));
    assert_eq!(s.active().unwrap().revision, revision);
    assert_eq!(s.active().unwrap().history.past_len(), history);
    assert_eq!(s.extensions.installed[OWNER].state, json!({"runs":7}));
}

#[test]
fn successful_batch_is_one_undo_redo_step_at_all_depths_and_multiple_modes() {
    for depth in SampleType::ALL {
        for mode in [ColorMode::Grayscale, ColorMode::Rgb, ColorMode::Cmyk] {
            let mut s = session_with(depth, mode, false);
            install(&mut s, &editor_manifest(OWNER), &json!({}), false).unwrap();
            let snapshot = snapshot(&mut s, OWNER, json!({}));
            let before = Arc::clone(&s.active().unwrap().doc);
            let history = s.active().unwrap().history.past_len();
            let result = s.execute("plugin.document.edit", edit_params(&snapshot, OWNER)).unwrap();
            assert_eq!(result["edited"], 2);
            assert_eq!(s.active().unwrap().history.past_len(), history + 1);
            assert_eq!(s.active().unwrap().doc.layers[0].name, "Organized First");
            assert!(!s.active().unwrap().doc.layers[0].visible);
            assert_eq!(s.active().unwrap().doc.depth, depth);
            assert_eq!(s.active().unwrap().doc.mode, mode);
            let after = Arc::clone(&s.active().unwrap().doc);
            s.execute("edit.undo", json!({})).unwrap();
            assert_eq!(s.active().unwrap().doc, before);
            s.execute("edit.redo", json!({})).unwrap();
            assert_eq!(s.active().unwrap().doc, after);
        }
    }
}

#[test]
fn plugin_batch_never_coalesces_with_an_earlier_edit() {
    let mut s = installed_editor();
    let first_layer = s.active().unwrap().doc.layers[0].id.0;
    s.execute("layer.setProps", json!({"layer":first_layer,"name":"Earlier edit","coalesce":"same-group"})).unwrap();
    let before_plugin = Arc::clone(&s.active().unwrap().doc);
    let history = s.active().unwrap().history.past_len();
    let snapshot = snapshot(&mut s, OWNER, json!({}));
    let mut params = edit_params(&snapshot, OWNER);
    params["coalesce"] = json!("same-group");
    s.execute("plugin.document.edit", params).unwrap();
    assert_eq!(s.active().unwrap().history.past_len(), history + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc, before_plugin);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc.layers[0].name, "First");
}

#[test]
fn view_capability_failure_precedes_an_otherwise_valid_edit_and_preserves_guest_state() {
    let mut s = installed_editor();
    let snapshot = snapshot(&mut s, OWNER, json!({}));
    let mut params = edit_params(&snapshot, OWNER);
    params.as_object_mut().unwrap().remove("id");
    let response = json!({"state":{"runs":1},"requests":[{"id":"edit","method":"documents.layers.edit","params":params}],
        "view":{"title":"Forbidden","rows":[]}});
    s.extensions.installed.get_mut(OWNER).unwrap().state = response.clone();
    let before = Arc::clone(&s.active().unwrap().doc);
    let history = s.active().unwrap().history.past_len();
    assert_code(s.execute("extension.invoke", json!({"id":OWNER,"event":"apply"})), ErrorCode::PermissionDenied);
    assert!(Arc::ptr_eq(&before, &s.active().unwrap().doc));
    assert_eq!(s.active().unwrap().history.past_len(), history);
    assert_eq!(s.extensions.installed[OWNER].state, response);
}

#[test]
fn observer_batches_cannot_edit_and_edit_batches_cannot_hide_other_effects() {
    let mut s = installed_editor();
    let snapshot = snapshot(&mut s, OWNER, json!({}));
    let mut params = edit_params(&snapshot, OWNER);
    params.as_object_mut().unwrap().remove("id");
    let owner = principal(&mut s, OWNER).unwrap();
    let editing = broker_request("edit", "documents.layers.edit", params);
    let before = Arc::clone(&s.active().unwrap().doc);
    assert_code(broker::apply_requests(&mut s, &owner, std::slice::from_ref(&editing), true), ErrorCode::PermissionDenied);
    assert_code(
        broker::apply_requests(&mut s, &owner, &[editing, broker_request("read", "documents.layers.read", json!({}))], false),
        ErrorCode::PermissionDenied,
    );
    assert!(Arc::ptr_eq(&before, &s.active().unwrap().doc));
}

#[test]
fn dynamic_commands_share_discovery_enablement_and_authorization() {
    let mut value = descriptor(OWNER, &[]);
    value["contributes"] = json!({"commands":[{"id":"org.example.platform.action","label":"Neutral Action","event":"apply","needsDocument":true}]});
    let mut s = Session::new();
    install(&mut s, &value, &json!({"message":"contributed action"}), false).unwrap();
    let command = "org.example.platform.action";
    assert!(s.has_command(command));
    assert!(!s.is_enabled(command));
    assert!(s.command_entries().iter().any(|entry| entry["id"] == command && entry["enabled"] == false));
    let doc = session().close(0).unwrap();
    s.add_document((*doc.doc).clone(), None);
    assert!(s.is_enabled(command));
    assert_eq!(s.execute(command, json!({})).unwrap()["message"], "contributed action");
    s.authorize = Some(deny_contributed);
    assert!(s.execute(command, json!({})).unwrap_err().to_string().contains("command denied"));
    s.authorize = None;
    s.execute("extension.enable", json!({"id":OWNER,"enabled":false})).unwrap();
    assert!(!s.is_enabled(command));
}

#[test]
fn recovery_blocks_new_entry_points_and_dynamic_commands_without_harming_normal_editing() {
    let mut value = descriptor(OWNER, &[KnownCapability::DocumentRead, KnownCapability::DocumentEdit]);
    value["contributes"] = json!({"commands":[{"id":"org.example.platform.action","label":"Action","event":"apply"}]});
    let mut s = session();
    install(&mut s, &value, &json!({}), false).unwrap();
    s.plugins_disabled = true;
    for id in ["plugin.document.read", "plugin.document.edit", "extension.view.open", "org.example.platform.action"] {
        assert!(!s.is_enabled(id), "{id}");
        assert!(s.execute(id, json!({"id":OWNER})).is_err(), "{id}");
    }
    assert!(s.execute("extension.capabilities", json!({})).is_ok());
    s.execute("layer.new.layer", json!({"name":"Recovery edit"})).unwrap();
}

#[test]
fn document_pages_are_bounded_and_unknown_methods_are_explicit_errors() {
    let mut s = installed_editor();
    for params in [json!({"id":OWNER,"limit":0}), json!({"id":OWNER,"limit":129})] {
        assert_code(s.execute("plugin.document.read", params), ErrorCode::LimitExceeded);
    }
    let owner = principal(&mut s, OWNER).unwrap();
    assert_code(broker::apply_requests(&mut s, &owner, &[broker_request("x", "not.implemented", json!({}))], false), ErrorCode::UnsupportedOperation);
}

#[test]
fn contributed_commands_cannot_replace_builtins_or_another_owner() {
    let command = |id: &str| json!({"commands":[{"id":id,"label":"Collision","event":"apply"}]});
    let mut builtin = descriptor("layer", &[]);
    builtin["contributes"] = command("layer.new.layer");
    let mut s = session();
    assert!(install(&mut s, &builtin, &json!({}), false).is_err());
    assert!(!s.extensions.installed.contains_key("layer"));
    let mut shell = descriptor("help", &[]);
    shell["contributes"] = command("help.about");
    assert!(install(&mut s, &shell, &json!({}), false).is_err());
    assert!(!s.extensions.installed.contains_key("help"));
    let mut first = descriptor("org.example", &[]);
    first["contributes"] = command("org.example.nested.action");
    install(&mut s, &first, &json!({}), false).unwrap();
    let mut second = descriptor("org.example.nested", &[]);
    second["contributes"] = command("org.example.nested.action");
    assert_code(install(&mut s, &second, &json!({}), false), ErrorCode::Conflict);
    assert!(!s.extensions.installed.contains_key("org.example.nested"));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn startup_revalidates_contribution_collisions_without_running_guest_handlers() {
    let folder = tempfile::tempdir().unwrap();
    let mut builtin = descriptor("layer", &[]);
    builtin["contributes"] = json!({"commands":[{"id":"layer.new.layer","label":"Collision","event":"apply"}]});
    std::fs::write(folder.path().join("layer.pcext"), guest(&builtin, &json!({}))).unwrap();
    for owner in ["org.example", "org.example.nested"] {
        let mut value = descriptor(owner, &[]);
        value["contributes"] = json!({"commands":[{"id":"org.example.nested.action","label":"Collision","event":"apply"}]});
        std::fs::write(folder.path().join(format!("{owner}.pcext")), guest(&value, &json!({}))).unwrap();
    }
    let mut s = session();
    s.extensions.attach_folder(folder.path().into()).unwrap();
    assert!(!s.extensions.installed.contains_key("layer"));
    assert_eq!(s.extensions.installed.len(), 1);
    assert_eq!(s.extensions.load_errors.len(), 2);
    assert!(s.has_command("org.example.nested.action"));
}

#[test]
fn continuously_dirty_views_receive_fair_slots_after_registry_changes() {
    let mut ids = (0..20).map(|index| format!("org.example.view{index:02}")).collect::<Vec<_>>();
    let mut cursor = None;
    let mut scheduled = std::collections::BTreeSet::new();
    for _ in 0..10 {
        let batch = events::rotated(ids.clone(), cursor.as_deref(), 2);
        assert_eq!(batch.len(), 2);
        cursor = batch.last().cloned();
        scheduled.extend(batch);
    }
    assert_eq!(scheduled.len(), ids.len(), "continuously dirty early IDs must not starve later views");
    ids.retain(|id| Some(id) != cursor.as_ref());
    ids.push("org.example.view99".into());
    scheduled.clear();
    for _ in 0..10 {
        let batch = events::rotated(ids.clone(), cursor.as_deref(), 2);
        cursor = batch.last().cloned();
        scheduled.extend(batch);
    }
    assert_eq!(scheduled.into_iter().collect::<Vec<_>>(), ids);
    assert!(events::rotated(Vec::new(), Some("removed"), 4).is_empty());
    assert!(events::rotated(ids, None, 0).is_empty());
}

pub(super) fn inspector_manifest() -> Value {
    let mut value = descriptor(OWNER, &[KnownCapability::DocumentRead, KnownCapability::DocumentEdit, KnownCapability::UiViews, KnownCapability::Events]);
    value["panel"]["buttons"] = json!([]);
    value["contributes"] = json!({"inspectors":[{"id":"org.example.platform.view","label":"Inspector","event":"inspect"}],
        "subscriptions":[{"id":"org.example.platform.changed","event":"document.changed","handler":"inspect"}]});
    value
}

fn pump_until(s: &mut Session, predicate: impl Fn(&Session) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !predicate(s) && Instant::now() < deadline {
        s.pump_plugin_events();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(predicate(s), "plugin event did not reach its expected state");
}

#[test]
fn deferred_inspector_rejects_document_mutation_without_committing_any_state() {
    let mut s = session();
    install(&mut s, &inspector_manifest(), &json!({}), false).unwrap();
    let snapshot = snapshot(&mut s, OWNER, json!({}));
    let mut params = edit_params(&snapshot, OWNER);
    params.as_object_mut().unwrap().remove("id");
    let malicious = json!({"state":{"runs":9},"requests":[{"id":"edit","method":"documents.layers.edit","params":params}]});
    s.extensions.installed.get_mut(OWNER).unwrap().state = malicious.clone();
    let before = Arc::clone(&s.active().unwrap().doc);
    s.execute("extension.view.open", json!({"id":OWNER})).unwrap();
    pump_until(&mut s, |s| s.plugin_view_error(OWNER).is_some());
    assert!(s.plugin_view_error(OWNER).unwrap().contains("sole request"));
    assert!(Arc::ptr_eq(&before, &s.active().unwrap().doc));
    assert_eq!(s.extensions.installed[OWNER].state, malicious);
    s.execute("extension.view.close", json!({"id":OWNER})).unwrap();
}

#[test]
fn inspector_updates_only_after_invalidations_and_disable_clears_the_view() {
    let mut s = session();
    install(&mut s, &inspector_manifest(), &json!({"view":{"title":"Inspector","rows":[{"label":"Status","value":"Ready"}]}}), false).unwrap();
    s.execute("extension.view.open", json!({"id":OWNER})).unwrap();
    pump_until(&mut s, |s| s.plugin_inspector_view(OWNER).is_some() && !s.plugin_events_pending());
    let first = s.plugin_inspector_view(OWNER).cloned().unwrap();
    assert_eq!(s.pump_plugin_events(), 0);
    assert!(!s.plugin_events_pending());
    s.execute("layer.new.layer", json!({"name":"Changed"})).unwrap();
    pump_until(&mut s, |s| !s.plugin_events_pending());
    assert_eq!(s.plugin_inspector_view(OWNER), Some(&first));
    s.execute("extension.enable", json!({"id":OWNER,"enabled":false})).unwrap();
    assert!(!s.plugin_view_is_open(OWNER));
    assert!(s.plugin_inspector_view(OWNER).is_none());
}

fn float_selection(s: &mut Session) {
    let layer = s.active().unwrap().active_layer.unwrap();
    s.edit("Paint fixture", |doc, _| {
        doc.layer_mut(layer).unwrap().surface_mut().unwrap().fill_rect(photocraft_geom::Rect::new(1, 1, 4, 4), &[1.0, 0.0, 0.0, 1.0]);
        Ok(())
    })
    .unwrap();
    s.execute("select.rect", json!({"x":1,"y":1,"width":3,"height":3})).unwrap();
    s.execute("select.float", json!({"dx":1})).unwrap();
}

#[test]
fn read_views_and_failed_edits_preserve_a_floating_selection_without_anchoring_it() {
    let mut s = session();
    let mut value = inspector_manifest();
    value["panel"]["buttons"] = json!([{"id":"apply","label":"Apply"}]);
    install(&mut s, &value, &json!({}), false).unwrap();
    float_selection(&mut s);
    let before = Arc::clone(&s.active().unwrap().doc);
    let floating = s.active().unwrap().floating.as_ref().unwrap().clone();
    assert_eq!(floating.offset, (1, 0));
    let history = s.active().unwrap().history.past_len();
    let revision = s.active().unwrap().revision;
    let snapshot = snapshot(&mut s, OWNER, json!({}));
    s.execute("extension.view.open", json!({"id":OWNER})).unwrap();
    s.execute("extension.capabilities", json!({})).unwrap();
    s.execute("extension.invoke", json!({"id":OWNER,"event":"apply"})).unwrap();
    assert_code(s.execute("plugin.document.edit", edit_params(&snapshot, OWNER)), ErrorCode::Conflict);
    let st = s.active().unwrap();
    assert!(Arc::ptr_eq(&before, &st.doc));
    assert_eq!(st.history.past_len(), history);
    assert_eq!(st.revision, revision);
    let current = st.floating.as_ref().unwrap();
    assert_eq!(current.offset, floating.offset);
    assert_eq!(current.layer, floating.layer);
    assert!(Arc::ptr_eq(&current.parts, &floating.parts));
    s.execute("extension.view.close", json!({"id":OWNER})).unwrap();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn contributed_native_tasks_use_background_job_dispatch_and_preserve_state_on_worker_failure() {
    use crate::extension_cmds::{Installed, StorageSize};
    use crate::extension_package::{InstalledNative, NativeInfo, PackageInfo};

    for route in ["direct", "panel1", "contributed2"] {
        let mut value = descriptor(OWNER, &[KnownCapability::CommandsInvoke]);
        value["permissions"]["commands"] = json!(["plugin.processImage", "plugin.runTask"]);
        if route == "panel1" {
            value["apiVersion"] = json!(1);
            value.as_object_mut().unwrap().remove("capabilities");
        } else {
            value["contributes"] = json!({"commands":[{"id":"org.example.platform.task","label":"Task","event":"apply"}]});
        }
        let extension = Arc::new(Extension::load(&guest(&value, &json!({"state":{"runs":1},"commands":[{"command":"plugin.runTask","params":{}}]}))).unwrap());
        let mut s = session();
        s.extensions.installed.insert(
            OWNER.into(),
            Installed {
                extension,
                enabled: true,
                state: Value::Null,
                storage: StorageSize::default(),
                filters: std::collections::BTreeMap::new(),
                package: None,
                native: Some(Arc::new(InstalledNative {
                    generation: None,
                    worker: "missing-photocraft-dynamic-task-fixture.exe".into(),
                    package: PackageInfo {
                        package_version: 2,
                        module: "plugin.pcext".into(),
                        filters: vec![],
                        native: Some(NativeInfo { service: "plugin.processImage".into(), platform: "windows-x86_64".into(), entry: "worker.exe".into() }),
                    },
                })),
            },
        );
        float_selection(&mut s);
        let before = Arc::clone(&s.active().unwrap().doc);
        let history = s.active().unwrap().history.past_len();
        let revision = s.active().unwrap().revision;
        let floating = s.active().unwrap().floating.as_ref().unwrap().clone();
        let started = match route {
            "direct" => s.start("plugin.runTask", json!({"id":OWNER})),
            "panel1" => s.start("extension.invoke", json!({"id":OWNER,"event":"apply"})),
            _ => s.start("org.example.platform.task", json!({})),
        };
        let crate::jobs::Started::Job(job) = started.unwrap() else { panic!("expected {route} task to start a job") };
        assert!(s.job(job).unwrap().document.is_none());
        assert!(s.has_jobs());
        assert!(Arc::ptr_eq(&before, &s.active().unwrap().doc), "{route}");
        s.execute("jobs.list", json!({})).unwrap();
        let _ = s.execute("image.pluginPreview.discard", json!({"preview":u64::MAX}));
        let unknown = s.execute("jobs.cancel", json!({"job":u64::MAX})).unwrap_err();
        assert!(unknown.to_string().contains("no running job"));
        let failure = s.wait_job(job).unwrap_err();
        assert!(!matches!(&failure, EngineError::Cancelled), "{route}: expected the absent worker to fail, got {failure}");
        assert!(s.extensions.installed[OWNER].state.is_null());
        assert!(!s.has_jobs());
        let st = s.active().unwrap();
        assert!(Arc::ptr_eq(&before, &st.doc), "{route}");
        assert_eq!(st.history.past_len(), history, "{route}");
        assert_eq!(st.revision, revision, "{route}");
        let current = st.floating.as_ref().unwrap();
        assert_eq!(current.offset, floating.offset, "{route}");
        assert!(Arc::ptr_eq(&current.parts, &floating.parts), "{route}");
    }
}

#[test]
fn contributed_shortcuts_use_the_headless_registry_and_round_trip_through_kys() {
    let command = "org.example.platform.action";
    let other = "org.example.platform.other";
    let mut value = descriptor(OWNER, &[]);
    value["contributes"] = json!({"commands":[
        {"id":command,"label":"Neutral Action","event":"apply"},
        {"id":other,"label":"Other Action","event":"apply"}
    ]});
    let mut s = Session::new();
    install(&mut s, &value, &json!({}), false).unwrap();

    s.execute("edit.keyboardShortcuts", json!({"set":{(command):"Cmd+C"}})).unwrap();
    assert_eq!(s.prefs().shortcut("edit.copy", Some("Cmd+C")), None);
    s.execute("edit.keyboardShortcuts", json!({"set":{(other):"Cmd+C"}})).unwrap();
    assert_eq!(s.prefs().shortcut(command, None), None, "moving a key clears its previous plugin owner");

    let main = "Cmd+Shift+F9";
    let alternate = "F8";
    let listed = s.execute("edit.keyboardShortcuts", json!({"set":{(command):main},"alternates":{(command):[alternate]},"list":true})).unwrap();
    let row = listed["commands"].as_array().unwrap().iter().find(|row| row["id"] == command).unwrap();
    assert_eq!(row["label"], "Neutral Action");
    assert_eq!(row["menu"][0], "Plugins");
    assert_eq!(row["shortcut"], main);
    assert_eq!(row["alternates"], json!([alternate]));

    let exported = s.execute("edit.keyboardShortcuts", json!({"exportKys":true,"summarize":true})).unwrap();
    let xml = exported["kys"].as_str().unwrap().to_owned();
    assert!(xml.contains(command));
    assert!(exported["html"].as_str().unwrap().contains("Neutral Action"));
    s.execute("edit.keyboardShortcuts", json!({"reset":true})).unwrap();
    let imported = s.execute("edit.keyboardShortcuts", json!({"importKys":xml})).unwrap();
    assert!(imported["import"]["set"].get(command).is_some());
    assert!(!imported["import"]["unknown"].as_array().unwrap().iter().any(|label| label == "Neutral Action" || label == "Other Action"));
    assert_eq!(s.prefs().shortcut(command, None), Some(main));
    assert_eq!(s.prefs().alternates(command, Some(main)), [alternate]);

    let before = s.prefs().clone();
    assert!(s.execute("edit.keyboardShortcuts", json!({"set":{"org.example.missing":"F9"}})).is_err());
    assert_eq!(s.prefs(), &before);
    let reserved = ["Cmd+Tab", "Alt+F4"].into_iter().find(|key| crate::prefs::reserved_shortcut(key)).unwrap();
    assert!(s.execute("edit.keyboardShortcuts", json!({"set":{(command):reserved}})).is_err());
    assert_eq!(s.prefs(), &before);

    s.execute("extension.remove", json!({"id":OWNER})).unwrap();
    assert!(s.execute("edit.keyboardShortcuts", json!({"set":{(command):"F9"}})).is_err());
    let listed = s.execute("edit.keyboardShortcuts", json!({"list":true})).unwrap();
    assert!(!listed["commands"].as_array().unwrap().iter().any(|row| row["id"] == command));
}
