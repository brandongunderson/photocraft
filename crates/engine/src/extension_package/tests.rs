use super::*;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
use crate::Session;
use photocraft_format::zip::ZipWriter;
use serde_json::json;

fn storage_variant(id: &str, name: u8) -> Vec<u8> {
    let mut bytes = guest(id, false);
    bytes.extend_from_slice(&[0, 2, 1, name]); // A distinct, valid empty WASM custom section.
    bytes
}

#[test]
fn reviewed_publication_rejects_cross_process_changes_and_identical_reinstallation() {
    let temporary = tempfile::tempdir().unwrap();
    let root = root(temporary.path()).unwrap();
    let id = "org.example.reviewed-update";
    let a = storage_variant(id, b'a');
    let b = storage_variant(id, b'b');
    let absent = review_guard(&root, id).unwrap();
    install_checked(&root, id, &a, None, false, Some(&absent)).unwrap();
    let reviewed = review_guard(&root, id).unwrap();
    install(&root, id, &a, None, true).unwrap();
    assert!(install_checked(&root, id, &b, None, true, Some(&reviewed)).err().unwrap().to_string().contains("changed since review"));
    assert_eq!(load(&root, id).unwrap().module, a);
    let other = tempfile::tempdir().unwrap();
    let other_root = super::root(other.path()).unwrap();
    install(&other_root, id, &a, None, false).unwrap();
    install(&other_root, id, &a, None, true).unwrap();
    let current = review_guard(&root, id).unwrap();
    let other_guard = review_guard(&other_root, id).unwrap();
    assert_eq!(current.expected_installed_hash, other_guard.expected_installed_hash);
    assert_eq!(current.expected_activation, other_guard.expected_activation);
    assert_ne!(current.expected_store_hash, other_guard.expected_store_hash);
    assert!(install_checked(&other_root, id, &b, None, true, Some(&current)).is_err(), "review is bound to its store");
    let reviewed = review_guard(&root, id).unwrap();
    install(&root, id, &b, None, true).unwrap();
    assert!(install_checked(&root, id, &a, None, true, Some(&reviewed)).is_err());
    let reviewed = review_guard(&root, id).unwrap();
    remove(&root, id).unwrap();
    assert!(install_checked(&root, id, &a, None, true, Some(&reviewed)).is_err());
    assert!(!record_path(&root, id).unwrap().exists());
}

#[test]
fn failed_active_publication_preserves_the_previous_rollback_and_managed_data() {
    let temporary = tempfile::tempdir().unwrap();
    let root = root(temporary.path()).unwrap();
    let id = "org.example.rollback-write";
    let b = storage_variant(id, b'b');
    let a = storage_variant(id, b'a');
    let c = storage_variant(id, b'c');
    install(&root, id, &b, None, false).unwrap();
    let before_b = read_record(&root, id).unwrap().unwrap();
    let state_b = managed::paths(&root, id, &before_b.hash).unwrap();
    std::fs::write(state_b.data.join("durable.bin"), b"previous state").unwrap();
    install(&root, id, &a, None, true).unwrap();
    let before_a = read_record(&root, id).unwrap().unwrap();
    let state_a = managed::paths(&root, id, &before_a.hash).unwrap();
    std::fs::write(state_a.data.join("durable.bin"), b"current state").unwrap();
    let failure = install_with_publication(&root, id, &c, None, true, |_, _| {
        assert!(rollback_journal_path(&root, id).unwrap().is_file());
        assert_eq!(rollback_record(&root, id, Some(&before_a.hash)).unwrap().unwrap(), before_b);
        Err(error("injected active-record write failure"))
    });
    assert!(failure.err().unwrap().to_string().contains("injected active-record write failure"));
    assert_eq!(read_record(&root, id).unwrap().unwrap(), before_a);
    assert_eq!(rollback_record(&root, id, Some(&before_a.hash)).unwrap().unwrap(), before_b);
    assert_eq!(std::fs::read(state_a.data.join("durable.bin")).unwrap(), b"current state");
    assert_eq!(std::fs::read(state_b.data.join("durable.bin")).unwrap(), b"previous state");
    // Restart resolution ignores a prepared journal whose target was never selected.
    assert_eq!(discover(&root, id).unwrap().extension.manifest.id, id);
    rollback(&root, id).unwrap();
    assert_eq!(load(&root, id).unwrap().module, b);
    assert_eq!(std::fs::read(state_b.data.join("durable.bin")).unwrap(), b"previous state");
}

#[test]
fn committed_journal_preserves_rollback_when_final_metadata_write_fails() {
    let temporary = tempfile::tempdir().unwrap();
    let root = root(temporary.path()).unwrap();
    let id = "org.example.rollback-finalize";
    let b = storage_variant(id, b'b');
    let a = storage_variant(id, b'a');
    let c = storage_variant(id, b'c');
    install(&root, id, &b, None, false).unwrap();
    install(&root, id, &a, None, true).unwrap();
    let previous = read_record(&root, id).unwrap().unwrap();
    let state = managed::paths(&root, id, &previous.hash).unwrap();
    std::fs::write(state.data.join("durable.bin"), b"rollback pair").unwrap();
    let backup = root.join(format!("{id}.rollback.json"));
    let saved = root.join("test-saved-rollback.json");
    let installed = install_with_publication(&root, id, &c, None, true, |path, bytes| {
        std::fs::rename(&backup, &saved).unwrap();
        std::fs::create_dir(&backup).unwrap(); // Deterministically fail only finalization.
        photocraft_format::atomic_write(path, bytes).map_err(error)
    })
    .unwrap();
    assert!(installed.warning.unwrap().contains("rollback metadata"));
    let selected = read_record(&root, id).unwrap().unwrap();
    assert_ne!(selected.hash, previous.hash);
    assert!(rollback_journal_path(&root, id).unwrap().is_file());
    assert_eq!(rollback_record(&root, id, Some(&selected.hash)).unwrap().unwrap(), previous);
    assert_eq!(discover(&root, id).unwrap().extension.manifest.id, id);
    std::fs::remove_dir(&backup).unwrap();
    rollback(&root, id).unwrap();
    assert_eq!(load(&root, id).unwrap().module, a);
    assert_eq!(std::fs::read(state.data.join("durable.bin")).unwrap(), b"rollback pair");
    assert!(!rollback_journal_path(&root, id).unwrap().exists());
}

