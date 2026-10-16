use super::*;

fn initialized_windows() -> impl FnMut(&mut egui::Ui, &mut PhotocraftApp) {
    let mut ready = false;
    move |ui, app| {
        if ready {
            windows(app, ui.ctx());
        } else {
            PhotocraftApp::setup_context(ui.ctx(), crate::theme::ThemeKind::ProMedium);
            ready = true;
            ui.ctx().request_repaint();
        }
    }
}

#[test]
fn long_choice_explanations_fit_the_settings_column() {
    let choice = photocraft_plugins::extension::Field {
        id: "model".into(),
        label: "Model".into(),
        spec: photocraft_plugins::ParamSpec::Choice {
            options: vec!["General quality model — Finds small details in complex subjects and soft edges at a slower speed".into()],
            default: None,
        },
    };
    let mut values = json!({});
    let ctx = egui::Context::default();
    let mut width = 0.0_f32;
    let mut output =
        ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0))), ..Default::default() }, |ui| {
            ui.allocate_ui_with_layout(egui::vec2(220.0, 100.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                field(ui, &choice, &mut values);
                width = ui.min_rect().width();
            });
        });
    output.textures_delta.clear();
    assert!(width <= 221.0, "long options must not widen the settings column: {width}");
    assert_eq!(values["model"], "General quality model — Finds small details in complex subjects and soft edges at a slower speed");
}

#[cfg(not(target_arch = "wasm32"))]
mod general;

fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    let manifest = include_str!("../../../../examples/extensions/quick-tools/manifest.json");
    let data = base64::engine::general_purpose::STANDARD.encode(module(manifest));
    app.run("extension.install", json!({"data":data})).unwrap();
    app
}

pub(super) fn module(manifest: &str) -> Vec<u8> {
    let quote = manifest.bytes().map(|b| format!("\\{b:02x}")).collect::<String>();
    wat::parse_str(format!(
        r#"(module (memory (export "memory") 4)
        (data (i32.const 32) "{quote}") (data (i32.const 65536) "{{}}")
        (func (export "pce_api_version") (result i32) i32.const 1)
        (func (export "pce_manifest") (result i64) i64.const {})
        (func (export "pce_alloc") (param i32) (result i32) i32.const 196608)
        (func (export "pce_handle") (param i32 i32) (result i64) i64.const {}))"#,
        ((manifest.len() as u64) << 32) | 32,
        (2u64 << 32) | 65536
    ))
    .unwrap()
}

#[test]
fn installed_extensions_have_native_menu_entries_and_persist_panel_state() {
    let mut app = app();
    let id = "window.extension.org.photocraft.quick-tools";
    let entries = crate::menus::menu_items(&app);
    let row = entries.iter().find(|i| i.id == id).unwrap();
    assert_eq!(row.path, ["Plugins"]);
    assert!(crate::menus::TOP_MENUS.contains(&"Plugins"));
    assert_eq!(entries.iter().rfind(|i| i.path == ["Plugins"]).unwrap().id, MANAGER);
    assert!(row.enabled);
    menu(&mut app, id).unwrap().unwrap();
    let panel = &app.ui.extensions.panels["org.photocraft.quick-tools"];
    assert!(panel.open);
    assert_eq!(panel.values["percent"], 200);
    let restored: crate::state::UiState = serde_json::from_value(serde_json::to_value(&app.ui).unwrap()).unwrap();
    assert!(restored.extensions.panels["org.photocraft.quick-tools"].open);
    let old_hash = crate::native_menu::state_hash(&app, None);
    app.run("extension.enable", json!({"id":"org.photocraft.quick-tools","enabled":false})).unwrap();
    assert_eq!(is_enabled(&app, id), Some(false));
    assert_ne!(old_hash, crate::native_menu::state_hash(&app, None));
}

#[test]
fn manager_and_panels_render_without_running_code() {
    let mut app = app();
    menu(&mut app, MANAGER).unwrap().unwrap();
    menu(&mut app, "window.extension.org.photocraft.quick-tools").unwrap().unwrap();
    let ctx = egui::Context::default();
    let mut output = ctx
        .run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 800.0))), ..Default::default() }, |ui| {
            windows(&mut app, ui.ctx())
        });
    output.textures_delta.clear();
    assert!(app.session.extensions.installed["org.photocraft.quick-tools"].state.is_null());
    assert!(app.ui.extensions.panels["org.photocraft.quick-tools"].open);
}

#[test]
fn manager_shows_standalone_plugin_size_without_a_model_total() {
    use egui_kittest::{Harness, kittest::Queryable};
    let mut app = app();
    menu(&mut app, MANAGER).unwrap().unwrap();
    let size = app.session.extensions.installed["org.photocraft.quick-tools"].storage;
    assert!(size.installed_bytes > 0);
    assert_eq!(size.model_bytes, 0);
    let mut h = Harness::builder().with_size(egui::vec2(1100.0, 800.0)).build_ui_state(initialized_windows(), app);
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ALL[0]);
    h.run_steps(4);
    assert!(h.query_by_label(&storage::label(size)).is_some());
    assert!(!storage::label(size).contains("Models:"));
}

#[test]
fn plugin_sizes_use_explicit_binary_units() {
    for (bytes, expected) in [(0, "0 B"), (1023, "1023 B"), (1024, "1.0 KiB"), (1536, "1.5 KiB"), (170_073_963, "162.2 MiB"), (1 << 30, "1.0 GiB")] {
        assert_eq!(storage::bytes(bytes), expected);
    }
    assert_eq!(storage::bytes(u64::MAX), "16.0 EiB");
}

