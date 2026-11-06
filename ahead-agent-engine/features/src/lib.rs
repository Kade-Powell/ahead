//! Centralized feature flags and metadata.
//!
//! This crate defines the feature registry plus the logic used to resolve an
//! effective feature set from config-like inputs.

use codex_otel::SessionTelemetry;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WarningEvent;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use toml::Table;

mod feature_configs;
pub use feature_configs::CodeModeConfigToml;
pub use feature_configs::CodeModeHostConfigToml;
pub use feature_configs::CurrentTimeReminderConfigToml;
pub use feature_configs::CurrentTimeReminderDeliveryMode;
pub use feature_configs::CurrentTimeSource;
pub use feature_configs::MultiAgentV2ConfigToml;
pub use feature_configs::NonPrefixedMcpToolNamesConfigToml;
pub use feature_configs::RolloutBudgetConfigToml;
pub use feature_configs::SleepToolConfigToml;
pub use feature_configs::SleepToolMode;
pub use feature_configs::TokenBudgetConfigToml;
pub use feature_configs::ToolRegistryConfigToml;

/// High-level lifecycle stage for a feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Features that are still under development, not ready for external use
    UnderDevelopment,
    /// Stable features. The feature flag is kept for ad-hoc enabling/disabling
    Stable,
    /// Deprecated feature that should not be used anymore.
    Deprecated,
}

/// Unique features toggled via configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature {
    // Stable.
    /// Enable the default shell tool.
    ShellTool,
    /// Enable the built-in local image viewer.
    ViewImage,
    /// Allow registration of the built-in sleep tool.
    SleepTool,
    // Experimental
    /// Send per-content-entry classifications in internal Responses metadata.
    ContentItemKinds,
    /// Record model-attempted tool calls in internal Responses metadata.
    ExecutedToolCallMetadata,
    /// Enable JavaScript code mode backed by the standalone host process.
    CodeMode,
    /// Run JavaScript code mode in the standalone host process.
    CodeModeHost,
    /// Establish the code-mode host connection during session startup.
    CodeModePrewarm,
    /// Terminate active code mode cells when their turn is interrupted.
    CodeModeInterrupt,
    /// Restrict model-visible tools to code mode entrypoints (`exec`, `wait`).
    CodeModeOnly,
    /// Use the single unified PTY-backed exec tool.
    UnifiedExec,
    /// Stream structured progress while apply_patch input is being generated.
    ApplyPatchStreamingEvents,
    /// Preserve existing line endings when apply_patch updates files.
    ApplyPatchPreserveLineEndings,
    /// Allow exec tools to request additional permissions while staying sandboxed.
    ExecPermissionApprovals,
    /// Require approval before writing input to escalated unified-exec terminals.
    WriteStdinApproval,
    /// Expose the built-in request_permissions tool.
    RequestPermissionsTool,
    /// Allow the model to request web searches that fetch live content.
    WebSearchRequest,
    /// Allow the model to request web searches that fetch cached content.
    /// Takes precedence over `WebSearchRequest`.
    WebSearchCached,
    /// Use the legacy Landlock Linux sandbox fallback instead of the default
    /// bubblewrap pipeline.
    UseLegacyLandlock,
    /// Experimental shell snapshotting.
    ShellSnapshot,
    /// Expose the selected PowerShell execution host's bounded major/minor version.
    PowerShellShellVersion,
    /// Keep policy-filtered shell snapshots entirely in executor memory.
    ShellSnapshotV2,
    /// Use the current working directory for turn diff display paths.
    CwdRelativeTurnDiffs,
    /// Enable runtime metrics snapshots via a manual reader.
    RuntimeMetrics,
    /// Keep active sampling turns alive until a failed network connection recovers.
    UnboundedConnectionRetries,
    /// Respect host system proxy settings for Codex-owned network clients.
    RespectSystemProxy,
    /// Enable collab tools.
    Collab,
    /// Enable task-path-based multi-agent routing.
    MultiAgentV2,
    /// Enable MCP protocol version 2026-07-28 support.
    Mcp20260728,
    /// Describe deferred tool namespaces in the model-visible world state.
    DeferredToolWorldState,
    /// Expose MCP model-visible namespaces without the legacy `mcp__` prefix.
    NonPrefixedMcpToolNames,
    /// Tell the model when a prompt image was resized and include its dimensions.
    ImageResizeNotice,
    /// Apply one shared pixel and token budget to every image, regardless of legacy detail hints.
    UnifiedImageBudget,
    /// Request sequential cutoff reasoning summary delivery.
    ConcurrentReasoningSummaries,
    /// Allow request_user_input in Default collaboration mode.
    DefaultModeRequestUserInput,
    /// Add current context-window metadata to model-visible context.
    TokenBudget,
    /// Track and report a shared token budget across a session's agent threads.
    RolloutBudget,
    /// Add current-time reminders to model-visible context.
    CurrentTimeReminder,
    /// Route MCP tool approval prompts through the MCP elicitation request path.
    ToolCallMcpElicitation,
    /// Enable personality selection in the TUI.
    Personality,
    /// Enable Fast mode selection in the TUI and request layer.
    FastMode,
    /// Enable explicitly requested model changes for later step captures.
    StepModelSwitching,
    /// Enable remote compaction v2 over the normal Responses API.
    RemoteCompactionV2,
    /// Include retained images in the remote compaction context budget.
    CompactionImageBudget,
    /// Retain client-authored developer messages across compacted context windows.
    RetainClientDeveloperMessages,
}

