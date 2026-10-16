use super::*;
use crate::extension_ui::{ManagerJob, ManagerWork};
use serde_json::json;

fn fixture_app() -> PhotocraftApp {
    use base64::Engine;
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    let manifest = include_str!("../../../../../examples/extensions/quick-tools/manifest.json");
    let data = base64::engine::general_purpose::STANDARD.encode(crate::extension_ui::tests::module(manifest));
    app.session.execute("extension.install", json!({"data":data})).unwrap();
    app.background_jobs = true;
    app
}

#[test]
fn startup_daily_restart_and_clock_changes_are_bounded() {
    let now = 1_800_000_000;
    let mut schedule = Schedule::default();
    assert_eq!(schedule.wait(0.0, now, 0), 30.0);
    assert_eq!(schedule.wait(29.0, now + 29, 0), 1.0);
    assert_eq!(schedule.wait(30.0, now + 30, 0), 0.0);
    schedule.attempted_at = Some(30.0);
    assert_eq!(schedule.wait(31.0, now + DAY * 2, now + 30), (DAY - 1) as f64, "forward clock jumps cannot retry immediately");
    assert_eq!(schedule.wait(60.0, now - DAY, now + 30), (DAY - 30) as f64, "backward wall clock retains monotonic cooldown");
    assert_eq!(schedule.wait(DAY as f64 + 30.0, now + DAY + 30, now + 30), 0.0);
    let mut restarted = Schedule::default();
    assert_eq!(restarted.wait(0.0, now + 3600, now), (DAY - 3600) as f64);
    assert_eq!(restarted.wait((DAY - 3600) as f64, now + DAY, now), 0.0);
    let mut corrupt = Schedule::default();
    assert_eq!(corrupt.wait(0.0, now, u64::MAX), 30.0);
    assert_eq!(corrupt.wait(30.0, now, u64::MAX), 0.0);
    assert!(corrupt.wait(f64::NAN, now, 0) > 0.0);
    assert!(corrupt.wait(f64::INFINITY, now, 0) > 0.0);
    assert!(schedule.wait(0.0, now + DAY, now) > 0.0, "reset monotonic clock delays rather than repeatedly firing");
}

