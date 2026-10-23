//! Central approval policy-stage execution and human review.

use crate::approval::ApprovalReviewContext;
use crate::command_canonicalization::canonicalize_command_for_approval;
use crate::exec_policy::prompt_is_rejected_by_policy;
use crate::mcp_tool_call::request_mcp_tool_user_approval;
use crate::sandboxing::SandboxPermissions;
use crate::session::session::Session;
use crate::tools::runtimes::apply_patch::ApplyPatchApprovalKey;
use crate::tools::runtimes::unified_exec::UnifiedExecApprovalKey;
use crate::tools::sandboxing::ToolError;
use crate::tools::sandboxing::with_cached_approval;
use codex_config::types::AppToolApproval;
use codex_otel::ToolDecisionSource;
use codex_protocol::approvals::ExecApprovalKind;
use codex_protocol::approvals::ExecPolicyAmendment;
use codex_protocol::error::CodexErr;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::FileChange;
use codex_protocol::protocol::ReviewDecision;
use codex_tools::ToolName;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::error;

#[derive(Clone)]
pub(crate) struct ApprovalContext {
    pub(crate) review_context: ApprovalReviewContext,
    pub(crate) call_id: String,
    pub(crate) tool_name: ToolName,
    pub(crate) approval_reason: Option<String>,
    pub(crate) retry_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ApprovalAction {
    ExecCommand {
        id: String,
        environment_id: String,
        command: Vec<String>,
        cwd: PathUri,
        sandbox_permissions: SandboxPermissions,
        additional_permissions: Option<AdditionalPermissionProfile>,
        justification: Option<String>,
        tty: bool,
        proposed_execpolicy_amendment: Option<ExecPolicyAmendment>,
    },
    WriteStdin {
        id: String,
        approval_id: String,
        environment_id: String,
        process_id: i32,
        input: String,
        cwd: PathUri,
        tty: bool,
        sandbox_permissions: SandboxPermissions,
        additional_permissions: Option<AdditionalPermissionProfile>,
    },
    ApplyPatch {
        id: String,
        environment_id: String,
        cwd: PathUri,
        files: Vec<PathUri>,
        patch: String,
        changes: Arc<HashMap<PathBuf, FileChange>>,
        permissions_preapproved: bool,
    },
    McpToolCall {
        id: String,
        server: String,
        tool_name: String,
        arguments: Option<serde_json::Value>,
        tool_title: Option<String>,
        tool_description: Option<String>,
        annotations: Option<crate::approval::McpToolAnnotations>,
        approval_policy: AskForApproval,
        approval_mode: AppToolApproval,
        allow_session_remember: bool,
    },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize)]
#[serde(untagged)]
pub(crate) enum ApprovalCacheKey {
    ExecCommand(UnifiedExecApprovalKey),
    ApplyPatch(ApplyPatchApprovalKey),
}

impl ApprovalAction {
    pub(crate) fn cache_keys(&self) -> Vec<ApprovalCacheKey> {
        match self {
            Self::ExecCommand {
                environment_id,
                command,
                cwd,
                tty,
                sandbox_permissions,
                additional_permissions,
                ..
            } => vec![ApprovalCacheKey::ExecCommand(UnifiedExecApprovalKey {
                environment_id: environment_id.clone(),
                executable: command.first().cloned(),
                command: canonicalize_command_for_approval(command),
                cwd: cwd.clone(),
                tty: *tty,
                sandbox_permissions: *sandbox_permissions,
                additional_permissions: additional_permissions.clone(),
            })],
            Self::McpToolCall { .. } | Self::WriteStdin { .. } => Vec::new(),
            Self::ApplyPatch {
                environment_id,
                files,
                ..
            } => files
                .iter()
                .cloned()
                .map(|path| {
                    ApprovalCacheKey::ApplyPatch(ApplyPatchApprovalKey {
                        environment_id: environment_id.clone(),
                        path,
                    })
                })
                .collect(),
        }
    }

