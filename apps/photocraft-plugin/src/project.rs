//! Verify ownership from Cargo's resolved graph, then build an exported standalone starter.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::{Result, fail};

const MAX_SOURCE_FILES: usize = 8192;
const MAX_SOURCE_BYTES: u64 = 256 << 20;

fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_type().is_symlink() || meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    meta.file_type().is_symlink()
}

pub(crate) fn files(root: &Path, source: bool) -> Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    let mut total = 0u64;
    let mut visited = 0usize;
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path)?;
            visited = visited.saturating_add(1);
            if visited > MAX_SOURCE_FILES {
                return Err(fail("source/package exceeds 8192 filesystem entries"));
            }
            if is_link(&meta) {
                return Err(fail(format!("standalone source/package cannot contain links or reparse points: {}", path.display())));
            }
            if meta.is_dir() {
                let excluded = source && matches!(entry.file_name().to_str(), Some("target" | "dist" | ".git" | "__pycache__"));
                if !excluded {
                    pending.push(path);
                }
            } else if meta.is_file() {
                total = total.checked_add(meta.len()).ok_or_else(|| fail("source size overflow"))?;
                if source && total > MAX_SOURCE_BYTES {
                    return Err(fail("clean source export exceeds 256 MiB"));
                }
                files.push(path);
            } else {
                return Err(fail("only regular source/package files are supported"));
            }
        }
    }
    files.sort();
    Ok(files)
}

fn cargo() -> PathBuf {
    std::env::var_os("PHOTOCRAFT_PLUGIN_CARGO").map(PathBuf::from).unwrap_or_else(|| "cargo".into())
}

pub(crate) fn cargo_command() -> Command {
    let mut command = Command::new(cargo());
    if let Some(path) = std::env::var_os("PHOTOCRAFT_PLUGIN_BUILD_PATH") {
        command.env("PATH", path);
    }
    command.env_remove("RUSTC_WRAPPER").env_remove("RUSTC_WORKSPACE_WRAPPER");
    command
}

// Windows canonicalization adds an extended-length prefix which GNU linkers do not
// understand. Keep canonical paths for ownership checks, and ordinary paths for tools.
fn tool_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    if let Some(text) = path.to_str() {
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(drive) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(drive);
        }
    }
    path.to_path_buf()
}

fn metadata(project: &Path) -> Result<Value> {
    let output = cargo_command()
        .args(["metadata", "--locked", "--offline", "--format-version", "1"])
        .current_dir(tool_path(project))
        .env_remove("CARGO_TARGET_DIR")
        .output()?;
    if !output.status.success() {
        return Err(fail(format!("Cargo metadata failed for {}: {}", project.display(), String::from_utf8_lossy(&output.stderr))));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value.get(key).and_then(Value::as_str).ok_or_else(|| fail(format!("missing Cargo metadata field {key}")))
}

fn owned_metadata(root: &Path, meta: &Value) -> Result<()> {
    let workspace = Path::new(string(meta, "workspace_root")?).canonicalize()?;
    if !workspace.starts_with(root) {
        return Err(fail("plugin inherits a workspace outside its own source export"));
    }
    let packages = meta.get("packages").and_then(Value::as_array).ok_or_else(|| fail("Cargo metadata packages missing"))?;
    for package in packages {
        let name = string(package, "name")?;
        match package.get("source").and_then(Value::as_str) {
            None => {
                let manifest = Path::new(string(package, "manifest_path")?).canonicalize()?;
                if !manifest.starts_with(root) {
                    return Err(fail(format!("local dependency {name} is outside this plugin; vendor it or use a published crate")));
                }
                if package.get("license").and_then(Value::as_str).is_none_or(str::is_empty) {
                    let license_file = package
                        .get("license_file")
                        .and_then(Value::as_str)
                        .ok_or_else(|| fail(format!("local package {name} must declare license or license-file")))?;
                    let license_path = manifest.parent().ok_or_else(|| fail("invalid local manifest"))?.join(license_file).canonicalize()?;
                    if !license_path.starts_with(root) || !license_path.is_file() {
                        return Err(fail(format!("local package {name} has an unowned or missing license-file")));
                    }
                }
            }
            Some(source) if source.starts_with("git+") => {
                let revision = source.rsplit('#').next().unwrap_or_default();
                if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(fail(format!("git dependency {name} must resolve to a pinned commit")));
                }
            }
            Some(source) if source.starts_with("registry+") => {}
            Some(_) => return Err(fail(format!("unrecognized dependency source for {name}"))),
        }
    }
    Ok(())
}