#[test]
fn native_admission_holds_publication_but_execution_only_leases_its_generation() {
    if host_platform() == "unsupported" {
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let root = root(temporary.path()).unwrap();
    let id = "org.example.admission";
    let info = v2_info(true, &[]);
    let bytes = v2_package(id, &info, &[(info.native.as_ref().unwrap().entry.as_str(), b"owned worker")]);
    let inspected = inspect(&bytes).unwrap();
    let native = install(&root, id, &bytes, Some(&inspected), false).unwrap().native.unwrap();
    let execution = native.lease().unwrap();
    assert!(StoreGuard::write(&root).is_ok(), "long jobs do not hold publication locks");
    let admission = native.admission().unwrap();
    assert!(StoreGuard::write(&root).is_err(), "disable cannot race a final result/state commit");
    drop(admission);
    let mutation = StoreGuard::write(&root).unwrap();
    revoke_activation(&root, id).unwrap();
    drop(mutation);
    assert!(native.admission().is_err(), "a later admission sees the revoked epoch");
    drop(execution);
}

#[test]
fn deferred_filter_watch_cancels_after_an_external_epoch_change() {
    let temporary = tempfile::tempdir().unwrap();
    let root = root(temporary.path()).unwrap();
    let id = "org.example.filter-watch";
    let bytes = guest(id, false);
    install(&root, id, &bytes, None, false).unwrap();
    let deferred = discover(&root, id).unwrap().deferred.unwrap();
    let context = crate::jobs::JobCtx::new();
    let _watch = deferred.watch(&context).unwrap();
    {
        let _mutation = StoreGuard::write(&root).unwrap();
        revoke_activation(&root, id).unwrap();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !context.cancelled() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(context.cancelled(), "deferred verification/evaluation must observe external revocation");
    assert!(deferred.activate(&context).is_err());
}

#[test]
fn store_readers_prevent_mutation_without_changing_the_active_record() {
    let temporary = tempfile::tempdir().unwrap();
    let root = root(temporary.path()).unwrap();
    let id = "org.example.store";
    let bytes = guest(id, false);
    install(&root, id, &bytes, None, false).unwrap();
    let original = std::fs::read(record_path(&root, id).unwrap()).unwrap();
    let reader = StoreGuard::read(&root).unwrap();
    assert!(StoreGuard::read(&root).is_ok());
    assert!(StoreGuard::write(&root).is_err());
    assert!(install(&root, id, &bytes, None, true).is_err());
    assert!(remove(&root, id).is_err());
    assert_eq!(load(&root, id).unwrap().module, bytes);
    assert_eq!(std::fs::read(record_path(&root, id).unwrap()).unwrap(), original);
    drop(reader);
    assert!(install(&root, id, &bytes, None, false).err().unwrap().to_string().contains("replace:true"));
    install(&root, id, &bytes, None, true).unwrap();
    remove(&root, id).unwrap();
    assert!(root.join(".photocraft-store.lock").is_file());
}

#[test]
#[ignore = "helper launched by the cross-process store regression"]
fn store_lock_process_helper() {
    let root = PathBuf::from(std::env::var_os("PHOTOCRAFT_STORE_LOCK_TEST_ROOT").unwrap());
    let exclusive = std::env::var_os("PHOTOCRAFT_STORE_LOCK_TEST_EXCLUSIVE").is_some();
    let _guard = if let Ok(hash) = std::env::var("PHOTOCRAFT_GENERATION_LOCK_TEST_HASH") {
        generation_lease(&root, "org.example.process-lock", &hash, exclusive).unwrap()._guard
    } else {
        if exclusive { StoreGuard::write(&root) } else { StoreGuard::read(&root) }.unwrap()
    };
    std::fs::write(root.join("ready"), b"ready").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !root.join("stop").exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(root.join("stop").is_file(), "parent did not release helper before deadline");
}

struct LockChild(std::process::Child);

impl Drop for LockChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn lock_child(root: &Path, exclusive: bool) -> LockChild {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", "extension_package::tests::store_lock_process_helper"])
        .env("PHOTOCRAFT_STORE_LOCK_TEST_ROOT", root)
        .stdout(std::process::Stdio::null());
    if exclusive {
        command.env("PHOTOCRAFT_STORE_LOCK_TEST_EXCLUSIVE", "1");
    } else {
        command.env_remove("PHOTOCRAFT_STORE_LOCK_TEST_EXCLUSIVE");
    }
    let mut child = LockChild(command.spawn().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !root.join("ready").exists() {
        assert!(child.0.try_wait().unwrap().is_none(), "store lock helper exited early");
        assert!(std::time::Instant::now() < deadline, "store lock helper did not start");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    child
}

#[test]
fn store_locks_coordinate_separate_processes_and_release_after_process_exit() {
    let temporary = tempfile::tempdir().unwrap();
    let root = root(temporary.path()).unwrap();
    let id = "org.example.process-lock";
    let bytes = guest(id, false);
    install(&root, id, &bytes, None, false).unwrap();
    let original = std::fs::read(record_path(&root, id).unwrap()).unwrap();
    let child = lock_child(&root, false);
    assert!(StoreGuard::read(&root).is_ok());
    assert!(install(&root, id, &bytes, None, true).is_err());
    assert!(remove(&root, id).is_err());
    assert_eq!(std::fs::read(record_path(&root, id).unwrap()).unwrap(), original);
    drop(child); // Abrupt process termination must release the OS lease.
    assert!(StoreGuard::write(&root).is_ok());
    std::fs::remove_file(root.join("ready")).unwrap();
    let mut child = lock_child(&root, true);
    assert!(StoreGuard::read(&root).is_err());
    assert!(StoreGuard::write(&root).is_err());
    assert!(load(&root, id).is_err());
    std::fs::write(root.join("stop"), b"stop").unwrap();
    assert!(child.0.wait().unwrap().success());
    assert_eq!(load(&root, id).unwrap().module, bytes);
    install(&root, id, &bytes, None, true).unwrap();
}

#[test]
fn store_lock_rejects_non_regular_paths() {
    let temporary = tempfile::tempdir().unwrap();
    std::fs::create_dir(temporary.path().join(".photocraft-store.lock")).unwrap();
    assert!(StoreGuard::read(temporary.path()).is_err());
    assert!(StoreGuard::write(temporary.path()).is_err());
}

#[test]
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn managed_native_leases_reject_changed_removed_or_disabled_installations() {
    let temporary = tempfile::tempdir().unwrap();
    let root = root(temporary.path()).unwrap();
    let id = "org.example.generation";
    let first = package(id, true, b"first", &[]);
    let inspected = inspect(&first).unwrap();
    let native = install(&root, id, &first, Some(&inspected), false).unwrap().native.unwrap();
    let lease = native.lease().unwrap();
    let warning = remove(&root, id).unwrap();
    assert!(warning.is_some());
    assert!(native.worker.is_file(), "a leased generation must not be deleted");
    drop(lease);
    let second = package(id, true, b"second", &[]);
    let inspected = inspect(&second).unwrap();
    let current = install(&root, id, &second, Some(&inspected), true).unwrap().native.unwrap();
    assert!(native.lease().is_err());
    assert!(current.lease().is_ok());
    {
        let _store = StoreGuard::write(&root).unwrap();
        std::fs::write(root.join(format!("{id}.disabled")), b"").unwrap();
    }
    assert!(current.lease().is_err());
    let installed = install(&root, id, &second, Some(&inspected), true).unwrap();
    assert!(!installed.enabled);
    remove(&root, id).unwrap();
    assert!(!root.join(format!("{id}.disabled")).exists());
    assert!(current.lease().is_err());
}

#[test]
fn a_thousand_dormant_descriptors_do_not_open_payloads_or_start_workers() {
    let temporary = tempfile::tempdir().unwrap();
    let root = root(temporary.path()).unwrap();
    let id = "org.example.dormant";
    let bytes = guest(id, false);
    install(&root, id, &bytes, None, false).unwrap();
    let template = read_record(&root, id).unwrap().unwrap();
    remove(&root, id).unwrap();
    for index in 0..1000 {
        let mut record = template.clone();
        record.id = format!("org.example.dormant-{index}");
        record.descriptor.as_mut().unwrap().id = record.id.clone();
        let directory = generation(&root, &record).unwrap();
        std::fs::create_dir_all(&directory).unwrap();
        let bytes = serde_json::to_vec(&record).unwrap();
        std::fs::write(directory.join(OWNERSHIP), &bytes).unwrap();
        std::fs::write(record_path(&root, &record.id).unwrap(), &bytes).unwrap();
        // Deliberately no WASM or native payload. Discovery must not touch either.
    }
    let mut host = crate::extension_cmds::ExtensionHost::default();
    host.attach_folder(root.clone()).unwrap();
    assert!(host.load_errors.is_empty(), "{:?}", host.load_errors);
    assert_eq!(host.installed.len(), 1000);
    assert!(host.installed.values().all(|installed| installed.extension.lifecycle() == "idle"));
    let extension = &host.installed["org.example.dormant-0"].extension;
    for _ in 0..3 {
        assert!(extension.activate().is_err());
    }
    assert_eq!(extension.lifecycle(), "failed");
    extension.retry_activation();
    assert_eq!(extension.lifecycle(), "idle");
}

#[test]
fn independent_generation_leases_preserve_in_use_bytes_and_allow_other_updates() {
    let temporary = tempfile::tempdir().unwrap();
    let root = root(temporary.path()).unwrap();
    let id = "org.example.process-lock";
    let bytes = guest(id, false);
    install(&root, id, &bytes, None, false).unwrap();
    let first = read_record(&root, id).unwrap().unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", "extension_package::tests::store_lock_process_helper"])
        .env("PHOTOCRAFT_STORE_LOCK_TEST_ROOT", &root)
        .env("PHOTOCRAFT_GENERATION_LOCK_TEST_HASH", &first.hash)
        .stdout(std::process::Stdio::null());
    let mut child = LockChild(command.spawn().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !root.join("ready").exists() {
        assert!(child.0.try_wait().unwrap().is_none());
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let other_id = "org.example.independent";
    install(&root, other_id, &guest(other_id, false), None, false).unwrap();
    assert!(install(&root, id, &guest(id, true), None, true).is_err());
    assert!(generation(&root, &first).unwrap().join("plugin.pcext").is_file());
    assert!(cleanup(&root, &first).is_err());
    drop(child);
    install(&root, id, &guest(id, true), None, true).unwrap();
    cleanup(&root, &first).unwrap();
    assert!(!generation(&root, &first).unwrap().exists());
}

#[test]
fn rollback_restores_code_and_managed_data_and_failed_preparation_keeps_active_pair() {
    let temporary = tempfile::tempdir().unwrap();
    let root = root(temporary.path()).unwrap();
    let id = "org.example.rollback";
    let first_bytes = guest(id, false);
    install(&root, id, &first_bytes, None, false).unwrap();
    let first = read_record(&root, id).unwrap().unwrap();
    let first_state = managed::paths(&root, id, &first.hash).unwrap();
    std::fs::write(&first_state.settings, br#"{"value":"before"}"#).unwrap();
    let second_bytes = guest(id, true);
    install(&root, id, &second_bytes, None, true).unwrap();
    let second = read_record(&root, id).unwrap().unwrap();
    let second_state = managed::paths(&root, id, &second.hash).unwrap();
    std::fs::write(&second_state.settings, br#"{"value":"after"}"#).unwrap();
    rollback(&root, id).unwrap();
    assert_eq!(load(&root, id).unwrap().module, first_bytes);
    assert_eq!(std::fs::read(&first_state.settings).unwrap(), br#"{"value":"before"}"#);
    rollback(&root, id).unwrap();
    assert_eq!(load(&root, id).unwrap().module, second_bytes);
    let third_bytes = guest_for(id, "edit.undo");
    let third_hash = blake3::hash(&third_bytes).to_hex().to_string();
    let collision = checked_path(&root, &format!("managed/{id}/{third_hash}")).unwrap();
    std::fs::write(&collision, b"unexpected existing file").unwrap();
    assert!(install(&root, id, &third_bytes, None, true).is_err());
    assert_eq!(read_record(&root, id).unwrap().unwrap(), second);
    assert_eq!(std::fs::read(&second_state.settings).unwrap(), br#"{"value":"after"}"#);
}

#[test]
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn native_preview_apply_rejects_store_changes_from_another_session() {
    for action in ["disable", "disable-enable", "remove", "replace", "reinstall"] {
        let temporary = tempfile::tempdir().unwrap();
        let root = root(temporary.path()).unwrap();
        let id = "org.example.preview-store";
        let input = temporary.path().join("input.pcplugin");
        std::fs::write(&input, package(id, true, b"first", &[])).unwrap();
        let mut first = Session::new();
        first.extensions.attach_folder(root.clone()).unwrap();
        first.execute("extension.install", json!({"path":input,"allowNative":true})).unwrap();
        first.execute("file.new", json!({"width":8,"height":6})).unwrap();
        let source = Arc::clone(&first.active().unwrap().doc);
        let revision = first.active().unwrap().revision;
        let native = Arc::clone(first.extensions.installed[id].native.as_ref().unwrap());
        let binding = crate::plugin_preview::PreviewBinding::for_native(&first, &native, None).unwrap();
        let token = crate::plugin_preview::reserve(&mut first, source.id, &binding).unwrap();
        crate::plugin_preview::stage(&mut first, token, Arc::clone(&source), revision, (*source).clone(), binding).unwrap();
        let mut other = Session::new();
        other.extensions.attach_folder(root.clone()).unwrap();
        match action {
            "disable" => {
                other.execute("extension.enable", json!({"id":id,"enabled":false})).unwrap();
            }
            "disable-enable" => {
                other.execute("extension.enable", json!({"id":id,"enabled":false})).unwrap();
                other.execute("extension.enable", json!({"id":id,"enabled":true})).unwrap();
                assert!(other.extensions.installed[id].native.as_ref().unwrap().lease().is_ok());
            }
            "remove" => {
                other.execute("extension.remove", json!({"id":id})).unwrap();
            }
            "replace" => {
                std::fs::write(&input, package(id, true, b"second", &[])).unwrap();
                other.execute("extension.install", json!({"path":input,"allowNative":true,"replace":true})).unwrap();
            }
            "reinstall" => {
                other.execute("extension.remove", json!({"id":id})).unwrap();
                other.execute("extension.install", json!({"path":input,"allowNative":true})).unwrap();
                assert!(other.extensions.installed[id].native.as_ref().unwrap().lease().is_ok());
            }
            _ => panic!("unknown management action: {action}"),
        }
        assert!(first.execute("image.pluginPreview.apply", json!({"preview":token})).is_err(), "{action}");
        assert_eq!(first.documents().len(), 1);
        assert!(Arc::ptr_eq(&source, &first.active().unwrap().doc));
        assert!(first.plugin_preview(token).is_none());
    }
}

#[test]
fn activation_metadata_is_bounded_and_cannot_wrap_or_be_silently_reset() {
    let temporary = tempfile::tempdir().unwrap();
    let root = root(temporary.path()).unwrap();
    let id = "org.example.activation";
    let _store = StoreGuard::write(&root).unwrap();
    assert_eq!(activation(&root, id).unwrap(), 0);
    assert_eq!(revoke_activation(&root, id).unwrap(), 1);
    let path = root.join(format!("{id}.activation.json"));
    for invalid in ["x".repeat(33), "null".into(), "-1".into(), u64::MAX.to_string()] {
        std::fs::write(&path, &invalid).unwrap();
        assert!(revoke_activation(&root, id).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
    }
}

fn guest(id: &str, native: bool) -> Vec<u8> {
    guest_for(id, if native { "plugin.processImage" } else { "" })
}

#[test]
fn corrupt_activation_metadata_never_triggers_owned_payload_repair() {
    let temporary = tempfile::tempdir().unwrap();
    let root = root(temporary.path()).unwrap();
    let id = "org.example.activation-repair";
    let bytes = guest(id, false);
    install(&root, id, &bytes, None, false).unwrap();
    let record = read_record(&root, id).unwrap().unwrap();
    let payload = generation(&root, &record).unwrap().join("plugin.pcext");
    {
        let _store = StoreGuard::write(&root).unwrap();
        std::fs::write(root.join(format!("{id}.activation.json")), b"!").unwrap();
    }
    assert!(install(&root, id, &bytes, None, true).is_err());
    assert!(remove(&root, id).is_err());
    assert_eq!(read_record(&root, id).unwrap().unwrap(), record);
    assert_eq!(std::fs::read(payload).unwrap(), bytes);
}

fn guest_for(id: &str, service: &str) -> Vec<u8> {
    let manifest = serde_json::to_string(&json!({
        "apiVersion":2,"id":id,"name":"Package test","version":"1",
        "capabilities":{"required":[{"id":"host.commands.invoke","minVersion":1}]},"permissions":{
            "commands":if service.is_empty() {vec![]} else {vec![service]}},
        "panel":{"buttons":[{"id":"run","label":"Run"}]}
    }))
    .unwrap();
    let escaped = manifest.bytes().map(|b| format!("\\{b:02x}")).collect::<String>();
    wat::parse_str(format!(
        r#"(module
        (memory (export "memory") 1)
        (data (i32.const 16) "{escaped}")
        (data (i32.const 2048) "{{}}")
        (func (export "pce_api_version") (result i32) i32.const 2)
        (func (export "pce_manifest") (result i64) i64.const {})
        (func (export "pce_alloc") (param i32) (result i32) i32.const 4096)
        (func (export "pce_handle") (param i32 i32) (result i64) i64.const {}))"#,
        ((manifest.len() as u64) << 32) | 16,
        (2u64 << 32) | 2048
    ))
    .unwrap()
}

fn package(id: &str, native_grant: bool, tag: &[u8], extra: &[(&str, &[u8])]) -> Vec<u8> {
    package_for(id, "plugin.processImage", if native_grant { "plugin.processImage" } else { "" }, tag, extra)
}

fn package_for(id: &str, service: &str, grant: &str, tag: &[u8], extra: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = ZipWriter::new();
    let descriptor = serde_json::to_vec(&json!({"packageVersion":2,"module":"plugin.pcext","native":{
        "service":service,"platform":"windows-x86_64","entry":"bin/processor.exe"}}))
    .unwrap();
    let module = guest_for(id, grant);
    for name in [
        "package.json",
        "plugin.pcext",
        "bin/processor.exe",
        "runtime/models/weights.bin",
        "runtime/models/a.bin",
        "runtime/models/b.bin",
        "runtime/models/c.bin",
        "runtime/models/d.bin",
        "runtime/models/e.bin",
        "LICENSE",
        "THIRD-PARTY-NOTICES.md",
    ] {
        zip.add(
            name,
            match name {
                "package.json" => &descriptor,
                "plugin.pcext" => &module,
                _ => tag,
            },
        )
        .unwrap();
    }
    for (path, bytes) in extra {
        zip.add(path, bytes).unwrap();
    }
    zip.finish().unwrap()
}

#[test]
fn storage_size_counts_model_directory_as_a_subset_and_rejects_overflow() {
    let record = |path: &str, size| FileRecord { path: path.into(), size, hash: "a".repeat(64) };
    let files = [
        record("plugin.pcext", 13),
        record("runtime/models/model.bin", 100),
        record("RUNTIME/MODELS/model.param", 10),
        record("runtime/models-extra/model.bin", 7),
        record("models/model.onnx", 3),
    ];
    let size = storage_size(&files).unwrap();
    assert_eq!(size.installed_bytes, 133);
    assert_eq!(size.model_bytes, 110);
    assert!(storage_size(&[record("runtime/models/one.bin", u64::MAX), record("runtime/models/two.bin", 1)]).is_err());
}

#[test]
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn package_storage_tracks_payload_and_models_across_inspect_install_and_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Extensions");
    let path = tmp.path().join("bundle.pcplugin");
    let id = "org.example.bundle";
    let tag = b"model asset";
    let extra: &[(&str, &[u8])] = &[("runtime/models/extra.param", b"param"), ("runtime/models-extra/non-model.txt", b"other")];
    let bytes = package(id, true, tag, extra);
    let zip = ZipReader::new(&bytes).unwrap();
    let expected_installed = zip.entries.iter().map(|entry| zip.read(entry, MAX_FILE).unwrap().len() as u64).sum::<u64>();
    let expected = crate::extension_cmds::StorageSize { installed_bytes: expected_installed, model_bytes: 6 * tag.len() as u64 + 5 };
    assert_eq!(inspect(&bytes).unwrap().storage, expected);

    std::fs::write(&path, &bytes).unwrap();
    let mut session = Session::new();
    session.extensions.attach_folder(folder.clone()).unwrap();
    let review = session.execute("extension.inspect", json!({"path":path})).unwrap();
    assert_eq!(review["downloadBytes"], bytes.len());
    assert_eq!(review["storage"], serde_json::to_value(expected).unwrap());
    let installed = session.execute("extension.install", json!({"path":path,"allowNative":true})).unwrap();
    assert_eq!(installed["storage"], review["storage"]);
    assert_eq!(session.extensions.installed[id].storage, expected);
    let mut restarted = Session::new();
    restarted.extensions.attach_folder(folder.clone()).unwrap();
    assert!(restarted.extensions.load_errors.is_empty());
    assert_eq!(restarted.extensions.installed[id].storage, expected);
    let listed = restarted.execute("extension.list", json!({})).unwrap();
    assert_eq!(listed["extensions"][0]["storage"], review["storage"]);

    // Existing records need no schema migration: derive totals from their verified file list.
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(folder.join(format!("{id}{RECORD_SUFFIX}"))).unwrap()).unwrap();
    assert!(record.get("storage").is_none());
    let updated = package(id, true, b"a larger model asset", extra);
    std::fs::write(&path, &updated).unwrap();
    restarted.execute("extension.install", json!({"path":path,"allowNative":true,"replace":true})).unwrap();
    let updated_size = inspect(&updated).unwrap().storage;
    assert!(updated_size.installed_bytes > expected.installed_bytes);
    assert!(updated_size.model_bytes > expected.model_bytes);
    assert_eq!(restarted.extensions.installed[id].storage, updated_size);
    let mut final_session = Session::new();
    final_session.extensions.attach_folder(folder).unwrap();
    assert_eq!(final_session.extensions.installed[id].storage, updated_size);
}

#[test]
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn native_packages_coexist_and_keep_their_own_payloads() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Extensions");
    let path = tmp.path().join("bundle.pcplugin");
    let mut session = Session::new();
    session.extensions.attach_folder(folder.clone()).unwrap();
    for id in ["org.example.first", "org.example.second"] {
        std::fs::write(&path, package(id, true, id.as_bytes(), &[])).unwrap();
        session.execute("extension.install", json!({"path":path,"allowNative":true})).unwrap();
    }
    let mut restarted = Session::new();
    restarted.extensions.attach_folder(folder).unwrap();
    assert!(restarted.extensions.load_errors.is_empty());
    assert_eq!(restarted.extensions.installed.len(), 2);
    for id in ["org.example.first", "org.example.second"] {
        let worker = &restarted.extensions.installed[id].native.as_ref().unwrap().worker;
        assert_eq!(std::fs::read(worker).unwrap(), id.as_bytes());
    }
    restarted.execute("extension.remove", json!({"id":"org.example.second"})).unwrap();
    assert!(restarted.extensions.installed["org.example.first"].native.as_ref().unwrap().worker.is_file());
    // An editing grant cannot authorize the package's native processor.
    std::fs::write(&path, package_for("org.example.bad", "plugin.processImage", "image.adjustments.invert", b"asset", &[])).unwrap();
    assert!(restarted.execute("extension.install", json!({"path":path,"allowNative":true})).is_err());
    assert!(!restarted.extensions.installed.contains_key("org.example.bad"));
}

#[test]
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn generic_package_validates_declared_worker_and_all_recorded_assets() {
    let inspected = inspect(&package("org.example.processor", true, b"asset", &[])).unwrap();
    assert!(required_files(&inspected.info).unwrap().contains(&"bin/processor.exe"));
    let mut record = Record {
        descriptor: None,
        format_version: 1,
        id: "org.example.processor".into(),
        hash: "a".repeat(64),
        package: Some(inspected.info),
        files: inspected.files,
    };
    assert!(validate_record(&record, &record.id).is_ok());
    record.files.iter_mut().find(|f| f.path == "runtime/models/weights.bin").unwrap().size = 176_000_000;
    assert!(validate_record(&record, &record.id).is_ok());
    record.files.retain(|f| f.path != "bin/processor.exe");
    assert!(validate_record(&record, &record.id).is_err());
}

#[test]
fn rejects_unsafe_archive_paths_duplicates_and_collisions() {
    for path in ["../escape", "/absolute", "C:/drive", "a\\b", "a:b", "a/../b", "a//b", "a/./b", "CON.txt", "LPT1", "a./b", "a /b", OWNERSHIP] {
        assert!(safe_name(path).is_err(), "{path}");
        assert!(inspect(&package("org.example.bundle", true, b"asset", &[(path, b"attack")])).is_err(), "{path}");
    }
    for extra in [vec![("LICENSE", b"duplicate".as_slice())], vec![("license", b"alias".as_slice())], vec![("runtime", b"collision".as_slice())]] {
        assert!(inspect(&package("org.example.bundle", true, b"asset", &extra)).is_err());
    }
}

#[test]
fn rejects_corrupt_and_oversized_archives_without_extraction() {
    let mut bytes = package("org.example.bundle", true, b"asset", &[]);
    bytes[44] ^= 1;
    assert!(inspect(&bytes).is_err());
    let mut zip = ZipWriter::new();
    zip.add("package.json", b"{}").unwrap();
    let bytes = zip.finish().unwrap();
    assert!(inspect(&bytes).is_err());
    let mut record = Record {
        descriptor: None,
        format_version: 1,
        id: "org.example.bundle".into(),
        hash: "a".repeat(64),
        package: None,
        files: vec![FileRecord { path: "plugin.pcext".into(), size: (MAX_FILE + 1) as u64, hash: "a".repeat(64) }],
    };
    assert!(validate_record(&record, &record.id).is_err());
    record.files[0].size = 1;
    record.files[0].path = "../outside".into();
    assert!(validate_record(&record, &record.id).is_err());
}

#[test]
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn native_grant_hash_persistence_updates_and_uninstall_are_one_installation() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Extensions");
    let path = tmp.path().join("bundle.pcplugin");
    let id = "org.example.bundle";
    std::fs::write(&path, package(id, true, b"first", &[])).unwrap();
    let mut s = Session::new();
    s.extensions.attach_folder(folder.clone()).unwrap();
    let review = s.execute("extension.inspect", json!({"path":path})).unwrap();
    assert_eq!(review["package"]["native"]["service"], "plugin.processImage");
    assert!(s.execute("extension.install", json!({"path":path})).is_err());
    assert!(s.extensions.installed.is_empty());
    assert_eq!(
        std::fs::read_dir(&folder).unwrap().map(|entry| entry.unwrap().file_name()).collect::<Vec<_>>(),
        vec![std::ffi::OsString::from(".photocraft-store.lock")]
    );
    s.execute("extension.install", json!({"path":path,"allowNative":true,"expectedHash":review["hash"]})).unwrap();
    let old_worker = s.extensions.installed[id].native.as_ref().unwrap().worker.clone();
    assert_eq!(std::fs::read(&old_worker).unwrap(), b"first");
    let mut next = Session::new();
    next.extensions.attach_folder(folder.clone()).unwrap();
    assert_eq!(next.extensions.installed[id].native.as_ref().unwrap().worker, old_worker);
    std::fs::write(&path, package(id, true, b"second", &[])).unwrap();
    assert!(next.execute("extension.install", json!({"path":path,"replace":true,"allowNative":true,"expectedHash":review["hash"]})).is_err());
    assert!(old_worker.is_file());
    let lease = next.extensions.installed[id].native.as_ref().unwrap().lease().unwrap();
    assert!(next.execute("extension.install", json!({"path":path,"replace":true,"allowNative":true})).is_err());
    assert!(old_worker.is_file());
    drop(lease);
    next.execute("extension.install", json!({"path":path,"replace":true,"allowNative":true})).unwrap();
    let current = next.extensions.installed[id].native.as_ref().unwrap().worker.clone();
    assert!(old_worker.exists(), "the previous generation is retained for rollback");
    assert_eq!(std::fs::read(&current).unwrap(), b"second");
    next.execute("extension.enable", json!({"id":id,"enabled":false})).unwrap();
    let mut final_session = Session::new();
    final_session.extensions.attach_folder(folder.clone()).unwrap();
    assert!(!final_session.extensions.installed[id].enabled);
    final_session.execute("extension.remove", json!({"id":id})).unwrap();
    assert!(!current.exists());
    let mut after = Session::new();
    after.extensions.attach_folder(folder).unwrap();
    assert!(after.extensions.installed.is_empty());
}

#[test]
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn raw_and_native_replacements_and_bad_packages_preserve_valid_state() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Extensions");
    std::fs::create_dir_all(&folder).unwrap();
    let id = "org.example.bundle";
    // Exercise migration from a legacy standalone installation.
    std::fs::write(folder.join(format!("{id}.pcext")), guest(id, false)).unwrap();
    let mut s = Session::new();
    s.extensions.attach_folder(folder.clone()).unwrap();
    assert!(s.extensions.installed[id].native.is_none());
    let path = tmp.path().join("bundle.pcplugin");
    std::fs::write(&path, package(id, false, b"invalid permission", &[])).unwrap();
    assert!(s.execute("extension.install", json!({"path":path,"replace":true,"allowNative":true})).is_err());
    assert!(s.extensions.installed[id].native.is_none());
    std::fs::write(&path, package(id, true, b"valid", &[])).unwrap();
    s.execute("extension.install", json!({"path":path,"replace":true,"allowNative":true})).unwrap();
    let worker = s.extensions.installed[id].native.as_ref().unwrap().worker.clone();
    std::fs::write(&path, guest(id, false)).unwrap();
    s.execute("extension.install", json!({"path":path,"replace":true})).unwrap();
    assert!(s.extensions.installed[id].native.is_none());
    assert!(worker.exists(), "the native generation is the rollback choice");
    let mut next = Session::new();
    next.extensions.attach_folder(folder.clone()).unwrap();
    assert!(next.extensions.installed[id].native.is_none());
    next.execute("extension.remove", json!({"id":id})).unwrap();
    assert!(!folder.join(format!("{id}.pcext")).exists());
}

#[test]
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn tampered_payload_and_bad_neighbor_records_are_reported_without_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Extensions");
    let path = tmp.path().join("bundle.pcplugin");
    let id = "org.example.bundle";
    std::fs::write(&path, package(id, true, b"valid", &[])).unwrap();
    let mut s = Session::new();
    s.extensions.attach_folder(folder.clone()).unwrap();
    s.execute("extension.install", json!({"path":path,"allowNative":true})).unwrap();
    std::fs::write(folder.join("BAD.installation.json"), b"{}").unwrap();
    let mut next = Session::new();
    next.extensions.attach_folder(folder.clone()).unwrap();
    assert!(next.extensions.installed.contains_key(id));
    assert_eq!(next.extensions.load_errors.len(), 1);
    let worker = &s.extensions.installed[id].native.as_ref().unwrap().worker;
    std::fs::write(worker, b"tampered").unwrap();
    std::fs::write(folder.join(format!("{id}.pcext")), guest(id, false)).unwrap();
    let mut broken = Session::new();
    broken.extensions.attach_folder(folder.clone()).unwrap();
    assert!(broken.extensions.installed.contains_key(id));
    crate::extension_cmds::activate(&mut broken, id).unwrap();
    assert!(broken.extensions.installed[id].native.as_ref().unwrap().verify_payload(&crate::jobs::JobCtx::new()).is_err());
    assert_eq!(broken.extensions.load_errors.len(), 1);
    broken.execute("extension.install", json!({"path":path,"allowNative":true})).unwrap();
    assert_eq!(std::fs::read(&broken.extensions.installed[id].native.as_ref().unwrap().worker).unwrap(), b"valid");
    let mut repaired = Session::new();
    repaired.extensions.attach_folder(folder).unwrap();
    assert!(repaired.extensions.installed.contains_key(id));
}

#[test]
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn reinstall_repairs_an_entire_missing_owned_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Extensions");
    let path = tmp.path().join("bundle.pcplugin");
    let id = "org.example.bundle";
    std::fs::write(&path, package(id, true, b"model data", &[])).unwrap();
    let mut s = Session::new();
    s.extensions.attach_folder(folder.clone()).unwrap();
    s.execute("extension.install", json!({"path":path,"allowNative":true})).unwrap();
    let models = s.extensions.installed[id].native.as_ref().unwrap().worker.parent().unwrap().parent().unwrap().join("runtime/models");
    assert!(models.starts_with(std::fs::canonicalize(tmp.path()).unwrap()));
    std::fs::remove_dir_all(&models).unwrap();
    let mut restarted = Session::new();
    restarted.extensions.attach_folder(folder.clone()).unwrap();
    assert!(restarted.extensions.installed.contains_key(id));
    crate::extension_cmds::activate(&mut restarted, id).unwrap();
    assert!(restarted.extensions.installed[id].native.as_ref().unwrap().verify_payload(&crate::jobs::JobCtx::new()).is_err());
    restarted.execute("extension.install", json!({"path":path,"allowNative":true})).unwrap();
    for model in ["weights", "a", "b", "c", "d", "e"] {
        assert_eq!(std::fs::read(models.join(format!("{model}.bin"))).unwrap(), b"model data");
    }
    let mut verified = Session::new();
    verified.extensions.attach_folder(folder).unwrap();
    assert!(verified.extensions.installed.contains_key(id));
    assert!(verified.extensions.load_errors.is_empty());
}

#[test]
fn managed_paths_refuse_symlinks_and_never_delete_external_data() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("keep"), b"keep").unwrap();
    let root = root(&store).unwrap();
    #[cfg(windows)]
    let link = std::os::windows::fs::symlink_dir(&outside, root.join("bundles"));
    #[cfg(not(windows))]
    let link = std::os::unix::fs::symlink(&outside, root.join("bundles"));
    if link.is_err() {
        return;
    }
    assert!(checked_path(&root, "bundles/org.example.bundle").is_err());
    assert!(install(&root, "org.example.bundle", &guest("org.example.bundle", false), None, false).is_err());
    assert_eq!(std::fs::read(outside.join("keep")).unwrap(), b"keep");
}

#[test]
fn corrupt_active_metadata_requires_explicit_replacement_and_preserves_previous_data() {
    let tmp = tempfile::tempdir().unwrap();
    let root = root(tmp.path()).unwrap();
    let id = "org.example.repair";
    let original = guest(id, false);
    install(&root, id, &original, None, false).unwrap();
    let previous = read_record(&root, id).unwrap().unwrap();
    let previous_dir = generation(&root, &previous).unwrap();
    let unrelated = root.join("unrelated-user-data.txt");
    std::fs::write(&unrelated, b"keep me").unwrap();
    std::fs::write(previous_dir.join("untracked-user-data.txt"), b"keep old data").unwrap();

    // Startup rejects corrupt metadata without registering any fallback installation.
    let damaged = b"{ malformed metadata";
    let active = record_path(&root, id).unwrap();
    std::fs::write(&active, damaged).unwrap();
    assert!(load(&root, id).is_err());
    let mut replacement = original.clone();
    replacement.extend_from_slice(&[0, 2, 1, b'x']); // Valid neutral custom WASM section.
    photocraft_plugins::extension::Extension::load(&replacement).unwrap();
    let error = install(&root, id, &replacement, None, false).err().unwrap().to_string();
    assert!(error.contains("replace:true"));
    assert_eq!(std::fs::read(&active).unwrap(), damaged);

    let repaired = install(&root, id, &replacement, None, true).unwrap();
    assert!(repaired.warning.unwrap().contains("Repaired damaged plugin metadata"));
    assert_eq!(load(&root, id).unwrap().module, replacement);
    assert_eq!(std::fs::read(&unrelated).unwrap(), b"keep me");
    assert_eq!(std::fs::read(previous_dir.join("plugin.pcext")).unwrap(), original);
    assert_eq!(std::fs::read(previous_dir.join("untracked-user-data.txt")).unwrap(), b"keep old data");
    let backup = preserve_corrupt_record(&root, id, damaged).unwrap();
    assert_eq!(std::fs::read(&backup).unwrap(), damaged);
    assert_eq!(record_ids(&root).unwrap(), vec![id.to_owned()]);

    // A structurally valid JSON record with a mismatched id is also repairable.
    let mut invalid = read_record(&root, id).unwrap().unwrap();
    invalid.id = "org.example.other".into();
    let damaged = serde_json::to_vec(&invalid).unwrap();
    std::fs::write(&active, &damaged).unwrap();
    assert!(install(&root, id, &replacement, None, false).is_err());
    assert!(install(&root, id, &replacement, None, true).is_ok());
    assert_eq!(load(&root, id).unwrap().module, replacement);
    assert_eq!(std::fs::read(previous_dir.join("untracked-user-data.txt")).unwrap(), b"keep old data");
}

#[test]
fn corrupt_metadata_repair_refuses_to_overwrite_existing_sidecars_or_oversized_records() {
    let tmp = tempfile::tempdir().unwrap();
    let root = root(tmp.path()).unwrap();
    let id = "org.example.repair";
    let module = guest(id, false);
    let damaged = b"{ malformed metadata";
    let active = record_path(&root, id).unwrap();
    std::fs::write(&active, damaged).unwrap();
    let backup = preserve_corrupt_record(&root, id, damaged).unwrap();
    std::fs::write(&backup, b"unrelated existing file").unwrap();
    assert!(install(&root, id, &module, None, true).is_err());
    assert_eq!(std::fs::read(&backup).unwrap(), b"unrelated existing file");
    assert_eq!(std::fs::read(&active).unwrap(), damaged);
    std::fs::write(&active, vec![b'x'; MAX_RECORD + 1]).unwrap();
    assert!(install(&root, id, &module, None, true).is_err());
    assert_eq!(std::fs::metadata(&active).unwrap().len(), (MAX_RECORD + 1) as u64);
}

fn v2_info(native: bool, filters: &[&str]) -> PackageInfo {
    PackageInfo {
        package_version: 2,
        module: "plugin.pcext".into(),
        native: native.then(|| NativeInfo {
            service: "plugin.processImage".into(),
            platform: host_platform().into(),
            entry: if cfg!(windows) { "bin/process.exe" } else { "bin/process" }.into(),
        }),
        filters: filters.iter().map(|module| FilterInfo { module: (*module).into() }).collect(),
    }
}

fn filter_module(id: &str, extra: &str) -> Vec<u8> {
    let manifest = serde_json::to_string(&json!({"id":id,"name":"Packaged filter","version":"1","kind":"filter"})).unwrap();
    let escaped = manifest.bytes().map(|byte| format!("\\{byte:02x}")).collect::<String>();
    wat::parse_str(format!(
        r#"(module
        {extra}
        (memory (export "memory") 1)
        (data (i32.const 16) "{escaped}")
        (func (export "pc_abi_version") (result i32) i32.const 1)
        (func (export "pc_manifest") (result i64) i64.const {})
        (func (export "pc_alloc") (param i32) (result i32) i32.const 4096)
        (func (export "pc_filter") (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32) i32.const 0))"#,
        ((manifest.len() as u64) << 32) | 16
    ))
    .unwrap()
}

