use crate::config_requirements::ConfigRequirements;
use crate::config_requirements::ConfigRequirementsToml;
use crate::format_config_layer_source;

use super::fingerprint::record_origins;
use super::fingerprint::version_for_toml;
use super::key_aliases::normalized_with_key_aliases;
use super::merge::merge_toml_values;
use crate::ConfigLayer;
use crate::ConfigLayerMetadata;
use crate::ConfigLayerSource;
use crate::shell_environment_policy::validate_shell_environment_policy_filter_config;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use toml::Value as TomlValue;

/// LoaderOverrides controls optional configuration inputs and loader behavior.
#[derive(Debug, Default, Clone)]
pub struct LoaderOverrides {
    /// Optional configuration file supplied with the AHEAD runtime package.
    pub packaged_defaults_path: Option<AbsolutePathBuf>,
    pub user_config_path: Option<AbsolutePathBuf>,
    pub system_requirements_path: Option<PathBuf>,
    pub ignore_managed_requirements: bool,
    pub ignore_user_config: bool,
    pub ignore_user_exec_policy_rules: bool,
    #[cfg(target_os = "macos")]
    pub managed_preferences_requirements_base64: Option<String>,
}

impl LoaderOverrides {
    /// Returns overrides isolated from host configuration for tests.
    ///
    /// This is intended for tests that should load only repo-controlled config fixtures.
    pub fn isolated_for_tests() -> Self {
        let base = std::env::temp_dir().join("ahead-config-tests");
        Self {
            packaged_defaults_path: None,
            user_config_path: None,
            system_requirements_path: Some(base.join("requirements.toml")),
            ignore_managed_requirements: false,
            ignore_user_config: false,
            ignore_user_exec_policy_rules: false,
            #[cfg(target_os = "macos")]
            managed_preferences_requirements_base64: Some(String::new()),
        }
    }

