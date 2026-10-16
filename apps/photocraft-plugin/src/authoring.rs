//! SDK export and single-manifest authoring. No plugin feature implementation is bundled.
use crate::{Result, fail, read_limited};
use photocraft_plugin_sdk::{api::AuthoringManifest, authoring::generate};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{io::Write, path::Path};

include!(concat!(env!("OUT_DIR"), "/sdk_sources.rs"));

fn write_new(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

fn sdk_files(directory: &Path) -> Result<Value> {
    let mut index = Vec::new();
    for (name, bytes) in SDK_FILES {
        write_new(directory, name, bytes)?;
        index.push(json!({"path":name,"bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(bytes))}));
    }
    write_new(directory, "Cargo.toml", b"[workspace]\nresolver = \"3\"\nmembers = [\"plugin-api\", \"plugin-sdk\"]\n")?;
    let report = json!({"formatVersion":1,"sdkVersion":photocraft_plugin_sdk::SDK_VERSION,"panelApi":2,"packageVersion":2,"files":index});
    write_new(directory, "SDK-RELEASE.json", &serde_json::to_vec_pretty(&report)?)?;
    Ok(report)
}

pub(crate) fn export(directory: &Path) -> Result<Value> {
    std::fs::create_dir(directory)?;
    let report = sdk_files(directory)?;
    Ok(json!({"formatVersion":1,"status":"passed","sdk":directory,"release":report}))
}

pub(crate) fn source(path: &Path) -> Result<AuthoringManifest> {
    Ok(AuthoringManifest::parse(&read_limited(path, 64 << 10)?)?)
}

pub(crate) fn manifests(input: &Path, destination: &Path) -> Result<Value> {
    let source = source(input)?;
    let generated = generate(&source)?;
    std::fs::create_dir(destination)?;
    generated.write_to(destination)?;
    Ok(json!({"formatVersion":1,"status":"passed","output":destination,"plugin":source.descriptor.id,
        "files":["manifest.json","package.json","bindings.rs"]}))
}

pub(crate) fn new_project(input: &Path, destination: &Path) -> Result<Value> {
    let source = source(input)?;
    if source.package.native.is_some() || !source.package.filters.is_empty() {
        return Err(fail("new creates a WASM panel project; use generate with your own filter/native source layout"));
    }
    std::fs::create_dir(destination)?;
    std::fs::create_dir(destination.join("vendor"))?;
    sdk_files(&destination.join("vendor"))?;
    write_new(destination, "authoring.json", &serde_json::to_vec_pretty(&source)?)?;
    write_new(
        destination,
        "Cargo.toml",
        br#"[package]
name = "photocraft-guest"
version = "0.1.0"
edition = "2024"
license = "MIT OR Apache-2.0"
rust-version = "1.95"

[workspace]
resolver = "3"
members = ["."]
exclude = ["vendor"]

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
photocraft-plugin-sdk = { path = "vendor/plugin-sdk", version = "0.1.0" }

[build-dependencies]
photocraft-plugin-sdk = { path = "vendor/plugin-sdk", version = "0.1.0", features = ["authoring"] }

[lints.rust]
unsafe_code = "deny"
"#,
    )?;
    write_new(
        destination,
        "build.rs",
        br#"fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=authoring.json");
    let source = photocraft_plugin_sdk::api::AuthoringManifest::parse(&std::fs::read("authoring.json")?)?;
    let generated = photocraft_plugin_sdk::authoring::generate(&source)?;
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("missing Cargo output directory")?);
    generated.write_to(&output)?;
    Ok(())
}
"#,
    )?;
    write_new(
        destination,
        "src/lib.rs",
        br#"#![deny(unsafe_code)]
use photocraft_plugin_sdk::api::{EventRequest, PanelResponse, Result};
pub mod bindings { include!(concat!(env!("OUT_DIR"), "/bindings.rs")); }

fn handle(request: &EventRequest) -> Result<PanelResponse> {
    Ok(PanelResponse {
        state: request.state.clone(),
        message: format!("{}: {}", bindings::PLUGIN_ID, request.event),
        ..PanelResponse::default()
    })
}

photocraft_plugin_sdk::export_plugin!(bindings::MANIFEST_JSON, handle);
"#,
    )?;
    write_new(destination,"README.md",b"# Independent PhotoCraft plugin\n\nEdit authoring.json for identity, permissions and contributions. Cargo generates the embedded\nmanifest and bindings from it. Implement your feature in src/lib.rs using the PhotoCraft Plugin\nAPI and guest SDK.\n\nRun cargo test --locked, cargo clippy --locked --all-targets -- -D warnings, then\ncargo build --locked --release --target wasm32-unknown-unknown. Use photocraft-plugin\nbuild-export SOURCE FRESH_DESTINATION to validate a clean export and create the package.\n\nThe SDK source under vendor/ is private to this project. No editor source, other plugin or\nshared runtime installation is needed. Review vendor/SDK-RELEASE.json before SDK updates.\n")?;
    write_new(destination,"AGENTS.md",b"# Standalone plugin project\n\nRead README.md, authoring.json and src/lib.rs. Keep local dependencies inside this root.\nEdit authoring.json, not generated Cargo output; preserve the stable plugin identity on updates.\nThe SDK macro owns bounded ABI buffers. Never add unsafe pointer operations or host imports.\nImplement feature logic here and request only required host capabilities. Generated bindings\nand discovery do not grant authority. Run native checks, actual release WASM host validation,\nand install/invoke/Undo tests in a disposable store. Keep target/, dist/ and generated scratch\nfiles out of source commits. Plugin releases, package formats, filter ABI and native worker\nprotocols are versioned independently.\n")?;
    write_new(destination, ".gitignore", b"/target/\n/dist/\n")?;
    for name in ["LICENSE-MIT", "LICENSE-APACHE"] {
        let key = format!("plugin-sdk/{name}");
        let (_, bytes) = SDK_FILES.iter().find(|(path, _)| *path == key).ok_or_else(|| fail("SDK license missing"))?;
        write_new(destination, name, bytes)?;
        if name == "LICENSE-MIT" {
            write_new(destination, "LICENSE", bytes)?;
        }
    }
    write_new(destination,"THIRD-PARTY-NOTICES.md",b"# Dependency notices\n\nThe privately included PhotoCraft API and SDK are MIT OR Apache-2.0; their licenses\nare retained under vendor/. Published Rust dependencies and versions are recorded in\nCargo.lock. The package builder collects dependency license files from Cargo sources.\n")?;
    for project in [destination.join("vendor"), destination.to_path_buf()] {
        let status = crate::project::cargo_command().args(["generate-lockfile", "--offline"]).current_dir(project).status()?;
        if !status.success() {
            return Err(fail(
                "project sources created, but offline lockfile resolution failed; populate Cargo caches and resolve dependencies before building",
            ));
        }
    }
    Ok(json!({"formatVersion":1,"status":"passed","project":destination,"plugin":source.descriptor.id,
        "dependencyOwnership":"SDK sources belong to this project; no editor or sibling plugin paths", "buildVerified":false}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_export_is_complete_and_existing_destinations_are_not_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("sdk");
        export(&output).unwrap();
        assert!(output.join("plugin-api/src/events.rs").is_file());
        assert!(output.join("plugin-sdk/src/lib.rs").is_file());
        let original = std::fs::read(output.join("SDK-RELEASE.json")).unwrap();
        assert!(export(&output).is_err());
        assert_eq!(std::fs::read(output.join("SDK-RELEASE.json")).unwrap(), original);
        assert!(SDK_FILES.iter().all(|(path, _)| !path.contains("engine") && !path.contains("ui-egui")));
    }
}
