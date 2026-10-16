//! Neutral package UI coverage uses an independently compiled filter fixture.
use super::*;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

const ID: &str = "org.example.general-ui";
const FILTER: &[u8] = include_bytes!("../../../../plugins/tests/fixtures/invert.wasm");
const NATIVE_WARNING: &str = "Native code runs with your operating system permissions. Install plugins only from publishers you trust.";

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "photocraft-general-plugin-ui-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn panel_module(manifest: &Value, response: &Value) -> Vec<u8> {
    let manifest = manifest.to_string();
    let response = response.to_string();
    let quote = |value: &str| value.bytes().map(|byte| format!("\\{byte:02x}")).collect::<String>();
    wat::parse_str(format!(
        r#"(module (memory (export "memory") 4)
        (data (i32.const 32) "{}") (data (i32.const 65536) "{}")
        (func (export "pce_api_version") (result i32) i32.const 1)
        (func (export "pce_manifest") (result i64) i64.const {})
        (func (export "pce_alloc") (param i32) (result i32) i32.const 196608)
        (func (export "pce_handle") (param i32 i32) (result i64) i64.const {}))"#,
        quote(&manifest),
        quote(&response),
        ((manifest.len() as u64) << 32) | 32,
        ((response.len() as u64) << 32) | 65536
    ))
    .unwrap()
}

fn filter_package(dir: &Path) -> PathBuf {
    let manifest = json!({
        "apiVersion":1,"id":ID,"name":"General Image Processor","version":"1.0.0",
        "author":"Example author","description":"An independently packaged sandboxed image processor.",
        "permissions":{"documentRead":true,"commands":["plugin.processImage"]},
        "capabilities":{"required":[{"id":"host.commands.invoke","minVersion":1}]},
        "panel":{"preview":true,"fields":[],"buttons":[{"id":"process","label":"Generate preview"}]}
    });
    let response = json!({"state":{"runs":1},"commands":[{"command":"plugin.processImage","params":{
        "processor":"org.photocraft.example.invert","options":{},"preview":true
    }}]});
    let mut zip = photocraft_format::zip::ZipWriter::new();
    zip.add("package.json", &serde_json::to_vec(&json!({"packageVersion":2,"module":"plugin.pcext","filters":[{"module":"filters/invert.wasm"}]})).unwrap())
        .unwrap();
    zip.add("plugin.pcext", &panel_module(&manifest, &response)).unwrap();
    zip.add("filters/invert.wasm", FILTER).unwrap();
    zip.add("LICENSE", b"Test license").unwrap();
    zip.add("THIRD-PARTY-NOTICES.md", b"Test fixture").unwrap();
    let path = dir.join("General-Image-Processor.pcplugin");
    std::fs::write(&path, zip.finish().unwrap()).unwrap();
    path
}

