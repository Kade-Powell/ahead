use crate::RolloutItem;
use crate::protocol::EventMsg;
use codex_protocol::items::TurnItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::SubAgentActivityKind;
use codex_protocol::protocol::ThreadHistoryMode;

/// Whether a history `item` belongs in durable thread replay.
pub fn is_persisted_rollout_item(item: &RolloutItem, history_mode: ThreadHistoryMode) -> bool {
    match item {
        RolloutItem::ResponseItem(item) => should_persist_response_item(&item.item),
        RolloutItem::InterAgentCommunication(_)
        | RolloutItem::InterAgentCommunicationMetadata { .. } => true,
        RolloutItem::EventMsg(ev) => should_persist_event_msg(ev, history_mode),
        RolloutItem::RealtimeItem(_) => matches!(history_mode, ThreadHistoryMode::Paginated),
        // Preserve runtime control records required to reconstruct a managed turn.
        RolloutItem::Compacted(_)
        | RolloutItem::TurnContext(_)
        | RolloutItem::WorldState(_)
        | RolloutItem::SecurityRiskScore(_)
        | RolloutItem::SessionMeta(_) => true,
    }
}

/// Return the history items retained by the managed thread store.
pub fn persisted_rollout_items(
    items: &[RolloutItem],
    history_mode: ThreadHistoryMode,
) -> Vec<RolloutItem> {
    let mut persisted = Vec::new();
    for item in items {
        if is_persisted_rollout_item(item, history_mode) {
            persisted.push(item.clone());
        }
    }
    persisted
}

/// Whether a response item belongs in durable model replay.
#[inline]
pub fn should_persist_response_item(item: &ResponseItem) -> bool {
    match item {
        ResponseItem::Message { .. }
        | ResponseItem::AgentMessage { .. }
        | ResponseItem::Reasoning { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::FunctionCall { .. }
        | ResponseItem::ToolSearchCall { .. }
        | ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::CustomToolCall { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::WebSearchCall { .. }
        | ResponseItem::ImageGenerationCall { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::ContextCompaction { .. } => true,
        ResponseItem::AdditionalTools { .. }
        | ResponseItem::CompactionTrigger { .. }
        | ResponseItem::Other => false,
    }
}

/// Whether an event belongs in durable model replay.
#[inline]
pub fn should_persist_event_msg(ev: &EventMsg, history_mode: ThreadHistoryMode) -> bool {
    match ev {
        EventMsg::ItemCompleted(event) => {
            // Paginated rollouts store TurnItems.
            // Legacy rollouts keep only items with no lossless raw ResponseItem or legacy
            // equivalent.
            matches!(history_mode, ThreadHistoryMode::Paginated)
                || matches!(
                    event.item,
                    TurnItem::FunctionCallOutput(_) | TurnItem::Plan(_) | TurnItem::Sleep(_)
                )
                || matches!(
                    &event.item,
                    TurnItem::SubAgentActivity(item)
                        if item.kind == SubAgentActivityKind::Completed
                )
        }
        EventMsg::TokenCount(_)
        | EventMsg::ThreadGoalUpdated(_)
        | EventMsg::ThreadRolledBack(_)
        | EventMsg::TurnAborted(_)
        | EventMsg::TurnStarted(_)
        | EventMsg::TurnComplete(_)
        | EventMsg::ThreadSettingsApplied(_) => true,

        // Only persist these legacy events when the thread's history mode is Legacy.
        // New, paginated rollouts persist ItemCompleted events with TurnItems.
        EventMsg::UserMessage(_)
        | EventMsg::AgentMessage(_)
        | EventMsg::AgentReasoning(_)
        | EventMsg::AgentReasoningRawContent(_)
        | EventMsg::PatchApplyEnd(_)
        | EventMsg::ContextCompacted(_)
        | EventMsg::McpToolCallEnd(_)
        | EventMsg::WebSearchEnd(_)
        | EventMsg::ImageGenerationEnd(_) => {
            matches!(history_mode, ThreadHistoryMode::Legacy)
        }
        EventMsg::SubAgentActivity(event) => {
            matches!(history_mode, ThreadHistoryMode::Legacy)
                && event.kind != SubAgentActivityKind::Completed
        }

        // Transient, non-durable events.
        EventMsg::Error(_)
        | EventMsg::ThreadQueueChanged(_)
        | EventMsg::ExecCommandEnd(_)
        | EventMsg::ViewImageToolCall(_)
        | EventMsg::CollabAgentSpawnEnd(_)
        | EventMsg::CollabAgentInteractionEnd(_)
        | EventMsg::CollabWaitingEnd(_)
        | EventMsg::CollabCloseEnd(_)
        | EventMsg::CollabResumeEnd(_)
        | EventMsg::DynamicToolCallRequest(_)
        | EventMsg::DynamicToolCallResponse(_)
        | EventMsg::Warning(_)
        | EventMsg::SkillContextLoaded(_)
        | EventMsg::AuthRecoveryStarted(_)
        | EventMsg::AuthRecoveryCompleted(_)
        | EventMsg::RealtimeConversationStarted(_)
        | EventMsg::RealtimeConversationSdp(_)
        | EventMsg::RealtimeConversationRealtime(_)
        | EventMsg::RealtimeConversationClosed(_)
        | EventMsg::SafetyBuffering(_)
        | EventMsg::ModelMismatch(_)
        | EventMsg::ModelVerification(_)
        | EventMsg::TurnModerationMetadata(_)
        | EventMsg::AgentReasoningSectionBreak(_)
        | EventMsg::RawResponseItem(_)
        | EventMsg::RawResponseCompleted(_)
        | EventMsg::SessionConfigured(_)
        | EventMsg::McpToolCallBegin(_)
        | EventMsg::ExecCommandBegin(_)
        | EventMsg::TerminalInteraction(_)
        | EventMsg::ExecCommandOutputDelta(_)
        | EventMsg::ExecApprovalRequest(_)
        | EventMsg::RequestPermissions(_)
        | EventMsg::RequestUserInput(_)
        | EventMsg::ElicitationRequest(_)
        | EventMsg::ApplyPatchApprovalRequest(_)
        | EventMsg::StreamError(_)
        | EventMsg::PatchApplyBegin(_)
        | EventMsg::PatchApplyUpdated(_)
        | EventMsg::TurnDiff(_)
        | EventMsg::RealtimeConversationListVoicesResponse(_)
        | EventMsg::McpStartupUpdate(_)
        | EventMsg::McpStartupComplete(_)
        | EventMsg::WebSearchBegin(_)
        | EventMsg::PlanUpdate(_)
        | EventMsg::ShutdownComplete
        | EventMsg::DeprecationNotice(_)
        | EventMsg::ItemStarted(_)
        | EventMsg::AgentMessageContentDelta(_)
        | EventMsg::PlanDelta(_)
        | EventMsg::ReasoningContentDelta(_)
        | EventMsg::ReasoningRawContentDelta(_)
        | EventMsg::ImageGenerationBegin(_)
        | EventMsg::CollabAgentSpawnBegin(_)
        | EventMsg::CollabAgentInteractionBegin(_)
        | EventMsg::CollabWaitingBegin(_)
        | EventMsg::CollabCloseBegin(_)
        | EventMsg::CollabResumeBegin(_) => false,
    }
}
