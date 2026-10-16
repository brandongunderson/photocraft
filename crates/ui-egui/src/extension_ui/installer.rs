//! All installer inputs converge on review; dropping a file never installs it.
use super::*;

fn supported(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| ["pcplugin", "pcext", "wasm"].iter().any(|supported| extension.eq_ignore_ascii_case(supported)))
}

fn review_source(app: &mut PhotocraftApp, name: &str, input: Value) -> Result<Value, String> {
    ensure_idle(app)?;
    app.ui.extensions.manager_open = true;
    app.ui.extensions.pending = None;
    app.ui.extensions.status.clear();
    if !supported(name) {
        return Err("Choose one .pcplugin, .pcext or .wasm plugin file.".into());
    }
    app.ui.extensions.path = name.into();
    let panel = inspect(app, input.clone());
    if panel.is_err()
        && !app.session.plugins_disabled
        && name.to_ascii_lowercase().ends_with(".wasm")
        && let Some(id) = filter_input(&input)
    {
        // Pixel filters use their existing sandboxed ABI and explicit confirmation dialog.
        // A .wasm panel still takes the normal manifest/permission review above.
        app.ui.extensions.status.clear();
        let result = crate::plugin_ui::review_filter(app, input)?;
        app.ui.extensions.filter_review = Some(id);
        return Ok(result);
    }
    panel
}

fn ensure_idle(app: &PhotocraftApp) -> Result<(), String> {
    if app.ui.extensions.manager_job.is_some() {
        return Err("Wait for the current plugin operation to finish, or cancel it.".into());
    }
    Ok(())
}

/// Detect the standalone filter ABI without registering it or hiding a panel's incompatibility error.
fn filter_input(input: &Value) -> Option<String> {
    let limits = photocraft_plugins::Limits::default();
    if let Some(data) = input.get("data").and_then(Value::as_str) {
        if data.len() > (limits.max_module_bytes / 3 + 1) * 4 {
            return None;
        }
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else { return None };
        return photocraft_plugins::Plugin::load(&bytes, limits).ok().map(|plugin| plugin.id().to_owned());
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = input.get("path").and_then(Value::as_str) {
        return photocraft_plugins::registry::load_file(std::path::Path::new(path), limits).ok().map(|plugin| plugin.id().to_owned());
    }
    None
}

fn contents_input(name: &str, bytes: &[u8]) -> Result<Value, String> {
    if name.to_ascii_lowercase().ends_with(".pcplugin") {
        return Err("Install .pcplugin packages from a file on the desktop.".into());
    }
    let limit = if name.to_ascii_lowercase().ends_with(".wasm") { photocraft_plugins::Limits::default().max_module_bytes } else { MAX_MODULE_BYTES };
    if bytes.len() > limit {
        return Err(format!("The plugin exceeds the {} MiB module limit.", limit / (1024 * 1024)));
    }
    Ok(json!({"data": base64::engine::general_purpose::STANDARD.encode(bytes)}))
}

fn review_error(app: &mut PhotocraftApp, result: Result<Value, String>) -> Result<Value, String> {
    if let Err(error) = &result {
        app.ui.extensions.status = error.clone();
    }
    result
}

pub(super) fn review_answer(app: &mut PhotocraftApp, answer: FileDialogAnswer) -> Result<Value, String> {
    if let Err(error) = ensure_idle(app) {
        return review_error(app, Err(error));
    }
    app.ui.extensions.manager_open = true;
    app.ui.extensions.pending = None;
    let result = (|| match answer {
        FileDialogAnswer::Paths(paths) if paths.len() == 1 => {
            let path = paths.first().ok_or("Choose one plugin file.")?;
            review_source(app, path, json!({"path": path}))
        }
        FileDialogAnswer::Contents(name, bytes) => {
            if !supported(&name) {
                return Err("Choose one .pcplugin, .pcext or .wasm plugin file.".into());
            }
            let input = contents_input(&name, &bytes)?;
            review_source(app, &name, input)
        }
        _ => Err("Choose one plugin file at a time.".into()),
    })();
    review_error(app, result)
}

pub(super) fn review_arrived(app: &mut PhotocraftApp, files: &[(String, Vec<u8>)]) -> bool {
    let plugin = files.iter().any(|(name, _)| {
        let name = name.to_ascii_lowercase();
        name.ends_with(".pcplugin") || name.ends_with(".pcext") || (app.ui.extensions.manager_open && name.ends_with(".wasm"))
    });
    if !plugin {
        return false;
    }
    if let Err(error) = ensure_idle(app) {
        app.ui.extensions.status = error;
        return true;
    }
    app.ui.extensions.manager_open = true;
    app.ui.extensions.pending = None;
    let result = (|| {
        if app.file_dialog_open() {
            return Err("Close the file browser before dropping a plugin.".into());
        }
        let [(name, bytes)] = files else {
            return Err("Drop one plugin file at a time.".into());
        };
        let input = contents_input(name, bytes)?;
        review_source(app, name, input)
    })();
    let _ = review_error(app, result);
    true
}

pub(super) fn review_path(app: &mut PhotocraftApp) -> Result<Value, String> {
    let path = app.ui.extensions.path.trim().to_owned();
    let result = review_source(app, &path, json!({"path": path}));
    review_error(app, result)
}

pub(super) fn review_drop(app: &mut PhotocraftApp, files: &[egui::DroppedFileHandle]) -> Result<(), String> {
    ensure_idle(app)?;
    app.ui.extensions.manager_open = true;
    app.ui.extensions.pending = None;
    app.ui.extensions.status.clear();
    let result = (|| {
        if app.file_dialog_open() {
            return Err("Close the file browser before dropping a plugin.".into());
        }
        let [file] = files else {
            return Err("Drop one plugin file at a time.".into());
        };
        let name = file.path().to_string_lossy();
        if !supported(&name) {
            return Err("Drop a .pcplugin, .pcext or .wasm plugin file.".into());
        }
        if file.path().is_absolute() {
            review_source(app, &name, json!({"path": name}))
        } else {
            if name.to_ascii_lowercase().ends_with(".pcplugin") {
                return Err("Install .pcplugin packages from a file on the desktop.".into());
            }
            let bytes = crate::read_dropped(&**file)?;
            let input = contents_input(&name, &bytes)?;
            review_source(app, &name, input)
        }
    })();
    review_error(app, result).map(|_| ())
}
