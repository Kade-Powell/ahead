//! AHEAD-owned approval context, action formatting and human permission routing.

mod approval_request;
mod prompt;

use codex_protocol::protocol::AskForApproval;
use std::sync::Arc;

use crate::environment_selection::TurnEnvironmentSnapshot;
use crate::session::step_context::StepContext;
use crate::session::step_settings::ResolvedStepSettings;
use crate::session::turn_context::TurnContext;

pub(crate) use approval_request::ApprovalRequest;
pub(crate) use approval_request::McpToolAnnotations;
pub(crate) use approval_request::NetworkAccessTrigger;
const AHEAD_MAX_APPROVAL_ACTION_STRING_TOKENS: usize = 16_000;

/// Captures approval policy and environments without retaining step-scoped MCP bindings or tools.
/// Background permission checks use turn-only settings when there is no issuing step.
#[derive(Clone)]
pub(crate) struct ApprovalReviewContext {
    turn: Arc<TurnContext>,
    environments: TurnEnvironmentSnapshot,
    pub(crate) approval_policy: AskForApproval,
}

impl ApprovalReviewContext {
    pub(crate) fn from_resolved_settings(
        turn: Arc<TurnContext>,
        settings: &ResolvedStepSettings,
    ) -> Self {
        Self {
            environments: turn.environments.clone(),
            approval_policy: settings.approval_policy(),
            turn,
        }
    }

    pub(crate) fn turn(&self) -> &Arc<TurnContext> {
        &self.turn
    }

    pub(crate) fn environments(&self) -> &TurnEnvironmentSnapshot {
        &self.environments
    }
}

impl From<&Arc<StepContext>> for ApprovalReviewContext {
    fn from(step: &Arc<StepContext>) -> Self {
        Self {
            turn: Arc::clone(&step.turn),
            environments: step.environments.clone(),
            approval_policy: step.settings.approval_policy(),
        }
    }
}

impl From<Arc<TurnContext>> for ApprovalReviewContext {
    fn from(turn: Arc<TurnContext>) -> Self {
        Self {
            environments: turn.environments.clone(),
            approval_policy: turn.approval_policy(),
            turn,
        }
    }
}

impl From<&Arc<TurnContext>> for ApprovalReviewContext {
    fn from(turn: &Arc<TurnContext>) -> Self {
        Self::from(Arc::clone(turn))
    }
}

pub(crate) use approval_request::format_approval_action_pretty;
