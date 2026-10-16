use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{ApiError, Capabilities, Contributions, ErrorCode, MAX_DESCRIPTOR_BYTES, PANEL_API_VERSION, Result, event_identifier, identifier, invalid, label};

/// Complete independent descriptor for the current PhotoCraft Plugin API.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Descriptor {
    #[cfg_attr(feature = "schema", schemars(range(min = 1, max = 1)))]
    pub api_version: u32,
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 64), regex(pattern = "^[a-z0-9][a-z0-9._-]*$")))]
    pub id: String,
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 64)))]
    pub name: String,
    #[cfg_attr(feature = "schema", schemars(length(max = 32)))]
    pub version: String,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(length(max = 128)))]
    pub author: String,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(length(max = 1024)))]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(length(max = 160)))]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updates: Option<UpdateSource>,
    pub permissions: Permissions,
    pub panel: Panel,
    #[serde(default, skip_serializing_if = "Capabilities::is_empty")]
    pub capabilities: Capabilities,
    #[serde(default, skip_serializing_if = "Contributions::is_empty")]
    pub contributes: Contributions,
}

/// A plugin-owned release source. Declaring it never starts a network request or installs code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct UpdateSource {
    pub provider: UpdateProvider,
    /// Exact release filename, optionally containing one `{platform}` substitution.
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 160)))]
    pub asset: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum UpdateProvider {
    Github,
}

/// Validate a canonical public GitHub repository URL without depending on an HTTP client.
pub fn github_repository_parts(repository: &str) -> Result<(&str, &str)> {
    let Some(path) = repository.strip_prefix("https://github.com/") else {
        return Err(invalid("repository", "repository must be https://github.com/owner/repo"));
    };
    let Some((owner, name)) = path.split_once('/') else {
        return Err(invalid("repository", "repository must include its owner and name"));
    };
    if repository.len() > 160
        || owner.is_empty()
        || owner.len() > 39
        || owner.starts_with('-')
        || owner.ends_with('-')
        || owner.contains("--")
        || !owner.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || name.is_empty()
        || name.len() > 100
        || matches!(name, "." | "..")
        || name.ends_with(".git")
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(invalid("repository", "repository must be a canonical public GitHub repository URL"));
    }
    Ok((owner, name))
}

impl UpdateSource {
    pub fn validate(&self) -> Result<()> {
        let filename = self.asset.replace("{platform}", "windows-x86_64");
        let stem = filename.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$")
            || ["COM", "LPT"]
                .iter()
                .any(|prefix| stem.strip_prefix(prefix).is_some_and(|number| matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")));
        if self.asset.is_empty()
            || self.asset.len() > 160
            || self.asset.matches("{platform}").count() > 1
            || !filename.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
            || !filename.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || reserved
            || !(filename.ends_with(".pcplugin") || filename.ends_with(".pcext"))
        {
            return Err(invalid("updates.asset", "update asset must be an exact safe .pcplugin or .pcext filename with at most one {platform} token"));
        }
        Ok(())
    }

    pub fn asset_for_platform(&self, platform: &str) -> Result<String> {
        self.validate()?;
        if !["windows-x86_64", "windows-aarch64", "linux-x86_64", "linux-aarch64", "macos-x86_64", "macos-aarch64"].contains(&platform) {
            return Err(invalid("updates.asset", "unsupported update platform"));
        }
        Ok(self.asset.replace("{platform}", platform))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Permissions {
    #[serde(default)]
    pub document_read: bool,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(length(max = 128)))]
    pub commands: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Panel {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preview: bool,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(length(max = 32)))]
    pub fields: Vec<Field>,
    #[cfg_attr(feature = "schema", schemars(length(max = 16)))]
    pub buttons: Vec<Button>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Field {
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 64), regex(pattern = "^[A-Za-z0-9][A-Za-z0-9_]*$")))]
    pub id: String,
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 128)))]
    pub label: String,
    #[serde(flatten)]
    pub spec: Parameter,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Button {
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 64), regex(pattern = "^[A-Za-z0-9][A-Za-z0-9_]*$")))]
    pub id: String,
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 128)))]
    pub label: String,
}