fn projects(root: &Path) -> Result<Vec<PathBuf>> {
    let projects = files(root, true)?
        .into_iter()
        .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.toml"))
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect::<Vec<_>>();
    if projects.is_empty() {
        return Err(fail("no independent Cargo projects found"));
    }
    Ok(projects)
}

pub(crate) fn audit(root: &Path) -> Result<Value> {
    let root = root.canonicalize()?;
    let mut reports = Vec::new();
    for project in projects(&root)? {
        let meta = metadata(&project)?;
        owned_metadata(&root, &meta)?;
        reports.push(json!({"manifest":project.join("Cargo.toml"),"dependencyOwnership":"passed","lockedOfflineResolution":"passed"}));
    }
    Ok(json!({"formatVersion":1,"status":"passed","source":root,"projects":reports,
        "scope":"resolved Rust dependency ownership; native binary dependency closure requires platform-specific audit"}))
}

fn license_files(meta: &Value, stage: &Path, copied: &mut BTreeSet<String>) -> Result<()> {
    let packages = meta.get("packages").and_then(Value::as_array).ok_or_else(|| fail("Cargo metadata packages missing"))?;
    for package in packages.iter().filter(|package| !package.get("source").is_none_or(Value::is_null)) {
        let identity = format!("{}-{}", string(package, "name")?, string(package, "version")?);
        if !copied.insert(identity.clone()) {
            continue;
        }
        let manifest = Path::new(string(package, "manifest_path")?);
        let directory = manifest.parent().ok_or_else(|| fail("invalid dependency manifest path"))?;
        let mut count = 0;
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let upper = entry.file_name().to_string_lossy().to_ascii_uppercase();
            if entry.file_type()?.is_file() && ["LICENSE", "LICENCE", "COPYING", "UNLICENSE"].iter().any(|prefix| upper.starts_with(prefix)) {
                let destination = stage.join("licenses").join(&identity);
                std::fs::create_dir_all(&destination)?;
                std::fs::copy(entry.path(), destination.join(entry.file_name()))?;
                count += 1;
            }
        }
        if count == 0 {
            return Err(fail(format!("no dependency license files found for {identity}")));
        }
    }
    Ok(())
}

fn artifact_target<'a>(meta: &'a Value, manifest: &Path, kind: &str) -> Result<&'a str> {
    let packages = meta.get("packages").and_then(Value::as_array).ok_or_else(|| fail("Cargo metadata packages missing"))?;
    for package in packages {
        if Path::new(string(package, "manifest_path")?).canonicalize()? == manifest.canonicalize()? {
            let targets = package.get("targets").and_then(Value::as_array).ok_or_else(|| fail("Cargo metadata targets missing"))?;
            for target in targets {
                if target.get("kind").and_then(Value::as_array).is_some_and(|kinds| kinds.iter().any(|value| value.as_str() == Some(kind))) {
                    return string(target, "name");
                }
            }
        }
    }
    Err(fail(format!("project {} has no {kind} target", manifest.display())))
}

fn build(project: &Path, target_dir: &Path, wasm: bool) -> Result<Value> {
    let mut command = cargo_command();
    command.args(["build", "--locked", "--offline", "--release"]);
    if wasm {
        command.args(["--target", "wasm32-unknown-unknown"]);
    }
    let status =
        command.current_dir(tool_path(project)).env("CARGO_TARGET_DIR", tool_path(target_dir)).stdout(Stdio::inherit()).stderr(Stdio::inherit()).status()?;
    if !status.success() {
        return Err(fail(format!("exported build failed: {}", project.display())));
    }
    metadata(project)
}