fn v2_package(id: &str, info: &PackageInfo, entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = ZipWriter::new();
    zip.add("package.json", &serde_json::to_vec(info).unwrap()).unwrap();
    zip.add("plugin.pcext", &guest_for(id, if info.native.is_some() { "plugin.processImage" } else { "" })).unwrap();
    zip.add("LICENSE", b"Test license").unwrap();
    zip.add("THIRD-PARTY-NOTICES.md", b"Test notices").unwrap();
    for (path, bytes) in entries {
        zip.add(path, bytes).unwrap();
    }
    zip.finish().unwrap()
}

#[test]
fn v2_panel_filter_native_and_combined_packages_survive_verified_reload() {
    let filter = filter_module("org.example.embedded", "");
    for (with_native, with_filter) in [(false, false), (false, true), (true, false), (true, true)] {
        if with_native && host_platform() == "unsupported" {
            continue;
        }
        let info = v2_info(with_native, if with_filter { &["filters/embedded.wasm"] } else { &[] });
        let mut entries = Vec::new();
        if let Some(native) = &info.native {
            entries.push((native.entry.as_str(), b"test worker".as_slice()));
        }
        if with_filter {
            entries.push(("filters/embedded.wasm", filter.as_slice()));
        }
        let bytes = v2_package("org.example.v2", &info, &entries);
        let package = inspect(&bytes).unwrap();
        assert_eq!(package.info, info);
        assert_eq!(package.filters.contains_key("org.example.embedded"), with_filter);
        let tmp = tempfile::tempdir().unwrap();
        let root = root(tmp.path()).unwrap();
        let installed = install(&root, "org.example.v2", &bytes, Some(&package), false).unwrap();
        assert_eq!(installed.native.is_some(), with_native);
        let loaded = load(&root, "org.example.v2").unwrap();
        assert_eq!(loaded.package, Some(info.clone()));
        assert_eq!(loaded.native.is_some(), with_native);
        assert_eq!(loaded.filters.contains_key("org.example.embedded"), with_filter);
        assert_eq!(loaded.storage, package.storage);
        assert_eq!(loaded.module, package.module);
        let descriptor = serde_json::to_value(&info).unwrap();
        assert_eq!(descriptor.get("native").is_some(), with_native);
        assert_eq!(descriptor.get("filters").is_some(), with_filter);
        #[cfg(unix)]
        if let Some(native) = loaded.native {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&native.worker).unwrap().permissions().mode() & 0o777, 0o700);
        }
        remove(&root, "org.example.v2").unwrap();
        assert!(load(&root, "org.example.v2").is_err());
    }
}

