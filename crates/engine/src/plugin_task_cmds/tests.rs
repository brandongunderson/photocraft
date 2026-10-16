use super::*;

#[test]
fn task_options_are_bounded_and_reject_ambient_paths_and_bad_types() {
    for bad in [
        Value::Null,
        json!([]),
        json!({"id":"../other"}),
        json!({"options":null}),
        json!({"worker":"x.exe"}),
        json!({"options":{"huge":"x".repeat(MAX_OPTIONS_BYTES)}}),
    ] {
        assert!(params(&bad).is_err(), "{bad:.80}");
    }
    assert!(params(&json!({"options":{"kind":"refresh"}})).is_ok());
    assert!(params(&json!({})).is_ok());
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use super::*;
    use crate::extension_cmds::{Installed, StorageSize};
    use crate::extension_package::{InstalledNative, NativeInfo, PackageInfo};
    use std::{collections::BTreeMap, sync::Arc};

    const ID: &str = "org.example.task";

    fn module(grant: bool, calls: Value) -> Vec<u8> {
        let manifest = json!({"apiVersion":2,"id":ID,"name":"Task test","version":"1",
            "capabilities":{"required":[{"id":"host.commands.invoke","minVersion":1}]},
            "permissions":{"commands":if grant {vec!["plugin.processImage","plugin.runTask","layer.new.layer"]} else {vec![]}},
            "panel":{"buttons":[{"id":"refresh","label":"Refresh"}]}})
        .to_string();
        let response = json!({"state":{"runs":1},"commands":calls}).to_string();
        let escape = |s: &str| s.bytes().map(|b| format!("\\{b:02x}")).collect::<String>();
        wat::parse_str(format!(
            r#"(module (memory (export "memory") 8)
            (data (i32.const 32) "{}") (data (i32.const 65536) "{}")
            (func (export "pce_api_version") (result i32) i32.const 2)
            (func (export "pce_manifest") (result i64) i64.const {})
            (func (export "pce_alloc") (param i32) (result i32) i32.const 196608)
            (func (export "pce_handle") (param i32 i32) (result i64) i64.const {}))"#,
            escape(&manifest),
            escape(&response),
            ((manifest.len() as u64) << 32) | 32,
            ((response.len() as u64) << 32) | 65536
        ))
        .unwrap()
    }

    fn session(grant: bool, calls: Value) -> Session {
        let extension = Arc::new(photocraft_plugins::extension::Extension::load(&module(grant, calls)).unwrap());
        let mut session = Session::new();
        session.extensions.installed.insert(
            ID.into(),
            Installed {
                #[cfg(not(target_arch = "wasm32"))]
                deferred: None,
                extension,
                enabled: true,
                state: json!({"runs":0}),
                storage: StorageSize::default(),
                filters: BTreeMap::new(),
                package: None,
                native: Some(Arc::new(InstalledNative {
                    generation: None,
                    worker_v2: None,
                    worker: "missing-task-fixture.exe".into(),
                    package: PackageInfo {
                        package_version: 2,
                        module: "plugin.pcext".into(),
                        filters: vec![],
                        native: Some(NativeInfo { service: "plugin.processImage".into(), platform: "windows-x86_64".into(), entry: "worker.exe".into() }),
                    },
                })),
            },
        );
        session
    }

    fn binding(s: &mut Session) -> Binding {
        let installed = &s.extensions.installed[ID];
        Binding {
            id: ID.into(),
            extension: Arc::clone(&installed.extension),
            native: Arc::clone(installed.native.as_ref().unwrap()),
            state: Some(json!({"runs":1})),
            source_state: installed.state.clone(),
            epoch: Arc::clone(s.extensions.task_epochs.entry(ID.into()).or_default()),
        }
    }

    fn result() -> crate::plugin_runtime::TaskResult {
        serde_json::from_value(json!({"apiVersion":1,"message":"Cache refreshed","data":{"ready":true}})).unwrap()
    }

    #[test]
    fn standalone_task_grants_require_a_reviewed_native_package() {
        let bytes = module(true, json!([]));
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut data = String::new();
        for chunk in bytes.chunks(3) {
            let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
            for i in 0..4 {
                data.push(if i <= chunk.len() { TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char } else { '=' });
            }
        }
        let mut s = Session::new();
        for command in ["extension.inspect", "extension.install"] {
            assert!(s.execute(command, json!({"data":data})).unwrap_err().to_string().contains("own native worker"));
        }
    }

    #[test]
    fn completion_needs_no_image_and_changes_only_guest_state() {
        let mut s = session(true, json!([]));
        let result = binding(&mut s).complete(&mut s, result()).unwrap();
        assert_eq!(result["message"], "Cache refreshed");
        assert_eq!(result["data"]["ready"], true);
        assert!(s.documents().is_empty());
        assert!(s.journal.is_empty());
        assert_eq!(s.extensions.installed[ID].state["runs"], 1);
    }

    #[test]
    fn tasks_preserve_equal_state_previews_but_cannot_overwrite_newer_state_on_apply() {
        for task_state in [0, 2] {
            let mut s = session(true, json!([]));
            s.execute("file.new", json!({"width":8,"height":6})).unwrap();
            let source = Arc::clone(&s.active().unwrap().doc);
            let revision = s.active().unwrap().revision;
            let extension = Arc::clone(&s.extensions.installed[ID].extension);
            let preview_binding = crate::plugin_preview::PreviewBinding::for_installed(&s, ID, &extension, Some(json!({"runs":99}))).unwrap();
            let preview = crate::plugin_preview::reserve(&mut s, source.id, &preview_binding).unwrap();
            let mut image = photocraft_doc::Document::new("Preview", source.size, photocraft_color::ColorMode::Rgb, photocraft_color::SampleType::U8);
            image.layers.push(photocraft_doc::Layer::raster("Result", image.pixel_format()));
            crate::plugin_preview::stage(&mut s, preview, Arc::clone(&source), revision, image, preview_binding).unwrap();
            let mut task = binding(&mut s);
            task.state = Some(json!({"runs":task_state}));
            task.complete(&mut s, result()).unwrap();
            assert!(s.plugin_preview(preview).is_some(), "task completion does not discard comparison pixels");
            if task_state == 0 {
                s.validate_plugin_preview(preview).unwrap();
                s.execute("image.pluginPreview.apply", json!({"preview":preview})).unwrap();
                assert_eq!(s.extensions.installed[ID].state["runs"], 99);
                assert_eq!(s.documents().len(), 2);
            } else {
                assert!(s.validate_plugin_preview(preview).unwrap_err().to_string().contains("state changed"));
                assert!(s.execute("image.pluginPreview.apply", json!({"preview":preview})).unwrap_err().to_string().contains("state changed"));
                assert_eq!(s.extensions.installed[ID].state["runs"], 2);
                assert_eq!(s.documents().len(), 1);
                assert!(Arc::ptr_eq(&source, &s.active().unwrap().doc));
            }
        }
    }

    #[test]
    fn pending_tasks_do_not_clobber_a_newer_state_only_panel_action() {
        for proposed in [0, 2] {
            let mut s = session(true, json!([]));
            let mut task = binding(&mut s);
            task.state = Some(json!({"runs":proposed}));
            let (tx, rx) = std::sync::mpsc::channel();
            let crate::jobs::Started::Job(job) = s
                .start_job(
                    "plugin.runTask",
                    json!({"id":ID}),
                    "Task fixture",
                    false,
                    move |_| {
                        rx.recv().unwrap();
                        Ok(result())
                    },
                    move |s, result| task.complete(s, result),
                )
                .unwrap()
            else {
                panic!("expected task job")
            };
            assert!(s.job(job).unwrap().document.is_none());
            // The test fixture panel returns state {runs:1} with no editor commands.
            s.execute("extension.invoke", json!({"id":ID,"event":"refresh"})).unwrap();
            assert_eq!(s.extensions.installed[ID].state["runs"], 1);
            assert!(s.job(job).is_some());
            tx.send(()).unwrap();
            let completed = s.wait_job(job);
            if proposed == 0 {
                assert_eq!(completed.unwrap()["message"], "Cache refreshed");
            } else {
                assert!(completed.unwrap_err().to_string().contains("state changed while the task ran"));
            }
            assert_eq!(s.extensions.installed[ID].state["runs"], 1);
            assert!(s.documents().is_empty());
        }
    }

    #[test]
    fn disable_reenable_and_removal_revoke_task_completions_without_committing_state() {
        for remove in [false, true] {
            let mut s = session(true, json!([]));
            let binding = binding(&mut s);
            if remove {
                assert_eq!(s.execute("extension.remove", json!({"id":ID})).unwrap()["removed"], ID);
                assert!(!s.extensions.installed.contains_key(ID));
            } else {
                s.execute("extension.enable", json!({"id":ID,"enabled":false})).unwrap();
                s.execute("extension.enable", json!({"id":ID,"enabled":true})).unwrap();
            }
            let failure = binding.complete(&mut s, result()).unwrap_err().to_string();
            assert!(failure.contains(if remove { "disabled, removed or replaced" } else { "revoked" }), "{failure}");
            if let Some(installed) = s.extensions.installed.get(ID) {
                assert_eq!(installed.state["runs"], 0);
            }
            assert!(s.documents().is_empty());
            assert!(s.journal.is_empty());
        }
    }

    #[test]
    fn task_exclusivity_authorization_ownership_and_recovery_are_checked_before_work() {
        let calls = json!([{"command":"plugin.runTask","params":{}},{"command":"layer.new.layer","params":{}}]);
        let mut s = session(true, calls);
        assert!(s.execute("extension.invoke", json!({"id":ID,"event":"refresh"})).unwrap_err().to_string().contains("only command"));
        let mut s = session(true, json!([{"command":"plugin.runTask","params":{}}]));
        s.authorize = Some(|id, _| if id == "plugin.runTask" { Err(error("task denied")) } else { Ok(()) });
        assert!(s.execute("extension.invoke", json!({"id":ID,"event":"refresh"})).unwrap_err().to_string().contains("task denied"));
        s.authorize = None;
        let extension = Arc::clone(&s.extensions.installed[ID].extension);
        assert!(run(&mut s, &json!({"id":"org.example.other"}), Some((ID.into(), extension, json!({})))).unwrap_err().to_string().contains("own installation"));
        s.plugins_disabled = true;
        assert!(s.execute("plugin.runTask", json!({"id":ID})).unwrap_err().to_string().contains("recovery"));
        let mut s = session(false, json!([]));
        assert!(s.execute("plugin.runTask", json!({"id":ID})).unwrap_err().to_string().contains("permission"));
        assert!(!s.has_jobs());
    }

    #[test]
    fn async_tasks_have_no_document_lock_and_do_not_overwrite_state_when_worker_fails() {
        let mut s = session(true, json!([{"command":"plugin.runTask","params":{}}]));
        // Hold a real admission permit so both native requests deterministically queue;
        // no timing assumption about the missing worker's eventual spawn failure is needed.
        let owner = crate::extension_cmds::scheduler_owner(&s, ID);
        let permit = s
            .plugin_scheduler
            .enqueue(
                &owner,
                crate::plugin_scheduler::Priority::Background,
                crate::plugin_scheduler::Resources { cpu_units: 1, memory_bytes: 1, ..Default::default() },
            )
            .unwrap()
            .wait(&crate::jobs::JobCtx::new())
            .unwrap();
        let crate::jobs::Started::Job(id) = s.start("extension.invoke", json!({"id":ID,"event":"refresh"})).unwrap() else { panic!("expected task") };
        let crate::jobs::Started::Job(queued) = s.start("plugin.runTask", json!({"id":ID})).unwrap() else { panic!("expected queued task") };
        assert!(s.job(id).unwrap().document.is_none());
        assert!(s.job(queued).unwrap().document.is_none());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while ![id, queued].into_iter().all(|id| s.job(id).unwrap().message.contains("Waiting for plugin resource admission")) {
            assert!(std::time::Instant::now() < deadline, "both requests must wait behind their owner's active admission");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(s.jobs().len(), 2, "a second request queues instead of failing an Arc-count busy check");
        assert!(s.cancel_job(queued));
        assert!(matches!(s.wait_job(queued), Err(EngineError::Cancelled)));
        drop(permit);
        let failure = s.wait_job(id).unwrap_err().to_string();
        assert!(failure.contains("Could not start"), "the original worker failure remains visible: {failure}");
        s.join_cancelled_jobs();
        assert!(s.documents().is_empty());
        assert_eq!(s.extensions.installed[ID].state["runs"], 0);
        assert!(!s.has_jobs());
    }
}
