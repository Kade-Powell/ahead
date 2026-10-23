/*
Module: orchestrator

Central place for approvals + sandbox selection + retry semantics. Drives a
simple sequence for any ToolRuntime: approval → select sandbox → attempt →
retry with an escalated sandbox strategy on denial (no re‑approval thanks to
caching).
*/
use crate::approval::ApprovalReviewContext;
use crate::tools::approvals::ApprovalContext;
use crate::tools::flat_tool_name;
use crate::tools::sandboxing::ExecApprovalRequirement;
use crate::tools::sandboxing::SandboxAttempt;
use crate::tools::sandboxing::SandboxOverride;
use crate::tools::sandboxing::ToolCtx;
use crate::tools::sandboxing::ToolError;
use crate::tools::sandboxing::ToolRuntime;
use crate::tools::sandboxing::default_exec_approval_requirement;
use crate::tools::sandboxing::sandbox_override_for_first_attempt;
use crate::tools::sandboxing::unsandboxed_execution_allowed;
use codex_otel::ToolDecisionSource;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::error::SandboxErr;
use codex_protocol::exec_output::ExecToolCallOutput;
use codex_protocol::protocol::ReviewDecision;
use codex_sandboxing::SandboxManager;
use codex_sandboxing::SandboxType;
use std::time::Instant;

pub(crate) struct ToolOrchestrator {
    sandbox: SandboxManager,
}

pub(crate) struct OrchestratorRunResult<Out> {
    pub output: Out,
}

impl ToolOrchestrator {
    pub fn new() -> Self {
        Self {
            sandbox: SandboxManager::new(),
        }
    }

    async fn run_attempt<Rq, Out, T>(
        tool: &mut T,
        req: &Rq,
        tool_ctx: &ToolCtx,
        attempt: &SandboxAttempt<'_>,
    ) -> Result<Out, ToolError>
    where
        T: ToolRuntime<Rq, Out>,
    {
        tool.run(req, attempt, tool_ctx).await
    }