    pub(crate) fn into_approval_request(self) -> std::io::Result<crate::approval::ApprovalRequest> {
        Ok(match self {
            Self::ExecCommand {
                id,
                environment_id,
                command,
                cwd,
                sandbox_permissions,
                additional_permissions,
                justification,
                tty,
                ..
            } => crate::approval::ApprovalRequest::ExecCommand {
                id,
                command,
                cwd: approval_cwd(&environment_id, cwd)?,
                sandbox_permissions,
                additional_permissions,
                justification,
                tty,
            },
            Self::WriteStdin {
                id,
                approval_id,
                environment_id,
                process_id,
                input,
                cwd,
                tty,
                sandbox_permissions,
                additional_permissions,
            } => crate::approval::ApprovalRequest::WriteStdin {
                id,
                approval_id,
                environment_id,
                process_id,
                input,
                cwd,
                tty,
                sandbox_permissions,
                additional_permissions,
            },
            Self::ApplyPatch {
                id,
                environment_id,
                cwd,
                files,
                patch,
                ..
            } => crate::approval::ApprovalRequest::ApplyPatch {
                id,
                cwd: approval_cwd(&environment_id, cwd)?,
                files: files
                    .into_iter()
                    .map(|path| path.to_abs_path())
                    .collect::<std::io::Result<Vec<_>>>()?,
                patch,
            },
            Self::McpToolCall {
                id,
                server,
                tool_name,
                arguments,
                tool_title,
                tool_description,
                annotations,
                ..
            } => crate::approval::ApprovalRequest::McpToolCall {
                id,
                server,
                tool_name,
                arguments,
                tool_title,
                tool_description,
                annotations,
            },
        })
    }
}

fn approval_cwd(environment_id: &str, cwd: PathUri) -> std::io::Result<AbsolutePathBuf> {
    match cwd.to_abs_path() {
        Ok(cwd) => Ok(cwd),
        Err(err) if environment_id != codex_exec_server::LOCAL_ENVIRONMENT_ID => Err(err),
        Err(_) => {
            let cwd_display = cwd.to_string();
            let path = cwd.to_url().to_file_path().map_err(|()| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("local cwd URI `{cwd_display}` is not a host-native path"),
                )
            })?;
            AbsolutePathBuf::from_absolute_path_checked(path).map_err(|err| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("local cwd URI `{cwd_display}` is not absolute: {err}"),
                )
            })
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ApprovalResolution {
    decision: ReviewDecision,
}

impl ApprovalResolution {
    fn into_tool_result(self) -> Result<ReviewDecision, ToolError> {
        match self.decision {
            ReviewDecision::ApprovedMcpPolicyAmendment => {
                error!("Tool approval received ApprovedMcpPolicyAmendment");
                Err(ToolError::Rejected(
                    "Error while requesting approval".to_string(),
                ))
            }
            ReviewDecision::Denied { rejection } => Err(ToolError::Rejected(rejection)),
            ReviewDecision::TimedOut => Err(ToolError::Rejected(
                super::APPROVAL_TIMEOUT_MESSAGE.to_string(),
            )),
            ReviewDecision::Abort => Err(ToolError::Codex(CodexErr::TurnAborted)),
            decision => Ok(decision),
        }
    }
}

impl Session {
    pub(crate) async fn request_approval(
        self: &Arc<Self>,
        action: ApprovalAction,
        ctx: ApprovalContext,
    ) -> Result<ReviewDecision, ToolError> {
        // Stdin that exceeds current permissions needs a fresh sandbox approval.
        // Strict review of ordinary input follows the same routing as ordinary exec.
        let policy = ctx.review_context.turn().approval_policy();
        if matches!(&action, ApprovalAction::WriteStdin { sandbox_permissions, .. }
            if sandbox_permissions.requests_sandbox_override())
            && let Some(reason) =
                prompt_is_rejected_by_policy(policy, /*prompt_is_rule*/ false)
        {
            return Err(ToolError::Rejected(reason.to_string()));
        }
        let is_mcp_tool_call = matches!(&action, ApprovalAction::McpToolCall { .. });
        let resolution = self.request_human_approval(action, &ctx).await;
        record_resolution(&ctx, &resolution);
        if is_mcp_tool_call && resolution.decision == ReviewDecision::ApprovedMcpPolicyAmendment {
            return Ok(resolution.decision);
        }
        resolution.into_tool_result()
    }

    async fn request_human_approval(
        self: &Arc<Self>,
        action: ApprovalAction,
        ctx: &ApprovalContext,
    ) -> ApprovalResolution {
        ApprovalResolution {
            decision: self.request_user_approval(&action, ctx).await,
        }
    }

