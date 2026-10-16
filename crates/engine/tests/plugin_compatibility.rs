//! Compatibility tests consume frozen compiled guests, never rebuild guest source.
//! Native tests fail when explicitly selected without their pinned transport executable.
#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use photocraft_engine::{EngineError, Session, jobs::JobCtx};
use photocraft_format::zip::ZipWriter;
use photocraft_plugins::extension::Extension;
use serde_json::{Value, json};

const PANEL: &[u8] = include_bytes!("../../../tests/plugin-compatibility/frozen/image-gain-panel-v2.wasm");
const FILTER: &[u8] = include_bytes!("../../../tests/plugin-compatibility/frozen/channel-gain-filter-v1.wasm");
const ID: &str = "org.example.image-gain";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/plugin-compatibility")
}

fn catalog() -> Value {
    serde_json::from_slice(include_bytes!("../../../tests/plugin-compatibility/catalog.json")).unwrap()
}

fn portable_package() -> Vec<u8> {
    // This cross-platform wrapper uses the current panel fixture and the original filter.
    let mut zip = ZipWriter::new();
    let info = json!({"packageVersion":2,"module":"plugin.pcext","filters":[{"module":"filters/channel-gain.wasm"}]});
    zip.add("package.json", &serde_json::to_vec(&info).unwrap()).unwrap();
    zip.add("plugin.pcext", PANEL).unwrap();
    zip.add("filters/channel-gain.wasm", FILTER).unwrap();
    zip.add("LICENSE", include_bytes!("../../../tests/plugin-compatibility/licenses/LICENSE")).unwrap();
    zip.add("THIRD-PARTY-NOTICES.md", include_bytes!("../../../tests/plugin-compatibility/licenses/THIRD-PARTY-NOTICES.md")).unwrap();
    zip.finish().unwrap()
}

fn new_document(session: &mut Session) {
    session.execute("file.new", json!({"width":8,"height":6,"depth":16,"background":"white"})).unwrap();
}

#[test]
fn frozen_guest_bytes_and_golden_requests_remain_compatible() {
    let manifest = catalog();
    for artifact in manifest["frozen"].as_array().unwrap() {
        let bytes = std::fs::read(fixtures().join(artifact["path"].as_str().unwrap())).unwrap();
        assert_eq!(bytes.len() as u64, artifact["size"].as_u64().unwrap());
        assert_eq!(blake3::hash(&bytes).to_hex().as_str(), artifact["blake3"].as_str().unwrap());
    }
    let cases: Value = serde_json::from_slice(&std::fs::read(fixtures().join("panel-golden.json")).unwrap()).unwrap();
    for case in cases.as_array().unwrap() {
        let panel = std::fs::read(fixtures().join(case["module"].as_str().unwrap())).unwrap();
        let extension = Extension::load(&panel).unwrap();
        assert_eq!(extension.manifest.id, case["id"].as_str().unwrap());
        assert_eq!(serde_json::to_value(&extension.manifest).unwrap(), case["manifest"]);
        let actual = serde_json::to_value(extension.handle(&case["request"]).unwrap()).unwrap();
        assert_eq!(actual, case["response"], "{}", extension.manifest.id);
    }
}

#[test]
fn frozen_filter_preserves_precision_alpha_and_its_original_abi() {
    use photocraft_color::{ColorMode, PixelFormat, SampleType};
    let filter = photocraft_plugins::Plugin::load(FILTER, Default::default()).unwrap();
    let bounds = photocraft_geom::Rect::new(0, 0, 3, 2);
    for depth in [SampleType::U8, SampleType::U16, SampleType::F32] {
        let mut surface = photocraft_raster::Surface::new(PixelFormat { mode: ColorMode::Rgb, sample: depth, alpha: true });
        surface.fill_rect(bounds, &[0.2, 0.4, 0.6, 0.5]);
        let output = filter.apply(&surface, bounds, None, &json!({"gain":1.5})).unwrap();
        assert_eq!(output.format(), surface.format());
        let pixel = output.pixel(1, 1);
        for (actual, expected) in pixel.iter().zip([0.3, 0.6, 0.9, 0.5]) {
            assert!((actual - expected).abs() < 0.01, "{depth:?}: {pixel:?}");
        }
    }
}