#[test]
fn defaults_opt_out_and_attempt_time_survive_preferences_reload() {
    let mut app = fixture_app();
    app.session.load_prefs_json(r#"{"plugIns":{"additionalPluginsFolder":""}}"#).unwrap();
    assert!(app.session.prefs().plug_ins.automatic_update_checks);
    assert_eq!(app.session.prefs().plug_ins.last_update_check_unix_secs, 0);
    app.run("prefs.set", json!({"path":"plugIns.automaticUpdateChecks","value":false})).unwrap();
    remember_attempt(&mut app);
    let serialized = app.session.prefs_to_json();
    let mut restarted = fixture_app();
    restarted.session.load_prefs_json(&serialized).unwrap();
    assert!(!restarted.session.prefs().plug_ins.automatic_update_checks);
    assert!(restarted.session.prefs().plug_ins.last_update_check_unix_secs > 0);
    assert!(restarted.run("prefs.set", json!({"path":"plugIns.lastUpdateCheckUnixSecs","value":-1})).is_err());
}

#[test]
fn available_results_badge_and_installation_changes_are_consistent() {
    let mut app = fixture_app();
    let row = json!({"id":"org.photocraft.quick-tools","installedVersion":"1.0.0","version":"2.0.0","status":"available","candidate":"opaque"});
    accept_results(&mut app, &json!({"updates":[row]})).unwrap();
    assert_eq!(available_count(&app), 1);
    app.run("extension.enable", json!({"id":"org.photocraft.quick-tools","enabled":false})).unwrap();
    invalidate_changed_installations(&mut app);
    assert_eq!(available_count(&app), 1, "enable changes preserve valid publisher/version discovery");
    assert!(crate::menus::menu_items(&app).iter().any(|item| item.id == super::super::MANAGER && item.label.contains("1 update")));
    app.run("extension.remove", json!({"id":"org.photocraft.quick-tools"})).unwrap();
    assert_eq!(available_count(&app), 0);
    invalidate_changed_installations(&mut app);
    assert!(app.ui.extensions.updates.is_empty());
}

#[test]
fn installing_another_plugin_preserves_existing_updates() {
    use base64::Engine;
    let mut app = fixture_app();
    accept_results(
        &mut app,
        &json!({"updates":[{
            "id":"org.photocraft.quick-tools","installedVersion":"1.0.0","version":"2.0.0","status":"available"
        }]}),
    )
    .unwrap();
    let mut descriptor: Value = serde_json::from_str(include_str!("../../../../../examples/extensions/quick-tools/manifest.json")).unwrap();
    descriptor["id"] = json!("org.example.another-plugin");
    let data = base64::engine::general_purpose::STANDARD.encode(crate::extension_ui::tests::module(&descriptor.to_string()));
    app.session.execute("extension.install", json!({"data":data})).unwrap();
    invalidate_changed_installations(&mut app);
    assert_eq!(available_count(&app), 1);
    app.run("extension.remove", json!({"id":"org.example.another-plugin"})).unwrap();
    invalidate_changed_installations(&mut app);
    assert_eq!(available_count(&app), 1);
    // Same-version changes of source metadata are not interchangeable discovery results.
    app.ui.extensions.update_runtime.sources.get_mut("org.photocraft.quick-tools").unwrap().repository =
        Some("https://github.com/example-org/old-source".into());
    app.session.extensions.revision = app.session.extensions.revision.wrapping_add(1);
    invalidate_changed_installations(&mut app);
    assert_eq!(available_count(&app), 0);
}

#[test]
fn stale_versions_and_failed_checks_never_claim_up_to_date() {
    let mut app = fixture_app();
    accept_results(&mut app, &json!({"updates":[{"id":"org.photocraft.quick-tools","installedVersion":"0.9.0","version":"2.0.0","status":"available"}]}))
        .unwrap();
    assert!(app.ui.extensions.updates.is_empty());
    app.ui.extensions.update_runtime.job = Some(JobId(19));
    let event = JobEvent {
        id: JobId(19),
        command: "extension.updates.check".into(),
        label: "Check plugin updates".into(),
        document: None,
        outcome: JobOutcome::Failed("Offline".into()),
    };
    assert!(automatic_job_finished(&mut app, &event));
    assert!(app.ui.extensions.update_runtime.error.contains("Offline"));
    assert!(!app.ui.extensions.manager_open);
    assert!(app.ui.notices.is_empty());
    assert_eq!(available_count(&app), 0);
}

#[test]
fn completion_from_a_replaced_installation_cannot_restore_its_badge() {
    let mut app = fixture_app();
    app.ui.extensions.update_runtime.job = Some(JobId(19));
    app.ui.extensions.update_runtime.job_revision = Some(app.session.extensions.revision);
    // Installation/source changes advance this revision even for a same-version repair.
    app.session.extensions.revision = app.session.extensions.revision.wrapping_add(1);
    let event = JobEvent {
        id: JobId(19),
        command: "extension.updates.check".into(),
        label: "Check plugin updates".into(),
        document: None,
        outcome: JobOutcome::Done(json!({"updates":[{
            "id":"org.photocraft.quick-tools","installedVersion":"1.0.0","version":"2.0.0","status":"available"
        }]})),
    };
    assert!(automatic_job_finished(&mut app, &event));
    assert!(automatic_job(&app).is_none());
    assert!(app.ui.extensions.updates.is_empty());
    assert_eq!(available_count(&app), 0);
}

#[test]
fn manager_opt_out_is_persistent_and_update_refreshes_old_authority() {
    use egui_kittest::{Harness, kittest::Queryable};
    let mut app = fixture_app();
    // This neutral fixture has no release source, so refreshing it cannot contact GitHub.
    app.background_jobs = false;
    app.ui.extensions.manager_open = true;
    accept_results(
        &mut app,
        &json!({"updates":[{
            "id":"org.photocraft.quick-tools","installedVersion":"1.0.0","version":"2.0.0",
            "status":"available","candidate":"expired-candidate-must-not-be-prepared"
        }]}),
    )
    .unwrap();
    let mut harness = Harness::builder().with_size(egui::vec2(1100.0, 900.0)).build_ui_state(
        |ui, app| {
            crate::extension_ui::windows(app, ui.ctx());
        },
        app,
    );
    PhotocraftApp::setup_context(&harness.ctx, crate::theme::ThemeKind::ProMedium);
    harness.run_steps(4);
    harness.get_by_label("Automatically check for plugin updates").click();
    harness.run_steps(3);
    assert!(!harness.state().session.prefs().plug_ins.automatic_update_checks);
    harness.get_by_label("Update…").click();
    harness.run_steps(3);
    assert_eq!(harness.state().ui.extensions.updates["org.photocraft.quick-tools"]["status"], "unsupported");
    assert_eq!(harness.state().ui.extensions.status, "Update check complete.");
    assert!(harness.state().ui.extensions.pending.is_none());
    assert!(harness.query_by_label("Up to date.").is_none());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn scan_requires_explicit_metadata_and_defers_to_user_work() {
    use base64::Engine;
    let mut app = fixture_app();
    assert!(!eligible(&app), "no GitHub metadata means no network work");
    let mut descriptor: Value = serde_json::from_str(include_str!("../../../../../examples/extensions/quick-tools/manifest.json")).unwrap();
    descriptor["id"] = json!("org.example.update-test");
    descriptor["repository"] = json!("https://github.com/example-org/update-test");
    descriptor["updates"] = json!({"provider":"github","asset":"Test.pcext"});
    let data = base64::engine::general_purpose::STANDARD.encode(crate::extension_ui::tests::module(&descriptor.to_string()));
    app.session.execute("extension.install", json!({"data":data})).unwrap();
    assert!(eligible(&app));
    app.automation_input = true;
    assert!(!eligible(&app));
    app.automation_input = false;
    app.session.authorize = Some(|_, _| Err(photocraft_engine::EngineError::Other("Restricted".into())));
    assert!(!eligible(&app));
    app.session.authorize = None;
    app.services.automation_authorize = Some(|_, _| Err(photocraft_engine::EngineError::Other("Restricted".into())));
    assert!(!eligible(&app), "an attached restricted automation service never triggers unattended network work");
    app.services.automation_authorize = None;
    app.background_jobs = false;
    assert!(!eligible(&app));
    app.background_jobs = true;
    app.ui.extensions.manager_job = Some(ManagerJob { id: 9, work: ManagerWork::Check });
    assert!(!eligible(&app));
    app.ui.extensions.manager_job = None;
    let started = app
        .session
        .start_job(
            "test.auto-update",
            json!({}),
            "Neutral scan",
            false,
            |ctx| {
                while !ctx.cancelled() {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                ctx.check()
            },
            |_, ()| Ok(json!({"updates":[]})),
        )
        .unwrap();
    let photocraft_engine::jobs::Started::Job(job) = started else { panic!("must be background") };
    assert!(!eligible(&app), "a running user job defers automatic checks");
    app.ui.extensions.update_runtime.job = Some(job);
    assert!(crate::jobs_ui::job_in_view(&app).is_none(), "automatic work does not capture Esc/status");
    assert!(app.ui.extensions.manager_job.is_none(), "automatic checks never occupy manager operation state");
    let (reply, received) = std::sync::mpsc::channel();
    app.jobs.waiters.push((job, reply));
    cancel_automatic(&mut app);
    crate::jobs_ui::tick(&mut app, &egui::Context::default());
    app.session.join_cancelled_jobs();
    assert!(automatic_job(&app).is_none());
    assert!(app.ui.notices.is_empty());
    assert!(app.ui.status.is_empty());
    assert_eq!(received.try_recv().unwrap()["error"], "cancelled", "quiet jobs still answer explicit control waiters");
}
