//! AHEAD-owned approval context, action formatting and human permission routing.

mod approval_request;
mod prompt;

use std::sync::Arc;

use crate::environment_selection::TurnEnvironmentSnapshot;
use crate::session::step_context::StepContext;
use crate::session::turn_context::TurnContext;

pub(crate) use approval_request::ApprovalRequest;
pub(crate) use approval_request::McpToolAnnotations;
const AHEAD_MAX_APPROVAL_ACTION_STRING_TOKENS: usize = 16_000;

/// Captures turn-scoped environments without retaining MCP bindings or tools.
#[derive(Clone)]
pub(crate) struct ApprovalReviewContext {
    turn: Arc<TurnContext>,
    environments: TurnEnvironmentSnapshot,
}

impl ApprovalReviewContext {
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
        }
    }
}

impl From<Arc<TurnContext>> for ApprovalReviewContext {
    fn from(turn: Arc<TurnContext>) -> Self {
        Self {
            environments: turn.environments.clone(),
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
