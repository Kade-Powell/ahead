//! Context fragments injected into model input.

mod approved_command_prefix_saved;
mod base_instructions;
mod compaction_summary;
mod contextual_user_message;
mod current_time_reminder;
mod developer_instructions;
mod environment_context;
mod image_resize_notice;
mod inter_agent_completion_message;
#[cfg(test)]
mod inter_agent_message;
mod internal_model_context;
mod model_switch_instructions;
mod permissions_instructions;
mod personality_spec_instructions;
#[cfg(feature = "codex-realtime-voice")]
mod realtime_delegation;
#[cfg(feature = "codex-realtime-voice")]
mod realtime_end_instructions;
#[cfg(feature = "codex-realtime-voice")]
mod realtime_start_instructions;
#[cfg(feature = "codex-realtime-voice")]
mod realtime_start_with_instructions;
mod rollout_budget;
mod token_budget_context;
mod turn_aborted;
mod unsupported_media;
mod update_plan_instructions;
mod user_instructions;
mod user_shell_command;
pub(crate) mod world_state;

pub(crate) use approved_command_prefix_saved::APPROVED_COMMAND_PREFIX_SAVED_MESSAGE_PREFIX;
pub(crate) use approved_command_prefix_saved::ApprovedCommandPrefixSaved;
pub(crate) use base_instructions::BaseInstructionsFragment;
pub(crate) use codex_context_fragments::AdditionalContextDeveloperFragment;
pub(crate) use codex_context_fragments::AdditionalContextUserFragment;
pub use codex_context_fragments::ContextualUserFragment;
pub(crate) use compaction_summary::CompactionSummary;
pub(crate) use contextual_user_message::is_contextual_user_fragment;
pub(crate) use contextual_user_message::is_user_authorization_message;
pub(crate) use current_time_reminder::CurrentTimeReminder;
pub(crate) use developer_instructions::DeveloperInstructions;
pub(crate) use image_resize_notice::ImageResizeNotice;
pub(crate) use image_resize_notice::ImageResizeNoticeSource;
pub(crate) use image_resize_notice::ResizedImage;
pub(crate) use inter_agent_completion_message::InterAgentCompletionMessage;
#[cfg(test)]
pub(crate) use inter_agent_message::InterAgentMessage;
#[cfg(test)]
pub(crate) use inter_agent_message::InterAgentMessageType;
pub use internal_model_context::InternalContextSource;
pub use internal_model_context::InternalModelContextFragment;
pub use internal_model_context::InvalidInternalContextSource;
pub(crate) use model_switch_instructions::ModelSwitchInstructions;
pub use permissions_instructions::ApprovalPromptContext;
pub use permissions_instructions::PermissionsInstructions;
pub(crate) use personality_spec_instructions::PersonalitySpecInstructions;
#[cfg(feature = "codex-realtime-voice")]
pub(crate) use realtime_delegation::RealtimeDelegation;
#[cfg(feature = "codex-realtime-voice")]
pub(crate) use realtime_delegation::RealtimeDelegationSource;
#[cfg(feature = "codex-realtime-voice")]
pub(crate) use realtime_end_instructions::RealtimeEndInstructions;
#[cfg(feature = "codex-realtime-voice")]
pub(crate) use realtime_start_instructions::RealtimeStartInstructions;
#[cfg(feature = "codex-realtime-voice")]
pub(crate) use realtime_start_with_instructions::RealtimeStartWithInstructions;
pub(crate) use rollout_budget::RolloutBudgetContext;
pub(crate) use token_budget_context::AutoCompactFallbackPrompt;
pub(crate) use token_budget_context::ContextWindowGuidance;
pub(crate) use token_budget_context::TokenBudgetContext;
pub(crate) use token_budget_context::TokenBudgetRemainingContext;
pub(crate) use token_budget_context::TokenBudgetReminder;
pub(crate) use turn_aborted::TurnAborted;
pub(crate) use unsupported_media::UnsupportedMedia;
pub use update_plan_instructions::without_update_plan_instructions;
pub(crate) use user_instructions::UserInstructions;
pub(crate) use user_shell_command::UserShellCommand;
pub(crate) use world_state::ManagedDeveloperInstructions;