#[test]
fn frozen_panel_and_filter_install_restart_invoke_apply_cancel_and_repair() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Extensions");
    let download = dir.path().join("download.pcplugin");
    let package = portable_package();
    std::fs::write(&download, &package).unwrap();
    let mut first = Session::new();
    first.extensions.attach_folder(folder.clone()).unwrap();
    first.execute("extension.install", json!({"path":download})).unwrap();
    first.execute("extension.enable", json!({"id":ID,"enabled":false})).unwrap();
    drop(first);
    std::fs::remove_file(&download).unwrap();
    let mut session = Session::new();
    session.extensions.attach_folder(folder.clone()).unwrap();
    assert!(session.extensions.load_errors.is_empty());
    assert!(!session.extensions.installed[ID].enabled);
    session.execute("extension.enable", json!({"id":ID,"enabled":true})).unwrap();
    new_document(&mut session);
    let source = Arc::clone(&session.active().unwrap().doc);
    let invoke = json!({"id":ID,"event":"process","values":{"backend":"Sandboxed filter","gain":0.5}});
    let response = session.execute("extension.invoke", invoke.clone()).unwrap();
    let token = response["preview"].as_u64().unwrap();
    assert!(Arc::ptr_eq(&source, &session.active().unwrap().doc));
    session.execute("image.pluginPreview.apply", json!({"preview":token,"target":"layer"})).unwrap();
    assert_eq!(session.active().unwrap().doc.layers.len(), source.layers.len() + 1);
    assert_eq!(session.active().unwrap().doc.depth, source.depth);
    assert!(session.undo());
    assert_eq!(session.active().unwrap().doc, source);
    assert!(session.redo());
    assert!(session.undo());
    let ctx = JobCtx::new();
    ctx.cancel();
    session.set_inline_job_ctx(Some(ctx));
    assert!(matches!(session.execute("extension.invoke", invoke), Err(EngineError::Cancelled)));
    session.set_inline_job_ctx(None);
    assert_eq!(session.active().unwrap().doc, source);
    session.execute("extension.enable", json!({"id":ID,"enabled":false})).unwrap();
    // Cached discovery stays lazy; activating corrupted code must fail before use.
    let record: Value = serde_json::from_slice(&std::fs::read(folder.join(format!("{ID}.installation.json"))).unwrap()).unwrap();
    let installed = folder.join("bundles").join(ID).join(record["hash"].as_str().unwrap()).join("plugin.pcext");
    std::fs::write(&installed, b"damaged").unwrap();
    drop(session);
    let mut broken = Session::new();
    broken.extensions.attach_folder(folder.clone()).unwrap();
    assert!(broken.extensions.load_errors.is_empty());
    assert!(!broken.extensions.installed[ID].enabled);
    assert_eq!(broken.extensions.installed[ID].extension.lifecycle(), "idle");
    broken.execute("extension.enable", json!({"id":ID,"enabled":true})).unwrap();
    let failure = broken.execute("extension.invoke", json!({"id":ID,"event":"process"})).unwrap_err().to_string();
    assert!(failure.contains("installed panel module changed"), "{failure}");
    assert!(broken.documents().is_empty());
    broken.execute("extension.enable", json!({"id":ID,"enabled":false})).unwrap();
    std::fs::write(&download, &package).unwrap();
    broken.execute("extension.install", json!({"path":download,"replace":true})).unwrap();
    drop(broken);
    let mut repaired = Session::new();
    repaired.extensions.attach_folder(folder).unwrap();
    assert!(repaired.extensions.load_errors.is_empty());
    assert_eq!(std::fs::read(installed).unwrap(), PANEL);
    assert!(!repaired.extensions.installed[ID].enabled, "repair must preserve the user's disabled flag");
    repaired.execute("extension.enable", json!({"id":ID,"enabled":true})).unwrap();
}

fn corpus_path(artifact: &Value) -> PathBuf {
    let root = PathBuf::from(
        std::env::var_os("PHOTOCRAFT_PLUGIN_COMPAT_ROOT")
            .expect("set PHOTOCRAFT_PLUGIN_COMPAT_ROOT to the pinned artifact store; see tests/plugin-compatibility/README.md"),
    );
    let flat = root.join(artifact["name"].as_str().unwrap());
    let relative = root.join(artifact["path"].as_str().unwrap());
    if flat.is_file() {
        flat
    } else if relative.is_file() {
        relative
    } else {
        root.join("upstream/photocraft").join(artifact["path"].as_str().unwrap())
    }
}

