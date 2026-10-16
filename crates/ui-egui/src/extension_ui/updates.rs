//! Desktop update discovery: cheap scheduling in logic, all network work in engine jobs.
use photocraft_engine::jobs::{JobEvent, JobId, JobOutcome};
use serde_json::Value;
#[cfg(not(target_arch = "wasm32"))]
use serde_json::json;

use crate::PhotocraftApp;

#[cfg(any(test, not(target_arch = "wasm32")))]
const DAY: u64 = 24 * 60 * 60;
#[cfg(any(test, not(target_arch = "wasm32")))]
const STARTUP_DELAY: f64 = 30.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Runtime {
    #[cfg(any(test, not(target_arch = "wasm32")))]
    schedule: Schedule,
    job: Option<JobId>,
    job_revision: Option<u64>,
    revision: Option<u64>,
    sources: std::collections::BTreeMap<String, Source>,
    pub(super) error: String,
}

/// Keep only discovery identity, not potentially large panel descriptors or expired tokens.
#[derive(Clone, Debug, PartialEq)]
struct Source {
    version: String,
    repository: Option<String>,
    updates: Option<photocraft_plugin_api::UpdateSource>,
}

impl Source {
    fn capture(manifest: &photocraft_plugins::extension::Manifest) -> Self {
        Self { version: manifest.version.clone(), repository: manifest.repository.clone(), updates: manifest.updates.clone() }
    }

    fn matches(&self, manifest: &photocraft_plugins::extension::Manifest) -> bool {
        self.version == manifest.version && self.repository == manifest.repository && self.updates == manifest.updates
    }
}

#[cfg(any(test, not(target_arch = "wasm32")))]
#[derive(Clone, Debug, Default, PartialEq)]
struct Schedule {
    opened_at: Option<f64>,
    attempted_at: Option<f64>,
    observed_at: Option<f64>,
}

#[cfg(any(test, not(target_arch = "wasm32")))]
impl Schedule {
    /// Wall time survives restart; monotonic time prevents a changed clock from hammering
    /// GitHub during this session. A corrupt/future persisted timestamp cannot disable checks.
    fn wait(&mut self, monotonic: f64, unix_secs: u64, last: u64) -> f64 {
        if !monotonic.is_finite() || monotonic < 0.0 {
            return STARTUP_DELAY;
        }
        if self.observed_at.is_some_and(|previous| monotonic < previous) {
            self.opened_at = Some(monotonic);
            self.attempted_at = self.attempted_at.map(|_| monotonic);
        }
        self.observed_at = Some(monotonic);
        let started = *self.opened_at.get_or_insert(monotonic);
        let startup = (STARTUP_DELAY - (monotonic - started)).max(0.0);
        let cooldown = self.attempted_at.map_or(0.0, |at| (DAY as f64 - (monotonic - at)).max(0.0));
        let persisted = if last == 0 || last > unix_secs { 0 } else { DAY.saturating_sub(unix_secs - last) };
        startup.max(cooldown).max(persisted as f64)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs())
}

pub(super) fn remember_attempt(app: &mut PhotocraftApp) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        app.ui.extensions.update_runtime.schedule.attempted_at = app.ui.extensions.update_runtime.schedule.observed_at;
        let now = now_unix();
        // Bookkeeping alone does not reapply document/history or plugin-folder preferences.
        app.session.prefs.edit(|prefs| prefs.plug_ins.last_update_check_unix_secs = now);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = app;
}

pub(crate) fn automatic_job(app: &PhotocraftApp) -> Option<JobId> {
    app.ui.extensions.update_runtime.job
}

/// Cancelling only the automatic scan lets explicit actions take priority immediately.
/// Retain its ID until the completion event so cancellation stays silent.
pub(super) fn cancel_automatic(app: &mut PhotocraftApp) {
    if let Some(id) = automatic_job(app) {
        app.session.cancel_job(id);
    }
}

fn invalidate_changed_installations(app: &mut PhotocraftApp) {
    let revision = app.session.extensions.revision;
    let runtime = &mut app.ui.extensions.update_runtime;
    if runtime.revision.is_some_and(|old| old != revision) {
        // Installing one update must not hide updates for every other plugin until tomorrow.
        // This is informational metadata only; download authority is always refreshed.
        app.ui.extensions.updates.retain(|id, _| {
            runtime
                .sources
                .get(id)
                .is_some_and(|source| app.session.extensions.installed.get(id).is_some_and(|installed| source.matches(&installed.extension.manifest)))
        });
        runtime.sources.retain(|id, _| app.ui.extensions.updates.contains_key(id));
        app.menu_cache = None;
    }
    runtime.revision = Some(revision);
}