#[test]
fn browse_shows_permissions_before_installing_and_clears_the_completed_selection() {
    use egui_kittest::{Harness, kittest::Queryable};
    let manifest = include_str!("../../../../examples/extensions/plugin-starter/manifest.json");
    let (dialog, requests) = crate::file_dialog::fake(vec![Some(FileDialogAnswer::Contents("Example.pcext".into(), module(manifest)))]);
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services { file_dialog: Some(dialog), ..Default::default() });
    crate::menus::invoke(&mut app, &egui::Context::default(), "plugin.install", json!({})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1100.0, 800.0)).build_ui_state(
        |ui, app| {
            windows(app, ui.ctx());
            app.poll_file_dialog(ui.ctx(), None);
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ProMedium);
    h.run_steps(4);
    let path = h.get_by_role(egui::accesskit::Role::TextInput).rect();
    let browse = h.get_by_label("Browse…").rect();
    assert!(browse.left() >= path.right(), "Browse must be beside the path field");
    assert!((browse.center().y - path.center().y).abs() < 6.0);
    h.get_by_label("Browse…").click();
    h.run_steps(5);
    assert!(
        matches!(&requests.borrow()[0], FileDialogRequest::Open { multiple: false, extensions: Some(extensions), .. } if extensions.contains(&"pcext".to_owned()))
    );
    assert_eq!(h.state().ui.extensions.path, "Example.pcext");
    assert!(h.state().ui.extensions.pending.is_some());
    assert!(h.state().session.extensions.installed.is_empty(), "choosing a file only reviews it");
    assert!(h.query_by_label("Install plugin").is_some());
    assert!(h.query_by_label("Review plugin").is_none());
    assert!(h.query_by_label("Show permissions").is_none(), "Browse already shows the permissions");
    assert!(h.query_by_label("Drop a plugin file here\nor click to browse").is_none());
    let expected_hash = h.state().ui.extensions.pending.as_ref().unwrap().input["expectedHash"].clone();
    h.state_mut().ui.extensions.pending.as_mut().unwrap().input["expectedHash"] = json!("a changed file must fail review binding");
    h.get_by_label("Install plugin").click();
    h.run_steps(3);
    assert!(h.state().ui.extensions.pending.is_some(), "a failed install keeps the reviewed selection available");
    assert_eq!(h.state().ui.extensions.path, "Example.pcext");
    assert!(h.state().ui.extensions.status.contains("changed since review"));
    assert!(h.state().session.extensions.installed.is_empty());
    h.state_mut().ui.extensions.pending.as_mut().unwrap().input["expectedHash"] = expected_hash;
    h.get_by_label("Install plugin").click();
    h.run_steps(3);
    assert!(h.state().ui.extensions.pending.is_none());
    assert!(h.state().ui.extensions.path.is_empty());
    assert_eq!(h.state().ui.extensions.status, "Installed Hello PhotoCraft 1.0.0.");
    assert_eq!(h.state().session.extensions.installed.len(), 1);
    assert!(h.query_by_label("Install plugin").is_none());
}

#[test]
fn manager_path_row_keeps_its_width_across_frames() {
    use egui_kittest::{Harness, kittest::Queryable};
    let mut app = app();
    app.ui.extensions.manager_open = true;
    app.ui.extensions.path = "long-plugin-directory/".repeat(12) + "Example.pcplugin";
    let mut h = Harness::builder().with_size(egui::vec2(1800.0, 900.0)).build_ui_state(initialized_windows(), app);
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ProMedium);
    h.run_steps(6);
    let before = h.state().ui.extensions.drop_rect.unwrap().width();
    assert!(h.state().ui.extensions.drop_rect.unwrap().contains_rect(h.get_by_label("Done").rect()), "the whole manager remains a drop target");
    h.run_steps(20);
    let after = h.state().ui.extensions.drop_rect.unwrap().width();
    assert!((after - before).abs() < 1.0, "the path row must not grow its window each frame: {before} → {after}");
}

#[test]
fn plugin_drop_reviews_guest_bytes_and_rejects_bad_batches_without_stale_consent() {
    #[derive(Debug)]
    struct Drop(std::path::PathBuf, Vec<u8>);
    impl egui::DroppedFile for Drop {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            Ok(self.1.clone())
        }
    }
    let bytes = module(include_str!("../../../../examples/extensions/plugin-starter/manifest.json"));
    let file: egui::DroppedFileHandle = std::sync::Arc::new(Drop("Example.PCEXT".into(), bytes));
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    review_drop(&mut app, std::slice::from_ref(&file)).unwrap();
    assert!(app.ui.extensions.manager_open && app.ui.extensions.pending.is_some());
    assert!(app.session.extensions.installed.is_empty());
    assert!(review_drop(&mut app, &[file.clone(), file]).is_err());
    assert!(app.ui.extensions.pending.is_none());
    assert!(app.ui.extensions.status.contains("one plugin"));
    let invalid: egui::DroppedFileHandle = std::sync::Arc::new(Drop("Example.pcext.png".into(), vec![]));
    assert!(review_drop(&mut app, &[invalid]).is_err());
    assert!(app.session.documents().is_empty() && app.session.extensions.installed.is_empty());
}

