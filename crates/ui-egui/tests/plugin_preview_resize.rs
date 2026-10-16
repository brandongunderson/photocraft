//! Real installed panel, full editor hit testing, and actual edge drags.
#![cfg(not(target_arch = "wasm32"))]

use egui::{Event, Modifiers, PointerButton, Pos2, Rect, Vec2, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use photocraft_ui_egui::{PhotocraftApp, theme::ThemeKind};
use serde_json::json;

const PLUGIN: &str = "org.example.image-gain";

fn portable_package() -> Vec<u8> {
    use photocraft_format::zip::ZipWriter;
    let mut zip = ZipWriter::new();
    let info = json!({"packageVersion":2,"module":"plugin.pcext","filters":[{"module":"filters/channel-gain.wasm"}]});
    zip.add("package.json", &serde_json::to_vec(&info).unwrap()).unwrap();
    zip.add("plugin.pcext", include_bytes!("../../../tests/plugin-compatibility/frozen/image-gain-panel-v1.wasm")).unwrap();
    zip.add("filters/channel-gain.wasm", include_bytes!("../../../tests/plugin-compatibility/frozen/channel-gain-filter-v1.wasm")).unwrap();
    zip.add("LICENSE", include_bytes!("../../../tests/plugin-compatibility/licenses/LICENSE")).unwrap();
    zip.add("THIRD-PARTY-NOTICES.md", include_bytes!("../../../tests/plugin-compatibility/licenses/THIRD-PARTY-NOTICES.md")).unwrap();
    zip.finish().unwrap()
}

struct Scratch(std::path::PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn window(h: &Harness<'_, PhotocraftApp>) -> Rect {
    h.ctx.memory(|memory| memory.area_rect(egui::Id::new(("extension-preview", PLUGIN)))).unwrap()
}

fn drag(h: &mut Harness<'_, PhotocraftApp>, from: Pos2, delta: Vec2) {
    h.hover_at(from);
    h.run_steps(2);
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for step in 1..=6 {
        h.hover_at(from + delta * (step as f32 / 6.0));
        h.run_steps(2);
    }
    h.event(Event::PointerButton { pos: from + delta, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(4);
}

#[test]
fn installed_preview_resizes_in_the_full_editor_and_keeps_actions_visible() {
    let folder = Scratch(std::env::temp_dir().join(format!(
        "photocraft-preview-resize-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    )));
    std::fs::create_dir(&folder.0).unwrap();
    let package = folder.0.join("neutral-preview.pcplugin");
    std::fs::write(&package, portable_package()).unwrap();
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.extensions.attach_folder(folder.0.join("Extensions")).unwrap();
    app.run("extension.install", json!({"path":package})).unwrap();
    app.run("file.new", json!({"width":64,"height":48,"background":"white"})).unwrap();
    app.background_jobs = true;
    app.ui.extensions.panels.entry(PLUGIN.into()).or_default().open = true;
    let source = app.session.active().unwrap().doc.clone();
    let captures = std::env::var_os("PHOTOCRAFT_PLUGIN_PREVIEW_CAPTURES").map(std::path::PathBuf::from);
    let builder = Harness::builder().with_size(vec2(1800.0, 1100.0));
    let builder = if captures.is_some() { builder.wgpu() } else { builder };
    let mut h = builder.build_eframe(|cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, ThemeKind::ProMedium);
        app
    });
    h.run_steps(6);
    let ctx = h.ctx.clone();
    h.state_mut().set_theme(&ctx, ThemeKind::ProMedium);
    h.run_steps(6);
    assert_eq!(h.state().ui.extensions.panels[PLUGIN].values["gain"], 1.25);
    let original = window(&h);
    drag(&mut h, original.center_bottom() - vec2(0.0, 1.0), vec2(0.0, -180.0));
    let shorter = window(&h);
    assert!(shorter.height() < original.height() - 100.0, "wide height: {original:?} -> {shorter:?}");
    h.run_steps(20);
    let initial = window(&h);
    assert!((initial.height() - shorter.height()).abs() < 2.0, "wide height must not snap back: {shorter:?} -> {initial:?}");
    drag(&mut h, initial.right_bottom() - vec2(1.0, 1.0), vec2(-350.0, -100.0));
    let smaller = window(&h);
    assert!(smaller.width() < initial.width() - 250.0, "width: {initial:?} -> {smaller:?}");
    assert!(smaller.height() < initial.height() - 50.0, "height: {initial:?} -> {smaller:?}");
    h.run_steps(15);
    assert!((window(&h).size() - smaller.size()).length() < 2.0, "chosen size must remain stable");
    for label in ["Apply", "Done", "Discard preview"] {
        assert!(smaller.contains_rect(h.get_by_label(label).rect()), "{label} must remain inside {smaller:?}");
    }
    drag(&mut h, smaller.right_bottom() - vec2(1.0, 1.0), vec2(250.0, 120.0));
    let larger = window(&h);
    assert!(larger.width() > smaller.width() + 150.0 && larger.height() > smaller.height() + 60.0, "grow: {smaller:?} -> {larger:?}");
    h.run_steps(20);
    assert!((window(&h).size() - larger.size()).length() < 2.0, "wide layout keeps the enlarged size");
    if let Some(captures) = &captures {
        std::fs::create_dir_all(captures).unwrap();
        h.render().unwrap().save(captures.join("plugin-resized-wide.png")).unwrap();
    }
    // Completed results add labels and a footer hint. They must not
    // reintroduce height growth after the initial settings-only state is fixed.
    h.state_mut().ui.extensions.panels.get_mut(PLUGIN).unwrap().values["backend"] = json!("Sandboxed filter");
    h.state_mut().ui.extensions.panels.get_mut(PLUGIN).unwrap().values["gain"] = json!(0.5);
    h.run_steps(3);
    // Resizing can put this action below the settings scroll area; querying it
    // still succeeds, but a pointer click needs the control to be visible.
    h.get_by_label("Generate preview").scroll_to_me();
    h.run_steps(3);
    let generate = h.get_by_label("Generate preview").rect();
    assert!(window(&h).contains_rect(generate), "Generate preview must be visible before clicking");
    assert!(generate.bottom() < h.get_by_label("Apply").rect().top(), "Generate preview must be above the footer");
    h.get_by_label("Generate preview").click();
    h.run_steps(3);
    // The small neutral filter may have completed during those frames.
    if let Some(job) = h.state().ui.extensions.panels[PLUGIN].job {
        h.state_mut().session.wait_job(photocraft_engine::jobs::JobId(job)).unwrap();
    }
    h.run_steps(20);
    assert!(h.state().ui.extensions.panels[PLUGIN].preview.is_some(), "{}", h.state().ui.extensions.panels[PLUGIN].message);
    let completed = window(&h);
    assert!((completed.height() - larger.height()).abs() < 2.0, "completed preview keeps the chosen height: {larger:?} -> {completed:?}");
    drag(&mut h, completed.center_bottom() - vec2(0.0, 1.0), vec2(0.0, -40.0));
    let resized_result = window(&h);
    assert!(resized_result.height() < completed.height() - 20.0, "completed preview can shrink: {completed:?} -> {resized_result:?}");
    h.run_steps(20);
    assert!((window(&h).height() - resized_result.height()).abs() < 2.0, "completed preview height stays reduced");
    h.set_size(vec2(640.0, 480.0));
    h.run_steps(10);
    let viewport = Rect::from_min_size(Pos2::ZERO, vec2(640.0, 480.0));
    for label in ["Apply", "Done", "Discard preview"] {
        assert!(viewport.contains_rect(h.get_by_label(label).rect()), "compact {label} must remain visible");
    }
    assert!(std::sync::Arc::ptr_eq(&source, &h.state().session.active().unwrap().doc));
    assert_eq!(h.state().session.documents().len(), 1);
    if let Some(captures) = captures {
        for theme in ThemeKind::ALL {
            let ctx = h.ctx.clone();
            h.state_mut().set_theme(&ctx, theme);
            for (width, height) in [(1152.0, 768.0), (640.0, 480.0)] {
                h.set_size(vec2(width, height));
                h.run_steps(10);
                let current = window(&h);
                let target = if width > 1000.0 { vec2(940.0, 560.0) } else { vec2(560.0, 400.0) };
                drag(&mut h, current.right_bottom() - vec2(1.0, 1.0), target - current.size());
                let chosen = window(&h);
                h.run_steps(20);
                assert!((window(&h).size() - chosen.size()).length() < 2.0, "{} {width}x{height} must not grow", theme.id());
                let viewport = Rect::from_min_size(Pos2::ZERO, vec2(width, height));
                for label in ["Apply", "Done", "Discard preview"] {
                    assert!(viewport.contains_rect(h.get_by_label(label).rect()), "{} {width}x{height}: {label} is clipped", theme.id());
                }
                h.render().unwrap().save(captures.join(format!("plugin-resize-{}-{width:.0}x{height:.0}.png", theme.id()))).unwrap();
            }
        }
    }
}
