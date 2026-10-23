use crate::FeatureConfig;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolRegistryConfigToml {
    /// Fail the turn when multiple tools share the same effective name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_on_tool_collisions: Option<bool>,
    /// Include authoritative tool information in per-turn request metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_metadata_includes_tool_info: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeModeConfigToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Default yield timeout for code-mode exec calls, in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_exec_yield_time_ms: Option<u64>,
    /// Exact tool namespaces to omit from the code-mode nested tool surface.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excluded_tool_namespaces: Option<Vec<String>>,
    /// Exact tool namespaces to expose only as direct model tools.
    /// These tools bypass deferral, remain top-level in code-mode-only sessions, and are omitted
    /// from the nested code-mode tool surface.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct_only_tool_namespaces: Option<Vec<String>>,
}

impl FeatureConfig for CodeModeConfigToml {
    fn enabled(&self) -> Option<bool> {
        self.enabled
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeModeHostConfigToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Keep code mode fail-closed when the standalone host is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_in_process_fallback: Option<bool>,
}

impl FeatureConfig for CodeModeHostConfigToml {
    fn enabled(&self) -> Option<bool> {
        self.enabled
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NonPrefixedMcpToolNamesConfigToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// MCP servers whose tools should omit the legacy `mcp__` namespace prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_names: Option<Vec<String>>,
}

impl FeatureConfig for NonPrefixedMcpToolNamesConfigToml {
    fn enabled(&self) -> Option<bool> {
        self.enabled
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MultiAgentV2ConfigToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub max_concurrent_threads_per_session: Option<usize>,
}

impl FeatureConfig for MultiAgentV2ConfigToml {
    fn enabled(&self) -> Option<bool> {
        self.enabled
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TokenBudgetConfigToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Whether to expose the built-in history and notes extension.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_history_notes_extension: Option<bool>,
    /// Number of tokens remaining before auto-compaction when the wrap-up reminder is emitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub reminder_threshold_tokens: Option<i64>,
    /// Reminder template. `{n_remaining}` is replaced with the tokens remaining before
    /// auto-compaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 2000))]
    pub reminder_message_template: Option<String>,
    /// Guidance appended to the context-window metadata in a developer message.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 2000))]
    pub guidance_message: Option<String>,
    /// Developer message sampled before an automatic context-window rollover.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 2000))]
    pub auto_compact_fallback_prompt: Option<String>,
    /// Additional tokens available after the compaction threshold for fallback note-taking.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub auto_compact_fallback_buffer_tokens: Option<i64>,
}

impl FeatureConfig for TokenBudgetConfigToml {
    fn enabled(&self) -> Option<bool> {
        self.enabled
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RolloutBudgetConfigToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub limit_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Remaining weighted-token values that trigger reminders when crossed.
    pub reminder_at_remaining_tokens: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0))]
    pub sampling_token_weight: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0))]
    pub prefill_token_weight: Option<f64>,
}

impl FeatureConfig for RolloutBudgetConfigToml {
    fn enabled(&self) -> Option<bool> {
        self.enabled
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CurrentTimeSource {
    #[default]
    System,
    External,
}

/// Which inference boundaries may receive current-time reminders.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CurrentTimeReminderDeliveryMode {
    /// Allow a reminder before any inference request once the interval is due.
    #[default]
    AnyInference,
    /// Allow reminders after user input or tool output; new context windows still force one.
    AfterUserOrToolOutput,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CurrentTimeReminderConfigToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reminder_interval_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clock_source: Option<CurrentTimeSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_mode: Option<CurrentTimeReminderDeliveryMode>,
    /// Expose the input-interruptible `clock.sleep` tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sleep_tool: Option<bool>,
}

impl FeatureConfig for CurrentTimeReminderConfigToml {
    fn enabled(&self) -> Option<bool> {
        self.enabled
    }
}

/// How the sleep tool is selected when its feature gate is enabled.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SleepToolMode {
    /// Preserve the existing model and legacy clock configuration defaults.
    #[default]
    ModelDriven,
    /// Register sleep regardless of the model or legacy clock configuration.
    AlwaysOn,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SleepToolConfigToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<SleepToolMode>,
}

impl FeatureConfig for SleepToolConfigToml {
    fn enabled(&self) -> Option<bool> {
        self.enabled
    }
}