pub(super) fn available_count(app: &PhotocraftApp) -> usize {
    app.ui
        .extensions
        .updates
        .iter()
        .filter(|(id, row)| {
            row.get("status").and_then(Value::as_str) == Some("available")
                && app
                    .session
                    .extensions
                    .installed
                    .get(*id)
                    .is_some_and(|installed| row.get("installedVersion").and_then(Value::as_str) == Some(installed.extension.manifest.version.as_str()))
                && app
                    .ui
                    .extensions
                    .update_runtime
                    .sources
                    .get(*id)
                    .is_some_and(|source| app.session.extensions.installed.get(*id).is_some_and(|installed| source.matches(&installed.extension.manifest)))
        })
        .count()
}

pub(super) fn accept_results(app: &mut PhotocraftApp, result: &Value) -> Result<(), String> {
    invalidate_changed_installations(app);
    let rows = result.get("updates").and_then(Value::as_array).ok_or("The update check returned no results. Try again.")?;
    for row in rows {
        if let Some(id) = row.get("id").and_then(Value::as_str)
            && app
                .session
                .extensions
                .installed
                .get(id)
                .is_some_and(|installed| row.get("installedVersion").and_then(Value::as_str) == Some(installed.extension.manifest.version.as_str()))
        {
            app.ui.extensions.updates.insert(id.into(), row.clone());
            if let Some(installed) = app.session.extensions.installed.get(id) {
                app.ui.extensions.update_runtime.sources.insert(id.into(), Source::capture(&installed.extension.manifest));
            }
        }
    }
    app.ui.extensions.update_runtime.error.clear();
    app.menu_cache = None;
    Ok(())
}

pub(crate) fn automatic_job_finished(app: &mut PhotocraftApp, event: &JobEvent) -> bool {
    if automatic_job(app) != Some(event.id) {
        return false;
    }
    app.ui.extensions.update_runtime.job = None;
    let checked_revision = app.ui.extensions.update_runtime.job_revision.take();
    if checked_revision.is_some_and(|revision| revision != app.session.extensions.revision) {
        invalidate_changed_installations(app);
        return true;
    }
    let error = match &event.outcome {
        JobOutcome::Done(result) => accept_results(app, result).err(),
        JobOutcome::Failed(error) => Some(format!("Automatic update check failed: {error}. You can retry with Check for updates.")),
        JobOutcome::Cancelled => None,
    };
    if let Some(error) = error {
        app.ui.extensions.update_runtime.error = error;
    }
    true
}

pub(crate) fn tick_updates(app: &mut PhotocraftApp, ctx: &egui::Context) {
    invalidate_changed_installations(app);
    #[cfg(not(target_arch = "wasm32"))]
    {
        if !app.session.prefs().plug_ins.automatic_update_checks || app.session.plugins_disabled {
            cancel_automatic(app);
            return;
        }
        let monotonic = ctx.input(|input| input.time);
        let wait = app.ui.extensions.update_runtime.schedule.wait(monotonic, now_unix(), app.session.prefs().plug_ins.last_update_check_unix_secs);
        // A minute is enough to observe idle/restricted-state changes without a frame timer.
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(if wait > 0.0 { wait.min(60.0) } else { 60.0 }));
        if wait > 0.0 || !eligible(app) {
            return;
        }
        remember_attempt(app);
        // start() always uses a worker, independently of drawing and app.run's inline path.
        match app.session.start("extension.updates.check", json!({})) {
            Ok(photocraft_engine::jobs::Started::Job(id)) => {
                app.ui.extensions.update_runtime.job = Some(id);
                app.ui.extensions.update_runtime.job_revision = Some(app.session.extensions.revision);
            }
            Ok(photocraft_engine::jobs::Started::Done(result)) => {
                if let Err(error) = accept_results(app, &result) {
                    app.ui.extensions.update_runtime.error = error;
                }
            }
            Err(error) => app.ui.extensions.update_runtime.error = format!("Automatic update check failed: {error}. You can retry with Check for updates."),
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = ctx;
}

#[cfg(not(target_arch = "wasm32"))]
fn eligible(app: &PhotocraftApp) -> bool {
    app.background_jobs
        && !app.automation_input
        && app.session.authorize.is_none()
        && app.services.automation_authorize.is_none()
        && app.ui.extensions.manager_job.is_none()
        && app.ui.extensions.pending.is_none()
        && automatic_job(app).is_none()
        && !app.session.has_jobs()
        && !app.file_dialog_open()
        && app.session.extensions.installed.values().any(|plugin| plugin.extension.manifest.repository.is_some() && plugin.extension.manifest.updates.is_some())
}

#[cfg(test)]
mod tests;