fn package_app(dir: &Path) -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.session.extensions.attach_folder(dir.join("Extensions")).unwrap();
    app
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[test]
fn native_tasks_need_no_image_and_failed_tasks_preserve_an_existing_comparison() {
    for with_image in [false, true] {
        let dir = Scratch::new();
        let manifest = json!({"apiVersion":1,"id":ID,"name":"General task test","version":"1",
            "permissions":{"commands":["plugin.processImage","plugin.runTask"]},
            "capabilities":{"required":[{"id":"host.commands.invoke","minVersion":1}]},
            "panel":{"preview":true,"buttons":[{"id":"refresh","label":"Check assets"}]}});
        let response = json!({"state":{"runs":1},"commands":[{"command":"plugin.runTask","params":{"options":{"action":"refresh"}}}]});
        let mut zip = photocraft_format::zip::ZipWriter::new();
        zip.add(
            "package.json",
            &serde_json::to_vec(&json!({"packageVersion":2,"module":"plugin.pcext",
            "native":{"service":"plugin.processImage","platform":"windows-x86_64","entry":"worker.exe"},
            "filters":[{"module":"invert.wasm"}]}))
            .unwrap(),
        )
        .unwrap();
        zip.add("plugin.pcext", &panel_module(&manifest, &response)).unwrap();
        zip.add("invert.wasm", FILTER).unwrap();
        zip.add("worker.exe", b"Invalid executable fixture: launching must fail safely").unwrap();
        zip.add("LICENSE", b"Fixture").unwrap();
        zip.add("THIRD-PARTY-NOTICES.md", b"Fixture").unwrap();
        let path = dir.0.join("task.pcplugin");
        std::fs::write(&path, zip.finish().unwrap()).unwrap();
        let mut app = package_app(&dir.0);
        app.run("extension.install", json!({"path":path,"allowNative":true})).unwrap();
        let preview = if with_image {
            app.run("file.new", json!({"width":16,"height":12})).unwrap();
            Some(app.run("plugin.processImage", json!({"id":ID,"processor":"org.photocraft.example.invert"})).unwrap()["preview"].as_u64().unwrap())
        } else {
            None
        };
        menu(&mut app, &format!("{PREFIX}{ID}")).unwrap().unwrap();
        app.background_jobs = true;
        let mut h = harness(app);
        h.run_steps(5);
        assert!(!h.get_by_label("Check assets").accesskit_node().is_disabled());
        h.get_by_label("Check assets").click();
        h.run_steps(2);
        let job = photocraft_engine::jobs::JobId(h.state().ui.extensions.panels[ID].job.unwrap());
        assert!(h.state().session.job(job).unwrap().document.is_none());
        assert!(h.get_by_label("Check assets").accesskit_node().is_disabled());
        assert!(h.query_by_label("Cancel processing").is_some());
        assert_eq!(h.state().ui.extensions.panels[ID].preview, preview);
        if let Some(preview) = preview {
            assert!(h.state().session.plugin_preview(preview).is_some());
        }
        assert!(h.state_mut().session.wait_job(job).is_err());
        h.run_steps(3);
        assert!(h.state().ui.extensions.panels[ID].error);
        assert_eq!(h.state().ui.extensions.panels[ID].preview, preview);
        assert_eq!(h.state().session.documents().len(), usize::from(with_image));
        assert!(h.state().session.extensions.installed[ID].state.is_null());
        h.get_by_label("Done").click();
        h.run_steps(3);
        assert!(!h.state().ui.extensions.panels[ID].open);
    }
}

fn harness(app: PhotocraftApp) -> Harness<'static, PhotocraftApp> {
    let mut ready = false;
    Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_ui_state(
        move |ui, app| {
            if !ready {
                PhotocraftApp::setup_context(ui.ctx(), crate::theme::ThemeKind::ProMedium);
                ready = true;
                return;
            }
            windows(app, ui.ctx());
        },
        app,
    )
}

