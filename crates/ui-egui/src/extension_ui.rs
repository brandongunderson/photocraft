//! Host-rendered extension panels and the installer. Extension authors provide data and WASM,
//! never toolkit objects. Button panels stay passive; contributed inspectors activate on opening.
use std::collections::BTreeMap;

use base64::Engine;
use photocraft_plugins::{
    ParamSpec,
    extension::{MAX_MODULE_BYTES, Manifest},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{FileDialogAnswer, FileDialogRequest, PhotocraftApp, menus::MenuItem};

const PREFIX: &str = "window.extension.";
pub const MANAGER: &str = "window.extensions";

mod installer;
mod platform;
mod preview;
mod storage;
mod updates;

pub(crate) use updates::{automatic_job, automatic_job_finished, tick_updates};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtensionUi {
    pub manager_open: bool,
    pub panels: BTreeMap<String, PanelState>,
    #[serde(skip)]
    pending: Option<Pending>,
    #[serde(skip)]
    status: String,
    #[serde(skip)]
    path: String,
    #[serde(skip)]
    manager_job: Option<ManagerJob>,
    #[serde(skip)]
    updates: BTreeMap<String, Value>,
    #[serde(skip)]
    update_runtime: updates::Runtime,
    #[serde(skip)]
    filter_review: Option<String>,
    #[serde(skip)]
    pub(crate) drop_rect: Option<egui::Rect>,
}

#[derive(Clone, Debug, PartialEq)]
struct Pending {
    input: Value,
    manifest: Manifest,
    native: bool,
    storage: photocraft_engine::extension_cmds::StorageSize,
    download_bytes: u64,
    action: InstallAction,
    installed_version: Option<String>,
    reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum InstallAction {
    Install,
    Update,
    Reinstall,
    Repair,
    Downgrade,
    Replace,
}

impl InstallAction {
    fn intent(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::Reinstall => "reinstall",
            Self::Repair => "repair",
            Self::Downgrade => "downgrade",
            Self::Replace => "replace",
        }
    }

    fn primary_label(self) -> Option<&'static str> {
        match self {
            Self::Install => Some("Install plugin"),
            Self::Update => Some("Update plugin"),
            Self::Repair => Some("Repair plugin"),
            _ => None,
        }
    }

    fn completed(self) -> &'static str {
        match self {
            Self::Install => "Installed",
            Self::Update => "Updated",
            Self::Reinstall => "Reinstalled",
            Self::Repair => "Repaired",
            Self::Downgrade => "Installed older version of",
            Self::Replace => "Replaced",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum ManagerWork {
    Inspect(Value),
    Install(InstallAction),
    Check,
    CheckAndPrepare(String),
    Prepare,
}

#[derive(Clone, Debug, PartialEq)]
struct ManagerJob {
    id: u64,
    work: ManagerWork,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PanelState {
    pub open: bool,
    pub values: Value,
    pub message: String,
    pub error: bool,
    #[serde(skip)]
    view_active: bool,
    #[serde(skip)]
    pub job: Option<u64>,
    #[serde(skip)]
    pub preview: Option<u64>,
    #[serde(skip)]
    preview_values: Value,
    #[serde(skip)]
    preview_view: preview::View,
}

pub fn insert_menu_items(app: &PhotocraftApp, items: &mut Vec<MenuItem>) {
    let Some(manager) = items.iter().position(|i| i.id == MANAGER) else { return };
    // The manager remains the final entry after dynamically inserted filter plugins too.
    let mut manager = items.remove(manager);
    let available = updates::available_count(app);
    if available > 0 {
        manager.label = format!("Manage Plugins… ({available} {})", if available == 1 { "update" } else { "updates" });
    }
    let at = items.iter().rposition(|i| i.path.first().is_some_and(|top| top == "Plugins")).map_or(items.len(), |at| at + 1);
    items.insert(at, manager);
    for (offset, (id, extension)) in app.session.extensions.installed.iter().enumerate() {
        let open = app.ui.extensions.panels.get(id).is_some_and(|p| p.open);
        let manifest = &extension.extension.manifest;
        let command_group = !manifest.contributes.commands.is_empty();
        items.insert(
            at + offset,
            MenuItem {
                id: format!("{PREFIX}{id}"),
                label: if command_group { "Open panel".into() } else { manifest.name.clone() },
                path: if command_group { vec!["Plugins".into(), manifest.name.clone()] } else { vec!["Plugins".into()] },
                shortcut: None,
                enabled: extension.enabled,
                checked: Some(open),
                color: None,
            },
        );
    }
    platform::insert_commands(app, items);
}

pub fn is_enabled(app: &PhotocraftApp, id: &str) -> Option<bool> {
    if id == MANAGER {
        return Some(true);
    }
    id.strip_prefix(PREFIX).map(|id| app.session.extensions.installed.get(id).is_some_and(|p| p.enabled))
}

pub fn menu(app: &mut PhotocraftApp, id: &str) -> Option<Result<Value, String>> {
    if id == MANAGER {
        app.ui.extensions.manager_open = true;
        return Some(Ok(json!({"open": true})));
    }
    let id = id.strip_prefix(PREFIX)?;
    let Some(installed) = app.session.extensions.installed.get(id).filter(|e| e.enabled) else {
        return Some(Err("extension is unavailable or disabled".into()));
    };
    let has_preview = installed.extension.manifest.panel.preview;
    let inspector = !installed.extension.manifest.contributes.inspectors.is_empty();
    let panel = app.ui.extensions.panels.entry(id.into()).or_default();
    panel.open = !panel.open;
    // Revalidate saved values when an extension has been updated.
    panel.values =
        Value::Object(installed.extension.manifest.values(&panel.values).or_else(|_| installed.extension.manifest.values(&Value::Null)).unwrap_or_default());
    let open = panel.open;
    if inspector && let Err(error) = platform::set_view_open(app, id, open) {
        return Some(Err(error));
    }
    if !open && has_preview {
        let mut panel = app.ui.extensions.panels.remove(id).unwrap_or_default();
        preview::discard(app, &mut panel);
        app.ui.extensions.panels.insert(id.into(), panel);
    }
    Some(Ok(json!({"open": open})))
}

fn inspect(app: &mut PhotocraftApp, mut input: Value) -> Result<Value, String> {
    if app.ui.extensions.manager_job.is_some() {
        return Err("Wait for the current plugin operation to finish, or cancel it.".into());
    }
    app.ui.extensions.manager_open = true;
    app.ui.extensions.pending = None;
    app.ui.extensions.filter_review = None;
    app.ui.extensions.status.clear();
    let reviewed = inspect_inner(app, &mut input);
    if let Err(error) = &reviewed {
        app.ui.extensions.status = error.clone();
    }
    reviewed
}

fn inspect_inner(app: &mut PhotocraftApp, input: &mut Value) -> Result<Value, String> {
    let result = app.run("extension.inspect", input.clone())?;
    if let Some(id) = result.get("job").and_then(Value::as_u64) {
        app.ui.extensions.manager_job = Some(ManagerJob { id, work: ManagerWork::Inspect(input.clone()) });
        app.ui.extensions.status = "Reading plugin permissions…".into();
        return Ok(result);
    }
    accept_review(app, input, &result)
}

fn accept_review(app: &mut PhotocraftApp, input: &mut Value, result: &Value) -> Result<Value, String> {
    let manifest: Manifest = serde_json::from_value(result.get("manifest").cloned().unwrap_or(Value::Null)).map_err(|e| e.to_string())?;
    input["expectedHash"] = result.get("hash").cloned().unwrap_or(Value::Null);
    let installation = result.get("installation").ok_or("The installation action is unavailable. Select the plugin file again.")?;
    let action: InstallAction = serde_json::from_value(installation.get("action").cloned().unwrap_or(Value::Null)).map_err(|e| e.to_string())?;
    for guard in ["expectedStoreHash", "expectedInstalledHash", "expectedActivation", "expectedSessionRevision"] {
        if let Some(value) = installation.get(guard) {
            input[guard] = value.clone();
        }
    }
    input["intent"] = json!(action.intent());
    app.ui.extensions.manager_open = true;
    let native = result.get("package").and_then(|package| package.get("native")).is_some_and(Value::is_object);
    let storage = serde_json::from_value(result.get("storage").cloned().unwrap_or(Value::Null)).map_err(|e| e.to_string())?;
    let download_bytes = result.get("downloadBytes").and_then(Value::as_u64).ok_or("Plugin size is unavailable. Select the plugin file again.")?;
    app.ui.extensions.pending = Some(Pending {
        input: input.clone(),
        manifest,
        native,
        storage,
        download_bytes,
        action,
        installed_version: installation.get("installedVersion").and_then(Value::as_str).map(str::to_owned),
        reason: installation.get("reason").and_then(Value::as_str).map(str::to_owned),
    });
    app.ui.extensions.status.clear();
    Ok(json!({"review": true}))
}

fn start_manager_work(app: &mut PhotocraftApp, command: &str, params: Value, work: ManagerWork) {
    if app.ui.extensions.manager_job.is_some() {
        return;
    }
    updates::cancel_automatic(app);
    if command == "extension.updates.check" && params.get("id").is_none() {
        updates::remember_attempt(app);
    }
    let result = app.run(command, params);
    if let Ok(value) = &result
        && let Some(id) = value.get("job").and_then(Value::as_u64)
    {
        app.ui.extensions.manager_job = Some(ManagerJob { id, work });
        app.ui.extensions.status = match command {
            "extension.updates.check" => "Checking for updates…",
            "extension.updates.prepare" => "Downloading update for permission review…",
            _ => "Installing plugin…",
        }
        .into();
        return;
    }
    if let Err(error) = result.and_then(|value| finish_manager_work(app, work, &value)) {
        app.ui.extensions.status = error;
    }
}

fn finish_manager_work(app: &mut PhotocraftApp, work: ManagerWork, result: &Value) -> Result<(), String> {
    match work {
        ManagerWork::Inspect(mut input) => {
            accept_review(app, &mut input, result)?;
        }
        ManagerWork::Prepare => {
            let mut input = result.get("input").filter(|input| input.is_object()).cloned().ok_or("The downloaded update could not be reviewed. Try again.")?;
            accept_review(app, &mut input, result)?;
            app.ui.extensions.path = input.get("path").and_then(Value::as_str).unwrap_or_default().into();
        }
        ManagerWork::Check => {
            updates::accept_results(app, result)?;
            app.ui.extensions.status = "Update check complete.".into();
        }
        ManagerWork::CheckAndPrepare(id) => {
            finish_manager_work(app, ManagerWork::Check, result)?;
            let candidate = app
                .ui
                .extensions
                .updates
                .get(&id)
                .filter(|update| update.get("status").and_then(Value::as_str) == Some("available"))
                .and_then(|update| update.get("candidate"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            if let Some(candidate) = candidate {
                start_manager_work(app, "extension.updates.prepare", json!({"id":id,"candidate":candidate}), ManagerWork::Prepare);
            }
        }
        ManagerWork::Install(action) => {
            let manifest = result
                .get("manifest")
                .filter(|manifest| manifest.is_object())
                .ok_or("The plugin installation returned no result. Check the installed list.")?;
            let name = manifest.get("name").and_then(Value::as_str).unwrap_or("Plugin");
            let version = manifest.get("version").and_then(Value::as_str).unwrap_or_default();
            if let Some(id) = manifest.get("id").and_then(Value::as_str) {
                app.ui.extensions.updates.remove(id);
            }
            app.ui.extensions.status = format!("{} {name} {version}.", action.completed());
            app.ui.extensions.pending = None;
            app.ui.extensions.path.clear();
            app.menu_cache = None;
        }
    }
    Ok(())
}

fn poll_manager_job(app: &mut PhotocraftApp) {
    let Some(current) = app.ui.extensions.manager_job.clone() else { return };
    let job = app.session.jobs_with_recent().into_iter().find(|job| job.id.0 == current.id);
    let Some(job) = job else {
        app.ui.extensions.manager_job = None;
        app.ui.extensions.status = "The plugin operation is no longer available. Try again.".into();
        return;
    };
    if job.state == "running" {
        return;
    }
    app.ui.extensions.manager_job = None;
    if job.state != "done" {
        app.ui.extensions.status = if job.state == "cancelled" { "Plugin operation cancelled.".into() } else { job.error.unwrap_or(job.message) };
        return;
    }
    let open = app.ui.extensions.manager_open;
    let result =
        job.result.ok_or_else(|| "The plugin operation returned no result. Try again.".into()).and_then(|value| finish_manager_work(app, current.work, &value));
    app.ui.extensions.manager_open = open;
    if let Err(error) = result {
        app.ui.extensions.status = error;
    }
}

fn choose(app: &mut PhotocraftApp) -> Result<Value, String> {
    if app.services.file_dialog.is_none() {
        return Err("The file browser is unavailable. Enter a file path or drop a plugin file.".into());
    }
    let extensions = if cfg!(target_arch = "wasm32") { vec!["pcext".into(), "wasm".into()] } else { vec!["pcplugin".into(), "pcext".into(), "wasm".into()] };
    app.ask_file(FileDialogRequest::Open { multiple: false, initial_dir: None, extensions: Some(extensions) }, |app, answer| {
        installer::review_answer(app, answer)
    })
}

pub(crate) fn review_drop(app: &mut PhotocraftApp, files: &[egui::DroppedFileHandle]) -> Result<(), String> {
    installer::review_drop(app, files)
}

/// Browser drops arrive as a batch of already-read bytes through the platform inbox.
pub(crate) fn review_arrived(app: &mut PhotocraftApp, files: &[(String, Vec<u8>)]) -> bool {
    installer::review_arrived(app, files)
}

fn field(ui: &mut egui::Ui, field: &photocraft_plugins::extension::Field, values: &mut Value) {
    match &field.spec {
        ParamSpec::Number { min, max, default } => {
            let mut value = values.get(&field.id).and_then(Value::as_f64).unwrap_or(*default);
            if ui.add(egui::Slider::new(&mut value, *min..=*max).text(&field.label)).changed() {
                values[&field.id] = json!(value);
            }
        }
        ParamSpec::Int { min, max, default } => {
            let mut value = values.get(&field.id).and_then(Value::as_i64).unwrap_or(*default);
            if ui.add(egui::Slider::new(&mut value, *min..=*max).text(&field.label)).changed() {
                values[&field.id] = json!(value);
            }
        }
        ParamSpec::Bool { default } => {
            let mut value = values.get(&field.id).and_then(Value::as_bool).unwrap_or(*default);
            if ui.checkbox(&mut value, &field.label).changed() {
                values[&field.id] = json!(value);
            }
        }
        ParamSpec::Choice { options, default } => {
            let mut value = values
                .get(&field.id)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| default.clone())
                .or_else(|| options.first().cloned())
                .unwrap_or_default();
            ui.label(&field.label);
            egui::ComboBox::from_id_salt(&field.id)
                .width(ui.available_width().max(40.0))
                .selected_text(&value)
                .truncate()
                .show_ui(ui, |ui| {
                    ui.set_max_width((ui.ctx().content_rect().width() - 32.0).clamp(40.0, 480.0));
                    for option in options {
                        if ui.add(egui::Button::selectable(value == *option, option).wrap()).on_hover_text(option).clicked() {
                            value = option.clone();
                        }
                    }
                })
                .response
                .on_hover_text(&value);
            values[&field.id] = json!(value);
        }
    }
}

pub fn windows(app: &mut PhotocraftApp, ctx: &egui::Context) {
    poll_manager_job(app);
    if app.session.pump_plugin_events() > 0 {
        ctx.request_repaint();
    }
    if app.session.plugin_events_pending() {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
    if app.ui.extensions.manager_open {
        manager(app, ctx);
    } else {
        app.ui.extensions.drop_rect = None;
    }
    // Retain settings for disabled installations, but release removed panel state/textures.
    // Recovery startup deliberately skips loading installations and must preserve their settings.
    if !app.session.plugins_disabled {
        app.ui.extensions.updates.retain(|id, _| app.session.extensions.installed.contains_key(id));
        app.ui.extensions.panels.retain(|id, _| app.session.extensions.installed.contains_key(id));
        for (id, panel) in &mut app.ui.extensions.panels {
            if app.session.extensions.installed.get(id).is_some_and(|installed| !installed.enabled) {
                panel.preview_view.clear_textures();
                panel.view_active = false;
            }
        }
    }
    let manifests = app.session.extensions.installed.values().filter(|p| p.enabled).map(|p| p.extension.manifest.clone()).collect::<Vec<_>>();
    // Session::job exposes running work only. Finished jobs remain in the recent history so
    // a panel can consume success, failure, or cancellation after the engine has applied it.
    let jobs = if app.ui.extensions.panels.values().any(|panel| panel.open && panel.job.is_some()) { app.session.jobs_with_recent() } else { Vec::new() };
    for manifest in manifests {
        let Some(mut panel) = app.ui.extensions.panels.get(&manifest.id).filter(|p| p.open).cloned() else { continue };
        let inspector = !manifest.contributes.inspectors.is_empty();
        if inspector && (!panel.view_active || !app.session.plugin_view_is_open(&manifest.id)) {
            match app.run("extension.view.open", json!({"id": manifest.id})) {
                Ok(_) => panel.view_active = true,
                Err(error) => {
                    panel.error = true;
                    panel.message = error;
                    panel.open = false;
                }
            }
        }
        panel.values = Value::Object(manifest.values(&panel.values).or_else(|_| manifest.values(&Value::Null)).unwrap_or_default());
        let mut event = None;
        let mut done = false;
        let mut cancel = None;
        let job = panel.job.and_then(|id| jobs.iter().find(|job| job.id.0 == id));
        if let Some(job) = job
            && job.state != "running"
        {
            panel.error = job.state == "failed";
            panel.message = job
                .error
                .clone()
                .or_else(|| job.result.as_ref().and_then(|r| r.get("message")).and_then(Value::as_str).map(str::to_owned))
                .unwrap_or_else(|| if job.state == "cancelled" { "Cancelled. The original is unchanged.".into() } else { "Completed.".into() });
            if let Some(preview) = job.result.as_ref().and_then(|result| result.get("preview")).and_then(Value::as_u64) {
                panel.preview = Some(preview);
            }
            panel.job = None;
        } else if panel.job.is_some() && job.is_none() {
            // A panel can remain closed while enough other jobs finish to evict its result.
            // Release the stale reference without inventing a successful or failed outcome.
            panel.error = false;
            panel.message = "Processing status is no longer available. Check the open images for the result.".into();
            panel.job = None;
        }
        if manifest.panel.preview {
            // A control-channel command may finish processing before the panel is opened.
            if panel.preview.is_none()
                && panel.job.is_none()
                && let Some(preview) = app.session.plugin_preview_for(&manifest.id)
            {
                panel.preview = Some(preview.id);
                panel.preview_values = panel.values.clone();
                panel.message = "Preview ready. Compare the image, then Apply or Discard.".into();
            }
            preview::validate(app, &mut panel);
            let action = preview::window(app, ctx, &manifest, &mut panel, job.filter(|job| job.state == "running"));
            event = action.event;
            done = action.done;
            cancel = action.cancel;
            if let Some(target) = action.apply {
                preview::apply(app, &mut panel, target);
            }
            if action.discard {
                preview::discard(app, &mut panel);
                panel.error = false;
                panel.message = "Preview discarded. Generate a new preview to compare another result.".into();
            }
        } else {
            egui::Window::new(&manifest.name)
                .id(egui::Id::new(("extension", &manifest.id)))
                .open(&mut panel.open)
                .default_width(330.0)
                .default_height(500.0)
                .min_height(180.0)
                .default_pos(egui::pos2(120.0, 105.0))
                .vscroll(false)
                .show(ctx, |ui| {
                    let scroll_height = (ui.available_height() - 36.0).clamp(100.0, 440.0);
                    egui::ScrollArea::vertical().max_height(scroll_height).show(ui, |ui| {
                        ui.label(&manifest.description);
                        ui.separator();
                        if let Some(doc) = app.session.active() {
                            ui.weak(format!("{} · {} × {}", doc.doc.name, doc.doc.size.width, doc.doc.size.height));
                        }
                        for f in &manifest.panel.fields {
                            ui.push_id(&f.id, |ui| field(ui, f, &mut panel.values));
                        }
                        if inspector {
                            platform::inspector(ui, app, &manifest.id);
                        }
                        if let Some(job) = job.filter(|j| j.state == "running") {
                            ui.add(egui::ProgressBar::new(job.progress).show_percentage().text(&job.message));
                            if ui.button("Cancel processing").clicked() {
                                cancel = Some(job.id.0);
                            }
                        }
                        ui.add_space(8.0);
                        for button in &manifest.panel.buttons {
                            if ui
                                .add_enabled(
                                    panel.job.is_none() && app.session.active_job().is_none() && crate::menus::modal_allows(app, "extension.invoke"),
                                    egui::Button::new(&button.label),
                                )
                                .clicked()
                            {
                                event = Some(button.id.clone());
                            }
                        }
                        if !panel.message.is_empty() {
                            ui.separator();
                            let text = if panel.error { format!("Action failed: {}", panel.message) } else { panel.message.clone() };
                            ui.label(text);
                        }
                    });
                    ui.separator();
                    if ui.button("Done").clicked() {
                        done = true;
                    }
                });
        }
        if done {
            panel.open = false;
        }
        if inspector && !panel.open {
            let _ = app.run("extension.view.close", json!({"id": manifest.id}));
            panel.view_active = false;
        }
        if let Some(id) = cancel {
            app.session.cancel_job(photocraft_engine::jobs::JobId(id));
        }
        if let Some(event) = event {
            match app.run("extension.invoke", json!({"id": manifest.id, "event": event, "values": panel.values})) {
                Ok(result) => {
                    panel.error = false;
                    panel.job = result.get("job").and_then(Value::as_u64);
                    if manifest.panel.preview {
                        // Image jobs revoke their old comparison in the engine. Maintenance
                        // tasks retain it, so clicking a task must never discard it here.
                        panel.preview =
                            result.get("preview").and_then(Value::as_u64).or_else(|| app.session.plugin_preview_for(&manifest.id).map(|preview| preview.id));
                        if panel.preview.is_none() || result.get("preview").is_some() {
                            panel.preview_values = panel.values.clone();
                            panel.preview_view.clear_textures();
                        }
                    }
                    panel.message =
                        result.get("message").and_then(Value::as_str).unwrap_or(if panel.job.is_some() { "Processing…" } else { "Completed." }).into();
                }
                Err(error) => {
                    panel.error = true;
                    panel.message = error;
                }
            }
        }
        if manifest.panel.preview {
            preview::validate(app, &mut panel);
            if !panel.open {
                preview::discard(app, &mut panel);
            }
        }
        app.ui.extensions.panels.insert(manifest.id, panel);
    }
}

fn source_link(ui: &mut egui::Ui, manifest: &Manifest) {
    if let Some(repository) = &manifest.repository {
        ui.hyperlink_to("Source repository", repository);
    }
}

/// The standalone-filter dialog calls this only after its install command succeeds.
pub(crate) fn filter_installed(app: &mut PhotocraftApp, result: &Value) {
    if result.get("job").is_some()
        || app.ui.extensions.filter_review.as_deref() != result.get("id").and_then(Value::as_str)
        || app.ui.extensions.filter_review.is_none()
    {
        return;
    }
    let name = result.get("name").and_then(Value::as_str).unwrap_or("Filter");
    let version = result.get("version").and_then(Value::as_str).unwrap_or_default();
    app.ui.extensions.status = format!("Installed {name} {version}.");
    app.ui.extensions.path.clear();
    app.ui.extensions.filter_review = None;
    app.menu_cache = None;
}

fn manager(app: &mut PhotocraftApp, ctx: &egui::Context) {
    let mut open = true;
    let mut browse = false;
    let mut inspect_path = false;
    let mut install = None;
    let mut check_updates = None;
    let mut prepare_update = None;
    let mut cancel_job = false;
    let mut change = None;
    let mut reveal = None;
    let mut done = false;
    let busy = app.ui.extensions.manager_job.is_some();
    #[cfg(not(target_arch = "wasm32"))]
    let unavailable = app.session.extensions.unavailable.values().cloned().collect::<Vec<_>>();
    #[cfg(not(target_arch = "wasm32"))]
    let has_unavailable = !unavailable.is_empty();
    #[cfg(target_arch = "wasm32")]
    let has_unavailable = false;
    let extensions = app
        .session
        .extensions
        .installed
        .iter()
        .map(|(id, p)| {
            #[cfg(not(target_arch = "wasm32"))]
            let native = p.native.is_some();
            #[cfg(target_arch = "wasm32")]
            let native = false;
            (id.clone(), p.extension.manifest.clone(), p.enabled, native, p.storage)
        })
        .collect::<Vec<_>>();
    let manager_window = egui::Window::new("Manage Plugins")
        .id(egui::Id::new("extension-manager"))
        .open(&mut open)
        .default_width(480.0)
        .min_width(280.0)
        .default_height(560.0)
        .min_height(180.0)
        .default_pos(egui::pos2(210.0, 115.0))
        .vscroll(false)
        .show(ctx, |ui| {
            if !app.ui.extensions.status.is_empty() {
                ui.label(&app.ui.extensions.status);
                ui.separator();
            }
            let scroll_height = (ui.available_height() - 36.0).clamp(100.0, 520.0);
            egui::ScrollArea::vertical().max_height(scroll_height).show(ui, |ui| {
                ui.label("Choose a plugin file or drag it into this window.");
                let path_label = ui.label("Plugin file");
                ui.horizontal(|ui| {
                    let font = egui::TextStyle::Button.resolve(ui.style());
                    let browse_width = ui.fonts_mut(|fonts| fonts.layout_no_wrap("Browse…".into(), font, ui.visuals().text_color()).size().x)
                        + 2.0 * ui.spacing().button_padding.x
                        + 4.0;
                    let path_width = (ui.available_width() - browse_width - ui.spacing().item_spacing.x).max(40.0);
                    let path = ui
                        .add_enabled_ui(!busy, |ui| {
                            ui.add_sized(
                                egui::vec2(path_width, ui.spacing().interact_size.y),
                                egui::TextEdit::singleline(&mut app.ui.extensions.path).hint_text("Plugin file path").desired_width(f32::INFINITY),
                            )
                        })
                        .inner
                        .labelled_by(path_label.id);
                    if path.changed() {
                        app.ui.extensions.pending = None;
                        app.ui.extensions.filter_review = None;
                        app.ui.extensions.status.clear();
                    }
                    if path.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                        inspect_path = !app.ui.extensions.path.trim().is_empty();
                    }
                    if ui.add_enabled(!busy && !app.file_dialog_open(), egui::Button::new("Browse…")).clicked() {
                        browse = true;
                    }
                });
                ui.horizontal(|ui| {
                    if app.ui.extensions.pending.is_none()
                        && !app.ui.extensions.path.trim().is_empty()
                        && ui.add_enabled(!busy && !app.file_dialog_open(), egui::Button::new("Show permissions")).clicked()
                    {
                        inspect_path = true;
                    }
                    ui.weak(".pcplugin, .pcext or .wasm");
                });
                if let Some(pending) = &mut app.ui.extensions.pending {
                    ui.separator();
                    ui.strong(format!("{} {}", pending.manifest.name, pending.manifest.version));
                    ui.label(&pending.manifest.description);
                    if !pending.manifest.author.is_empty() {
                        ui.weak(format!("By {}", pending.manifest.author));
                    }
                    if let Some(version) = &pending.installed_version {
                        ui.label(format!("Installed: {version}  →  Selected: {}", pending.manifest.version));
                    }
                    if let Some(reason) = &pending.reason {
                        ui.weak(reason);
                    }
                    source_link(ui, &pending.manifest);
                    ui.label(format!("Package size: {}", storage::bytes(pending.download_bytes)))
                        .on_hover_text(format!("{} bytes in the selected plugin file.", pending.download_bytes));
                    storage::row(ui, pending.storage);
                    ui.label("Requested access:");
                    if pending.manifest.permissions.document_read {
                        ui.label("• Active document name, size, color mode and active layer");
                    }
                    for id in &pending.manifest.permissions.commands {
                        let name = photocraft_engine::commands::find(id).map(|c| c.label).unwrap_or(id);
                        ui.label(format!("• {name}"));
                    }
                    platform::capabilities(ui, &pending.manifest);
                    if pending.native {
                        ui.label(if pending.storage.model_bytes > 0 {
                            "Includes its local processing engine and models."
                        } else {
                            "Includes its local processing engine."
                        });
                        ui.weak("Native code runs with your operating system permissions. Install plugins only from publishers you trust.");
                    } else {
                        ui.weak("Plugin code has no file, network or native process access.");
                    }
                    if pending.action.primary_label().is_none() {
                        ui.collapsing("Advanced", |ui| {
                            let label = match pending.action {
                                InstallAction::Reinstall => "Reinstall or repair plugin",
                                InstallAction::Downgrade => "Install older version",
                                _ => "Replace installed plugin",
                            };
                            if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                                install = Some(pending.action);
                            }
                        });
                    }
                }
                ui.separator();
                ui.strong("Installed plugins");
                if !cfg!(target_arch = "wasm32") {
                    let mut automatic = app.session.prefs().plug_ins.automatic_update_checks;
                    if ui
                        .checkbox(&mut automatic, "Automatically check for plugin updates")
                        .on_hover_text("Checks GitHub after startup and once a day. Downloads and installation always require your approval.")
                        .changed()
                    {
                        change = Some(("prefs.set", json!({"path":"plugIns.automaticUpdateChecks","value":automatic})));
                    }
                    if automatic_job(app).is_some() {
                        ui.weak("Checking GitHub for plugin updates…");
                    } else if !app.ui.extensions.update_runtime.error.is_empty() {
                        ui.weak(&app.ui.extensions.update_runtime.error);
                    }
                }
                if ui.add_enabled(!busy && !extensions.is_empty(), egui::Button::new("Check for updates")).clicked() {
                    check_updates = Some(json!({}));
                }
                if extensions.is_empty() && !has_unavailable {
                    ui.weak("No plugins installed. Choose a plugin file to get started.");
                }
                for (id, manifest, enabled, native, storage) in &extensions {
                    ui.push_id(id, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            let mut value = *enabled;
                            if ui.add_enabled(!busy, egui::Checkbox::new(&mut value, &manifest.name)).changed() {
                                change = Some(("extension.enable", json!({"id": id, "enabled": value})));
                            }
                            ui.weak(&manifest.version);
                        });
                        ui.weak(&manifest.description);
                        storage::row(ui, *storage);
                        if *native {
                            ui.weak("Local processing engine included.");
                        }
                        source_link(ui, manifest);
                        if let Some(update) = app.ui.extensions.updates.get(id) {
                            match update.get("status").and_then(Value::as_str) {
                                Some("available") => {
                                    ui.strong(format!("Update available: {}", update.get("version").and_then(Value::as_str).unwrap_or("new version")));
                                }
                                Some("upToDate") => {
                                    ui.weak("Up to date.");
                                }
                                _ => {
                                    ui.weak(update.get("message").and_then(Value::as_str).unwrap_or("Update checks are not configured for this plugin."));
                                }
                            }
                        }
                        ui.horizontal_wrapped(|ui| {
                            if ui.add_enabled(*enabled, egui::Button::new("Open")).clicked() {
                                reveal = Some(id.clone());
                            }
                            if let Some(update) = app.ui.extensions.updates.get(id)
                                && update.get("status").and_then(Value::as_str) == Some("available")
                                && ui.add_enabled(!busy, egui::Button::new("Update…")).clicked()
                            {
                                prepare_update = Some(json!({"id":id,"candidate":update.get("candidate")}));
                            }
                            if app.ui.extensions.updates.get(id).is_some_and(|update| update.get("status").and_then(Value::as_str) == Some("error"))
                                && ui.add_enabled(!busy, egui::Button::new("Retry update check")).clicked()
                            {
                                check_updates = Some(json!({"id":id}));
                            }
                            if ui.add_enabled(!busy, egui::Button::new("Remove")).clicked() {
                                change = Some(("extension.remove", json!({"id": id})));
                            }
                        });
                        ui.add_space(6.0);
                    });
                }
                #[cfg(not(target_arch = "wasm32"))]
                for plugin in &unavailable {
                    ui.push_id((&plugin.id, "unavailable"), |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(&plugin.name);
                            if let Some(version) = &plugin.version {
                                ui.weak(version);
                            }
                        });
                        ui.label("Could not load this plugin.");
                        ui.weak(&plugin.error);
                        ui.weak("Choose a compatible update or remove this installation. Your plugin data is kept.");
                        ui.horizontal_wrapped(|ui| {
                            if ui.add_enabled(!busy, egui::Button::new("Choose update file…")).clicked() {
                                browse = true;
                            }
                            if ui.add_enabled(!busy, egui::Button::new("Remove")).clicked() {
                                change = Some(("extension.remove", json!({"id":plugin.id})));
                            }
                        });
                        ui.add_space(6.0);
                    });
                }
                for error in &app.session.extensions.load_errors {
                    #[cfg(not(target_arch = "wasm32"))]
                    if unavailable.iter().any(|plugin| error.starts_with(&format!("{}: ", plugin.id))) {
                        continue;
                    }
                    ui.label(error);
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if let Some(pending) = &app.ui.extensions.pending
                    && let Some(label) = pending.action.primary_label()
                    && ui.add_enabled_ui(!busy, |ui| crate::widgets::primary_button(ui, label, 110.0)).inner.clicked()
                {
                    install = Some(pending.action);
                }
                if busy {
                    ctx.request_repaint_after(std::time::Duration::from_millis(32));
                    if ui.button("Cancel operation").clicked() {
                        cancel_job = true;
                    }
                }
                if ui.button("Done").clicked() {
                    done = true;
                }
            });
        });
    app.ui.extensions.manager_open = open && !done;
    app.ui.extensions.drop_rect = manager_window.map(|window| window.response.rect).filter(|_| app.ui.extensions.manager_open);
    if cancel_job && let Some(job) = &app.ui.extensions.manager_job {
        app.session.cancel_job(photocraft_engine::jobs::JobId(job.id));
    }
    let result = if browse {
        Some(choose(app))
    } else if inspect_path {
        Some(installer::review_path(app))
    } else if let Some(action) = install {
        if let Some(pending) = app.ui.extensions.pending.clone() {
            let mut input = pending.input;
            input["intent"] = json!(action.intent());
            // Permission applies only to the reviewed package hash and this installation.
            if pending.native {
                input["allowNative"] = json!(true);
            }
            start_manager_work(app, "extension.install", input, ManagerWork::Install(action));
        }
        None
    } else if let Some(params) = check_updates {
        start_manager_work(app, "extension.updates.check", params, ManagerWork::Check);
        None
    } else if let Some(params) = prepare_update {
        // Automatic discovery can be older than the short-lived download authority.
        // Always refresh the candidate before asking to download and review it.
        if let Some(id) = params.get("id").and_then(Value::as_str).map(str::to_owned) {
            start_manager_work(app, "extension.updates.check", json!({"id":id}), ManagerWork::CheckAndPrepare(id));
        }
        None
    } else if let Some((command, params)) = change {
        let result = app.run(command, params);
        app.menu_cache = None;
        Some(result)
    } else {
        None
    };
    if let Some(result) = result {
        app.ui.extensions.status = match result {
            Ok(value) if value.get("job").is_some() => "Reading plugin permissions…".into(),
            Ok(value) if value.get("fileDialog").is_some() || value.get("review").is_some() || value.get("dialog").is_some() => String::new(),
            Ok(_) => "Changes saved.".into(),
            Err(e) if e == crate::file_dialog::CANCELLED => String::new(),
            Err(e) => e,
        };
    }
    if let Some(id) = reveal {
        app.ui.extensions.panels.entry(id.clone()).or_default().open = false;
        let _ = menu(app, &format!("{PREFIX}{id}"));
    }
}

#[cfg(test)]
mod tests;