impl Feature {
    pub fn key(self) -> &'static str {
        self.info().key
    }

    pub fn stage(self) -> Stage {
        self.info().stage
    }

    pub fn default_enabled(self) -> bool {
        self.info().default_enabled
    }

    fn info(self) -> &'static FeatureSpec {
        FEATURES
            .iter()
            .find(|spec| spec.id == self)
            .unwrap_or_else(|| unreachable!("missing FeatureSpec for {self:?}"))
    }
}

/// Holds the effective set of enabled features.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Features {
    enabled: BTreeSet<Feature>,
}

#[derive(Debug, Clone, Default)]
pub struct FeatureOverrides {
    pub web_search_request: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FeatureConfigSource<'a> {
    pub features: Option<&'a FeaturesToml>,
}

impl FeatureOverrides {
    fn apply(self, features: &mut Features) {
        if let Some(enabled) = self.web_search_request {
            if enabled {
                features.enable(Feature::WebSearchRequest);
            } else {
                features.disable(Feature::WebSearchRequest);
            }
        }
    }
}

impl Features {
    /// Starts with built-in defaults.
    pub fn with_defaults() -> Self {
        let mut set = BTreeSet::new();
        for spec in FEATURES {
            if spec.default_enabled {
                set.insert(spec.id);
            }
        }
        Self { enabled: set }
    }

    pub fn enabled(&self, f: Feature) -> bool {
        self.enabled.contains(&f)
    }

    pub fn use_legacy_landlock(&self) -> bool {
        self.enabled(Feature::UseLegacyLandlock)
    }

    pub fn enable(&mut self, f: Feature) -> &mut Self {
        self.enabled.insert(f);
        self
    }

    pub fn disable(&mut self, f: Feature) -> &mut Self {
        self.enabled.remove(&f);
        self
    }

    pub fn set_enabled(&mut self, f: Feature, enabled: bool) -> &mut Self {
        if enabled {
            self.enable(f)
        } else {
            self.disable(f)
        }
    }

    pub fn emit_metrics(&self, otel: &SessionTelemetry) {
        for feature in FEATURES {
            if self.enabled(feature.id) != feature.default_enabled {
                otel.counter(
                    "codex.feature.state",
                    /*inc*/ 1,
                    &[
                        ("feature", feature.key),
                        ("value", &self.enabled(feature.id).to_string()),
                    ],
                );
            }
        }
    }

    /// Apply a table of key -> bool toggles (e.g. from TOML).
    pub fn apply_map(&mut self, m: &BTreeMap<String, bool>) {
        for (k, v) in m {
            match feature_for_key(k) {
                Some(feat) => {
                    if *v {
                        self.enable(feat);
                    } else {
                        self.disable(feat);
                    }
                }
                None => {
                    tracing::warn!("unknown feature key in config: {k}");
                }
            }
        }
    }

    pub fn from_sources(
        base: FeatureConfigSource<'_>,
        profile: FeatureConfigSource<'_>,
        overrides: FeatureOverrides,
    ) -> Self {
        let mut features = Features::with_defaults();

        for source in [base, profile] {
            if let Some(feature_entries) = source.features {
                features.apply_toml(feature_entries);
            }
        }

        overrides.apply(&mut features);
        features.normalize_dependencies();

        features
    }

    pub fn enabled_features(&self) -> Vec<Feature> {
        self.enabled.iter().copied().collect()
    }

    pub fn normalize_dependencies(&mut self) {
        if self.enabled(Feature::CodeModeOnly) && !self.enabled(Feature::CodeMode) {
            self.enable(Feature::CodeMode);
        }
    }
}

