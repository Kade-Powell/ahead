use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;

use super::AdditionalContextUserFragment;
use super::ContextualUserFragment;
use super::InternalModelContextFragment;
use super::TurnAborted;
use super::UserInstructions;
use super::UserShellCommand;
use super::world_state::EnvironmentsState;

const CONTEXTUAL_USER_FRAGMENT_MATCHERS: &[fn(&str) -> bool] = &[
    UserInstructions::matches_text,
    EnvironmentsState::matches_text,
    AdditionalContextUserFragment::matches_text,
    ahead_agent_skills::is_skill_prompt_fragment,
    UserShellCommand::matches_text,
    TurnAborted::matches_text,
    InternalModelContextFragment::matches_text,
];

/// Uses host annotations rather than text markers to identify user authorization changes.
pub(crate) fn is_user_authorization_message(item: &ResponseItem) -> bool {
    let ResponseItem::Message {
        role,
        content,
        internal_chat_message_metadata_passthrough,
        ..
    } = item
    else {
        return false;
    };
    role == "user"
        && internal_chat_message_metadata_passthrough
            .as_ref()
            .and_then(|metadata| metadata.content_item_kinds.as_ref())
            .is_none_or(|kinds| {
                // Unknown, incomplete, and legacy messages remain conservative.
                kinds.is_empty()
                    || kinds.len() != content.len()
                    || kinds.iter().any(|kind| {
                        kind.0.starts_with("user.")
                            || matches!(
                                kind.0.as_str(),
                                "" | "unknown"
                                    // Media preparation can replace real user input.
                                    | "images.preparation_error"
                                    | "images.unsupported"
                                    | "audio.unsupported"
                            )
                    })
            })
}

fn is_standard_contextual_user_text(text: &str) -> bool {
    CONTEXTUAL_USER_FRAGMENT_MATCHERS
        .iter()
        .any(|matches_text| matches_text(text))
}

pub(crate) fn is_contextual_user_fragment(content_item: &ContentItem) -> bool {
    let ContentItem::InputText { text } = content_item else {
        return false;
    };
    is_standard_contextual_user_text(text)
}

#[cfg(test)]
#[path = "contextual_user_message_tests.rs"]
mod tests;