#[test]
fn filter_only_package_review_install_menu_and_done_need_no_native_consent() {
    let dir = Scratch::new();
    let path = filter_package(&dir.0);
    let mut app = package_app(&dir.0);
    inspect(&mut app, json!({"path":path})).unwrap();
    let pending = app.ui.extensions.pending.as_ref().unwrap();
    assert!(!pending.native);
    assert!(pending.input.get("allowNative").is_none());
    assert!(app.session.extensions.installed.is_empty(), "review is not installation");
    assert_eq!(pending.storage.model_bytes, 0);
    let mut h = harness(app);
    h.run_steps(5);
    assert!(h.query_by_label("Plugin code has no file, network or native process access.").is_some());
    assert!(h.query_by_label(NATIVE_WARNING).is_none());
    assert!(h.query_by_label("Includes its local processing engine and models.").is_none());
    h.get_by_label("Install plugin").click();
    h.run_steps(3);
    assert!(h.state().ui.extensions.pending.is_none(), "{}", h.state().ui.extensions.status);
    let installed = &h.state().session.extensions.installed[ID];
    assert!(installed.native.is_none());
    assert_eq!(installed.filters.len(), 1);
    assert_eq!(installed.package.as_ref().unwrap().package_version, 2);
    assert!(installed.state.is_null());
    assert!(h.query_by_label("Local processing engine included.").is_none());
    let menu_id = format!("{PREFIX}{ID}");
    let entries = crate::menus::menu_items(h.state());
    let entry = entries.iter().find(|entry| entry.id == menu_id).unwrap();
    assert_eq!(entry.path, ["Plugins"]);
    assert!(entry.enabled);
    h.get_by_label("Done").click();
    h.run_steps(3);
    assert!(!h.state().ui.extensions.manager_open);
    menu(h.state_mut(), &menu_id).unwrap().unwrap();
    h.run_steps(3);
    assert!(h.state().ui.extensions.panels[ID].open);
    assert!(h.state().session.active_job().is_none(), "opening the panel must not run its image processor");
    assert!(h.get_by_label("Apply").accesskit_node().is_disabled());
    h.get_by_label("Done").click();
    h.run_steps(3);
    assert!(!h.state().ui.extensions.panels[ID].open);
    let mut restarted = photocraft_engine::Session::new();
    restarted.extensions.attach_folder(dir.0.join("Extensions")).unwrap();
    assert!(restarted.extensions.load_errors.is_empty());
    assert!(restarted.extensions.installed[ID].filters.is_empty());
    assert_eq!(restarted.extensions.installed[ID].package.as_ref().unwrap().filters.len(), 1);
    assert_eq!(restarted.extensions.installed[ID].extension.lifecycle(), "idle");
    assert!(restarted.extensions.installed[ID].native.is_none());
}

fn start_preview(h: &mut Harness<'_, PhotocraftApp>, id: &str) -> u64 {
    let source = Arc::clone(&h.state().session.active().unwrap().doc);
    let documents = h.state().session.documents().len();
    let state = h.state().session.extensions.installed[id].state.clone();
    assert!(h.get_by_label("Apply").accesskit_node().is_disabled());
    h.get_by_label("Generate preview").click();
    h.run_steps(2);
    let panel = &h.state().ui.extensions.panels[id];
    assert!(!panel.error, "{}", panel.message);
    let job = photocraft_engine::jobs::JobId(panel.job.expect("Generate preview starts background processing"));
    assert_eq!(h.state().session.extensions.installed[id].state, state, "proposed state must wait for Apply");
    h.state_mut().session.wait_job(job).unwrap();
    h.run_steps(3);
    let panel = &h.state().ui.extensions.panels[id];
    assert!(!panel.error, "{}", panel.message);
    assert!(panel.job.is_none());
    let token = panel.preview.expect("completed processing offers a comparison");
    assert!(h.state().session.plugin_preview(token).is_some());
    assert_eq!(h.state().session.documents().len(), documents);
    assert!(Arc::ptr_eq(&source, &h.state().session.active().unwrap().doc));
    assert_eq!(h.state().session.extensions.installed[id].state, state);
    assert!(!h.get_by_label("Apply").accesskit_node().is_disabled());
    token
}

fn select_output(h: &mut Harness<'_, PhotocraftApp>, output: &str) {
    let current = if output == "New layer" { "New tab" } else { "New layer" };
    h.query_all_by_role(egui::accesskit::Role::ComboBox).find(|node| node.value().as_deref() == Some(current)).unwrap().click();
    h.run_steps(2);
    h.get_by_label(output).click();
    h.run_steps(2);
}