    async fn request_user_approval(
        &self,
        action: &ApprovalAction,
        ctx: &ApprovalContext,
    ) -> ReviewDecision {
        match action {
            ApprovalAction::ExecCommand {
                environment_id,
                command,
                cwd,
                additional_permissions,
                justification,
                proposed_execpolicy_amendment,
                ..
            } => {
                let cwd = match approval_cwd(environment_id, cwd.clone()) {
                    Ok(cwd) => cwd,
                    Err(err) => {
                        tracing::error!(%err, "failed to resolve approval command cwd");
                        return ReviewDecision::denied(format!(
                            "failed to resolve approval command cwd: {err}"
                        ));
                    }
                };
                let tool_name = "unified_exec";
                let reason = ctx
                    .retry_reason
                    .clone()
                    .or_else(|| ctx.approval_reason.clone())
                    .or_else(|| justification.clone());
                let policy_fingerprint = ctx
                    .review_context
                    .environments()
                    .turn_environments()
                    .find(|environment| environment.selection.environment_id == *environment_id)
                    .and_then(|environment| environment.config().exec_policy.as_ref())
                    .map(codex_execpolicy::RequirementsExecPolicy::fingerprint);
                let cache_keys = action
                    .cache_keys()
                    .into_iter()
                    .map(|key| (key, &policy_fingerprint))
                    .collect();
                with_cached_approval(&self.services, tool_name, cache_keys, || async {
                    self.request_command_approval(
                        ctx.review_context.turn(),
                        ExecApprovalKind::Command,
                        ctx.call_id.clone(),
                        /*approval_id*/ None,
                        Some(environment_id.clone()),
                        command.clone(),
                        cwd.into(),
                        reason,
                        proposed_execpolicy_amendment.clone(),
                        additional_permissions.clone(),
                        /*available_decisions*/ None,
                    )
                    .await
                })
                .await
            }
            ApprovalAction::WriteStdin {
                id,
                approval_id,
                environment_id,
                process_id,
                input,
                cwd,
                additional_permissions,
                ..
            } => {
                self.request_command_approval(
                    ctx.review_context.turn(),
                    ExecApprovalKind::WriteStdin,
                    id.clone(),
                    Some(approval_id.clone()),
                    Some(environment_id.clone()),
                    vec![
                        "write_stdin".to_string(),
                        "--session-id".to_string(),
                        process_id.to_string(),
                        input.clone(),
                    ],
                    cwd.clone(),
                    ctx.approval_reason.clone(),
                    /*proposed_execpolicy_amendment*/ None,
                    additional_permissions.clone(),
                    Some(vec![ReviewDecision::Approved, ReviewDecision::Abort]),
                )
                .await
            }
            ApprovalAction::ApplyPatch {
                changes,
                permissions_preapproved,
                ..
            } => {
                let reason = ctx
                    .retry_reason
                    .clone()
                    .or_else(|| ctx.approval_reason.clone());
                if *permissions_preapproved && reason.is_none() {
                    return ReviewDecision::Approved;
                }
                if reason.is_some() {
                    return self
                        .request_patch_approval(
                            ctx.review_context.turn(),
                            ctx.call_id.clone(),
                            changes.as_ref().clone(),
                            reason,
                            /*grant_root*/ None,
                        )
                        .await;
                }
                with_cached_approval(
                    &self.services,
                    "apply_patch",
                    action.cache_keys(),
                    || async {
                        self.request_patch_approval(
                            ctx.review_context.turn(),
                            ctx.call_id.clone(),
                            changes.as_ref().clone(),
                            /*reason*/ None,
                            /*grant_root*/ None,
                        )
                        .await
                    },
                )
                .await
            }
            ApprovalAction::McpToolCall { .. } => {
                request_mcp_tool_user_approval(
                    self,
                    ctx.review_context.turn(),
                    &ctx.call_id,
                    action,
                )
                .await
            }
        }
    }
}

fn record_resolution(ctx: &ApprovalContext, resolution: &ApprovalResolution) {
    ctx.review_context.turn().session_telemetry.tool_decision(
        &ctx.tool_name,
        &ctx.call_id,
        &resolution.decision,
        Some(ToolDecisionSource::User),
    );
}

#[cfg(all(test, unix))]
#[path = "approvals_tests.rs"]
mod tests;
