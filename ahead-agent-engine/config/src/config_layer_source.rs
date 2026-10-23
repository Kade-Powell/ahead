use codex_utils_absolute_path::AbsolutePathBuf;
use serde_json::Value as JsonValue;

/// Provenance for one layer in the effective AHEAD runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigLayerSource {
    /// Default configuration supplied with the installed AHEAD package.
    PackagedDefaults { file: AbsolutePathBuf },
    /// User configuration.
    User { file: AbsolutePathBuf },
    /// Overrides supplied for the current session.
    SessionFlags,
}

impl ConfigLayerSource {
    /// A setting from a layer with a higher precedence overrides a setting
    /// from a layer with a lower precedence.
    pub fn precedence(&self) -> i16 {
        match self {
            ConfigLayerSource::PackagedDefaults { .. } => -10,
            ConfigLayerSource::User { .. } => 20,
            ConfigLayerSource::SessionFlags => 30,
        }
    }
}

/// Compares [`ConfigLayerSource`] by precedence, so `A < B` means settings
/// from layer `A` will be overridden by settings from layer `B`.
impl PartialOrd for ConfigLayerSource {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.precedence().cmp(&other.precedence()))
    }
}

/// Identity and version information for a configuration layer.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigLayerMetadata {
    pub name: ConfigLayerSource,
    pub version: String,
}

/// A materialized configuration layer and its provenance.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigLayer {
    pub name: ConfigLayerSource,
    pub version: String,
    pub config: JsonValue,
    pub disabled_reason: Option<String>,
}

pub fn format_config_layer_source(source: &ConfigLayerSource) -> String {
    match source {
        ConfigLayerSource::PackagedDefaults { file } => {
            format!("packaged defaults ({})", file.as_path().display())
        }
        ConfigLayerSource::User { file, .. } => {
            format!("user ({})", file.as_path().display())
        }
        ConfigLayerSource::SessionFlags => "session-flags".to_string(),
    }
}