#[test]
fn packaged_filter_panel_previews_applies_tabs_layers_undo_redo_discards_and_closes() {
    #[derive(Clone, Copy)]
    enum Case {
        Document,
        Layer,
        Discard,
        Done,
        Stale,
    }
    for target in [Case::Document, Case::Layer, Case::Discard, Case::Done, Case::Stale] {
        let dir = Scratch::new();
        let path = filter_package(&dir.0);
        let mut app = package_app(&dir.0);
        app.run("extension.install", json!({"path":path})).unwrap();
        app.run("file.new", json!({"width":32,"height":24,"depth":16,"background":"white"})).unwrap();
        let source = Arc::clone(&app.session.active().unwrap().doc);
        menu(&mut app, &format!("{PREFIX}{ID}")).unwrap().unwrap();
        app.background_jobs = true;
        let mut h = harness(app);
        h.run_steps(5);
        let token = start_preview(&mut h, ID);
        let processed = &h.state().session.plugin_preview(token).unwrap().result;
        assert_eq!((processed.size.width, processed.size.height, processed.depth.bits()), (32, 24, 8));
        let pixel = processed.layers[0].surface().unwrap().pixel(0, 0);
        assert!(
            pixel[0] < 0.01 && pixel[1] < 0.01 && pixel[2] < 0.01 && pixel[3] > 0.99,
            "the packaged invert algorithm must actually process pixels: {pixel:?}"
        );
        match target {
            Case::Document | Case::Layer => {
                if matches!(target, Case::Layer) {
                    select_output(&mut h, "New layer");
                }
                h.get_by_label("Apply").click();
                h.run_steps(3);
                assert!(h.state().ui.extensions.panels[ID].preview.is_none());
                assert_eq!(h.state().session.extensions.installed[ID].state, json!({"runs":1}));
                if matches!(target, Case::Document) {
                    assert_eq!(h.state().session.documents().len(), 2);
                    assert!(h.state().session.documents().iter().any(|document| Arc::ptr_eq(&document.doc, &source)));
                    assert_eq!(h.state().session.active().unwrap().doc.depth.bits(), 8);
                } else {
                    assert_eq!(h.state().session.documents().len(), 1);
                    assert_eq!(h.state().session.active().unwrap().doc.layers.len(), source.layers.len() + 1);
                    assert!(h.state().session.active().unwrap().doc.layers.iter().take(source.layers.len()).all(|layer| !layer.visible));
                    h.state_mut().run("edit.undo", json!({})).unwrap();
                    assert_eq!(h.state().session.active().unwrap().doc.layers.len(), source.layers.len());
                    assert_eq!(
                        h.state().session.active().unwrap().doc.layers.iter().map(|layer| layer.visible).collect::<Vec<_>>(),
                        source.layers.iter().map(|layer| layer.visible).collect::<Vec<_>>()
                    );
                    h.state_mut().run("edit.redo", json!({})).unwrap();
                    assert_eq!(h.state().session.active().unwrap().doc.layers.len(), source.layers.len() + 1);
                }
            }
            Case::Discard => {
                h.get_by_label("Discard preview").click();
                h.run_steps(3);
                assert!(h.state().ui.extensions.panels[ID].preview.is_none());
                assert!(h.state().ui.extensions.panels[ID].open);
                assert!(Arc::ptr_eq(&source, &h.state().session.active().unwrap().doc));
                assert!(h.state().session.extensions.installed[ID].state.is_null());
            }
            Case::Done => {
                h.get_by_label("Done").click();
                h.run_steps(3);
                assert!(!h.state().ui.extensions.panels[ID].open);
                assert!(h.state().ui.extensions.panels[ID].preview.is_none());
                assert!(Arc::ptr_eq(&source, &h.state().session.active().unwrap().doc));
                assert!(h.state().session.extensions.installed[ID].state.is_null());
            }
            Case::Stale => {
                h.state_mut().run("layer.new.layer", json!({"name":"Changed source"})).unwrap();
                h.run_steps(3);
                assert!(h.state().ui.extensions.panels[ID].preview.is_none());
                assert!(h.get_by_label("Apply").accesskit_node().is_disabled());
                assert!(h.state().session.extensions.installed[ID].state.is_null());
            }
        }
        assert!(h.state().session.plugin_preview(token).is_none());
    }
}

