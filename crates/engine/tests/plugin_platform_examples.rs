//! End-to-end qualification of independently compiled PhotoCraft plugin packages.
//!
//! Build both examples first, then explicitly run this ignored artifact suite:
//! `cargo test -p photocraft-engine --test plugin_platform_examples -- --ignored`.
//! PHOTOCRAFT_PLATFORM_EXAMPLES may point at an exported examples directory containing
//! `layer-organizer/dist/` and `document-inspector/dist/`. Missing artifacts always fail.
#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use photocraft_doc::LayerId;
use photocraft_engine::Session;
use serde_json::{Value, json};

const ORGANIZER: &str = "org.example.layer-organizer";
const INSPECTOR: &str = "org.example.document-inspector";
const ORGANIZE: &str = "org.example.layer-organizer.organize";

fn artifact(project: &str) -> PathBuf {
    let root = std::env::var_os("PHOTOCRAFT_PLATFORM_EXAMPLES")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions"));
    let name = match project {
        "layer-organizer" => "Layer-Organizer-1.0.0.pcplugin",
        "document-inspector" => "Document-Inspector-1.0.0.pcplugin",
        _ => panic!("unknown acceptance plugin"),
    };
    let path = root.join(project).join("dist").join(name);
    assert!(path.is_file(), "missing actual compiled package {}; run that independent project's build.py first", path.display());
    path
}

/// Install only copied downloads, delete them, then start a fresh host against its store.
/// The canonical build output is never changed and the returned host has no source path.
fn restarted(projects: &[&str]) -> (tempfile::TempDir, Session, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let folder = temporary.path().join("Extensions");
    let mut installer = Session::new();
    installer.extensions.attach_folder(folder.clone()).unwrap();
    for project in projects {
        let download = temporary.path().join(format!("{project}.pcplugin"));
        std::fs::copy(artifact(project), &download).unwrap();
        installer.execute("extension.install", json!({"path":download})).unwrap();
        std::fs::remove_file(&download).unwrap();
    }
    drop(installer);
    let mut session = Session::new();
    session.extensions.attach_folder(folder.clone()).unwrap();
    assert!(session.extensions.load_errors.is_empty(), "{:?}", session.extensions.load_errors);
    assert_eq!(session.extensions.installed.len(), projects.len());
    for installed in session.extensions.installed.values() {
        assert_eq!(installed.extension.manifest.api_version, photocraft_plugin_api::PANEL_API_VERSION);
        assert!(installed.native.is_none(), "the independent panel packages need no native runtime");
        assert!(installed.filters.is_empty());
    }
    (temporary, session, folder)
}

fn layered(session: &mut Session, depth: u32, name: &str) -> Vec<LayerId> {
    session.execute("file.new", json!({"name":name,"width":8,"height":6,"depth":depth,"background":"transparent"})).unwrap();
    session.execute("layer.setProps", json!({"name":"Base"})).unwrap();
    for name in ["Detail", "Notes"] {
        session.execute("layer.new.layer", json!({"name":name})).unwrap();
    }
    session.active().unwrap().doc.walk().into_iter().map(|(_, _, layer)| layer.id).collect()
}

fn names(session: &Session) -> Vec<String> {
    session.active().unwrap().doc.walk().into_iter().map(|(_, _, layer)| layer.name.clone()).collect()
}

fn history_len(session: &Session) -> usize {
    session.active().unwrap().history.past_len()
}

fn command(session: &mut Session) -> Option<Value> {
    session.execute("command.list", json!({})).unwrap().as_array().unwrap().iter().find(|row| row["id"] == ORGANIZE).cloned()
}

fn settle(session: &mut Session) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        session.pump_plugin_events();
        assert!(session.plugin_view_error(INSPECTOR).is_none(), "{:?}", session.plugin_view_error(INSPECTOR));
        if !session.plugin_events_pending() {
            return;
        }
        assert!(Instant::now() < deadline, "the actual WASM inspector did not settle within five seconds");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn row(session: &Session, label: &str) -> String {
    let view = session.plugin_inspector_view(INSPECTOR).expect("the actual WASM inspector should have published a native view");
    view.rows.iter().find(|row| row.label == label).unwrap_or_else(|| panic!("missing inspector row {label}: {view:?}")).value.clone()
}

fn selected_name(session: &Session) -> String {
    let active = session.active().unwrap();
    active.selected_layers().into_iter().filter_map(|id| active.doc.layer(id).map(|layer| layer.name.clone())).collect::<Vec<_>>().join(", ")
}