fn verify_artifact(artifact: &Value) -> PathBuf {
    let path = corpus_path(artifact);
    let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("missing pinned artifact {}: {error}; do not rebuild a baseline", path.display()));
    assert_eq!(bytes.len() as u64, artifact["size"].as_u64().unwrap(), "{}", path.display());
    assert_eq!(blake3::hash(&bytes).to_hex().as_str(), artifact["blake3"].as_str().unwrap(), "{}", path.display());
    path
}

fn native_package(artifact: &Value) -> Vec<u8> {
    let worker = std::fs::read(verify_artifact(artifact)).unwrap();
    let mut zip = ZipWriter::new();
    zip.add("package.json", &serde_json::to_vec(&artifact["package"]).unwrap()).unwrap();
    zip.add("plugin.pcext", PANEL).unwrap();
    zip.add("filters/channel-gain.wasm", FILTER).unwrap();
    zip.add(artifact["package"]["native"]["entry"].as_str().unwrap(), &worker).unwrap();
    zip.add("LICENSE", include_bytes!("../../../tests/plugin-compatibility/licenses/LICENSE")).unwrap();
    zip.add("THIRD-PARTY-NOTICES.md", include_bytes!("../../../tests/plugin-compatibility/licenses/THIRD-PARTY-NOTICES.md")).unwrap();
    zip.finish().unwrap()
}

#[test]
#[ignore = "requires the pinned native transport executable; run scripts/plugins/test-compatibility.ps1"]
fn pinned_native_packages_install_restart_cancel_and_repair() {
    let catalog = catalog();
    for artifact in catalog["artifacts"].as_array().unwrap() {
        let package = native_package(artifact);
        assert_eq!(
            artifact["package"]["native"]["platform"],
            format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            "native baseline must run on its declared platform"
        );
        let dir = tempfile::tempdir().unwrap();
        let download = dir.path().join("download.pcplugin");
        std::fs::write(&download, &package).unwrap();
        let folder = dir.path().join("Extensions");
        let mut installer = Session::new();
        installer.extensions.attach_folder(folder.clone()).unwrap();
        let installed = installer.execute("extension.install", json!({"path":download,"allowNative":true})).unwrap();
        let id = installed["manifest"]["id"].as_str().unwrap().to_string();
        installer.execute("extension.enable", json!({"id":id,"enabled":false})).unwrap();
        drop(installer);
        std::fs::remove_file(&download).unwrap();
        let mut restarted = Session::new();
        restarted.extensions.attach_folder(folder.clone()).unwrap();
        assert!(restarted.extensions.load_errors.is_empty(), "{:?}", restarted.extensions.load_errors);
        assert!(!restarted.extensions.installed[&id].enabled);
        restarted.execute("extension.enable", json!({"id":id,"enabled":true})).unwrap();
        new_document(&mut restarted);
        let document = Arc::clone(&restarted.active().unwrap().doc);
        let ctx = JobCtx::new();
        ctx.cancel();
        restarted.set_inline_job_ctx(Some(ctx));
        assert!(matches!(restarted.execute("plugin.processImage", json!({"id":id})), Err(EngineError::Cancelled)));
        restarted.set_inline_job_ctx(None);
        assert!(Arc::ptr_eq(&document, &restarted.active().unwrap().doc));
        restarted.execute("extension.enable", json!({"id":id,"enabled":false})).unwrap();
        let worker = restarted.extensions.installed[&id].native.as_ref().unwrap().worker.clone();
        std::fs::write(&worker, b"corrupted test worker").unwrap();
        drop(restarted);
        let mut damaged = Session::new();
        damaged.extensions.attach_folder(folder.clone()).unwrap();
        assert!(damaged.extensions.load_errors.is_empty());
        assert!(!damaged.extensions.installed[&id].enabled);
        damaged.execute("extension.enable", json!({"id":id,"enabled":true})).unwrap();
        new_document(&mut damaged);
        let document = Arc::clone(&damaged.active().unwrap().doc);
        let failure = damaged.execute("plugin.processImage", json!({"id":id})).unwrap_err().to_string();
        assert!(failure.contains("installed file changed"), "{failure}");
        assert!(Arc::ptr_eq(&document, &damaged.active().unwrap().doc));
        assert_eq!(damaged.plugin_previews().count(), 0);
        damaged.execute("extension.enable", json!({"id":id,"enabled":false})).unwrap();
        std::fs::write(&download, &package).unwrap();
        damaged.execute("extension.install", json!({"path":download,"allowNative":true,"replace":true})).unwrap();
        drop(damaged);
        let mut repaired = Session::new();
        repaired.extensions.attach_folder(folder).unwrap();
        assert!(repaired.extensions.load_errors.is_empty());
        assert!(repaired.extensions.installed.contains_key(&id));
        assert!(!repaired.extensions.installed[&id].enabled, "repair must preserve disabled state for {id}");
    }
}