/// This is the independently built developer-kit package, not a fake executable fixture.
#[test]
#[ignore = "requires PHOTOCRAFT_GENERAL_PACKAGE pointing at the built Image-Gain.pcplugin"]
fn installed_general_sdk_package_runs_native_and_filter_previews() {
    let package = std::env::var("PHOTOCRAFT_GENERAL_PACKAGE").expect("set PHOTOCRAFT_GENERAL_PACKAGE to the independent developer-kit .pcplugin");
    for (backend, target) in [("Native worker", "document"), ("Sandboxed filter", "layer")] {
        let dir = Scratch::new();
        let mut app = package_app(&dir.0);
        inspect(&mut app, json!({"path":package})).unwrap();
        let pending = app.ui.extensions.pending.as_ref().unwrap();
        let id = pending.manifest.id.clone();
        assert_eq!(id, "org.example.image-gain");
        assert!(pending.native);
        assert!(app.session.active_job().is_none());
        let mut h = harness(app);
        h.run_steps(5);
        assert!(h.query_by_label(NATIVE_WARNING).is_some());
        h.get_by_label("Install plugin").click();
        h.run_steps(3);
        assert!(h.state().ui.extensions.pending.is_none(), "{}", h.state().ui.extensions.status);
        h.get_by_label("Done").click();
        h.run_steps(3);
        h.state_mut().run("file.new", json!({"width":32,"height":24,"depth":16,"background":"white"})).unwrap();
        let source = Arc::clone(&h.state().session.active().unwrap().doc);
        menu(h.state_mut(), &format!("{PREFIX}{id}")).unwrap().unwrap();
        h.state_mut().background_jobs = true;
        let panel = h.state_mut().ui.extensions.panels.get_mut(&id).unwrap();
        panel.values["backend"] = json!(backend);
        panel.values["gain"] = json!(0.5);
        h.run_steps(3);
        assert!(h.query_all_by_role(egui::accesskit::Role::SpinButton).any(|node| node.accesskit_node().label().as_deref() == Some("Gain")));
        let token = start_preview(&mut h, &id);
        let preview = h.state().session.plugin_preview(token).unwrap();
        let pixel = preview.result.layers[0].surface().unwrap().pixel(0, 0);
        assert!((pixel[0] - 0.5).abs() < 0.02, "{backend}: {pixel:?}");
        if target == "layer" {
            select_output(&mut h, "New layer");
        }
        h.get_by_label("Apply").click();
        h.run_steps(3);
        assert!(!h.state().ui.extensions.panels[&id].error, "{}", h.state().ui.extensions.panels[&id].message);
        assert!(h.state().session.plugin_preview(token).is_none());
        if target == "document" {
            assert_eq!(h.state().session.documents().len(), 2);
            assert!(h.state().session.documents().iter().any(|document| Arc::ptr_eq(&document.doc, &source)));
        } else {
            assert_eq!(h.state().session.documents().len(), 1);
            assert_eq!(h.state().session.active().unwrap().doc.layers.len(), source.layers.len() + 1);
            h.state_mut().run("edit.undo", json!({})).unwrap();
            assert_eq!(h.state().session.active().unwrap().doc.layers.len(), source.layers.len());
            h.state_mut().run("edit.redo", json!({})).unwrap();
            assert_eq!(h.state().session.active().unwrap().doc.layers.len(), source.layers.len() + 1);
        }
        h.get_by_label("Done").click();
        h.run_steps(3);
        assert!(!h.state().ui.extensions.panels[&id].open);
    }
}