#[test]
fn v2_rejects_unknown_versions_services_platforms_and_unsafe_entries() {
    let valid = v2_info(false, &[]);
    assert!(check_info(&valid).is_ok());
    for version in [0, 3, u32::MAX] {
        let mut info = valid.clone();
        info.package_version = version;
        assert!(check_info(&info).is_err());
    }
    let mut info = valid.clone();
    info.module = "other.pcext".into();
    assert!(check_info(&info).is_err());
    if host_platform() == "unsupported" {
        return;
    }
    let native = v2_info(true, &[]);
    for service in ["org.example.worker", "image.legacyProcessor", ""] {
        let mut info = native.clone();
        info.native.as_mut().unwrap().service = service.into();
        assert!(check_info(&info).is_err(), "{service}");
    }
    for platform in ["windows-x86_64", "windows-aarch64", "linux-x86_64", "linux-aarch64", "macos-x86_64", "macos-aarch64", "any", ""] {
        let mut info = native.clone();
        info.native.as_mut().unwrap().platform = platform.into();
        assert_eq!(check_info(&info).is_ok(), platform == host_platform(), "{platform}");
    }
    for entry in ["../worker.exe", "/worker.exe", "bin\\worker.exe", "C:/worker.exe", "bin/CON.exe", "bin//worker.exe", ""] {
        let mut info = native.clone();
        info.native.as_mut().unwrap().entry = entry.into();
        assert!(check_info(&info).is_err(), "{entry}");
    }
    #[cfg(windows)]
    {
        let mut info = native;
        info.native.as_mut().unwrap().entry = "bin/worker".into();
        assert!(check_info(&info).is_err());
    }
}

