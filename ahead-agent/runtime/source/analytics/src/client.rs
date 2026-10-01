//! Local-only analytics sink.
//!
//! Upstream codex batched analytics facts into HTTP requests posted to
//! `{base_url}/codex/analytics-events/events` from a background queue and
//! blocked shutdown for up to 25 seconds while draining it. This build keeps the
//! call sites and the fact model (other modules use the fact types to describe
//! local turn state) but performs no delivery: nothing is buffered, nothing is
//! written to disk, and nothing leaves the process.
//!
//! Every `track_*` method therefore records nothing and returns immediately, and
//! [`AnalyticsEventsClient::is_enabled`] is always `false` so callers skip
//! telemetry-only work entirely.

use codex_protocol::items::CollabAgentToolCallItem;

use crate::facts::CodeModeToolCallFact;
use crate::facts::CodexCompactionEvent;
use crate::facts::ControlToolCallFact;
use crate::facts::HookRunFact;
use crate::facts::ImagePreparationFact;
use crate::facts::SkillInvocation;
use crate::facts::SubAgentThreadStartedInput;
use crate::facts::TrackEventsContext;
use crate::facts::TurnProfileFact;
use crate::facts::TurnResolvedConfigFact;
use crate::facts::TurnTokenUsageFact;

/// Sink for analytics facts that intentionally records nothing.
///
/// The type stays in place so that retained runtime call sites do not have to
/// thread a flag of their own; constructing it
/// is free and every method is a no-op.
#[derive(Clone)]
pub struct AnalyticsEventsClient;

impl AnalyticsEventsClient {
    pub fn disabled() -> Self {
        Self
    }

    pub fn is_enabled(&self) -> bool {
        false
    }

    pub fn track_skill_invocations(
        &self,
        _tracking: TrackEventsContext,
        _invocations: Vec<SkillInvocation>,
    ) {
    }

    pub fn track_subagent_thread_started(&self, _input: SubAgentThreadStartedInput) {}

    pub fn track_collab_tool_call(
        &self,
        _turn_id: String,
        _item: CollabAgentToolCallItem,
        _started_at_ms: i64,
        _completed_at_ms: i64,
    ) {
    }

    pub fn track_code_mode_tool_call(&self, _input: CodeModeToolCallFact) {}

    pub fn track_control_tool_call(&self, _input: ControlToolCallFact) {}

    pub fn track_hook_run(&self, _tracking: TrackEventsContext, _hook: HookRunFact) {}

    pub fn track_compaction(&self, _event: CodexCompactionEvent) {}

    pub fn track_image_preparation(&self, _fact: ImagePreparationFact) {}

    pub fn track_turn_resolved_config(&self, _fact: TurnResolvedConfigFact) {}

    pub fn track_turn_token_usage(&self, _fact: TurnTokenUsageFact) {}

    pub fn track_turn_profile(&self, _fact: TurnProfileFact) {}
}