/// Capture review, installation and the real native result in every supported theme.
#[test]
#[ignore = "requires PHOTOCRAFT_GENERAL_PACKAGE and PHOTOCRAFT_GENERAL_CAPTURES; executes the developer-kit native worker"]
fn installed_general_sdk_package_visuals() {
    let package = std::env::var("PHOTOCRAFT_GENERAL_PACKAGE").expect("set PHOTOCRAFT_GENERAL_PACKAGE");
    let captures = PathBuf::from(std::env::var_os("PHOTOCRAFT_GENERAL_CAPTURES").expect("set PHOTOCRAFT_GENERAL_CAPTURES"));
    std::fs::create_dir_all(&captures).unwrap();
    let dir = Scratch::new();
    let mut app = package_app(&dir.0);
    inspect(&mut app, json!({"path":package})).unwrap();
    let id = app.ui.extensions.pending.as_ref().unwrap().manifest.id.clone();
    assert_eq!(id, "org.example.image-gain");
    let mut ready = false;
    let mut h = Harness::builder().with_size(egui::vec2(1152.0, 768.0)).wgpu().build_ui_state(
        move |ui, app| {
            if !ready {
                PhotocraftApp::setup_context(ui.ctx(), crate::theme::ThemeKind::ProMedium);
                ready = true;
                return;
            }
            windows(app, ui.ctx());
        },
        app,
    );
    h.run_steps(5);
    for phase in ["review", "installed", "preview"] {
        for theme in crate::theme::ThemeKind::ALL {
            PhotocraftApp::setup_context(&h.ctx, theme);
            for (width, height) in [(1152.0, 768.0), (640.0, 480.0)] {
                h.set_size(egui::vec2(width, height));
                // Each capture represents a fresh window at this viewport. In particular,
                // the preceding narrow capture must not preserve its resized window state.
                h.ctx.memory_mut(|memory| {
                    memory.reset_areas();
                    memory.data.clear();
                });
                // Theme tokens also live in widget data; restore them after the reset.
                PhotocraftApp::setup_context(&h.ctx, theme);
                h.run_steps(6);
                let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height));
                for label in if phase == "preview" { vec!["Apply", "Done"] } else { vec!["Done"] } {
                    assert!(viewport.contains_rect(h.get_by_label(label).rect()), "{phase} {} {width}×{height}: {label} is clipped", theme.id());
                }
                if phase == "preview" && width > 1000.0 {
                    let window = h.ctx.memory(|memory| memory.area_rect(egui::Id::new(("extension-preview", &id)))).unwrap();
                    assert!(window.width() > 900.0 && window.height() > 550.0, "wide preview must use its fresh default size: {window:?}");
                    assert!(viewport.contains_rect(h.get_by_label("Image comparison").rect()), "the wide image comparison must be visible without scrolling");
                }
                h.render().unwrap().save(captures.join(format!("general-plugin-{phase}-{}-{width:.0}x{height:.0}.png", theme.id()))).unwrap();
            }
        }
        h.set_size(egui::vec2(1152.0, 768.0));
        h.ctx.memory_mut(|memory| {
            memory.reset_areas();
            memory.data.clear();
        });
        PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ProMedium);
        h.run_steps(6);
        if phase == "review" {
            h.get_by_label("Install plugin").click();
            h.run_steps(3);
            assert!(h.state().ui.extensions.pending.is_none(), "{}", h.state().ui.extensions.status);
        } else if phase == "installed" {
            h.get_by_label("Done").click();
            h.run_steps(3);
            h.state_mut().run("file.new", json!({"width":96,"height":64,"depth":8,"background":"white"})).unwrap();
            menu(h.state_mut(), &format!("{PREFIX}{id}")).unwrap().unwrap();
            h.state_mut().background_jobs = true;
            h.state_mut().ui.extensions.panels.get_mut(&id).unwrap().values["gain"] = json!(0.5);
            h.run_steps(4);
            start_preview(&mut h, &id);
        }
    }
    h.get_by_label("Done").click();
    h.run_steps(3);
    assert!(h.state().ui.extensions.panels[&id].preview.is_none());
    assert_eq!(h.state().session.documents().len(), 1);
}
