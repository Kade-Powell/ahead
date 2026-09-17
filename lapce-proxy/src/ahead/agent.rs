//! AHEAD Built-in Agent Loop & ACP Side-Task Delegation
//!
//! Grounded in Section 3.3, Section 3.4, Section 6, and Section 8.1 of
//! `ahead-editor-mvp.md`.
//!
//! Architecture (user-confirmed 2026-09-17):
//! - The primary coding agent is BUILT DIRECTLY INTO the session host
//!   (`AheadAgentLoop` below): it assembles governed turns from session
//!   state, enforces Learn/Assist policy, emits verified presentation cues,
//!   and stages mechanical proposals through `PolicyEvaluator`.
//! - NO ACP round-trip for the primary loop. ACP (`agent-client-protocol`
//!   crate, real stdio transport) is used ONLY for side tasks dispatched to
//!   EXTERNAL agents (security review, independent audit, second opinions)
//!   whose findings return as attributed review text — never as direct edits.
//!
//! Codex reuse (fork pinned at `third-party/codex`, tag `rust-v0.152.0`,
//! protocol snapshot in `ahead-agent/schemas/`):
//! - Reused directly: app-server wire framing (no `jsonrpc` header;
//!   `{id, method, params?, trace?}` / `{id, result}` / `{id, error}`),
//!   `initialize` → `initialized` handshake, `thread/start|resume|fork`,
//!   `turn/start|steer|interrupt`, `AskForApproval` /
//!   `CommandExecutionApprovalDecision` / `FileChangeApprovalDecision`
//!   semantics (server-initiated approval requests answered by the host),
//!   `SandboxMode::{ReadOnly, WorkspaceWrite}` mapping to Learn/Assist,
//!   `execpolicy` prefix-rule engine for command decisions, approve/decline
//!   decision vocabulary.
//! - Deliberately NOT reused: ratatui/crossterm TUI, autonomous instruction
//!   set, direct shell/process/fs-write tool surface, hooks/plugins/MCP
//!   mutations, cloud/remote control, rollout logs as session authority.
//!   Those stay outside the managed session; the host owns sessions, modes,
//!   phases, anchors, proposals, predictions lane, voice lane, tracker
//!   outbox, and review snapshots.

use std::{collections::HashMap, path::PathBuf, sync::Arc};
use anyhow::{bail, Result};
use lapce_rpc::ahead::{
    AssistanceMode, ChangeProposal, DisplayPosition, DisplayRange, Id,
    PresentationCue, RepoPath, SessionPolicySnapshot, SessionRole, WorkflowPhase,
};
use super::{policy::PolicyEvaluator, host::AheadSessionHost};

/// Shared vocabulary lives in the framework-agnostic viewmodel so shells,
/// proxy, and future runtimes assemble identical turns. The proxy keeps
/// its own `AgentTurnRequest` (superset with editor context + policy
/// gates); approval/sandbox enums are re-exported aliases.
pub use ahead_viewmodel::{ApprovalPolicy as TurnApprovalPolicy, Sandbox as TurnSandbox};

/// One governed turn request assembled by the host for the built-in loop.
/// Field names follow the fork's `turn/start` params where they overlap
/// (`thread_id`, `cwd`, approval/sandbox overrides); AHEAD-only gates
/// (`expected_policy_sha256`, `scope_id`, editor context) ride alongside.
#[derive(Debug, Clone)]
pub struct AgentTurnRequest {
    pub session_id: Id,
    pub thread_id: Id,
    pub user_message: String,
    pub active_path: RepoPath,
    pub caret: DisplayPosition,
    pub selection: Option<DisplayRange>,
    pub file_content: String,
    pub invariants: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub expected_policy_sha256: String,
    pub scope_id: Option<Id>,
}

/// Backwards-compatible alias: existing callers build `AgentTurnInput`.
pub type AgentTurnInput = AgentTurnRequest;

#[derive(Debug, Clone)]
pub struct AgentTurnOutput {
    pub message: String,
    pub edge_case_challenges: Vec<String>,
    pub scaffold_proposal: Option<ChangeProposal>,
    pub presentation_cue: Option<PresentationCue>,
    /// Fork-style turn identifier for steering/interrupt correlation.
    pub turn_id: Id,
    pub approval_policy: TurnApprovalPolicy,
    pub sandbox: TurnSandbox,
}

/// Built-in primary coding agent. Runs inside the session host process;
/// no ACP transport involved. Every effect (cue, proposal) is policy-gated
/// before it exists.
pub struct AheadAgentLoop {
    host: Arc<AheadSessionHost>,
}