#[test]
fn browser_plugin_drops_are_bounded_and_leave_image_arrivals_alone() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    assert!(!review_arrived(&mut app, &[("photo.png".into(), vec![1, 2, 3])]));
    let guest = module(include_str!("../../../../examples/extensions/plugin-starter/manifest.json"));
    assert!(review_arrived(&mut app, &[("Example.pcext".into(), guest)]));
    assert!(app.ui.extensions.pending.is_some());
    assert!(review_arrived(&mut app, &[("native.pcplugin".into(), vec![])]));
    assert!(app.ui.extensions.pending.is_none());
    assert!(app.ui.extensions.status.contains("desktop"));
    assert!(review_arrived(&mut app, &[("huge.pcext".into(), vec![0; MAX_MODULE_BYTES + 1])]));
    assert!(app.ui.extensions.status.contains("8 MiB"));
    assert!(app.session.extensions.installed.is_empty());
}

#[test]
fn raw_filter_files_retain_explicit_confirmation() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    let filter = include_bytes!("../../../plugins/tests/fixtures/invert.wasm");
    assert!(!review_arrived(&mut app, &[("filter.wasm".into(), filter.to_vec())]), "raw filters are routed only when their installer is open");
    app.ui.extensions.manager_open = true;
    assert!(review_arrived(&mut app, &[("filter.wasm".into(), filter.to_vec())]));
    assert!(app.ui.extensions.pending.is_none());
    let dialog = app.ui.dialogs.last().unwrap();
    assert_eq!(dialog.fields["__command"], "plugin.install");
    assert_eq!(dialog.fields["replace"], false);
    assert!(!dialog.fields.contains_key("__preview"));
}

#[test]
fn standalone_filter_selection_clears_only_after_its_successful_confirmation() {
    fn filter(id: &str) -> Vec<u8> {
        let manifest = json!({"id":id,"name":"Manager filter","version":"1.0.0","kind":"filter"}).to_string();
        let escaped = manifest.bytes().map(|byte| format!("\\{byte:02x}")).collect::<String>();
        wat::parse_str(format!(
            r#"(module (memory (export "memory") 1)
            (data (i32.const 16) "{escaped}")
            (func (export "pc_abi_version") (result i32) i32.const 1)
            (func (export "pc_manifest") (result i64) i64.const {})
            (func (export "pc_alloc") (param i32) (result i32) i32.const 0)
            (func (export "pc_filter") (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32) i32.const 0))"#,
            ((manifest.len() as u64) << 32) | 16,
        ))
        .unwrap()
    }
    for outcome in ["done", "failed", "cancelled", "unrelated"] {
        let id = format!("test.ui.manager-filter-{outcome}");
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        installer::review_answer(&mut app, FileDialogAnswer::Contents("Manager-filter.wasm".into(), filter(&id))).unwrap();
        assert_eq!(app.ui.extensions.filter_review.as_deref(), Some(id.as_str()));
        let dialog = app.ui.dialogs.last().unwrap().id;
        match outcome {
            "done" => {
                crate::dialogs::confirm(&mut app, dialog).unwrap();
            }
            "failed" => {
                app.ui.dialog_mut(dialog).unwrap().fields.insert("data".into(), json!("invalid filter bytes"));
                assert!(crate::dialogs::confirm(&mut app, dialog).is_err());
            }
            "cancelled" => {
                crate::dialogs::cancel(&mut app, dialog).unwrap();
            }
            _ => {
                crate::dialogs::cancel(&mut app, dialog).unwrap();
                let other = "test.ui.manager-filter-unrelated-other";
                let result =
                    crate::plugin_ui::review_filter(&mut app, json!({"data":base64::engine::general_purpose::STANDARD.encode(filter(other))})).unwrap();
                crate::dialogs::confirm(&mut app, result["dialog"].as_u64().unwrap()).unwrap();
                photocraft_plugins::registry::remove(other);
            }
        }
        if outcome == "done" {
            assert!(app.ui.extensions.path.is_empty() && app.ui.extensions.filter_review.is_none());
            assert_eq!(app.ui.extensions.status, "Installed Manager filter 1.0.0.");
            assert!(photocraft_plugins::registry::get(&id).is_some());
            photocraft_plugins::registry::remove(&id);
        } else {
            assert_eq!(app.ui.extensions.path, "Manager-filter.wasm");
            assert_eq!(app.ui.extensions.filter_review.as_deref(), Some(id.as_str()));
            assert!(!app.ui.extensions.status.contains("Installed"));
            assert!(photocraft_plugins::registry::get(&id).is_none());
        }
    }
}

#[test]
fn incompatible_wasm_panels_keep_their_error_and_never_open_filter_confirmation() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.ui.extensions.manager_open = true;
    let mut manifest: Value = serde_json::from_str(include_str!("../../../../examples/extensions/plugin-starter/manifest.json")).unwrap();
    manifest["apiVersion"] = json!(99);
    assert!(review_arrived(&mut app, &[("incompatible.wasm".into(), module(&manifest.to_string()))]));
    assert!(app.ui.dialogs.is_empty());
    assert!(app.ui.extensions.pending.is_none());
    assert!(!app.ui.extensions.status.is_empty());
}

#[test]
fn recovery_mode_never_classifies_or_offers_a_dropped_legacy_filter() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.session.plugins_disabled = true;
    app.ui.extensions.manager_open = true;
    let filter = include_bytes!("../../../plugins/tests/fixtures/invert.wasm");
    assert!(review_arrived(&mut app, &[("filter.wasm".into(), filter.to_vec())]));
    assert!(app.ui.dialogs.is_empty() && app.ui.extensions.pending.is_none());
    assert!(app.ui.extensions.status.contains("disabled"));
}