/// Keys accepted in `[features]` tables.
pub fn feature_for_key(key: &str) -> Option<Feature> {
    FEATURES
        .iter()
        .find(|spec| spec.key == key)
        .map(|spec| spec.id)
}

/// Returns `true` if the provided string matches a known `[features]` key.
pub fn is_known_feature_key(key: &str) -> bool {
    key == "tool_registry" || feature_for_key(key).is_some()
}

/// Deserializable features table for TOML.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, JsonSchema)]
pub struct FeaturesToml {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_registry: Option<ToolRegistryConfigToml>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_mode: Option<FeatureToml<CodeModeConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_mode_host: Option<FeatureToml<CodeModeHostConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub non_prefixed_mcp_tool_names: Option<FeatureToml<NonPrefixedMcpToolNamesConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi_agent_v2: Option<FeatureToml<MultiAgentV2ConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<FeatureToml<TokenBudgetConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollout_budget: Option<FeatureToml<RolloutBudgetConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_time_reminder: Option<FeatureToml<CurrentTimeReminderConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sleep_tool: Option<FeatureToml<SleepToolConfigToml>>,
    /// Boolean feature toggles keyed by canonical or legacy feature name.
    #[serde(flatten)]
    entries: BTreeMap<String, bool>,
}

impl Features {
    fn apply_toml(&mut self, features: &FeaturesToml) {
        let entries = features.entries();
        self.apply_map(&entries);
    }
}

impl FeaturesToml {
    pub fn entries(&self) -> BTreeMap<String, bool> {
        let mut entries = self.entries.clone();
        if let Some(enabled) = self.code_mode.as_ref().and_then(FeatureToml::enabled) {
            entries.insert(Feature::CodeMode.key().to_string(), enabled);
        }
        if let Some(enabled) = self.code_mode_host.as_ref().and_then(FeatureToml::enabled) {
            entries.insert(Feature::CodeModeHost.key().to_string(), enabled);
        }
        if let Some(enabled) = self
            .non_prefixed_mcp_tool_names
            .as_ref()
            .and_then(FeatureToml::enabled)
        {
            entries.insert(Feature::NonPrefixedMcpToolNames.key().to_string(), enabled);
        }
        if let Some(enabled) = self.multi_agent_v2.as_ref().and_then(FeatureToml::enabled) {
            entries.insert(Feature::MultiAgentV2.key().to_string(), enabled);
        }
        if let Some(enabled) = self.token_budget.as_ref().and_then(FeatureToml::enabled) {
            entries.insert(Feature::TokenBudget.key().to_string(), enabled);
        }
        if let Some(enabled) = self.rollout_budget.as_ref().and_then(FeatureToml::enabled) {
            entries.insert(Feature::RolloutBudget.key().to_string(), enabled);
        }
        if let Some(enabled) = self
            .current_time_reminder
            .as_ref()
            .and_then(FeatureToml::enabled)
        {
            entries.insert(Feature::CurrentTimeReminder.key().to_string(), enabled);
        }
        if let Some(enabled) = self.sleep_tool.as_ref().and_then(FeatureToml::enabled) {
            entries.insert(Feature::SleepTool.key().to_string(), enabled);
        }
        entries
    }
}

impl From<BTreeMap<String, bool>> for FeaturesToml {
    fn from(entries: BTreeMap<String, bool>) -> Self {
        Self {
            entries,
            ..Default::default()
        }
    }
}

// To be used for features that need more configuration than just enabled/disabled and
// require a custom config struct under `[features]`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(untagged)]
pub enum FeatureToml<T> {
    Enabled(bool),
    Config(T),
}

impl<T: FeatureConfig> FeatureToml<T> {
    pub fn enabled(&self) -> Option<bool> {
        match self {
            Self::Enabled(enabled) => Some(*enabled),
            Self::Config(config) => config.enabled(),
        }
    }
}

// A trait to be implemented by custom feature config structs when defining a feature that needs more configuration than
// just enabled/disabled.
pub trait FeatureConfig {
    fn enabled(&self) -> Option<bool>;
}

/// Single, easy-to-read registry of all feature definitions.
#[derive(Debug, Clone, Copy)]
pub struct FeatureSpec {
    pub id: Feature,
    pub key: &'static str,
    pub stage: Stage,
    pub default_enabled: bool,
}