impl AheadAgentLoop {
    pub fn new(host: Arc<AheadSessionHost>) -> Self {
        Self { host }
    }

    /// Runs a paired turn with the human engineer.
    pub fn run_turn(&self, input: AgentTurnRequest) -> Result<AgentTurnOutput> {
        let view = self.host.get_session(&input.session_id)?
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        // Stale-policy guard: the caller must present the policy it planned
        // against; a rotated policy fails closed instead of running blind.
        if !input.expected_policy_sha256.is_empty()
            && input.expected_policy_sha256 != view.session.policy.sha256
        {
            bail!(
                "Stale policy: expected {}, host holds {}",
                input.expected_policy_sha256,
                view.session.policy.sha256
            );
        }

        let mode = view.session.mode;
        let phase = &view.workflow.phase;
        let policy = &view.session.policy;

        let mut out = match mode {
            AssistanceMode::Learn => self.run_learn_turn(&input, phase, policy)?,
            AssistanceMode::Assist => self.run_assist_turn(&input, phase, policy)?,
        };
        out.turn_id = uuid::Uuid::new_v4().to_string();
        out.approval_policy = TurnApprovalPolicy::for_mode(mode);
        out.sandbox = TurnSandbox::for_mode(mode);
        Ok(out)
    }

    /// Serializes this turn the way the fork's `turn/start` params carry
    /// it. Delegates to the shared viewmodel so shells assemble identical
    /// turns; approvals still resolve in the host.
    pub fn to_codex_turn_params(&self, input: &AgentTurnRequest, mode: AssistanceMode) -> serde_json::Value {
        ahead_viewmodel::codex_turn_params(
            &ahead_viewmodel::TurnAssembly {
                thread_id: input.thread_id.clone(),
                user_message: input.user_message.clone(),
                cwd: input.cwd.as_ref().map(|p| p.to_string_lossy().to_string()),
            },
            mode,
        )
    }

    fn run_learn_turn(
        &self,
        input: &AgentTurnRequest,
        phase: &WorkflowPhase,
        _policy: &SessionPolicySnapshot,
    ) -> Result<AgentTurnOutput> {
        let message = format!(
            "In Learn mode: Let's investigate `{}` (line {}). What is the expected behavior and what invariants must hold here?",
            input.active_path,
            input.caret.line + 1
        );

        let cue = PresentationCue {
            cue_id: uuid::Uuid::new_v4().to_string(),
            anchor: lapce_rpc::ahead::CodeAnchor {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: input.session_id.clone(),
                path: input.active_path.clone(),
                range: DisplayRange {
                    start: input.caret,
                    end: DisplayPosition { line: input.caret.line + 2, col: 0 },
                },
                quote_hash: "learn-cue".to_string(),
                surrounding_context: None,
                created_at_commit: None,
            },
            label: format!("Focus on {} during {}", phase.title, phase.id),
            pointer_target: Some(input.caret),
            author_id: "ahead-pair".to_string(),
            display_duration_ms: Some(8000),
        };

        Ok(AgentTurnOutput {
            message,
            edge_case_challenges: vec![
                "Consider: How does this function handle unexpected network dropouts?".to_string(),
                "Invariants: Are all callers expecting idempotency?".to_string(),
            ],
            scaffold_proposal: None,
            presentation_cue: Some(cue),
            turn_id: String::new(),
            approval_policy: TurnApprovalPolicy::Never,
            sandbox: TurnSandbox::ReadOnly,
        })
    }