#[test]
fn browser_drop_respects_an_outstanding_file_picker() {
    let (dialog, _) = crate::file_dialog::fake(vec![None]);
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services { file_dialog: Some(dialog), ..Default::default() });
    choose(&mut app).unwrap();
    let guest = module(include_str!("../../../../examples/extensions/plugin-starter/manifest.json"));
    assert!(review_arrived(&mut app, &[("Example.pcext".into(), guest)]));
    assert!(app.file_dialog_open());
    assert!(app.ui.extensions.pending.is_none());
    assert!(app.ui.extensions.status.contains("Close the file browser"));
}

#[test]
fn done_closes_the_manager_and_plugin_panel() {
    use egui_kittest::{Harness, kittest::Queryable};
    for manager in [true, false] {
        let mut app = app();
        let id = if manager { MANAGER } else { "window.extension.org.photocraft.quick-tools" };
        menu(&mut app, id).unwrap().unwrap();
        let mut h = Harness::builder().with_size(egui::vec2(1100.0, 480.0)).build_ui_state(initialized_windows(), app);
        PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ALL[0]);
        h.run_steps(4);
        h.get_by_label("Done").click();
        h.run_steps(3);
        if manager {
            assert!(!h.state().ui.extensions.manager_open);
        } else {
            assert!(!h.state().ui.extensions.panels["org.photocraft.quick-tools"].open);
        }
    }
}

#[test]
fn same_version_reinstall_is_explicit_and_secondary() {
    use egui_kittest::{Harness, kittest::Queryable};
    let mut app = app();
    let manifest = include_str!("../../../../examples/extensions/quick-tools/manifest.json");
    let data = base64::engine::general_purpose::STANDARD.encode(module(manifest));
    inspect(&mut app, json!({"data":data})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1100.0, 800.0)).build_ui_state(initialized_windows(), app);
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ALL[0]);
    h.run_steps(4);
    assert_eq!(h.state().ui.extensions.pending.as_ref().unwrap().action, InstallAction::Reinstall);
    assert!(h.query_by_label("Install plugin").is_none());
    assert!(h.query_by_label("Update plugin").is_none());
    assert!(h.query_by_label("Replace or repair an existing installation").is_none());
    assert!(h.query_by_label("Reinstall or repair plugin").is_none());
    h.get_by_label("Advanced").click();
    h.run_steps(3);
    h.get_by_label("Reinstall or repair plugin").click();
    h.run_steps(3);
    assert!(h.state().ui.extensions.pending.is_none());
    assert_eq!(h.state().ui.extensions.status, "Reinstalled Quick Tools 1.0.0.");
    assert_eq!(h.state().session.extensions.installed.len(), 1);
}