#[test]
#[ignore = "build both actual independent WASM example packages before running this qualification suite"]
fn actual_organizer_survives_restart_and_renames_several_layers_in_one_undo_step() {
    let (_temporary, mut session, _folder) = restarted(&["layer-organizer"]);
    assert!(session.has_command(ORGANIZE));
    assert_eq!(command(&mut session).unwrap()["enabled"], false);
    assert!(session.execute(ORGANIZE, json!({})).is_err());
    for depth in [8, 16, 32] {
        let ids = layered(&mut session, depth, "Independent organizer proof");
        let source = Arc::clone(&session.active().unwrap().doc);
        let before = names(&session);
        let history = history_len(&session);
        assert_eq!(command(&mut session).unwrap()["enabled"], true);
        let result = session.execute(ORGANIZE, json!({"prefix":"Review"})).unwrap();
        assert!(result["message"].as_str().unwrap().contains("Undo"));
        assert_eq!(names(&session), ["Review 01", "Review 02", "Review 03"]);
        assert_eq!(history_len(&session), history + 1, "{depth}-bit: exactly one Undo step");
        assert_eq!(session.active().unwrap().doc.walk().into_iter().map(|(_, _, layer)| layer.id).collect::<Vec<_>>(), ids);
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(names(&session), before);
        assert_eq!(*session.active().unwrap().doc, *source, "Undo restores the complete source document");
        assert_eq!(history_len(&session), history);
        session.execute("edit.redo", json!({})).unwrap();
        assert_eq!(names(&session), ["Review 01", "Review 02", "Review 03"]);
        assert_eq!(history_len(&session), history + 1);
        session.execute("file.close", json!({})).unwrap();
    }
}

#[test]
#[ignore = "build both actual independent WASM example packages before running this qualification suite"]
fn actual_organizer_failure_on_a_later_locked_layer_commits_nothing() {
    let (_temporary, mut session, _folder) = restarted(&["layer-organizer"]);
    for depth in [8, 16, 32] {
        let ids = layered(&mut session, depth, "Atomic failure proof");
        let last = ids.last().copied().unwrap();
        session.execute("layer.setProps", json!({"layer":last.0,"locked":true})).unwrap();
        let source = Arc::clone(&session.active().unwrap().doc);
        let revision = session.active().unwrap().revision;
        let history = history_len(&session);
        let state = session.extensions.installed[ORGANIZER].state.clone();
        let error = session.execute(ORGANIZE, json!({})).unwrap_err();
        assert!(error.to_string().contains("locked"), "{error}");
        assert!(Arc::ptr_eq(&source, &session.active().unwrap().doc), "a rejected batch must retain the source, including its earlier layers");
        assert_eq!(session.active().unwrap().revision, revision);
        assert_eq!(history_len(&session), history);
        assert_eq!(session.extensions.installed[ORGANIZER].state, state);
        session.execute("file.close", json!({})).unwrap();
    }
}