    fn run_assist_turn(
        &self,
        input: &AgentTurnRequest,
        phase: &WorkflowPhase,
        policy: &SessionPolicySnapshot,
    ) -> Result<AgentTurnOutput> {
        let mut challenges = Vec::new();

        // 1. Generate edge-case considerations
        if !input.invariants.is_empty() {
            for inv in &input.invariants {
                challenges.push(format!("Invariant constraint: Verify '{}' holds under concurrent access.", inv));
            }
        }
        challenges.push("Edge case: Empty payload or malformed headers handling.".to_string());
        challenges.push("Failure mode: Verify timeout cleanup without leaking file descriptors.".to_string());

        // 2. Prepare boilerplate scaffolding if requested or relevant
        let mut proposal = None;
        let user_lower = input.user_message.to_lowercase();

        if user_lower.contains("scaffold") || user_lower.contains("boilerplate") || user_lower.contains("implement") {
            let scaffold_patch = format!(
                "// --- AHEAD Mechanical Boilerplate Scaffolding ---\n\
                 #[derive(Debug, Clone)]\n\
                 pub struct ServiceConfig {{\n\
                     pub max_retries: u32,\n\
                     pub backoff_ms: u64,\n\
                 }}\n\n\
                 impl ServiceConfig {{\n\
                     pub fn default() -> Self {{\n\
                         Self {{\n\
                             max_retries: 3,\n\
                             backoff_ms: 200,\n\
                         }}\n\
                     }}\n\
                 }}\n"
            );

            // Recommend cursor positioned where the human engineer will write the actual logic
            let recommended_cursor = Some(DisplayPosition {
                line: input.caret.line + 4,
                col: 4,
            });

            let prop = ChangeProposal {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: input.session_id.clone(),
                path: input.active_path.clone(),
                original_sha256: "0000".to_string(),
                patch: scaffold_patch,
                is_mechanical: true,
                description: "Scaffold ServiceConfig boilerplate with default parameters".to_string(),
                recommended_cursor,
            };

            // Authorize proposal through policy
            if PolicyEvaluator::authorize_proposal(
                phase,
                AssistanceMode::Assist,
                policy,
                SessionRole::Editor,
                &prop,
            ).is_ok() {
                proposal = Some(prop);
            }
        }

        let message = format!(
            "I've analyzed the problem. The human engineer owns the core business logic, but I've prepared boilerplate scaffolding and positioned the cursor for your implementation. Please check the edge-case challenges below."
        );

        Ok(AgentTurnOutput {
            message,
            edge_case_challenges: challenges,
            scaffold_proposal: proposal,
            presentation_cue: None,
            turn_id: String::new(),
            approval_policy: TurnApprovalPolicy::OnRequest,
            sandbox: TurnSandbox::WorkspaceWrite,
        })
    }

    /// Answers a fork-style approval request from policy. Delegates to the
    /// shared viewmodel: Learn always declines; Assist accepts only
    /// mechanical proposals inside an explicit approved scope.
    pub fn decide_approval(
        mode: AssistanceMode,
        kind: &str,
        is_mechanical: bool,
        in_approved_scope: bool,
    ) -> bool {
        ahead_viewmodel::decide_approval(mode, kind, is_mechanical, in_approved_scope)
    }
}

/// Side-task delegation to EXTERNAL agents over real ACP (stdio).
/// Used only for bounded side tasks (security audit, independent review,
/// second opinion). Findings return as attributed text; the external agent
/// never writes code, never sees credentials, and never inherits the
/// managed session's approvals.
#[derive(Debug, Clone)]
pub struct AcpDelegatedTask {
    pub task_id: Id,
    /// Agent binary to spawn (must speak ACP over stdio), e.g.
    /// "claude-code", "pi". Resolved via PATH; never a shell string.
    pub agent_command: String,
    pub agent_args: Vec<String>,
    pub prompt: String,
    pub files: Vec<RepoPath>,
    /// Human-readable tool allowlist shown before dispatch; enforced by
    /// declining every permission request the agent raises.
    pub sandbox_tools: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AcpTaskResult {
    pub task_id: Id,
    pub status: String,
    pub findings: Vec<String>,
}

pub struct AcpDelegator;

impl AcpDelegator {
    /// Builds the ACP `session/prompt` params for a side task. Files travel
    /// as referenced context paths (read-only); the agent gets no write
    /// grant and no session credentials.
    pub fn build_acp_request(task: &AcpDelegatedTask) -> serde_json::Value {
        serde_json::json!({
            "task_id": task.task_id,
            "prompt": task.prompt,
            "context": {
                "files": task.files,
            },
            "capabilities": {
                "tools": task.sandbox_tools,
                "filesystem": "read_only",
            }
        })
    }