    pub async fn run<Rq, Out, T>(
        &mut self,
        tool: &mut T,
        req: &Rq,
        tool_ctx: &ToolCtx,
    ) -> Result<OrchestratorRunResult<Out>, ToolError>
    where
        T: ToolRuntime<Rq, Out>,
    {
        let turn_ctx = tool_ctx.step_context.turn.as_ref();
        let approval_policy = tool_ctx.step_context.settings.approval_policy();
        let otel = turn_ctx.session_telemetry.clone();
        let otel_tn = flat_tool_name(&tool_ctx.tool_name).into_owned();
        let otel_ci = &tool_ctx.call_id;
        // 1) Approval
        let mut already_approved = false;

        let environment = tool.turn_environment(req);
        let sandbox_config = environment.config();
        let workspace_roots = environment.workspace_roots();
        let executor_managed_process_sandbox = tool.uses_executor_managed_process_sandbox(req);
        let permission_profile = environment.permission_profile();
        let permissions = if executor_managed_process_sandbox {
            // Executor-native roots remain symbolic until the executor applies its own sandbox.
            permission_profile.clone()
        } else {
            environment.permission_profile_with_workspace_roots()
        };
        let file_system_sandbox_policy = permissions.file_system_sandbox_policy();
        let requirement = tool.exec_approval_requirement(req).unwrap_or_else(|| {
            default_exec_approval_requirement(approval_policy, &file_system_sandbox_policy)
        });
        match &requirement {
            ExecApprovalRequirement::Skip { .. } => {
                otel.tool_decision(
                    &tool_ctx.tool_name,
                    otel_ci,
                    &ReviewDecision::Approved,
                    Some(ToolDecisionSource::Config),
                );
            }
            ExecApprovalRequirement::Forbidden { reason } => {
                return Err(ToolError::Rejected(reason.clone()));
            }
            ExecApprovalRequirement::NeedsApproval { reason, .. } => {
                let action = tool
                    .approval_action(req, &tool_ctx.call_id)
                    .map_err(|err| {
                        ToolError::Rejected(format!("could not prepare approval action: {err}"))
                    })?;
                let approval_ctx = ApprovalContext {
                    review_context: ApprovalReviewContext::from(&tool_ctx.step_context),
                    call_id: tool_ctx.call_id.clone(),
                    tool_name: tool_ctx.tool_name.clone(),
                    approval_reason: reason.clone(),
                    retry_reason: None,
                };
                tool_ctx
                    .session
                    .request_approval(action, approval_ctx)
                    .await?;
                already_approved = true;
            }
        }

        // 2) First attempt under the selected sandbox.
        let unsandboxed_allowed = unsandboxed_execution_allowed(&file_system_sandbox_policy)
            && permission_profile.network_sandbox_policy().is_enabled();
        let sandbox_override = if unsandboxed_allowed {
            sandbox_override_for_first_attempt(
                tool.sandbox_permissions(req),
                &requirement,
                &file_system_sandbox_policy,
            )
        } else {
            SandboxOverride::NoOverride
        };
        let sandbox_preference = tool.sandbox_preference();
        let sandbox_requested = match sandbox_override {
            SandboxOverride::BypassSandboxFirstAttempt => false,
            SandboxOverride::NoOverride => self
                .sandbox
                .should_sandbox(&permissions, sandbox_preference),
        };
        let initial_sandbox = if sandbox_requested && !executor_managed_process_sandbox {
            self.sandbox.select_initial(
                &permissions,
                sandbox_preference,
                sandbox_config.windows_sandbox_level,
            )
        } else {
            SandboxType::None
        };

        let sandbox_policy_cwd = tool
            .sandbox_cwd(req)
            .cloned()
            .unwrap_or_else(|| environment.cwd().clone());
        let initial_attempt = SandboxAttempt {
            sandbox: initial_sandbox,
            sandbox_requested,
            permissions: &permissions,
            exec_server_permissions: permission_profile,
            manager: &self.sandbox,
            sandbox_cwd: &sandbox_policy_cwd,
            workspace_roots,
            ahead_linux_sandbox_exe: turn_ctx.config.ahead_linux_sandbox_exe.as_ref(),
            use_legacy_landlock: sandbox_config.use_legacy_landlock,
            windows_sandbox_level: sandbox_config.windows_sandbox_level,
            windows_sandbox_private_desktop: sandbox_config.windows_sandbox_private_desktop,
        };

        let initial_attempt_start = Instant::now();
        let first_result = Self::run_attempt(tool, req, tool_ctx, &initial_attempt).await;
        let initial_duration = initial_attempt_start.elapsed();
        match first_result {
            Ok(out) => {
                // We have a successful initial result
                Ok(OrchestratorRunResult { output: out })
            }
            Err(ToolError::Codex(err)) => {
                let CodexErrorDetails::Sandbox(SandboxErr::Denied { output, .. }) = err.details()
                else {
                    let err = ToolError::Codex(err);
                    if let Some(outcome) = sandbox_outcome_from_tool_error(&err) {
                        otel.sandbox_outcome(
                            &otel_tn,
                            otel_ci,
                            outcome,
                            initial_duration,
                            /*escalated_duration*/ None,
                        );
                    }
                    return Err(err);
                };
                if !tool.escalate_on_failure() {
                    otel.sandbox_outcome(
                        &otel_tn,
                        otel_ci,
                        "denied",
                        initial_duration,
                        /*escalated_duration*/ None,
                    );
                    return Err(ToolError::Codex(err));
                }
                // Under `Never` or `OnRequest`, do not retry without sandbox;
                // surface a concise sandbox denial that preserves the
                // original output.
                if !tool.wants_no_sandbox_approval(approval_policy) {
                    otel.sandbox_outcome(
                        &otel_tn,
                        otel_ci,
                        "denied",
                        initial_duration,
                        /*escalated_duration*/ None,
                    );
                    return Err(ToolError::Codex(err));
                }
                if !unsandboxed_allowed {
                    otel.sandbox_outcome(
                        &otel_tn,
                        otel_ci,
                        "denied",
                        initial_duration,
                        /*escalated_duration*/ None,
                    );
                    return Err(ToolError::Codex(err));
                }
                let retry_reason = build_denial_reason_from_output(output.as_ref());

                let bypass_retry_approval =
                    tool.should_bypass_approval(approval_policy, already_approved);
                if !bypass_retry_approval {
                    let approval_reason = match &requirement {
                        ExecApprovalRequirement::NeedsApproval { reason, .. } => reason.clone(),
                        ExecApprovalRequirement::Skip { .. }
                        | ExecApprovalRequirement::Forbidden { .. } => None,
                    };
                    let action = tool
                        .approval_action(req, &tool_ctx.call_id)
                        .map_err(|err| {
                            ToolError::Rejected(format!("could not prepare approval action: {err}"))
                        })?;
                    let approval_ctx = ApprovalContext {
                        review_context: ApprovalReviewContext::from(&tool_ctx.step_context),
                        call_id: tool_ctx.call_id.clone(),
                        tool_name: tool_ctx.tool_name.clone(),
                        approval_reason,
                        retry_reason: Some(retry_reason),
                    };

                    tool_ctx
                        .session
                        .request_approval(action, approval_ctx)
                        .await?;
                }

                let retry_sandbox_requested = !unsandboxed_allowed
                    && self
                        .sandbox
                        .should_sandbox(&permissions, sandbox_preference);
                let retry_sandbox = if retry_sandbox_requested && !executor_managed_process_sandbox
                {
                    self.sandbox.select_initial(
                        &permissions,
                        sandbox_preference,
                        sandbox_config.windows_sandbox_level,
                    )
                } else {
                    SandboxType::None
                };
                let retry_ahead_linux_sandbox_exe = if unsandboxed_allowed {
                    None
                } else {
                    turn_ctx.config.ahead_linux_sandbox_exe.as_ref()
                };
                let retry_attempt = SandboxAttempt {
                    sandbox: retry_sandbox,
                    sandbox_requested: retry_sandbox_requested,
                    permissions: &permissions,
                    exec_server_permissions: permission_profile,
                    manager: &self.sandbox,
                    sandbox_cwd: &sandbox_policy_cwd,
                    workspace_roots,
                    ahead_linux_sandbox_exe: retry_ahead_linux_sandbox_exe,
                    use_legacy_landlock: sandbox_config.use_legacy_landlock,
                    windows_sandbox_level: sandbox_config.windows_sandbox_level,
                    windows_sandbox_private_desktop: sandbox_config.windows_sandbox_private_desktop,
                };

                // Second attempt.
                let escalated_attempt_start = Instant::now();
                let retry_result = Self::run_attempt(tool, req, tool_ctx, &retry_attempt).await;
                let escalated_duration = escalated_attempt_start.elapsed();
                match retry_result {
                    Ok(output) => {
                        otel.sandbox_outcome(
                            &otel_tn,
                            otel_ci,
                            "escalated",
                            initial_duration,
                            Some(escalated_duration),
                        );
                        Ok(OrchestratorRunResult { output })
                    }
                    Err(err) => {
                        if let Some(outcome) = sandbox_outcome_from_tool_error(&err) {
                            otel.sandbox_outcome(
                                &otel_tn,
                                otel_ci,
                                outcome,
                                initial_duration,
                                Some(escalated_duration),
                            );
                        }
                        Err(err)
                    }
                }
            }
            Err(err) => {
                if let Some(outcome) = sandbox_outcome_from_tool_error(&err) {
                    otel.sandbox_outcome(
                        &otel_tn,
                        otel_ci,
                        outcome,
                        initial_duration,
                        /*escalated_duration*/ None,
                    );
                }
                Err(err)
            }
        }
    }
}

fn sandbox_outcome_from_tool_error(err: &ToolError) -> Option<&'static str> {
    match err {
        ToolError::Codex(err) => match err.details() {
            CodexErrorDetails::Sandbox(SandboxErr::Denied { .. }) => Some("denied"),
            CodexErrorDetails::Sandbox(SandboxErr::Timeout { .. }) => Some("timed_out"),
            CodexErrorDetails::Sandbox(SandboxErr::Signal(_)) => Some("signal"),
            _ => None,
        },
        ToolError::Rejected(_) => None,
    }
}

fn build_denial_reason_from_output(_output: &ExecToolCallOutput) -> String {
    // Keep approval reason terse and stable for UX/tests, but accept the
    // output so we can evolve heuristics later without touching call sites.
    "command failed; retry without sandbox?".to_string()
}
