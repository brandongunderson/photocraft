//! One authoring input generates the embedded descriptor and package-2 metadata.
use crate::{ApiError, Descriptor, ErrorCode, MAX_DESCRIPTOR_BYTES, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthoringManifest {
    #[cfg_attr(feature = "schema", schemars(range(min = 1, max = 1)))]
    pub format_version: u32,
    pub descriptor: Descriptor,
    pub package: PackageV2,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageV2 {
    #[cfg_attr(feature = "schema", schemars(range(min = 2, max = 2)))]
    pub package_version: u32,
    pub module: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<NativePayload>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "schema", schemars(length(max = 16)))]
    pub filters: Vec<FilterPayload>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct NativePayload {
    pub service: String,
    pub platform: String,
    pub entry: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FilterPayload {
    pub module: String,
}

fn path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 240
        && value != ".photocraft-install.json"
        && value.is_ascii()
        && value.split('/').all(|part| {
            let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$")
                || ["COM", "LPT"]
                    .iter()
                    .any(|prefix| stem.strip_prefix(prefix).is_some_and(|number| matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")));
            !part.is_empty()
                && part.len() <= 128
                && !reserved
                && part != "."
                && part != ".."
                && !part.ends_with(['.', ' '])
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}

impl AuthoringManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_DESCRIPTOR_BYTES {
            return Err(ApiError::new(ErrorCode::LimitExceeded, "authoring manifest exceeds 64 KiB"));
        }
        let source: Self = serde_json::from_slice(bytes).map_err(|e| ApiError::new(ErrorCode::InvalidJson, e.to_string()))?;
        source.validate()?;
        Ok(source)
    }

    pub fn validate(&self) -> Result<()> {
        if self.format_version != 1 || self.package.package_version != 2 {
            return Err(ApiError::new(ErrorCode::UnsupportedVersion, "authoring 1 and package 2 are supported"));
        }
        self.descriptor.validate()?;
        let package = &self.package;
        if package.module != "plugin.pcext" || package.filters.iter().any(|filter| !filter.module.ends_with(".wasm")) {
            return Err(ApiError::new(ErrorCode::InvalidDescriptor, "package needs plugin.pcext and .wasm filter paths"));
        }
        let mut paths = std::collections::BTreeSet::from(["package.json".to_owned(), "license".into(), "third-party-notices.md".into()]);
        for name in
            std::iter::once(&package.module).chain(package.filters.iter().map(|filter| &filter.module)).chain(package.native.iter().map(|native| &native.entry))
        {
            if !path(name) || !paths.insert(name.to_ascii_lowercase()) {
                return Err(ApiError::new(ErrorCode::InvalidDescriptor, "invalid or colliding package path"));
            }
        }
        for name in &paths {
            for (offset, _) in name.match_indices('/') {
                if name.get(..offset).is_some_and(|prefix| paths.contains(prefix)) {
                    return Err(ApiError::new(ErrorCode::InvalidDescriptor, "package file/directory collision"));
                }
            }
        }
        if package.filters.len() > 16 {
            return Err(ApiError::new(ErrorCode::LimitExceeded, "too many packaged filters"));
        }
        if let Some(native) = &package.native {
            let targets = ["windows-x86_64", "windows-aarch64", "linux-x86_64", "linux-aarch64", "macos-x86_64", "macos-aarch64"];
            if !targets.contains(&native.platform.as_str())
                || native.service != "plugin.processImage"
                || !self.descriptor.permissions.commands.contains(&native.service)
            {
                return Err(ApiError::new(ErrorCode::InvalidDescriptor, "native target, service or command grant is invalid"));
            }
            if native.platform.starts_with("windows-") && !native.entry.to_ascii_lowercase().ends_with(".exe") {
                return Err(ApiError::new(ErrorCode::InvalidDescriptor, "Windows worker entries need .exe"));
            }
        }
        if (!package.filters.is_empty() && !self.descriptor.permissions.commands.iter().any(|id| id == "plugin.processImage"))
            || (self.descriptor.permissions.commands.iter().any(|id| id == "plugin.runTask") && package.native.is_none())
        {
            return Err(ApiError::new(ErrorCode::InvalidDescriptor, "processor/task payload and grants disagree"));
        }
        Ok(())
    }
}
