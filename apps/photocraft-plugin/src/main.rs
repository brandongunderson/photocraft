//! Offline developer conformance checks. This executable is a host development tool;
//! plugins built with it do not link to or otherwise require the editor checkout.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod authoring;
mod native;
mod project;

use std::io::{Read, Write};
use std::path::Path;

use photocraft_engine::Session;
use photocraft_format::zip::{ZipReader, ZipWriter};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "photocraft-plugin check PACKAGE | hash FILE | probe PANEL REQUEST_JSON | project SOURCE | build-export SOURCE FRESH_DESTINATION | pack STAGED_DIRECTORY FRESH_PACKAGE | sdk FRESH_DIRECTORY | generate AUTHORING_JSON FRESH_DIRECTORY | new AUTHORING_JSON FRESH_PROJECT";

fn fail(message: impl Into<String>) -> Box<dyn std::error::Error> {
    std::io::Error::other(message.into()).into()
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(fail(format!("{} must be a regular file of at most {limit} bytes", path.display())));
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(fail("input grew beyond its byte limit"));
    }
    Ok(bytes)
}

fn hashes(path: &Path) -> Result<Value> {
    let mut file = std::fs::File::open(path)?;
    let mut sha = Sha256::new();
    let mut blake = blake3::Hasher::new();
    let mut buffer = [0u8; 65536];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        let chunk = buffer.get(..n).ok_or_else(|| fail("invalid file read length"))?;
        sha.update(chunk);
        blake.update(chunk);
        size = size.checked_add(n as u64).ok_or_else(|| fail("file length overflow"))?;
    }
    Ok(json!({"path": path, "size": size, "sha256": format!("{:x}", sha.finalize()), "blake3": blake.finalize().to_hex().as_str()}))
}

fn check(path: &Path) -> Result<Value> {
    let mut host = Session::new();
    let inspected = host.execute("extension.inspect", json!({"path": path}))?;
    let mut worker_transport = Value::Null;
    if let Some(native) = inspected["package"].get("native").filter(|value| value.is_object()) {
        let entry = native["entry"].as_str().ok_or_else(|| fail("missing native entry"))?;
        let platform = native["platform"].as_str().ok_or_else(|| fail("missing native platform"))?;
        let archive = read_limited(path, 512 << 20)?;
        let zip = ZipReader::new(&archive)?;
        native::check(&zip.read_by_name(entry, 256 << 20)?, platform)?;
        if zip.find("runtime/worker-v2.json").is_some() {
            let declaration = photocraft_plugin_sdk::api::WorkerV2Descriptor::parse(
                &zip.read_by_name("runtime/worker-v2.json", photocraft_plugin_sdk::api::MAX_WORKER_DESCRIPTOR_BYTES)?,
            )?;
            worker_transport = serde_json::to_value(declaration)?;
        }
    }
    let artifact = hashes(path)?;
    if artifact["blake3"] != inspected["hash"] {
        return Err(fail("plugin changed during conformance inspection; rerun the check on an immutable artifact"));
    }
    Ok(json!({
        "formatVersion": 1, "status": "passed", "artifact": artifact,
        "hostPlatform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        "checks": ["bounded archive and safe paths", "complete payload and CRC", "package target and native executable header", "panel manifest and WASM ABI", "packaged filter manifests and WASM ABI", "host command grants"],
        "nativeDependencyAudit": "not performed: native code is never executed by check",
        "persistentWorker": worker_transport,
        "inspection": inspected
    }))
}

fn pack(root: &Path, output: &Path) -> Result<Value> {
    let root = root.canonicalize()?;
    // Publish only to a fresh file. Failed validation cannot overwrite a prior package.
    if output.exists() {
        return Err(fail("output already exists; choose a fresh package path"));
    }
    let files = project::files(&root, false)?;
    if files.len() > 512 {
        return Err(fail("package exceeds the host's 512-file limit"));
    }
    let mut zip = ZipWriter::new();
    let mut size = 0u64;
    for file in files {
        let relative = file.strip_prefix(&root)?.to_str().ok_or_else(|| fail("non-UTF-8 package path"))?.replace('\\', "/");
        let length = file.metadata()?.len();
        size = size.checked_add(length).ok_or_else(|| fail("package length overflow"))?;
        if length > 256 << 20 || size > 512 << 20 {
            return Err(fail("package exceeds developer-tool memory/size limit (256 MiB/file, 512 MiB total)"));
        }
        zip.add(&relative, &read_limited(&file, 256 << 20)?)?;
    }
    let bytes = zip.finish()?;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(output)?;
    file.write_all(&bytes)?;
    drop(file);
    let report = check(output);
    if report.is_err() {
        // The path is exactly the fresh file created above, never a directory.
        std::fs::remove_file(output)?;
    }
    report
}