#[test]
#[ignore = "build both actual independent WASM example packages before running this qualification suite"]
fn actual_inspector_tracks_edits_undo_selection_documents_and_idle_without_polling() {
    let (_temporary, mut session, _folder) = restarted(&["layer-organizer", "document-inspector"]);
    let ids = layered(&mut session, 16, "First image");
    let first = ids.first().copied().unwrap();
    let second = ids.get(1).copied().unwrap();
    session.execute("layer.select", json!({"layer":first.0})).unwrap();
    session.execute("extension.view.open", json!({"id":INSPECTOR})).unwrap();
    settle(&mut session);
    assert_eq!(row(&session, "Document"), "First image");
    assert_eq!(row(&session, "Dimensions"), "8 × 6 px");
    assert_eq!(row(&session, "Layers"), "3");
    assert_eq!(row(&session, "Selected in snapshot"), selected_name(&session));

    session.execute("layer.setProps", json!({"layer":first.0,"name":"Renamed by built-in"})).unwrap();
    settle(&mut session);
    assert_eq!(row(&session, "Selected in snapshot"), "Renamed by built-in");
    session.execute(ORGANIZE, json!({})).unwrap();
    settle(&mut session);
    assert_eq!(row(&session, "Selected in snapshot"), selected_name(&session));
    assert!(row(&session, "Selected in snapshot").starts_with("Organized"));
    session.execute("edit.undo", json!({})).unwrap();
    settle(&mut session);
    assert_eq!(row(&session, "Selected in snapshot"), "Renamed by built-in");
    session.execute("edit.redo", json!({})).unwrap();
    settle(&mut session);
    assert_eq!(row(&session, "Selected in snapshot"), selected_name(&session));
    session.execute("layer.setProps", json!({"layer":first.0,"visible":false})).unwrap();
    settle(&mut session);
    assert_eq!(row(&session, "Visible in snapshot"), "2");
    session.execute("layer.select", json!({"layer":second.0})).unwrap();
    settle(&mut session);
    assert_eq!(row(&session, "Selected in snapshot"), selected_name(&session));

    session.execute("file.new", json!({"name":"Second image","width":4,"height":3,"background":"transparent"})).unwrap();
    settle(&mut session);
    assert_eq!(row(&session, "Document"), "Second image");
    assert_eq!(row(&session, "Layers"), "1");
    session.execute("document.activate", json!({"document":0})).unwrap();
    settle(&mut session);
    assert_eq!(row(&session, "Document"), "First image");
    session.execute("file.close", json!({"document":0})).unwrap();
    settle(&mut session);
    assert_eq!(row(&session, "Document"), "Second image");
    session.execute("file.close", json!({})).unwrap();
    settle(&mut session);
    assert!(row(&session, "Document").contains("Open an image"));

    let view = session.plugin_inspector_view(INSPECTOR).cloned();
    let state = session.extensions.installed[INSPECTOR].state.clone();
    for _ in 0..100 {
        assert_eq!(session.pump_plugin_events(), 0);
        assert!(!session.plugin_events_pending());
    }
    assert_eq!(session.plugin_inspector_view(INSPECTOR), view.as_ref());
    assert_eq!(session.extensions.installed[INSPECTOR].state, state);
    session.execute("extension.view.close", json!({"id":INSPECTOR})).unwrap();
    assert!(!session.plugin_view_is_open(INSPECTOR));
    assert!(session.plugin_inspector_view(INSPECTOR).is_none());
    layered(&mut session, 8, "Closed inspector stays dormant");
    assert_eq!(session.pump_plugin_events(), 0);
    assert!(!session.plugin_events_pending());
    assert!(session.plugin_inspector_view(INSPECTOR).is_none());
}

#[test]
#[ignore = "build both actual independent WASM example packages before running this qualification suite"]
fn disabling_removing_and_reinstalling_one_actual_package_keeps_the_other_independent() {
    let (temporary, mut session, folder) = restarted(&["layer-organizer", "document-inspector"]);
    layered(&mut session, 8, "Standalone lifecycle proof");
    session.execute("extension.view.open", json!({"id":INSPECTOR})).unwrap();
    settle(&mut session);
    let inspector_module = Arc::clone(&session.extensions.installed[INSPECTOR].extension);
    session.execute("extension.enable", json!({"id":ORGANIZER,"enabled":false})).unwrap();
    assert_eq!(command(&mut session).unwrap()["enabled"], false);
    assert!(session.execute(ORGANIZE, json!({})).is_err());
    session.execute("layer.setProps", json!({"name":"Sibling disabled"})).unwrap();
    settle(&mut session);
    assert_eq!(row(&session, "Layers"), "3");
    session.execute("extension.remove", json!({"id":ORGANIZER})).unwrap();
    assert!(!session.has_command(ORGANIZE));
    assert!(command(&mut session).is_none());
    assert!(Arc::ptr_eq(&inspector_module, &session.extensions.installed[INSPECTOR].extension));
    session.execute("layer.new.layer", json!({"name":"Sibling removed"})).unwrap();
    settle(&mut session);
    assert_eq!(row(&session, "Layers"), "4");
    let download = temporary.path().join("reinstall-organizer.pcplugin");
    std::fs::copy(artifact("layer-organizer"), &download).unwrap();
    session.execute("extension.install", json!({"path":download})).unwrap();
    std::fs::remove_file(download).unwrap();
    assert!(session.has_command(ORGANIZE));
    assert!(Arc::ptr_eq(&inspector_module, &session.extensions.installed[INSPECTOR].extension));
    session.execute(ORGANIZE, json!({})).unwrap();
    settle(&mut session);
    assert_eq!(row(&session, "Layers"), "4");
    assert!(names(&session).iter().all(|name| name.starts_with("Organized")));
    drop(session);
    let mut restarted = Session::new();
    restarted.extensions.attach_folder(folder).unwrap();
    assert!(restarted.extensions.load_errors.is_empty());
    assert!(restarted.extensions.installed[INSPECTOR].enabled);
    assert!(restarted.extensions.installed[ORGANIZER].enabled);
    assert!(restarted.has_command(ORGANIZE));
}
