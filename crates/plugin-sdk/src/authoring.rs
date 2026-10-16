//! Optional native build-time generation; not needed by the WASM guest at runtime.
use crate::api;
use api::{ApiError, AuthoringManifest, ErrorCode, Result};
use std::path::Path;

pub struct Generated {
    pub manifest: Vec<u8>,
    pub package: Vec<u8>,
    pub bindings: String,
}

pub fn generate(source: &AuthoringManifest) -> Result<Generated> {
    source.validate()?;
    let manifest = serde_json::to_vec_pretty(&source.descriptor).map_err(|e| ApiError::new(ErrorCode::InvalidJson, e.to_string()))?;
    let package = serde_json::to_vec_pretty(&source.package).map_err(|e| ApiError::new(ErrorCode::InvalidJson, e.to_string()))?;
    let descriptor = &source.descriptor;
    let events = descriptor
        .panel
        .buttons
        .iter()
        .map(|button| button.id.as_str())
        .chain(descriptor.contributes.commands.iter().map(|command| command.event.as_str()))
        .chain(descriptor.contributes.inspectors.iter().map(|view| view.event.as_str()))
        .collect::<std::collections::BTreeSet<_>>();
    let mut bindings = format!(
        "// Generated from authoring.json.\npub const PLUGIN_ID: &str = {:?};\npub const MANIFEST_JSON: &str = include_str!(\"manifest.json\");\n",
        descriptor.id
    );
    for (index, event) in events.iter().enumerate() {
        bindings.push_str(&format!("pub const EVENT_{index}: &str = {event:?};\n"));
    }
    for (index, command) in descriptor.contributes.commands.iter().enumerate() {
        bindings.push_str(&format!("pub const COMMAND_{index}: &str = {:?};\n", command.id));
    }
    Ok(Generated { manifest, package, bindings })
}

impl Generated {
    /// Write only generated artifacts, normally into the caller's Cargo OUT_DIR.
    pub fn write_to(&self, directory: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(directory)?;
        std::fs::write(directory.join("manifest.json"), &self.manifest)?;
        std::fs::write(directory.join("package.json"), &self.package)?;
        std::fs::write(directory.join("bindings.rs"), &self.bindings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{api::AuthoringManifest, serde_json::json};

    #[test]
    fn source_changes_update_both_metadata_and_bindings() {
        let value = json!({"formatVersion":1,"descriptor":{"apiVersion":2,"id":"org.example.generated","name":"Generated","version":"1",
            "permissions":{"documentRead":false,"commands":[]},"panel":{"buttons":[{"id":"info","label":"Info"}]}},
            "package":{"packageVersion":2,"module":"plugin.pcext"}});
        let mut source = AuthoringManifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
        let first = generate(&source).unwrap();
        source.descriptor.id = "org.example.updated".into();
        source.descriptor.version = "2".into();
        let next = generate(&source).unwrap();
        let manifest: serde_json::Value = serde_json::from_slice(&next.manifest).unwrap();
        assert_eq!(manifest["id"], source.descriptor.id);
        assert_eq!(manifest["version"], "2");
        assert!(next.bindings.contains("org.example.updated"));
        assert!(!next.bindings.contains("org.example.generated"));
        assert_ne!(next.manifest, first.manifest);
        assert_eq!(next.package, first.package);
        source.package.package_version = 3;
        assert!(generate(&source).is_err());
    }
}