pub(crate) fn build_export(source: &Path, destination: &Path) -> Result<Value> {
    if !cargo().is_absolute() || !cargo().is_file() {
        return Err(fail("set PHOTOCRAFT_PLUGIN_CARGO to the absolute Rust Cargo executable for clean export builds"));
    }
    if std::env::var_os("PHOTOCRAFT_PLUGIN_BUILD_PATH").is_none_or(|path| path.is_empty()) {
        return Err(fail("set PHOTOCRAFT_PLUGIN_BUILD_PATH to an explicit Rust/linker/OS tool PATH for clean export builds"));
    }
    let source = source.canonicalize()?;
    if destination.exists() {
        return Err(fail("export destination must not already exist"));
    }
    let destination_parent = destination.parent().ok_or_else(|| fail("export destination needs a parent directory"))?.canonicalize()?;
    let destination = destination_parent.join(destination.file_name().ok_or_else(|| fail("invalid export destination"))?);
    if destination.starts_with(&source) {
        return Err(fail("clean export destination must be outside the plugin source"));
    }
    let source_files = files(&source, true)?;
    // Refuse before mutation if the original source cannot resolve without a sibling crate.
    let _ = audit(&source)?;
    std::fs::create_dir(&destination)?;
    for file in source_files {
        let output = destination.join(file.strip_prefix(&source)?);
        std::fs::create_dir_all(output.parent().ok_or_else(|| fail("invalid source path"))?)?;
        std::fs::copy(file, output)?;
    }
    let _ = audit(&destination)?;
    let stage = destination.join("dist/package");
    std::fs::create_dir_all(stage.join("filters"))?;
    let mut package: Value = if destination.join("authoring.json").is_file() {
        serde_json::to_value(crate::authoring::source(&destination.join("authoring.json"))?.package)?
    } else {
        serde_json::from_slice(&std::fs::read(destination.join("package.json"))?)?
    };
    let target = destination.join("target");
    let panel_meta = build(&destination, &target, true)?;
    let panel = artifact_target(&panel_meta, &destination.join("Cargo.toml"), "cdylib")?;
    std::fs::copy(target.join(format!("wasm32-unknown-unknown/release/{panel}.wasm")), stage.join("plugin.pcext"))?;
    let mut license_ids = BTreeSet::new();
    license_files(&panel_meta, &stage, &mut license_ids)?;
    let filters = package.get("filters").and_then(Value::as_array).cloned().unwrap_or_default();
    if filters.len() > 1 {
        return Err(fail("build-export's starter layout supports one filter/ project; package arbitrary layouts with pack"));
    }
    if let Some(filter) = filters.first() {
        let path = string(filter, "module")?;
        if Path::new(path).components().any(|part| !matches!(part, std::path::Component::Normal(_))) || !path.starts_with("filters/") {
            return Err(fail("filter module must be a safe filters/ path"));
        }
        let filter_dir = destination.join("filter");
        let meta = build(&filter_dir, &target, true)?;
        let name = artifact_target(&meta, &filter_dir.join("Cargo.toml"), "cdylib")?;
        let output = stage.join(path);
        std::fs::create_dir_all(output.parent().ok_or_else(|| fail("invalid filter path"))?)?;
        std::fs::copy(target.join(format!("wasm32-unknown-unknown/release/{name}.wasm")), output)?;
        license_files(&meta, &stage, &mut license_ids)?;
    }
    if package.get("native").is_some_and(Value::is_object) {
        let native = destination.join("native");
        let meta = build(&native, &target, false)?;
        let name = artifact_target(&meta, &native.join("Cargo.toml"), "bin")?;
        let suffix = std::env::consts::EXE_SUFFIX;
        let binary = format!("{name}{suffix}");
        let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        let entry = if destination.join("authoring.json").is_file() {
            if string(&package["native"], "platform")? != platform {
                return Err(fail(
                    "build-export builds native code for the current host; use a target-specific authoring input or your own cross-platform builder",
                ));
            }
            string(&package["native"], "entry")?.to_owned()
        } else {
            package["native"]["entry"] = json!(binary);
            package["native"]["platform"] = json!(platform);
            binary.clone()
        };
        let output = stage.join(entry);
        std::fs::create_dir_all(output.parent().ok_or_else(|| fail("invalid native entry path"))?)?;
        std::fs::copy(target.join("release").join(binary), output)?;
        license_files(&meta, &stage, &mut license_ids)?;
    }
    for name in ["LICENSE", "LICENSE-MIT", "LICENSE-APACHE", "THIRD-PARTY-NOTICES.md"] {
        let path = destination.join(name);
        if path.exists() {
            std::fs::copy(path, stage.join(name))?;
        }
    }
    std::fs::write(stage.join("package.json"), serde_json::to_vec_pretty(&package)?)?;
    std::fs::create_dir_all(stage.join("licenses"))?;
    std::fs::write(stage.join("licenses/DEPENDENCIES.txt"), license_ids.into_iter().collect::<Vec<_>>().join("\n") + "\n")?;
    let artifact = destination.join("dist/exported.pcplugin");
    let report = crate::pack(&stage, &artifact)?;
    Ok(json!({"formatVersion":1,"status":"passed","export":destination,"package":artifact,"conformance":report,
        "buildContract":"clean copied source, Cargo --locked --offline, explicit restricted tool PATH; caller supplies Rust toolchains and linker"}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_local_dependency_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("plugin");
        std::fs::create_dir(&plugin).unwrap();
        let external = dir.path().join("Cargo.toml");
        std::fs::write(&external, "").unwrap();
        let meta = json!({"workspace_root":plugin,"packages":[{"name":"editor-internals","source":null,"license":"MIT","manifest_path":external}]});
        assert!(owned_metadata(&plugin.canonicalize().unwrap(), &meta).unwrap_err().to_string().contains("outside this plugin"));
    }

    #[test]
    fn clean_export_excludes_generated_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/hidden"), "generated").unwrap();
        std::fs::write(dir.path().join("source.rs"), "source").unwrap();
        assert_eq!(files(dir.path(), true).unwrap(), vec![dir.path().join("source.rs")]);
    }

    #[test]
    fn inherited_editor_workspace_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("plugin");
        std::fs::create_dir(&plugin).unwrap();
        let meta = json!({"workspace_root":dir.path(),"packages":[]});
        assert!(owned_metadata(&plugin.canonicalize().unwrap(), &meta).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_tool_paths_do_not_leak_extended_length_prefixes_to_the_linker() {
        assert_eq!(tool_path(Path::new(r"\\?\C:\plugin\target")), Path::new(r"C:\plugin\target"));
        assert_eq!(tool_path(Path::new(r"\\?\UNC\server\share\target")), Path::new(r"\\server\share\target"));
    }

    #[test]
    fn owned_license_file_is_supported_but_a_sibling_license_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("plugin");
        std::fs::create_dir(&plugin).unwrap();
        let manifest = plugin.join("Cargo.toml");
        std::fs::write(&manifest, "").unwrap();
        std::fs::write(plugin.join("LICENSE"), "MIT").unwrap();
        std::fs::write(dir.path().join("LICENSE"), "unowned").unwrap();
        let mut meta =
            json!({"workspace_root":plugin,"packages":[{"name":"standalone","source":null,"license":null,"license_file":"LICENSE","manifest_path":manifest}]});
        let root = plugin.canonicalize().unwrap();
        owned_metadata(&root, &meta).unwrap();
        meta["packages"][0]["license_file"] = json!("../LICENSE");
        assert!(owned_metadata(&root, &meta).unwrap_err().to_string().contains("unowned"));
    }
}