fn process_case(package_name: &str, event: &str, values: Value) {
    // Native workers may own persistent caches. The explicit runner redirects them
    // into this workspace rather than touching the user's real plugin/runtime cache.
    let cache =
        PathBuf::from(std::env::var_os("PHOTOCRAFT_PLUGIN_COMPAT_CACHE").expect("run scripts/plugins/test-compatibility.ps1 to isolate native worker caches"))
            .canonicalize()
            .unwrap();
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("set an isolated LOCALAPPDATA using the compatibility runner")).canonicalize().unwrap();
    assert!(local.starts_with(&cache), "native compatibility tests require an isolated worker cache");
    let catalog = catalog();
    let artifact = catalog["artifacts"].as_array().unwrap().iter().find(|artifact| artifact["name"] == package_name).unwrap();
    let package = native_package(artifact);
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Extensions");
    let download = dir.path().join("download.pcplugin");
    std::fs::write(&download, &package).unwrap();
    let mut session = Session::new();
    session.extensions.attach_folder(folder.clone()).unwrap();
    let installed = session.execute("extension.install", json!({"path":download,"allowNative":true})).unwrap();
    let id = installed["manifest"]["id"].as_str().unwrap().to_owned();
    drop(session);
    std::fs::remove_file(&download).unwrap();
    let mut session = Session::new();
    session.extensions.attach_folder(folder).unwrap();
    if session.extensions.installed[&id].extension.manifest.panel.buttons.iter().any(|button| button.id == "model_status") {
        let status = session.execute("extension.invoke", json!({"id":id,"event":"model_status","values":values})).unwrap();
        assert!(status["data"].is_object(), "native task protocol must return structured data");
        assert!(session.documents().is_empty());
    }
    new_document(&mut session);
    let original = Arc::clone(&session.active().unwrap().doc);
    let request = json!({"id":id,"event":event,"values":values});
    let started = session.start("extension.invoke", request.clone()).unwrap();
    let photocraft_engine::jobs::Started::Job(job) = started else { panic!("expected a native processing job") };
    assert!(session.cancel_job(job));
    assert!(matches!(session.wait_job(job), Err(EngineError::Cancelled)));
    session.join_cancelled_jobs();
    assert!(Arc::ptr_eq(&original, &session.active().unwrap().doc));
    let response = session.execute("extension.invoke", request.clone()).unwrap();
    let preview = response["preview"].as_u64().unwrap();
    let result = &session.plugin_preview(preview).unwrap().result;
    assert_eq!(result.size, original.size);
    let pixel = result.layers.first().unwrap().surface().unwrap().pixel(1, 1);
    assert!(pixel.iter().all(|sample| sample.is_finite() && (0.0..=1.0).contains(sample)));
    if package_name == "Image-Gain-worker-windows-x86_64.exe" {
        for (actual, expected) in pixel.iter().zip([0.5, 0.5, 0.5, 1.0]) {
            assert!((actual - expected).abs() < 0.01, "native protocol must preserve gain, channel order and alpha: {pixel:?}");
        }
    }
    assert!(Arc::ptr_eq(&original, &session.active().unwrap().doc));
    session.execute("image.pluginPreview.discard", json!({"preview":preview})).unwrap();
    assert!(session.plugin_preview(preview).is_none());
    let response = session.execute("extension.invoke", request).unwrap();
    let preview = response["preview"].as_u64().unwrap();
    session.execute("image.pluginPreview.apply", json!({"preview":preview,"target":"layer"})).unwrap();
    assert_eq!(session.active().unwrap().doc.layers.len(), original.layers.len() + 1);
    assert_eq!(session.active().unwrap().doc.depth, original.depth);
    assert!(session.undo());
    assert_eq!(session.active().unwrap().doc, original);
    assert!(session.redo());
}

#[test]
#[ignore = "requires the pinned Windows native transport executable"]
fn pinned_image_gain_native_processes_and_applies() {
    process_case("Image-Gain-worker-windows-x86_64.exe", "process", json!({"backend":"Native worker","gain":0.5}));
}
