//! Governed-turn assembly shared by shells and the proxy loop.
//!
//! Mirrors the vocabulary in `lapce-proxy/src/ahead/agent.rs`
//! (`TurnApprovalPolicy`, `TurnSandbox`, Codex `turn/start` params,
//! approval decisions) without depending on the proxy crate, so both UI
//! shells and future runtimes assemble identical turns.

use lapce_rpc::ahead::AssistanceMode;

/// What the built-in loop may do on a turn. Mirrors the fork's
/// `AskForApproval` vocabulary; the host answers every approval from
/// `PolicyEvaluator`, never from the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ApprovalPolicy {
    /// Learn: read-only. Every mutation/execution approval is declined.
    Never,
    /// Assist: approvals routed to the human through proposal gates.
    OnRequest,
}

impl ApprovalPolicy {
    pub fn for_mode(mode: AssistanceMode) -> Self {
        match mode {
            AssistanceMode::Learn => Self::Never,
            AssistanceMode::Assist => Self::OnRequest,
        }
    }

    /// Fork wire value (`protocol/v2/shared.rs AskForApproval`).
    pub fn codex_wire_value(&self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::OnRequest => "untrusted",
        }
    }
}

/// Sandbox mapping: Learn → `read-only`, Assist → `workspace-write`.
/// `danger-full-access` is never emitted by a managed session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Sandbox {
    ReadOnly,
    WorkspaceWrite,
}

impl Sandbox {
    pub fn for_mode(mode: AssistanceMode) -> Self {
        match mode {
            AssistanceMode::Learn => Self::ReadOnly,
            AssistanceMode::Assist => Self::WorkspaceWrite,
        }
    }

    /// Fork wire value (`protocol/v2/shared.rs SandboxMode`).
    pub fn codex_wire_value(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
        }
    }
}

/// Minimal turn assembly input (framework-free subset of `AgentTurnRequest`).
#[derive(Debug, Clone)]
pub struct TurnAssembly {
    pub thread_id: String,
    pub user_message: String,
    pub cwd: Option<String>,
}

/// Serializes a turn the way the fork's `turn/start` params carry it:
/// `{thread_id, input:[{text}], cwd?, approvalPolicy?, sandboxPolicy?}`.
pub fn codex_turn_params(
    turn: &TurnAssembly,
    mode: AssistanceMode,
) -> serde_json::Value {
    let mut params = serde_json::Map::new();
    params.insert(
        "thread_id".to_string(),
        serde_json::Value::String(turn.thread_id.clone()),
    );
    params.insert(
        "input".to_string(),
        serde_json::json!([{ "text": turn.user_message }]),
    );
    if let Some(cwd) = &turn.cwd {
        params.insert("cwd".to_string(), serde_json::Value::String(cwd.clone()));
    }
    params.insert(
        "approvalPolicy".to_string(),
        serde_json::Value::String(
            ApprovalPolicy::for_mode(mode).codex_wire_value().to_string(),
        ),
    );
    params.insert(
        "sandboxPolicy".to_string(),
        serde_json::Value::String(Sandbox::for_mode(mode).codex_wire_value().to_string()),
    );
    serde_json::Value::Object(params)
}

/// Answers a fork-style approval request from policy. Learn always
/// declines; Assist accepts only mechanical proposals inside an explicit
/// scope the human already approved. Unknown kinds fail closed.
pub fn decide_approval(
    mode: AssistanceMode,
    kind: &str,
    is_mechanical: bool,
    in_approved_scope: bool,
) -> bool {
    match mode {
        AssistanceMode::Learn => false,
        AssistanceMode::Assist => {
            matches!(kind, "file_change" | "command_execution")
                && is_mechanical
                && in_approved_scope
        }
    }
}

/// Re-exports under the proxy's names so shells share one vocabulary.
pub fn approval_policy_for_mode(mode: AssistanceMode) -> ApprovalPolicy {
    ApprovalPolicy::for_mode(mode)
}

pub fn sandbox_for_mode(mode: AssistanceMode) -> Sandbox {
    Sandbox::for_mode(mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn() -> TurnAssembly {
        TurnAssembly {
            thread_id: "t".into(),
            user_message: "hello".into(),
            cwd: None,
        }
    }

    #[test]
    fn params_follow_codex_vocabulary() {
        let assist = codex_turn_params(&turn(), AssistanceMode::Assist);
        assert_eq!(assist["approvalPolicy"], "untrusted");
        assert_eq!(assist["sandboxPolicy"], "workspace-write");
        let learn = codex_turn_params(&turn(), AssistanceMode::Learn);
        assert_eq!(learn["approvalPolicy"], "never");
        assert_eq!(learn["sandboxPolicy"], "read-only");
    }

    #[test]
    fn approvals_fail_closed() {
        assert!(!decide_approval(AssistanceMode::Learn, "file_change", true, true));
        assert!(!decide_approval(AssistanceMode::Assist, "file_change", false, true));
        assert!(!decide_approval(AssistanceMode::Assist, "file_change", true, false));
        assert!(!decide_approval(AssistanceMode::Assist, "shell", true, true));
        assert!(decide_approval(AssistanceMode::Assist, "file_change", true, true));
    }

    #[test]
    fn no_danger_full_access_wire_value_exists() {
        for mode in [AssistanceMode::Learn, AssistanceMode::Assist] {
            assert_ne!(Sandbox::for_mode(mode).codex_wire_value(), "danger-full-access");
        }
    }
}