/// Declarative controls with no dependency on the host UI implementation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Parameter {
    Number {
        min: f64,
        max: f64,
        default: f64,
    },
    Int {
        #[serde(default = "int_min")]
        min: i64,
        #[serde(default = "int_max")]
        max: i64,
        #[serde(default)]
        default: i64,
    },
    Bool {
        #[serde(default)]
        default: bool,
    },
    Choice {
        #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 64)))]
        options: Vec<String>,
        #[serde(default)]
        default: Option<String>,
    },
}

fn int_min() -> i64 {
    -1_000_000
}

fn int_max() -> i64 {
    1_000_000
}

impl Descriptor {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_DESCRIPTOR_BYTES {
            return Err(ApiError::new(ErrorCode::LimitExceeded, "descriptor exceeds 64 KiB"));
        }
        let descriptor: Self = serde_json::from_slice(bytes).map_err(|e| ApiError::new(ErrorCode::InvalidJson, e.to_string()))?;
        descriptor.validate()?;
        Ok(descriptor)
    }

    pub fn validate(&self) -> Result<()> {
        if self.api_version != PANEL_API_VERSION {
            return Err(ApiError::at(ErrorCode::IncompatibleApi, "apiVersion", "this descriptor requires the current PhotoCraft Plugin API"));
        }
        if !identifier(&self.id, 64) || !label(&self.name, 64) {
            return Err(invalid("id/name", "plugin ID or name is invalid"));
        }
        if self.version.len() > 32 || self.author.len() > 128 || self.description.len() > 1024 {
            return Err(invalid("version/author/description", "plugin metadata exceeds its byte budget"));
        }
        if let Some(repository) = &self.repository {
            github_repository_parts(repository)?;
        }
        if let Some(updates) = &self.updates {
            if self.repository.is_none() {
                return Err(invalid("updates", "an update source requires a repository"));
            }
            updates.validate()?;
        }
        if self.permissions.commands.len() > 128 {
            return Err(invalid("permissions.commands", "at most 128 command permissions are allowed"));
        }
        let mut command_ids = BTreeSet::new();
        for command in &self.permissions.commands {
            if command.is_empty()
                || command.len() > 128
                || !command.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                || !command_ids.insert(command)
            {
                return Err(invalid("permissions.commands", "invalid or duplicate command permission"));
            }
        }
        self.panel.validate()?;
        if self.panel.buttons.is_empty() && self.contributes.inspectors.is_empty() {
            return Err(invalid("panel.buttons", "a panel without buttons must contribute an inspector"));
        }
        self.capabilities.validate()?;
        self.contributes.validate(&self.id)?;
        Ok(())
    }
}

impl Panel {
    pub fn validate(&self) -> Result<()> {
        if self.fields.len() > 32 || self.buttons.len() > 16 {
            return Err(invalid("panel", "panel permits at most 16 buttons and 32 fields"));
        }
        let mut ids = BTreeSet::new();
        for field in &self.fields {
            if !event_identifier(&field.id) || !label(&field.label, 128) || !ids.insert(&field.id) {
                return Err(invalid("panel.fields", "invalid or duplicate field"));
            }
            match &field.spec {
                Parameter::Number { min, max, default } => {
                    if !min.is_finite() || !max.is_finite() || !default.is_finite() || min > max {
                        return Err(invalid("panel.fields", "numeric parameter has a non-finite or reversed range"));
                    }
                }
                Parameter::Int { min, max, .. } if min > max => return Err(invalid("panel.fields", "integer parameter has a reversed range")),
                Parameter::Choice { options, default }
                    if options.is_empty()
                        || options.len() > 64
                        || options.iter().any(|option| option.is_empty() || option.contains(['|', '"']))
                        || default.as_ref().is_some_and(|value| !options.contains(value)) =>
                {
                    return Err(invalid("panel.fields", "choice parameter has invalid options or default"));
                }
                _ => {}
            }
        }
        ids.clear();
        for button in &self.buttons {
            if !event_identifier(&button.id) || !label(&button.label, 128) || !ids.insert(&button.id) {
                return Err(invalid("panel.buttons", "invalid or duplicate button"));
            }
        }
        Ok(())
    }
}