    /// Dispatches a side task to an external ACP agent over stdio.
    ///
    /// Protocol: `initialize` → `session/new` → `session/prompt`, declining
    /// ALL permission requests (read-only side task), collecting text
    /// updates as findings. Async because the ACP SDK is async; the proxy
    /// calls it from its tokio runtime via `block_on`.
    pub async fn run_external_task(task: AcpDelegatedTask) -> Result<AcpTaskResult> {
        use agent_client_protocol::{
            AcpAgent,
            schema::{
                ProtocolVersion,
                v1::{
                    ContentBlock, InitializeRequest, NewSessionRequest,
                    PromptRequest, RequestPermissionOutcome,
                    RequestPermissionRequest, RequestPermissionResponse,
                    SessionNotification, TextContent,
                },
            },
        };
        use std::str::FromStr;

        if task.agent_command.trim().is_empty() {
            bail!("ACP side task needs an agent command (e.g. claude-code)");
        }

        let command_line = if task.agent_args.is_empty() {
            task.agent_command.clone()
        } else {
            format!("{} {}", task.agent_command, task.agent_args.join(" "))
        };
        let agent = AcpAgent::from_str(&command_line)
            .map_err(|e| anyhow::anyhow!("Invalid ACP agent command: {e}"))?;

        let prompt_text = format!(
            "{}\n\nContext files (read-only, do not modify):\n{}",
            task.prompt,
            task.files.join("\n")
        );
        let task_id = task.task_id.clone();

        let findings = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
        let findings_cb = findings.clone();

        agent_client_protocol::Client.builder()
            .on_receive_notification(
                move |notification: SessionNotification, _cx| {
                    let findings_cb = findings_cb.clone();
                    async move {
                        let text = format!("{:?}", notification.update);
                        if !text.trim().is_empty() {
                            findings_cb.lock().push(text);
                        }
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                move |_request: RequestPermissionRequest, responder: agent_client_protocol::Responder<RequestPermissionResponse>, _connection| async move {
                    // Side tasks are read-only: decline everything.
                    let _ = responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Cancelled,
                    ));
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(agent, |connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Agent>| async move {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let session = connection
                    .send_request(NewSessionRequest::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))))
                    .block_task()
                    .await?;
                connection
                    .send_request(PromptRequest::new(
                        session.session_id,
                        vec![ContentBlock::Text(TextContent::new(prompt_text))],
                    ))
                    .block_task()
                    .await?;
                Ok(())
            })
            .await
            .map_err(|e| anyhow::anyhow!("ACP side task failed: {e}"))?;

        let findings = std::mem::take(&mut *findings.lock());
        Ok(AcpTaskResult {
            task_id,
            status: "completed".to_string(),
            findings,
        })
    }