#[test]
fn newer_files_update_automatically_and_older_or_ambiguous_versions_require_advanced_action() {
    use egui_kittest::{Harness, kittest::Queryable};
    for (version, action, label, success) in [
        ("2.0.0", InstallAction::Update, "Update plugin", "Updated"),
        ("0.9.0", InstallAction::Downgrade, "Install older version", "Installed older version of"),
        ("edition-next", InstallAction::Replace, "Replace installed plugin", "Replaced"),
    ] {
        let mut app = app();
        let mut manifest: Value = serde_json::from_str(include_str!("../../../../examples/extensions/quick-tools/manifest.json")).unwrap();
        manifest["version"] = json!(version);
        let data = base64::engine::general_purpose::STANDARD.encode(module(&manifest.to_string()));
        app.ui.extensions.path = format!("Quick-Tools-{version}.pcext");
        inspect(&mut app, json!({"data":data})).unwrap();
        let pending = app.ui.extensions.pending.as_ref().unwrap();
        assert_eq!(pending.action, action);
        assert!(pending.input.get("replace").is_none());
        for guard in ["expectedHash", "expectedStoreHash", "expectedInstalledHash", "expectedActivation", "expectedSessionRevision"] {
            assert!(pending.input.get(guard).is_some(), "the reviewed {guard} must reach the final install");
        }
        let mut h = Harness::builder().with_size(egui::vec2(1100.0, 800.0)).build_ui_state(initialized_windows(), app);
        PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ProMedium);
        h.run_steps(4);
        assert_eq!(h.state().session.extensions.installed["org.photocraft.quick-tools"].extension.manifest.version, "1.0.0");
        assert!(h.query_by_label("Replace or repair an existing installation").is_none());
        if action != InstallAction::Update {
            assert!(h.query_by_label("Install plugin").is_none() && h.query_by_label("Update plugin").is_none());
            assert!(h.query_by_label(label).is_none());
            h.get_by_label("Advanced").click();
            h.run_steps(3);
        }
        h.get_by_label(label).click();
        h.run_steps(3);
        assert!(h.state().ui.extensions.pending.is_none(), "{}", h.state().ui.extensions.status);
        assert!(h.state().ui.extensions.path.is_empty());
        assert_eq!(h.state().session.extensions.installed["org.photocraft.quick-tools"].extension.manifest.version, version);
        assert_eq!(h.state().ui.extensions.status, format!("{success} Quick Tools {version}."));
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn background_install_completes_before_clearing_selection_and_preserves_it_on_failure_or_cancellation() {
    use photocraft_engine::{EngineError, jobs::Started};
    for outcome in ["done", "failed", "cancelled"] {
        let mut app = app();
        let mut manifest: Value = serde_json::from_str(include_str!("../../../../examples/extensions/quick-tools/manifest.json")).unwrap();
        manifest["version"] = json!("2.0.0");
        let bytes = module(&manifest.to_string());
        app.ui.extensions.path = "Quick-Tools-2.0.0.pcext".into();
        inspect(&mut app, json!({"data":base64::engine::general_purpose::STANDARD.encode(&bytes)})).unwrap();
        let selected = app.ui.extensions.pending.clone();
        let input = selected.as_ref().unwrap().input.clone();
        let (release, waiting) = std::sync::mpsc::channel();
        let started = app
            .session
            .start_job(
                "test.plugin-install",
                json!({}),
                "Install plugin",
                false,
                move |ctx| {
                    loop {
                        ctx.check()?;
                        match waiting.recv_timeout(std::time::Duration::from_millis(5)) {
                            Ok(()) => break,
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                            Err(error) => return Err(EngineError::Other(error.to_string())),
                        }
                    }
                    if outcome == "failed" {
                        return Err(EngineError::Other("The selected package could not be read".into()));
                    }
                    Ok(input)
                },
                |session, input| session.execute("extension.install", input),
            )
            .unwrap();
        let Started::Job(job) = started else { panic!("the manager test requires a background operation") };
        app.ui.extensions.manager_job = Some(ManagerJob { id: job.0, work: ManagerWork::Install(InstallAction::Update) });
        poll_manager_job(&mut app);
        assert_eq!(app.ui.extensions.pending, selected, "starting a job must not claim installation success");
        assert_eq!(app.ui.extensions.path, "Quick-Tools-2.0.0.pcext");
        assert!(review_arrived(&mut app, &[("Other.pcext".into(), bytes)]));
        assert_eq!(app.ui.extensions.pending, selected, "a new drop cannot replace an active operation's reviewed authority");
        if outcome == "cancelled" {
            app.session.cancel_job(job);
            assert!(app.session.wait_job(job).is_err());
            app.session.join_cancelled_jobs();
        } else {
            release.send(()).unwrap();
            assert_eq!(app.session.wait_job(job).is_ok(), outcome == "done");
        }
        poll_manager_job(&mut app);
        assert!(app.ui.extensions.manager_job.is_none());
        if outcome == "done" {
            assert!(app.ui.extensions.pending.is_none() && app.ui.extensions.path.is_empty());
            assert_eq!(app.ui.extensions.status, "Updated Quick Tools 2.0.0.");
            assert_eq!(app.session.extensions.installed["org.photocraft.quick-tools"].extension.manifest.version, "2.0.0");
        } else {
            assert_eq!(app.ui.extensions.pending, selected);
            assert_eq!(app.ui.extensions.path, "Quick-Tools-2.0.0.pcext");
            assert_eq!(app.session.extensions.installed["org.photocraft.quick-tools"].extension.manifest.version, "1.0.0");
            assert!(app.ui.extensions.status.contains(if outcome == "failed" { "could not be read" } else { "cancelled" }));
        }
    }
}

#[test]
fn update_results_offer_available_updates_without_claiming_failed_checks_are_current() {
    use egui_kittest::{Harness, kittest::Queryable};
    for (status, message, available) in [
        ("available", "", true),
        ("error", "Could not reach the publisher. Try again.", false),
        ("unsupported", "This plugin does not publish automatic updates.", false),
    ] {
        let mut app = app();
        finish_manager_work(&mut app, ManagerWork::Check, &json!({"updates":[{
            "id":"org.photocraft.quick-tools", "status":status, "installedVersion":"1.0.0", "version":"2.0.0", "message":message, "candidate":"opaque-publisher-candidate"
        }]})).unwrap();
        app.ui.extensions.manager_open = true;
        let mut h = Harness::builder().with_size(egui::vec2(1100.0, 800.0)).build_ui_state(initialized_windows(), app);
        PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ProMedium);
        h.run_steps(4);
        assert!(h.query_by_label("Check for updates").is_some());
        assert_eq!(h.query_by_label("Update…").is_some(), available);
        assert_eq!(h.query_by_label("Update available: 2.0.0").is_some(), available);
        assert!(h.query_by_label("Up to date.").is_none());
        if !available {
            assert!(h.query_by_label(message).is_some());
        }
        assert_eq!(h.query_by_label("Retry update check").is_some(), status == "error");
    }
}

#[test]
fn prepared_updates_return_to_permission_review_without_installing() {
    let mut app = app();
    let mut manifest: Value = serde_json::from_str(include_str!("../../../../examples/extensions/quick-tools/manifest.json")).unwrap();
    manifest["version"] = json!("2.0.0");
    let data = base64::engine::general_purpose::STANDARD.encode(module(&manifest.to_string()));
    let mut inspected = app.session.execute("extension.inspect", json!({"data":data})).unwrap();
    inspected["input"] = json!({"data":data});
    finish_manager_work(&mut app, ManagerWork::Prepare, &inspected).unwrap();
    let pending = app.ui.extensions.pending.as_ref().unwrap();
    assert_eq!(pending.action, InstallAction::Update);
    assert_eq!(pending.input["expectedHash"], inspected["hash"]);
    assert_eq!(pending.input["expectedSessionRevision"], inspected["installation"]["expectedSessionRevision"]);
    assert!(pending.input.get("allowNative").is_none(), "download preparation never grants native access");
    assert!(app.ui.extensions.path.is_empty());
    assert_eq!(app.session.extensions.installed["org.photocraft.quick-tools"].extension.manifest.version, "1.0.0");
}

#[test]
fn removed_panel_state_is_released_while_disabled_and_recovery_settings_survive() {
    let mut app = app();
    let id = "org.photocraft.quick-tools";
    app.ui.extensions.panels.entry(id.into()).or_default().values = json!({"percent":150});
    app.run("extension.enable", json!({"id":id,"enabled":false})).unwrap();
    let ctx = egui::Context::default();
    let draw = |app: &mut PhotocraftApp| {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| windows(app, ui.ctx()));
        output.textures_delta.clear();
    };
    draw(&mut app);
    assert!(app.ui.extensions.panels.contains_key(id));
    app.run("extension.remove", json!({"id":id})).unwrap();
    app.session.plugins_disabled = true;
    draw(&mut app);
    assert!(app.ui.extensions.panels.contains_key(id), "recovery must preserve saved panel settings");
    app.session.plugins_disabled = false;
    draw(&mut app);
    assert!(!app.ui.extensions.panels.contains_key(id));
}

