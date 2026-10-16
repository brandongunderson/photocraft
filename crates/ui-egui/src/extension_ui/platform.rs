//! The native rendering boundary for generic API-2 command and inspector contributions.
use serde_json::{Value, json};

use super::{Manifest, MenuItem, PhotocraftApp};

pub(super) fn insert_commands(app: &PhotocraftApp, items: &mut Vec<MenuItem>) {
    let mut at = items.iter().position(|item| item.id == super::MANAGER).unwrap_or(items.len());
    for command in app.session.plugin_command_entries() {
        let Some(id) = command.get("id").and_then(Value::as_str) else { continue };
        let Some(label) = command.get("label").and_then(Value::as_str) else { continue };
        let path = command
            .get("menu")
            .and_then(Value::as_array)
            .map(|path| path.iter().filter_map(Value::as_str).map(str::to_owned).collect::<Vec<_>>())
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| vec!["Plugins".into()]);
        let shortcut = app.session.prefs().shortcut(id, command.get("shortcut").and_then(Value::as_str)).map(str::to_owned);
        items.insert(
            at,
            MenuItem { id: id.into(), label: label.into(), path, shortcut, enabled: crate::menus::is_enabled(app, id), checked: None, color: None },
        );
        at += 1;
    }
}

pub(super) fn set_view_open(app: &mut PhotocraftApp, id: &str, open: bool) -> Result<(), String> {
    let command = if open { "extension.view.open" } else { "extension.view.close" };
    let result = app.run(command, json!({"id": id}));
    if let Some(panel) = app.ui.extensions.panels.get_mut(id) {
        panel.view_active = open && result.is_ok();
        if let Err(error) = &result {
            panel.message = error.clone();
            panel.error = true;
        }
    }
    result.map(|_| ())
}

pub(super) fn inspector(ui: &mut egui::Ui, app: &PhotocraftApp, id: &str) {
    if let Some(error) = app.session.plugin_view_error(id) {
        ui.label(format!("Inspector unavailable: {error}"));
        ui.weak("Close and reopen this panel to try again.");
    } else if let Some(view) = app.session.plugin_inspector_view(id) {
        if !view.title.is_empty() {
            ui.strong(&view.title);
            ui.add_space(4.0);
        }
        // Use the existing native typography and wrapping behavior. Each label/value pair
        // occupies its own vertical group so long document names fit a compact panel.
        for (index, row) in view.rows.iter().enumerate() {
            ui.push_id(index, |ui| row_value(ui, &row.label, &row.value));
        }
        if view.rows.is_empty() {
            ui.weak("No information to show.");
        }
    } else {
        ui.weak("Loading document information…");
    }
}

fn row_value(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.weak(label);
    ui.add(egui::Label::new(value).wrap());
    ui.add_space(5.0);
}