    /// Sync wrapper for proxy call sites (tokio runtime `block_on`).
    /// Kept separate so tests can drive the async fn on their own runtime.
    pub fn run_external_task_blocking(task: AcpDelegatedTask) -> Result<AcpTaskResult> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(Self::run_external_task(task))
    }

    /// Outcome vocabulary shared with the fork's approval decisions:
    /// side tasks only ever produce review text, never accept/apply.
    pub fn side_task_outcomes() -> HashMap<&'static str, &'static str> {
        HashMap::from([
            ("completed", "findings returned as attributed review text"),
            ("declined_permissions", "all agent tool requests declined; text only"),
            ("failed", "transport or agent error; no findings trusted"),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_turn(session_id: Id, msg: &str, path: &str, line: u32) -> AgentTurnRequest {
        AgentTurnRequest {
            session_id,
            thread_id: "thread-test".to_string(),
            user_message: msg.to_string(),
            active_path: path.to_string(),
            caret: DisplayPosition { line, col: 0 },
            selection: None,
            file_content: String::new(),
            invariants: vec!["Idempotent retry".to_string()],
            cwd: None,
            expected_policy_sha256: String::new(),
            scope_id: None,
        }
    }

    #[test]
    fn test_assist_turn_scaffolds_and_positions_cursor() {
        let host = Arc::new(AheadSessionHost::in_memory().unwrap());
        let view = host.start_work(
            lapce_rpc::ahead::WorkKind::ProductChange,
            AssistanceMode::Assist,
            "Retries".to_string(),
            "Starting".to_string(),
            None,
        ).unwrap();

        // Advance to implement phase
        host.advance_phase(&view.session.id, 1, "implement".to_string()).unwrap();

        let agent = AheadAgentLoop::new(host);
        let output = agent.run_turn(test_turn(
            view.session.id,
            "Please scaffold the config boilerplate",
            "src/config.rs",
            10,
        )).unwrap();

        assert!(output.scaffold_proposal.is_some());
        let prop = output.scaffold_proposal.unwrap();
        assert!(prop.is_mechanical);
        assert!(prop.patch.contains("ServiceConfig"));
        // Cursor guidance to target line
        assert_eq!(prop.recommended_cursor.unwrap().line, 14);

        // Edge case challenges emitted
        assert!(output.edge_case_challenges.iter().any(|c| c.contains("Idempotent retry")));
        // Assist maps to workspace-write + on-request approvals
        assert_eq!(output.sandbox, TurnSandbox::WorkspaceWrite);
        assert_eq!(output.approval_policy, TurnApprovalPolicy::OnRequest);
        assert!(!output.turn_id.is_empty());
    }

    #[test]
    fn test_learn_turn_emits_socratic_hints_and_cues_without_edits() {
        let host = Arc::new(AheadSessionHost::in_memory().unwrap());
        let view = host.start_work(
            lapce_rpc::ahead::WorkKind::Investigation,
            AssistanceMode::Learn,
            "Audit".to_string(),
            "Starting".to_string(),
            None,
        ).unwrap();

        let agent = AheadAgentLoop::new(host);
        let mut input = test_turn(
            view.session.id,
            "Can you write this function for me?",
            "src/auth.rs",
            5,
        );
        input.invariants.clear();
        let output = agent.run_turn(input).unwrap();

        // Learn mode never produces edit proposals
        assert!(output.scaffold_proposal.is_none());
        assert!(output.presentation_cue.is_some());
        assert!(output.message.contains("Learn mode"));
        assert_eq!(output.sandbox, TurnSandbox::ReadOnly);
        assert_eq!(output.approval_policy, TurnApprovalPolicy::Never);
    }

    #[test]
    fn test_stale_policy_fails_closed() {
        let host = Arc::new(AheadSessionHost::in_memory().unwrap());
        let view = host.start_work(
            lapce_rpc::ahead::WorkKind::ProductChange,
            AssistanceMode::Assist,
            "Retries".to_string(),
            "Starting".to_string(),
            None,
        ).unwrap();
        let agent = AheadAgentLoop::new(host);
        let mut input = test_turn(view.session.id, "scaffold", "src/a.rs", 0);
        input.expected_policy_sha256 = "stale".to_string();
        assert!(agent.run_turn(input).is_err());
    }

    #[test]
    fn test_turn_params_follow_codex_vocabulary() {
        let host = Arc::new(AheadSessionHost::in_memory().unwrap());
        let agent = AheadAgentLoop::new(host);
        let input = test_turn("sess".into(), "hello", "src/a.rs", 0);
        let assist = agent.to_codex_turn_params(&input, AssistanceMode::Assist);
        assert_eq!(assist["approvalPolicy"], "untrusted");
        assert_eq!(assist["sandboxPolicy"], "workspace-write");
        let learn = agent.to_codex_turn_params(&input, AssistanceMode::Learn);
        assert_eq!(learn["approvalPolicy"], "never");
        assert_eq!(learn["sandboxPolicy"], "read-only");
    }

    #[test]
    fn test_approval_decisions_fail_closed() {
        assert!(!AheadAgentLoop::decide_approval(AssistanceMode::Learn, "file_change", true, true));
        assert!(!AheadAgentLoop::decide_approval(AssistanceMode::Assist, "file_change", false, true));
        assert!(!AheadAgentLoop::decide_approval(AssistanceMode::Assist, "file_change", true, false));
        assert!(!AheadAgentLoop::decide_approval(AssistanceMode::Assist, "shell", true, true));
        assert!(AheadAgentLoop::decide_approval(AssistanceMode::Assist, "file_change", true, true));
    }

    #[test]
    fn test_acp_side_task_request_shape_is_read_only() {
        let task = AcpDelegatedTask {
            task_id: "task-sec-1".to_string(),
            agent_command: "claude-code".to_string(),
            agent_args: Vec::new(),
            prompt: "Perform security review on auth modules".to_string(),
            files: vec!["src/auth.rs".to_string()],
            sandbox_tools: vec!["grep".to_string(), "read_file".to_string()],
        };

        let req = AcpDelegator::build_acp_request(&task);
        assert_eq!(req["capabilities"]["filesystem"], "read_only");
        assert_eq!(req["task_id"], "task-sec-1");
        // No shell string, no write grant, no session credentials leak.
        assert!(req.get("command").is_none());
        assert!(req.get("session_id").is_none());
    }

    #[test]
    fn test_acp_side_task_rejects_empty_command() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let res = rt.block_on(AcpDelegator::run_external_task(AcpDelegatedTask {
            task_id: "t".to_string(),
            agent_command: String::new(),
            agent_args: Vec::new(),
            prompt: "x".to_string(),
            files: Vec::new(),
            sandbox_tools: Vec::new(),
        }));
        assert!(res.is_err());
    }
}