    pub fn user_config_path(&self, codex_home: &Path) -> std::io::Result<AbsolutePathBuf> {
        match self.user_config_path.as_ref() {
            Some(path) => Ok(path.clone()),
            None => Ok(AbsolutePathBuf::resolve_path_against_base(
                crate::CONFIG_TOML_FILE,
                codex_home,
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigLayerEntry {
    pub name: ConfigLayerSource,
    pub config: TomlValue,
    pub version: String,
    pub disabled_reason: Option<String>,
    raw_toml: Option<RawTomlLayer>,
}

#[derive(Debug, Clone, PartialEq)]
struct RawTomlLayer {
    contents: String,
    base_dir: AbsolutePathBuf,
}

impl ConfigLayerEntry {
    pub fn new(name: ConfigLayerSource, config: TomlValue) -> Self {
        let version = version_for_toml(&config);
        Self {
            name,
            config,
            version,
            disabled_reason: None,
            raw_toml: None,
        }
    }

    pub fn new_with_raw_toml(
        name: ConfigLayerSource,
        config: TomlValue,
        raw_toml: String,
        raw_toml_base_dir: AbsolutePathBuf,
    ) -> Self {
        let version = version_for_toml(&config);
        Self {
            name,
            config,
            version,
            disabled_reason: None,
            raw_toml: Some(RawTomlLayer {
                contents: raw_toml,
                base_dir: raw_toml_base_dir,
            }),
        }
    }

    pub fn new_disabled(
        name: ConfigLayerSource,
        config: TomlValue,
        disabled_reason: impl Into<String>,
    ) -> Self {
        let version = version_for_toml(&config);
        Self {
            name,
            config,
            version,
            disabled_reason: Some(disabled_reason.into()),
            raw_toml: None,
        }
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled_reason.is_some()
    }

    pub fn raw_toml(&self) -> Option<&str> {
        self.raw_toml
            .as_ref()
            .map(|raw_toml| raw_toml.contents.as_str())
    }

    pub fn raw_toml_base_dir(&self) -> Option<&AbsolutePathBuf> {
        self.raw_toml.as_ref().map(|raw_toml| &raw_toml.base_dir)
    }

    pub fn metadata(&self) -> ConfigLayerMetadata {
        ConfigLayerMetadata {
            name: self.name.clone(),
            version: self.version.clone(),
        }
    }

    pub fn as_layer(&self) -> ConfigLayer {
        ConfigLayer {
            name: self.name.clone(),
            version: self.version.clone(),
            config: serde_json::to_value(&self.config).unwrap_or(JsonValue::Null),
            disabled_reason: self.disabled_reason.clone(),
        }
    }

    // Returns the directory containing this config layer, if any.
    pub fn config_folder(&self) -> Option<AbsolutePathBuf> {
        match &self.name {
            ConfigLayerSource::PackagedDefaults { .. } => None,
            ConfigLayerSource::User { file, .. } => file.parent(),
            ConfigLayerSource::SessionFlags => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConfigLayerStack {
    /// Layers are listed from lowest precedence (base) to highest (top), so
    /// later entries in the Vec override earlier ones.
    layers: Vec<ConfigLayerEntry>,

    /// Constraints that must be enforced when deriving a [Config] from the
    /// layers.
    requirements: ConfigRequirements,

    /// Raw requirements data as loaded from requirements.toml/MDM/legacy
    /// sources. This preserves the original allow-lists so they can be
    /// surfaced via APIs.
    requirements_toml: ConfigRequirementsToml,

    /// Whether execpolicy should skip `.rules` files from user config folders.
    ignore_user_exec_policy_rules: bool,
}

impl ConfigLayerStack {
    pub fn new(
        layers: Vec<ConfigLayerEntry>,
        requirements: ConfigRequirements,
        requirements_toml: ConfigRequirementsToml,
    ) -> std::io::Result<Self> {
        validate_enabled_config_layers(&layers)?;
        verify_layer_ordering(&layers)?;
        Ok(Self {
            layers,
            requirements,
            requirements_toml,
            ignore_user_exec_policy_rules: false,
        })
    }

    pub fn with_user_exec_policy_rules_ignored(
        mut self,
        ignore_user_exec_policy_rules: bool,
    ) -> Self {
        self.ignore_user_exec_policy_rules = ignore_user_exec_policy_rules;
        self
    }

    pub fn ignore_user_exec_policy_rules(&self) -> bool {
        self.ignore_user_exec_policy_rules
    }

    pub fn requirements(&self) -> &ConfigRequirements {
        &self.requirements
    }

    pub fn requirements_toml(&self) -> &ConfigRequirementsToml {
        &self.requirements_toml
    }

    /// Returns a new stack with the user layer copied from `other`, preserving
    /// every non-user layer already present in this stack.
    pub fn with_user_layer_from(&self, other: &Self) -> Self {
        let user_layers = other
            .layers
            .iter()
            .filter(|layer| matches!(layer.name, ConfigLayerSource::User { .. }))
            .cloned()
            .collect::<Vec<_>>();
        let mut layers = self
            .layers
            .iter()
            .filter(|layer| !matches!(layer.name, ConfigLayerSource::User { .. }))
            .cloned()
            .collect::<Vec<_>>();
        for user_layer in user_layers {
            match layers
                .iter()
                .position(|layer| layer.name.precedence() > user_layer.name.precedence())
            {
                Some(index) => layers.insert(index, user_layer),
                None => layers.push(user_layer),
            }
        }
        Self {
            layers,
            requirements: self.requirements.clone(),
            requirements_toml: self.requirements_toml.clone(),
            ignore_user_exec_policy_rules: self.ignore_user_exec_policy_rules,
        }
    }

    /// Returns the merged config-layer view.
    ///
    /// This only merges ordinary config layers. Requirements are composed and
    /// tracked separately.
    pub fn effective_config(&self) -> TomlValue {
        let mut merged = TomlValue::Table(toml::map::Map::new());
        for layer in self.layers_low_to_high() {
            merge_toml_values(&mut merged, &layer.config);
        }
        merged
    }

    /// Returns field origins for the merged config-layer view.
    ///
    /// Requirement sources are tracked separately and are not included here.
    pub fn origins(&self) -> HashMap<String, ConfigLayerMetadata> {
        let mut origins = HashMap::new();
        let mut path = Vec::new();

        for layer in self.layers_low_to_high() {
            let config = normalized_with_key_aliases(&layer.config, &[]);
            record_origins(&config, &layer.metadata(), &mut path, &mut origins);
        }

        origins
    }

    /// Returns enabled config layers from lowest precedence to highest.
    ///
    /// Requirement sources are tracked separately and are not included here.
    pub fn layers_low_to_high(&self) -> impl DoubleEndedIterator<Item = &ConfigLayerEntry> {
        self.all_layers_low_to_high()
            .filter(|layer| !layer.is_disabled())
    }

    /// Returns enabled config layers from highest precedence to lowest.
    ///
    /// Requirement sources are tracked separately and are not included here.
    pub fn layers_high_to_low(&self) -> impl DoubleEndedIterator<Item = &ConfigLayerEntry> {
        self.layers_low_to_high().rev()
    }

    /// Returns all config layers, including disabled layers, from lowest
    /// precedence to highest.
    ///
    /// Requirement sources are tracked separately and are not included here.
    pub fn all_layers_low_to_high(&self) -> impl DoubleEndedIterator<Item = &ConfigLayerEntry> {
        self.layers.iter()
    }

    /// Returns all config layers, including disabled layers, from highest
    /// precedence to lowest.
    ///
    /// Requirement sources are tracked separately and are not included here.
    pub fn all_layers_high_to_low(&self) -> impl DoubleEndedIterator<Item = &ConfigLayerEntry> {
        self.all_layers_low_to_high().rev()
    }
}

/// Validates before merging so mixed forms and malformed filter entries cannot be normalized away.
pub(crate) fn validate_enabled_config_layers(layers: &[ConfigLayerEntry]) -> std::io::Result<()> {
    for layer in layers.iter().filter(|layer| !layer.is_disabled()) {
        validate_shell_environment_policy_filter_config(&layer.config).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "invalid shell environment policy in {}: {error}",
                    format_config_layer_source(&layer.name)
                ),
            )
        })?;
    }
    Ok(())
}

/// Ensures precedence ordering of config layers is correct.
fn verify_layer_ordering(layers: &[ConfigLayerEntry]) -> std::io::Result<()> {
    if !layers.iter().map(|layer| &layer.name).is_sorted() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "config layers are not in correct precedence order",
        ));
    }

    Ok(())
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