pub(super) fn capabilities(ui: &mut egui::Ui, manifest: &Manifest) {
    for capability in &manifest.capabilities.required {
        ui.label(format!("• {} (version {} or later)", capability.id, capability.min_version));
    }
    for capability in &manifest.capabilities.optional {
        ui.label(format!("• {} (optional, version {} or later)", capability.id, capability.min_version));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install_manifest(app: &mut PhotocraftApp, manifest: &str, response: Value) {
        use base64::Engine;
        let quote = |text: &str| text.bytes().map(|byte| format!("\\{byte:02x}")).collect::<String>();
        let response = response.to_string();
        let module = wat::parse_str(format!(
            r#"(module (memory (export "memory") 4)
                (data (i32.const 32) "{}") (data (i32.const 65536) "{}")
                (func (export "pce_api_version") (result i32) i32.const 2)
                (func (export "pce_manifest") (result i64) i64.const {})
                (func (export "pce_alloc") (param i32) (result i32) i32.const 196608)
                (func (export "pce_handle") (param i32 i32) (result i64) i64.const {}))"#,
            quote(manifest),
            quote(&response),
            ((manifest.len() as u64) << 32) | 32,
            ((response.len() as u64) << 32) | 65536,
        ))
        .unwrap();
        app.run("extension.install", json!({"data": base64::engine::general_purpose::STANDARD.encode(module)})).unwrap();
    }

    fn app() -> PhotocraftApp {
        PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default())
    }

    #[test]
    fn contributed_commands_join_native_menus_palette_and_assigned_shortcuts() {
        let mut app = app();
        let plugin_id = "org.example.layer-organizer";
        let command_id = "org.example.layer-organizer.organize";
        install_manifest(&mut app, include_str!("../../../../examples/extensions/layer-organizer/manifest.json"), json!({}));
        let items = crate::menus::menu_items(&app);
        let contributed = items.iter().find(|item| item.id == command_id).unwrap();
        assert_eq!(contributed.path, ["Plugins", "Layer Organizer"]);
        assert!(!contributed.enabled, "requires a document");
        let manager = items.iter().position(|item| item.id == super::super::MANAGER).unwrap();
        assert!(items.iter().position(|item| item.id == command_id).unwrap() < manager);
        app.run("file.new", json!({"width":2,"height":2})).unwrap();
        app.session.prefs.edit(|prefs| prefs.shortcuts.insert(command_id.into(), "Ctrl+Shift+F9".into()));
        assert!(crate::menus::menu_items(&app).iter().find(|item| item.id == command_id).unwrap().enabled);
        assert!(crate::shortcut_dispatch::bindings(&app).iter().any(|(id, _)| id == command_id));
        app.run("extension.enable", json!({"id":plugin_id,"enabled":false})).unwrap();
        assert!(!crate::menus::menu_items(&app).iter().find(|item| item.id == command_id).unwrap().enabled);
        assert!(app.session.has_command(command_id));
        app.run("extension.remove", json!({"id":plugin_id})).unwrap();
        assert!(!crate::menus::menu_items(&app).iter().any(|item| item.id == command_id));
        assert!(!app.session.has_command(command_id));
    }

    #[test]
    fn opening_and_closing_a_native_inspector_activate_and_revoke_its_view() {
        let mut app = app();
        let plugin_id = "org.example.document-inspector";
        let command_id = "window.extension.org.example.document-inspector";
        install_manifest(
            &mut app,
            include_str!("../../../../examples/extensions/document-inspector/manifest.json"),
            json!({"view":{"title":"Document Inspector","rows":[{"label":"Document","value":"Example"}]}}),
        );
        super::super::menu(&mut app, command_id).unwrap().unwrap();
        assert!(app.ui.extensions.panels[plugin_id].view_active);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while app.session.plugin_events_pending() && std::time::Instant::now() < deadline {
            app.session.pump_plugin_events();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(app.session.plugin_inspector_view(plugin_id).unwrap().rows[0].value, "Example");
        super::super::menu(&mut app, command_id).unwrap().unwrap();
        assert!(!app.ui.extensions.panels[plugin_id].view_active);
        assert!(!app.session.plugin_events_pending());
    }

    #[test]
    fn contributed_commands_are_programmatic_and_match_shortcut_imports() {
        let mut app = app();
        let command_id = "org.example.layer-organizer.organize";
        install_manifest(&mut app, include_str!("../../../../examples/extensions/layer-organizer/manifest.json"), json!({"message":"Contributed command ran"}));
        app.run("file.new", json!({"width":2,"height":2})).unwrap();
        let (request, _receiver) = crate::control::ControlRequest::new("engine.execute", json!({"command":command_id,"params":{}}));
        let crate::control::Outcome::Done(response) = crate::control::handle(&mut app, &egui::Context::default(), &request) else {
            panic!("contributed commands must execute directly");
        };
        assert!(response.to_string().contains("Contributed command ran"), "{response}");
        assert!(app.ui.dialogs.is_empty());
        let set = crate::kys_import::KysSet {
            commands: vec![crate::kys_import::KysCommand {
                name: "Organize layer names".into(),
                shortcuts: vec!["Ctrl+Shift+F9".into()],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(crate::kys_import::plan(&app, &set).set.contains_key(command_id));
    }

    #[test]
    fn inspector_values_wrap_inside_a_compact_panel() {
        let ctx = egui::Context::default();
        let mut width = 0.0_f32;
        let mut output = ctx.run_ui(
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0))), ..Default::default() },
            |ui| {
                ui.allocate_ui_with_layout(egui::vec2(220.0, 100.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    row_value(ui, "Active layer", &"A long document layer name ".repeat(16));
                    width = ui.min_rect().width();
                });
            },
        );
        output.textures_delta.clear();
        assert!(width <= 221.0, "inspector values widened their native panel to {width}");
    }
}