#[cfg(not(target_arch = "wasm32"))]
fn panel_job(app: &mut PhotocraftApp, fail: bool) -> photocraft_engine::jobs::JobId {
    let started = app
        .session
        .start_job(
            "test.plugin-processing",
            json!({}),
            "Plugin processing",
            true,
            move |_| if fail { Err(photocraft_engine::EngineError::Other("Test processing failed".into())) } else { Ok(()) },
            |_, ()| Ok(json!({"message":"Created processed copy"})),
        )
        .unwrap();
    let photocraft_engine::jobs::Started::Job(job) = started else { panic!("background job expected") };
    job
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn panels_show_success_failure_cancellation_and_stale_source_outcomes() {
    use egui_kittest::{Harness, kittest::Queryable};

    for outcome in ["done", "failed", "cancelled", "stale"] {
        let mut app = app();
        app.run("file.new", json!({"width":8,"height":6})).unwrap();
        menu(&mut app, "window.extension.org.photocraft.quick-tools").unwrap().unwrap();
        let job = panel_job(&mut app, outcome == "failed");
        let panel = app.ui.extensions.panels.get_mut("org.photocraft.quick-tools").unwrap();
        panel.job = Some(job.0);
        panel.message = "Processing…".into();
        let mut h = Harness::builder().with_size(egui::vec2(1100.0, 800.0)).build_ui_state(initialized_windows(), app);
        PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ALL[0]);
        h.run_steps(3);
        assert!(h.query_by_label("Cancel processing").is_some());
        assert_eq!(h.state().ui.extensions.panels["org.photocraft.quick-tools"].job, Some(job.0));
        if outcome == "cancelled" {
            h.get_by_label("Cancel processing").click();
            h.run_steps(3);
            assert!(h.state_mut().session.wait_job(job).is_err());
            h.state_mut().session.join_cancelled_jobs();
        } else {
            if outcome == "stale" {
                std::sync::Arc::make_mut(&mut h.state_mut().session.active_mut().unwrap().doc).name = "Changed source".into();
            }
            assert_eq!(h.state_mut().session.wait_job(job).is_ok(), outcome == "done");
        }
        assert!(h.state().session.job(job).is_none(), "finished jobs leave the running-only lookup");
        h.run_steps(3);
        let panel = &h.state().ui.extensions.panels["org.photocraft.quick-tools"];
        assert!(panel.job.is_none(), "{outcome} must clear the running reference");
        assert_eq!(panel.error, matches!(outcome, "failed" | "stale"));
        let expected = match outcome {
            "done" => "Created processed copy",
            "failed" => "Test processing failed",
            "cancelled" => "Cancelled. The original is unchanged.",
            _ => "changed while it ran",
        };
        assert!(panel.message.contains(expected), "{outcome}: {}", panel.message);
        let visible = if panel.error { format!("Action failed: {}", panel.message) } else { panel.message.clone() };
        assert!(h.query_by_label(&visible).is_some(), "{outcome} must be visible");
        assert!(h.query_by_label("Cancel processing").is_none());
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn reopened_panel_clears_an_evicted_job_without_claiming_success() {
    use egui_kittest::{Harness, kittest::Queryable};

    let mut app = app();
    app.run("file.new", json!({"width":8,"height":6})).unwrap();
    let original = panel_job(&mut app, false);
    app.session.wait_job(original).unwrap();
    // More than the bounded recent history can complete while this panel is closed.
    for _ in 0..32 {
        let job = panel_job(&mut app, false);
        app.session.wait_job(job).unwrap();
    }
    assert!(!app.session.jobs_with_recent().iter().any(|job| job.id == original));
    menu(&mut app, "window.extension.org.photocraft.quick-tools").unwrap().unwrap();
    let panel = app.ui.extensions.panels.get_mut("org.photocraft.quick-tools").unwrap();
    panel.job = Some(original.0);
    panel.message = "Processing…".into();
    let mut h = Harness::builder().with_size(egui::vec2(1100.0, 800.0)).build_ui_state(initialized_windows(), app);
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ALL[0]);
    h.run_steps(3);
    let panel = &h.state().ui.extensions.panels["org.photocraft.quick-tools"];
    assert!(panel.job.is_none());
    assert!(!panel.error);
    assert!(panel.message.contains("status is no longer available"));
    assert!(h.query_by_label(&panel.message).is_some());
    assert!(h.query_by_label("Cancel processing").is_none());
    assert!(h.query_by_label("Completed.").is_none());
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[test]
fn native_packages_are_reviewed_installed_and_opened_without_running_workers() {
    use egui_kittest::{Harness, kittest::Queryable};

    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let dir = Scratch(std::env::temp_dir().join(format!(
        "photocraft-plugin-review-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    )));
    std::fs::create_dir(&dir.0).unwrap();
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.session.extensions.attach_folder(dir.0.join("Extensions")).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1100.0, 900.0)).build_ui_state(initialized_windows(), app);
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ALL[0]);
    struct NativePackageFixture {
        id: &'static str,
        name: &'static str,
        service: &'static str,
        worker: &'static str,
        files: &'static [&'static str],
    }
    let fixtures = [
        NativePackageFixture {
            id: "org.example.packaged-effect",
            name: "Packaged Effect",
            service: "plugin.processImage",
            worker: "effect-worker.exe",
            files: &[],
        },
        NativePackageFixture {
            id: "org.example.packaged-model",
            name: "Packaged Model Processor",
            service: "plugin.processImage",
            worker: "bin/model-worker.exe",
            files: &["runtime/bin/inference.dll", "runtime/models/weights.bin"],
        },
    ];
    for (installed_count, NativePackageFixture { id, name, service, worker, files }) in fixtures.iter().enumerate() {
        let mut manifest: Value = serde_json::from_str(include_str!("../../../../examples/extensions/quick-tools/manifest.json")).unwrap();
        manifest["id"] = json!(id);
        manifest["name"] = json!(name);
        manifest["permissions"]["commands"] = json!([service]);
        let mut zip = photocraft_format::zip::ZipWriter::new();
        zip.add(
            "package.json",
            &serde_json::to_vec(&json!({
                "packageVersion": 2,
                "module": "plugin.pcext",
                "native": {"service": service, "platform": "windows-x86_64", "entry": worker}
            }))
            .unwrap(),
        )
        .unwrap();
        zip.add("plugin.pcext", &module(&manifest.to_string())).unwrap();
        // Review, installation, and opening a panel must never execute these invalid programs.
        zip.add(worker, b"test fixture; never execute").unwrap();
        for file in files.iter().copied().chain(["LICENSE", "THIRD-PARTY-NOTICES.md"]) {
            zip.add(file, b"fixture").unwrap();
        }
        let package = dir.0.join(format!("{id}.pcplugin"));
        std::fs::write(&package, zip.finish().unwrap()).unwrap();
        #[derive(Debug)]
        struct NativeDrop(std::path::PathBuf);
        impl egui::DroppedFile for NativeDrop {
            fn path(&self) -> &std::path::Path {
                &self.0
            }
            fn bytes(&self) -> Result<Vec<u8>, String> {
                Err("Native plugin drops must use the bounded path reader".into())
            }
        }
        let dropped: egui::DroppedFileHandle = std::sync::Arc::new(NativeDrop(package.clone()));
        review_drop(h.state_mut(), &[dropped]).unwrap();
        assert_eq!(h.state().session.extensions.installed.len(), installed_count, "review must not install the package");
        let pending = h.state().ui.extensions.pending.as_ref().unwrap();
        assert!(pending.native);
        assert_eq!(pending.manifest.id, *id);
        assert!(pending.input.get("allowNative").is_none(), "native access is granted only when Install is clicked");
        let storage = pending.storage;
        let package_label = format!("Package size: {}", storage::bytes(pending.download_bytes));
        assert_eq!(pending.download_bytes, std::fs::metadata(&package).unwrap().len());
        assert!(storage.installed_bytes > storage.model_bytes);
        assert_eq!(storage.model_bytes, files.iter().filter(|file| file.starts_with("runtime/models/")).count() as u64 * 7);
        h.run_steps(4);
        assert!(h.query_by_label(&package_label).is_some());
        assert!(h.query_by_label(&storage::label(storage)).is_some());
        let included = if storage.model_bytes > 0 { "Includes its local processing engine and models." } else { "Includes its local processing engine." };
        assert!(h.query_by_label(included).is_some());
        assert_eq!(h.query_by_label("Includes its local processing engine and models.").is_some(), storage.model_bytes > 0);
        assert!(h.query_by_label("Native code runs with your operating system permissions. Install plugins only from publishers you trust.").is_some());
        for label in ["Choose worker…", "Disconnect worker"] {
            assert!(h.query_by_label(label).is_none(), "{label} must not be part of packaged plugin installation");
        }
        h.get_by_label("Install plugin").click();
        h.run_steps(3);
        assert!(h.state().ui.extensions.pending.is_none(), "{}", h.state().ui.extensions.status);
        assert_eq!(h.state().session.extensions.installed.len(), installed_count + 1);
        assert_eq!(h.state().session.extensions.installed[*id].native.as_ref().unwrap().package.native.as_ref().unwrap().service, *service);
        assert_eq!(h.state().session.extensions.installed[*id].storage, storage);
        assert!(h.query_by_label(&storage::label(storage)).is_some());
        assert_eq!(h.query_all_by_label("Local processing engine included.").count(), installed_count + 1);
        h.get_by_label("Done").click();
        h.run_steps(3);
        assert!(!h.state().ui.extensions.manager_open);

        let menu_id = format!("{PREFIX}{id}");
        menu(h.state_mut(), &menu_id).unwrap().unwrap();
        h.run_steps(3);
        assert!(h.state().ui.extensions.panels[*id].open);
        assert!(h.state().ui.extensions.panels[*id].job.is_none());
        assert!(h.state().session.active_job().is_none());
        assert!(h.state().session.extensions.installed[*id].state.is_null());
        h.get_by_label("Done").click();
        h.run_steps(3);
        assert!(!h.state().ui.extensions.panels[*id].open);
    }

    let entries = crate::menus::menu_items(h.state());
    for NativePackageFixture { id, name, .. } in fixtures {
        let menu_id = format!("{PREFIX}{id}");
        let matches: Vec<_> = entries.iter().filter(|item| item.id == menu_id).collect();
        assert_eq!(matches.len(), 1, "each installed package gets its own menu item");
        assert_eq!(matches[0].label, name);
        assert_eq!(matches[0].path, ["Plugins"]);
        assert!(matches[0].enabled);
        assert_eq!(matches[0].checked, Some(false));
    }
    assert_eq!(entries.iter().rfind(|item| item.path == ["Plugins"]).unwrap().id, MANAGER);
}

/// One bounded capture batch for the manager's existing native themes and compact layout.
#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "requires PHOTOCRAFT_PLUGIN_MANAGER_CAPTURES; renders the native plugin manager"]
fn plugin_manager_visuals() {
    use egui_kittest::{Harness, kittest::Queryable};
    let captures = std::path::PathBuf::from(std::env::var_os("PHOTOCRAFT_PLUGIN_MANAGER_CAPTURES").unwrap());
    std::fs::create_dir_all(&captures).unwrap();
    for phase in ["empty", "permissions", "installed", "updates", "update-permissions"] {
        let mut app = if ["installed", "updates", "update-permissions"].contains(&phase) {
            app()
        } else {
            PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default())
        };
        app.ui.extensions.manager_open = true;
        if phase == "permissions" {
            installer::review_answer(
                &mut app,
                FileDialogAnswer::Contents(
                    "Hello-PhotoCraft.pcext".into(),
                    module(include_str!("../../../../examples/extensions/plugin-starter/manifest.json")),
                ),
            )
            .unwrap();
        }
        if phase == "updates" {
            finish_manager_work(
                &mut app,
                ManagerWork::Check,
                &json!({"updates":[{
                    "id":"org.photocraft.quick-tools", "status":"available", "installedVersion":"1.0.0", "version":"2.0.0", "candidate":"test-visual-candidate"
                }]}),
            )
            .unwrap();
        }
        if phase == "update-permissions" {
            let mut manifest: Value = serde_json::from_str(include_str!("../../../../examples/extensions/quick-tools/manifest.json")).unwrap();
            manifest["version"] = json!("2.0.0");
            installer::review_answer(&mut app, FileDialogAnswer::Contents("Quick-Tools-2.0.0.pcext".into(), module(&manifest.to_string()))).unwrap();
        }
        let mut h = Harness::builder().with_size(egui::vec2(1152.0, 768.0)).wgpu().build_ui_state(initialized_windows(), app);
        for theme in [crate::theme::ThemeKind::ProMedium, crate::theme::ThemeKind::ProLight, crate::theme::ThemeKind::Classic] {
            for (width, height) in [(1152.0, 768.0), (640.0, 480.0)] {
                h.set_size(egui::vec2(width, height));
                h.ctx.memory_mut(|memory| {
                    memory.reset_areas();
                    memory.data.clear();
                });
                PhotocraftApp::setup_context(&h.ctx, theme);
                h.run_steps(8);
                let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height));
                let labels = match phase {
                    "permissions" => vec!["Browse…", "Install plugin", "Done"],
                    "update-permissions" => vec!["Browse…", "Update plugin", "Done"],
                    _ => vec!["Browse…", "Done"],
                };
                for label in labels {
                    assert!(viewport.contains_rect(h.get_by_label(label).rect()), "{phase} {} {width}x{height}: {label} is clipped", theme.id());
                }
                assert!(h.query_by_label("Review plugin").is_none());
                assert!(h.query_by_label("Drop a plugin file here\nor click to browse").is_none());
                h.render().unwrap().save(captures.join(format!("plugin-manager-{phase}-{}-{width:.0}x{height:.0}.png", theme.id()))).unwrap();
            }
        }
    }
}