fn run(args: &[String]) -> Result<Value> {
    match args {
        [command] if matches!(command.as_str(), "--help" | "-h" | "help") => Ok(json!({"formatVersion":1,"status":"help","usage":USAGE})),
        [command, path] if command == "check" => check(Path::new(path)),
        [command, path] if command == "hash" => hashes(Path::new(path)),
        [command, path, request] if command == "probe" => {
            let extension = photocraft_plugins::extension::Extension::load(&read_limited(Path::new(path), 8 << 20)?)?;
            let request: Value = serde_json::from_slice(&read_limited(Path::new(request), 256 << 10)?)?;
            Ok(serde_json::to_value(extension.handle(&request)?)?)
        }
        [command, path] if command == "project" => project::audit(Path::new(path)),
        [command, path] if command == "sdk" => authoring::export(Path::new(path)),
        [command, input, output] if command == "generate" => authoring::manifests(Path::new(input), Path::new(output)),
        [command, input, output] if command == "new" => authoring::new_project(Path::new(input), Path::new(output)),
        [command, source, destination] if command == "build-export" => project::build_export(Path::new(source), Path::new(destination)),
        [command, source, destination] if command == "pack" => pack(Path::new(source), Path::new(destination)),
        _ => Err(fail(format!("usage: {USAGE}"))),
    }
}

fn main() -> std::process::ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let result = run(&args);
    let (report, exit) = match result {
        Ok(value) => (value, std::process::ExitCode::SUCCESS),
        Err(error) => (json!({"formatVersion":1,"status":"failed","error":error.to_string()}), std::process::ExitCode::FAILURE),
    };
    if let Ok(text) = serde_json::to_string_pretty(&report) {
        let _ = writeln!(std::io::stdout().lock(), "{text}");
    }
    exit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_package_is_a_failure_and_never_executes_a_worker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.pcplugin");
        std::fs::write(&path, b"PKnot an archive").unwrap();
        assert!(check(&path).is_err());
    }

    #[test]
    fn standalone_panel_passes_the_real_host_abi_and_grant_checks() {
        let dir = tempfile::tempdir().unwrap();
        let panel = dir.path().join("panel.pcext");
        std::fs::write(&panel, include_bytes!("../../../tests/plugin-compatibility/frozen/image-gain-panel.wasm")).unwrap();
        let report = check(&panel).unwrap();
        assert_eq!(report["status"], "passed");
        assert_eq!(report["inspection"]["manifest"]["id"], "org.example.image-gain");
        assert_eq!(report["inspection"]["manifest"]["apiVersion"], photocraft_plugin_sdk::api::PANEL_API_VERSION);
    }

    #[test]
    fn failed_pack_does_not_publish_an_invalid_package() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("invalid.pcplugin");
        assert!(pack(dir.path(), &out).is_err());
        assert!(!out.exists());
    }

    #[test]
    fn existing_package_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("keep.pcplugin");
        std::fs::write(&out, b"original").unwrap();
        assert!(pack(dir.path(), &out).is_err());
        assert_eq!(std::fs::read(out).unwrap(), b"original");
    }

    #[test]
    #[ignore = "requires an actual independently built SDK package; see docs/plugin-sdk.md"]
    fn independently_built_sdk_guest_edits_through_the_real_broker_and_undoes_atomically() {
        let package =
            std::path::PathBuf::from(std::env::var_os("PHOTOCRAFT_PLUGIN_SDK_PACKAGE").expect("provide the generated neutral SDK validation package"));
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::new();
        session.extensions.attach_folder(dir.path().join("Extensions")).unwrap();
        let installed = session.execute("extension.install", json!({"path":package})).unwrap();
        let id = installed["manifest"]["id"].as_str().unwrap().to_owned();
        session.execute("file.new", json!({"width":8,"height":6,"background":"transparent"})).unwrap();
        let original = session.active().unwrap().doc.clone();
        let command = format!("{id}.inspect");
        session.execute(&command, json!({})).unwrap();
        assert_eq!(session.extensions.installed[&id].state["checked"], true);
        assert_eq!(session.active().unwrap().doc.layers[0].name, "SDK checked");
        assert!(session.undo());
        assert_eq!(session.active().unwrap().doc, original);
    }
}