pub const FEATURES: &[FeatureSpec] = &[
    // Stable features.
    FeatureSpec {
        id: Feature::ShellTool,
        key: "shell_tool",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::ViewImage,
        key: "view_image",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::SleepTool,
        key: "sleep_tool",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::UnifiedExec,
        key: "unified_exec",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::ShellSnapshot,
        key: "shell_snapshot",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::PowerShellShellVersion,
        key: "powershell_shell_version",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ShellSnapshotV2,
        key: "shell_snapshot_v2",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CwdRelativeTurnDiffs,
        key: "cwd_relative_turn_diffs",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ContentItemKinds,
        key: "content_item_kinds",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::ExecutedToolCallMetadata,
        key: "executed_tool_call_metadata",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CodeMode,
        key: "code_mode",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CodeModeHost,
        key: "code_mode_host",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::CodeModePrewarm,
        key: "code_mode_prewarm",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CodeModeInterrupt,
        key: "code_mode_interrupt",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CodeModeOnly,
        key: "code_mode_only",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::WebSearchRequest,
        key: "web_search_request",
        stage: Stage::Deprecated,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::WebSearchCached,
        key: "web_search_cached",
        stage: Stage::Deprecated,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::RuntimeMetrics,
        key: "runtime_metrics",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ApplyPatchStreamingEvents,
        key: "apply_patch_streaming_events",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ApplyPatchPreserveLineEndings,
        key: "apply_patch_preserve_line_endings",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ExecPermissionApprovals,
        key: "exec_permission_approvals",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::WriteStdinApproval,
        key: "write_stdin_approval",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::RequestPermissionsTool,
        key: "request_permissions_tool",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::UseLegacyLandlock,
        key: "use_legacy_landlock",
        stage: Stage::Deprecated,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::UnboundedConnectionRetries,
        key: "unbounded_connection_retries",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::RespectSystemProxy,
        key: "respect_system_proxy",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::Collab,
        key: "multi_agent",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::MultiAgentV2,
        key: "multi_agent_v2",
        stage: Stage::Stable,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::Mcp20260728,
        key: "mcp_2026_07_28",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::DeferredToolWorldState,
        key: "deferred_tool_world_state",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::NonPrefixedMcpToolNames,
        key: "non_prefixed_mcp_tool_names",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ImageResizeNotice,
        key: "image_resize_notice",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::UnifiedImageBudget,
        key: "unified_image_budget",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ConcurrentReasoningSummaries,
        key: "concurrent_reasoning_summaries",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::DefaultModeRequestUserInput,
        key: "default_mode_request_user_input",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::TokenBudget,
        key: "token_budget",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::RolloutBudget,
        key: "rollout_budget",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CurrentTimeReminder,
        key: "current_time_reminder",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ToolCallMcpElicitation,
        key: "tool_call_mcp_elicitation",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::Personality,
        key: "personality",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::FastMode,
        key: "fast_mode",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::StepModelSwitching,
        key: "step_model_switching",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::RemoteCompactionV2,
        key: "remote_compaction_v2",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::CompactionImageBudget,
        key: "compaction_image_budget",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::RetainClientDeveloperMessages,
        key: "retain_client_developer_messages",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
];

pub fn unstable_features_warning_event(
    effective_features: Option<&Table>,
    suppress_unstable_features_warning: bool,
    features: &Features,
    config_path: &str,
) -> Option<Event> {
    if suppress_unstable_features_warning {
        return None;
    }

    let mut under_development_feature_keys = Vec::new();
    if let Some(table) = effective_features {
        for (key, value) in table {
            let is_enabled = value.as_bool() == Some(true)
                || value
                    .as_table()
                    .and_then(|table| table.get("enabled"))
                    .and_then(toml::Value::as_bool)
                    == Some(true);
            if !is_enabled {
                continue;
            }
            let Some(spec) = FEATURES.iter().find(|spec| spec.key == key.as_str()) else {
                continue;
            };
            if !features.enabled(spec.id) {
                continue;
            }
            if matches!(spec.stage, Stage::UnderDevelopment) {
                under_development_feature_keys.push(spec.key.to_string());
            }
        }
    }

    if under_development_feature_keys.is_empty() {
        return None;
    }

    under_development_feature_keys.sort();
    let under_development_feature_keys = under_development_feature_keys.join(", ");
    let message = format!(
        "Under-development features enabled: {under_development_feature_keys}. Under-development features are incomplete and may behave unpredictably. To suppress this warning, set `suppress_unstable_features_warning = true` in {config_path}."
    );
    Some(Event {
        id: String::new(),
        msg: EventMsg::Warning(WarningEvent { message }),
    })
}

#[cfg(test)]
mod tests;