/// Optional visual check using a real package; never executes its worker or changes user settings.
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[test]
#[ignore = "requires PHOTOCRAFT_PLUGIN_STORAGE_PACKAGE and PHOTOCRAFT_PLUGIN_STORAGE_CAPTURES"]
fn installed_package_storage_visuals() {
    use egui_kittest::{Harness, kittest::Queryable};

    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let dir = Scratch(std::env::temp_dir().join(format!("photocraft-storage-visual-{}", std::process::id())));
    std::fs::create_dir(&dir.0).unwrap();
    let package = std::env::var("PHOTOCRAFT_PLUGIN_STORAGE_PACKAGE").unwrap();
    let captures = std::path::PathBuf::from(std::env::var_os("PHOTOCRAFT_PLUGIN_STORAGE_CAPTURES").unwrap());
    std::fs::create_dir_all(&captures).unwrap();
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.session.extensions.attach_folder(dir.0.join("Extensions")).unwrap();
    inspect(&mut app, json!({"path": package})).unwrap();
    let size = app.ui.extensions.pending.as_ref().unwrap().storage;
    assert!(size.installed_bytes > size.model_bytes && size.model_bytes > 0);
    let mut h = Harness::builder().with_size(egui::vec2(1152.0, 768.0)).wgpu().build_ui_state(initialized_windows(), app);
    for phase in ["review", "installed"] {
        for theme in crate::theme::ThemeKind::ALL {
            PhotocraftApp::setup_context(&h.ctx, theme);
            for (width, height) in [(1152.0, 768.0), (640.0, 480.0)] {
                h.set_size(egui::vec2(width, height));
                h.run_steps(6);
                let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height));
                for label in [&storage::label(size), "Done"] {
                    let node = h.get_by_label(label);
                    assert!(viewport.contains_rect(node.rect()), "{phase} {} {width}×{height}: {label} is clipped", theme.id());
                }
                h.render().unwrap().save(captures.join(format!("plugin-storage-{phase}-{}-{width:.0}x{height:.0}.png", theme.id()))).unwrap();
            }
        }
        if phase == "review" {
            h.state_mut().run("extension.install", json!({"path": package, "allowNative": true})).unwrap();
            h.state_mut().ui.extensions.pending = None;
        }
    }
    assert!(h.state().session.active_job().is_none());
}