#[test]
fn v2_filter_paths_are_bounded_unique_and_required() {
    for paths in [vec!["filter.wat"], vec!["../filter.wasm"], vec!["filters/a.wasm", "filters/a.wasm"], vec!["filters/A.wasm", "filters/a.wasm"]] {
        assert!(check_info(&v2_info(false, &paths)).is_err());
    }
    let paths: Vec<String> = (0..17).map(|i| format!("filters/{i}.wasm")).collect();
    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
    assert!(check_info(&v2_info(false, &paths)).is_err());
    let info = v2_info(false, &["filters/missing.wasm"]);
    assert!(inspect(&v2_package("org.example.v2", &info, &[])).is_err());
    let filter = filter_module("org.example.embedded", "");
    assert!(inspect(&v2_package("org.example.v2", &info, &[("filters/MISSING.wasm", &filter)])).is_err());
    if host_platform() != "unsupported" {
        let mut native = v2_info(true, &[]);
        assert!(inspect(&v2_package("org.example.v2", &native, &[])).is_err());
        native.native.as_mut().unwrap().entry = "plugin.pcext".into();
        assert!(check_info(&native).is_err());
    }
}

#[test]
fn packaged_filters_validate_abi_imports_and_distinct_nonreserved_ids() {
    let info = v2_info(false, &["filters/a.wasm"]);
    for data in [b"not WASM".to_vec(), guest("org.example.panel", false), filter_module("org.example.filter", r#"(import "host" "read" (func))"#)] {
        assert!(inspect(&v2_package("org.example.v2", &info, &[("filters/a.wasm", &data)])).is_err());
    }
    let reserved = filter_module("native", "");
    assert!(inspect(&v2_package("org.example.v2", &info, &[("filters/a.wasm", &reserved)])).is_err());
    let duplicate = filter_module("org.example.duplicate", "");
    let info = v2_info(false, &["filters/a.wasm", "filters/b.wasm"]);
    assert!(inspect(&v2_package("org.example.v2", &info, &[("filters/a.wasm", &duplicate), ("filters/b.wasm", &duplicate)])).is_err());
    let different = filter_module("org.example.different", "");
    assert_eq!(inspect(&v2_package("org.example.v2", &info, &[("filters/a.wasm", &duplicate), ("filters/b.wasm", &different)])).unwrap().filters.len(), 2);
}

#[test]
fn retired_package_versions_require_an_updated_package() {
    let mut info = v2_info(true, &[]);
    info.package_version = 1;
    assert!(check_info(&info).unwrap_err().to_string().contains("updated version 2 package"));
    info.filters.push(FilterInfo { module: "filter.wasm".into() });
    assert!(check_info(&info).is_err());
    info.package_version = 2;
    info.native.as_mut().unwrap().service = "image.legacyProcessor".into();
    assert!(check_info(&info).is_err());
}

#[test]
fn filter_tampering_or_failed_extraction_never_replaces_the_active_package() {
    let tmp = tempfile::tempdir().unwrap();
    let root = root(tmp.path()).unwrap();
    let id = "org.example.v2";
    let info = v2_info(false, &["filters/a.wasm"]);
    let first_filter = filter_module("org.example.first", "");
    let first = v2_package(id, &info, &[("filters/a.wasm", &first_filter)]);
    let package = inspect(&first).unwrap();
    install(&root, id, &first, Some(&package), false).unwrap();
    let previous = read_record(&root, id).unwrap().unwrap();
    let second_filter = filter_module("org.example.second", "");
    let second = v2_package(id, &info, &[("filters/a.wasm", &second_filter)]);
    // Even an internal caller pairing a reviewed package with different bytes cannot publish it.
    assert!(install(&root, id, &second, Some(&package), true).is_err());
    assert_eq!(read_record(&root, id).unwrap().unwrap(), previous);
    assert!(load(&root, id).unwrap().filters.contains_key("org.example.first"));

    let dir = generation(&root, &previous).unwrap();
    let path = dir.join("filters/a.wasm");
    std::fs::write(&path, b"corrupt").unwrap();
    assert!(load(&root, id).is_err());
    assert_eq!(read_record(&root, id).unwrap().unwrap(), previous);
    // Hash-valid metadata still cannot smuggle a module with a different ABI into startup.
    let invalid = guest("org.example.panel", false);
    std::fs::write(&path, &invalid).unwrap();
    let mut forged = previous.clone();
    let filter = forged.files.iter_mut().find(|file| file.path == "filters/a.wasm").unwrap();
    filter.size = invalid.len() as u64;
    filter.hash = blake3::hash(&invalid).to_hex().to_string();
    let record = serde_json::to_vec(&forged).unwrap();
    std::fs::write(dir.join(OWNERSHIP), &record).unwrap();
    std::fs::write(record_path(&root, id).unwrap(), &record).unwrap();
    assert!(load(&root, id).is_err());
}

#[test]
fn stored_filter_records_obey_module_limits_and_required_paths() {
    let info = v2_info(false, &["filters/a.wasm"]);
    let filter = filter_module("org.example.filter", "");
    let package = inspect(&v2_package("org.example.v2", &info, &[("filters/a.wasm", &filter)])).unwrap();
    let mut record =
        Record { descriptor: None, format_version: 1, id: "org.example.v2".into(), hash: "a".repeat(64), package: Some(info), files: package.files };
    validate_record(&record, &record.id).unwrap();
    record.files.iter_mut().find(|file| file.path == "filters/a.wasm").unwrap().size = photocraft_plugins::Limits::default().max_module_bytes as u64 + 1;
    assert!(validate_record(&record, &record.id).is_err());
    record.files.retain(|file| file.path != "filters/a.wasm");
    assert!(validate_record(&record, &record.id).is_err());
}

#[test]
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn retired_native_installation_can_be_replaced_without_loading_its_code() {
    let tmp = tempfile::tempdir().unwrap();
    let root = root(tmp.path()).unwrap();
    let id = "org.example.retired";
    let original = package(id, true, b"previous version", &[]);
    let inspected = inspect(&original).unwrap();
    install(&root, id, &original, Some(&inspected), false).unwrap();
    let mut retired = read_record(&root, id).unwrap().unwrap();
    let previous_dir = generation(&root, &retired).unwrap();
    retired.package.as_mut().unwrap().package_version = 1;
    retired.package.as_mut().unwrap().native.as_mut().unwrap().service = "image.legacyProcessor".into();
    let retired_bytes = serde_json::to_vec(&retired).unwrap();
    std::fs::write(record_path(&root, id).unwrap(), &retired_bytes).unwrap();
    std::fs::write(previous_dir.join(OWNERSHIP), &retired_bytes).unwrap();
    let mut session = Session::new();
    session.extensions.attach_folder(root.clone()).unwrap();
    assert!(!session.extensions.installed.contains_key(id));
    assert!(session.extensions.load_errors[0].contains("updated version 2 package"));
    let unrelated = "org.example.neighbor: missing payload".to_owned();
    let similar_id = format!("{id}.other: invalid record");
    session.extensions.load_errors.extend([unrelated.clone(), similar_id.clone()]);
    let updated = package(id, true, b"updated generic processor", &[]);
    let path = tmp.path().join("updated.pcplugin");
    std::fs::write(&path, &updated).unwrap();
    let errors_before = session.extensions.load_errors.clone();
    assert!(session.execute("extension.install", json!({"path":path,"allowNative":true})).is_err());
    assert_eq!(session.extensions.load_errors, errors_before, "failed replacement must preserve the startup report");
    let review = session.execute("extension.inspect", json!({"path":path})).unwrap();
    session.execute("extension.install", json!({"path":path,"allowNative":true,"replace":true,"expectedHash":review["hash"]})).unwrap();
    assert!(session.extensions.installed[id].enabled);
    assert!(session.execute("extension.invoke", json!({"id":id,"event":"run","values":{}})).is_ok());
    assert!(!session.extensions.load_errors.iter().any(|message| message.starts_with(&format!("{id}: "))));
    assert!(session.extensions.load_errors.contains(&unrelated));
    assert!(session.extensions.load_errors.contains(&similar_id));
    assert!(session.extensions.load_errors.iter().any(|message| message.contains("preserved the original")));
    let loaded = load(&root, id).unwrap();
    assert_eq!(std::fs::read(&loaded.native.unwrap().worker).unwrap(), b"updated generic processor");
    let mut restarted = Session::new();
    restarted.extensions.attach_folder(root.clone()).unwrap();
    assert!(restarted.extensions.installed.contains_key(id));
    assert!(restarted.extensions.load_errors.is_empty());
    assert_eq!(std::fs::read(previous_dir.join("bin/processor.exe")).unwrap(), b"previous version");
    let metadata_digest = blake3::hash(&retired_bytes).to_hex();
    let backup = root.join(format!("{id}.corrupt-{}.json", &metadata_digest.as_str()[..16]));
    assert_eq!(std::fs::read(backup).unwrap(), retired_bytes);
}
