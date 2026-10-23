//! Shared approvals and sandboxing traits used by tool runtimes.
//!
//! Consolidates the approval flow primitives (`ApprovalDecision`, `ApprovalStore`,
//! `Approvable`) together with the sandbox orchestration traits
//! and helpers (`Sandboxable`, `ToolRuntime`, `SandboxAttempt`, etc.).

use crate::sandboxing::ExecOptions;
use crate::sandboxing::SandboxPermissions;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use crate::session::turn_context::TurnEnvironment;
use crate::state::SessionServices;
use codex_file_system::FileSystemSandboxContext;
use codex_protocol::approvals::ExecPolicyAmendment;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::error::CodexErr;
use codex_protocol::permissions::FileSystemSandboxKind;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::ReviewDecision;
use codex_sandboxing::SandboxCommand;
use codex_sandboxing::SandboxManager;
use codex_sandboxing::SandboxTransformRequest;
use codex_sandboxing::SandboxType;
use codex_sandboxing::SandboxablePreference;
use codex_sandboxing::policy_transforms::effective_permission_profile;
use codex_tools::ToolName;
use codex_utils_path_uri::PathConvention;
use codex_utils_path_uri::PathUri;
use futures::Future;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Default, Debug)]
pub(crate) struct ApprovalStore {
    // Store serialized keys for generic caching across requests.
    map: HashMap<String, ReviewDecision>,
}

impl ApprovalStore {
    pub fn get<K>(&self, key: &K) -> Option<ReviewDecision>
    where
        K: Serialize,
    {
        let s = serde_json::to_string(key).ok()?;
        self.map.get(&s).cloned()
    }

    pub fn put<K>(&mut self, key: K, value: ReviewDecision)
    where
        K: Serialize,
    {
        if let Ok(s) = serde_json::to_string(&key) {
            self.map.insert(s, value);
        }
    }
}

/// Takes a vector of approval keys and returns a ReviewDecision.
/// There will be one key in most cases, but apply_patch can modify multiple files at once.
///
/// - If all keys are already approved for session, we skip prompting.
/// - If the user approves for session, we store the decision for each key individually
///   so future requests touching any subset can also skip prompting.
pub(crate) async fn with_cached_approval<K, F, Fut>(
    services: &SessionServices,
    // Name of the tool, used for metrics collection.
    tool_name: &str,
    keys: Vec<K>,
    fetch: F,
) -> ReviewDecision
where
    K: Serialize,
    F: FnOnce() -> Fut,
    Fut: Future<Output = ReviewDecision>,
{
    // To be defensive here, don't bother with checking the cache if keys are empty.
    if keys.is_empty() {
        return fetch().await;
    }

    let already_approved = {
        let store = services.tool_approvals.lock().await;
        keys.iter()
            .all(|key| matches!(store.get(key), Some(ReviewDecision::ApprovedForSession)))
    };

    if already_approved {
        return ReviewDecision::ApprovedForSession;
    }

    let decision = fetch().await;

    services.session_telemetry.counter(
        "codex.approval.requested",
        /*inc*/ 1,
        &[
            ("tool", tool_name),
            ("approved", decision.to_opaque_string()),
        ],
    );

    if matches!(decision, ReviewDecision::ApprovedForSession) {
        let mut store = services.tool_approvals.lock().await;
        for key in keys {
            store.put(key, ReviewDecision::ApprovedForSession);
        }
    }

    decision
}

pub(crate) use super::approvals::ApprovalAction;

// Specifies what tool orchestrator should do with a given tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExecApprovalRequirement {
    /// No approval required for this tool call.
    Skip {
        /// The first attempt should skip sandboxing (e.g., when explicitly
        /// greenlit by policy).
        bypass_sandbox: bool,
        /// Proposed execpolicy amendment to skip future approvals for similar commands
        /// Only applies if the command fails to run in sandbox and codex prompts the user to run outside the sandbox.
        proposed_execpolicy_amendment: Option<ExecPolicyAmendment>,
    },
    /// Approval required for this tool call.
    NeedsApproval {
        reason: Option<String>,
        /// Proposed execpolicy amendment to skip future approvals for similar commands
        /// See core/src/exec_policy.rs for more details on how proposed_execpolicy_amendment is determined.
        proposed_execpolicy_amendment: Option<ExecPolicyAmendment>,
    },
    /// Execution forbidden for this tool call.
    Forbidden { reason: String },
}

impl ExecApprovalRequirement {
    pub fn proposed_execpolicy_amendment(&self) -> Option<&ExecPolicyAmendment> {
        match self {
            Self::NeedsApproval {
                proposed_execpolicy_amendment: Some(prefix),
                ..
            } => Some(prefix),
            Self::Skip {
                proposed_execpolicy_amendment: Some(prefix),
                ..
            } => Some(prefix),
            _ => None,
        }
    }
}

/// - Never: do not ask
/// - OnRequest: ask unless filesystem access is unrestricted
/// - Granular: ask unless filesystem access is unrestricted, but auto-reject
///   when granular sandbox approval is disabled.
/// - UnlessTrusted: always ask
pub(crate) fn default_exec_approval_requirement(
    policy: AskForApproval,
    file_system_sandbox_policy: &FileSystemSandboxPolicy,
) -> ExecApprovalRequirement {
    let needs_approval = match policy {
        AskForApproval::Never => false,
        AskForApproval::OnRequest | AskForApproval::Granular(_) => {
            matches!(
                file_system_sandbox_policy.kind,
                FileSystemSandboxKind::Restricted
            )
        }
        AskForApproval::UnlessTrusted => true,
    };

    if needs_approval
        && matches!(
            policy,
            AskForApproval::Granular(granular_config)
                if !granular_config.allows_sandbox_approval()
        )
    {
        ExecApprovalRequirement::Forbidden {
            reason: "approval policy disallowed sandbox approval prompt".to_string(),
        }
    } else if needs_approval {
        ExecApprovalRequirement::NeedsApproval {
            reason: None,
            proposed_execpolicy_amendment: None,
        }
    } else {
        ExecApprovalRequirement::Skip {
            bypass_sandbox: false,
            proposed_execpolicy_amendment: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SandboxOverride {
    NoOverride,
    BypassSandboxFirstAttempt,
}

pub(crate) fn sandbox_override_for_first_attempt(
    sandbox_permissions: SandboxPermissions,
    exec_approval_requirement: &ExecApprovalRequirement,
    file_system_sandbox_policy: &FileSystemSandboxPolicy,
) -> SandboxOverride {
    // Deny-read restrictions are part of the active permission policy. Running
    // without a filesystem sandbox would discard them, even if the command was
    // otherwise approved by rules or explicit escalation.
    if !unsandboxed_execution_allowed(file_system_sandbox_policy) {
        return SandboxOverride::NoOverride;
    }

    // ExecPolicy `Allow` can intentionally imply full trust (Skip + bypass_sandbox=true),
    // which supersedes `with_additional_permissions` sandboxed execution hints.
    if matches!(
        exec_approval_requirement,
        ExecApprovalRequirement::Skip {
            bypass_sandbox: true,
            ..
        }
    ) {
        return SandboxOverride::BypassSandboxFirstAttempt;
    }

    if sandbox_permissions.requires_escalated_permissions() {
        SandboxOverride::BypassSandboxFirstAttempt
    } else {
        SandboxOverride::NoOverride
    }
}

/// Returns true when the active filesystem policy can be represented by
/// running without a filesystem sandbox.
///
/// Denied reads only exist inside the sandbox. If a policy contains any
/// denied-read paths, bypassing the sandbox would silently grant those reads,
/// so escalation must keep the command sandboxed with the denied reads intact.
pub(crate) fn unsandboxed_execution_allowed(
    file_system_sandbox_policy: &FileSystemSandboxPolicy,
) -> bool {
    !file_system_sandbox_policy.has_denied_read_restrictions()
}

pub(crate) trait Approvable<Req> {
    /// Return per-request sandbox permissions for first-attempt sandbox
    /// selection. Most tools use the ambient sandbox policy unchanged.
    fn sandbox_permissions(&self, _req: &Req) -> SandboxPermissions {
        SandboxPermissions::UseDefault
    }

    fn should_bypass_approval(&self, policy: AskForApproval, already_approved: bool) -> bool {
        if already_approved {
            // We do not ask one more time
            return true;
        }
        matches!(policy, AskForApproval::Never)
    }

    /// Return `Some(_)` to specify a custom exec approval requirement, or `None`
    /// to fall back to policy-based default.
    fn exec_approval_requirement(&self, _req: &Req) -> Option<ExecApprovalRequirement> {
        None
    }

    /// Decide we can request an approval for no-sandbox execution.
    fn wants_no_sandbox_approval(&self, policy: AskForApproval) -> bool {
        match policy {
            AskForApproval::UnlessTrusted => true,
            AskForApproval::Never => false,
            AskForApproval::OnRequest => false,
            AskForApproval::Granular(granular_config) => granular_config.sandbox_approval,
        }
    }

    fn approval_action(&self, req: &Req, call_id: &str) -> std::io::Result<ApprovalAction>;
}

pub(crate) trait Sandboxable {
    fn sandbox_preference(&self) -> SandboxablePreference;
    fn escalate_on_failure(&self) -> bool {
        true
    }
}

pub(crate) struct ToolCtx {
    pub session: Arc<Session>,
    pub step_context: Arc<StepContext>,
    pub call_id: String,
    pub tool_name: ToolName,
}

#[derive(Debug)]
pub(crate) enum ToolError {
    Rejected(String),
    Codex(CodexErr),
}

pub(crate) trait ToolRuntime<Req, Out>: Approvable<Req> + Sandboxable {
    fn turn_environment<'a>(&self, req: &'a Req) -> &'a TurnEnvironment;

    fn uses_executor_managed_process_sandbox(&self, _req: &Req) -> bool {
        false
    }

    fn sandbox_cwd<'a>(&self, _req: &'a Req) -> Option<&'a PathUri> {
        None
    }

    async fn run(
        &mut self,
        req: &Req,
        attempt: &SandboxAttempt<'_>,
        ctx: &ToolCtx,
    ) -> Result<Out, ToolError>;
}

pub(crate) struct SandboxAttempt<'a> {
    pub sandbox: SandboxType,
    /// Whether policy requested sandboxing, independent of this host's concrete wrapper.
    pub sandbox_requested: bool,
    pub permissions: &'a codex_protocol::models::PermissionProfile,
    /// Canonical permissions before this host materializes workspace roots.
    pub exec_server_permissions: &'a codex_protocol::models::PermissionProfile,
    pub(crate) manager: &'a SandboxManager,
    pub(crate) sandbox_cwd: &'a PathUri,
    pub(crate) workspace_roots: &'a [PathUri],
    pub ahead_linux_sandbox_exe: Option<&'a std::path::PathBuf>,
    // TODO(anp): Reconcile these attempt settings with TurnEnvironment::sandbox_context
    // so process execution and patch writes honor the selected environment's backend.
    pub use_legacy_landlock: bool,
    pub windows_sandbox_level: codex_protocol::config_types::WindowsSandboxLevel,
    pub windows_sandbox_private_desktop: bool,
}

pub(crate) fn executor_windows_sandbox_level(
    windows_sandbox_level: WindowsSandboxLevel,
    cwd: &PathUri,
) -> WindowsSandboxLevel {
    if windows_sandbox_level == WindowsSandboxLevel::Disabled
        && cwd.infer_path_convention() == Some(PathConvention::Windows)
    {
        WindowsSandboxLevel::RestrictedToken
    } else {
        windows_sandbox_level
    }
}

impl<'a> SandboxAttempt<'a> {
    /// Whether this attempt bypasses sandboxing required by its ambient policy.
    /// Returns whether this attempt bypasses sandboxing required by its policy.
    pub(crate) fn is_escalated(&self) -> bool {
        !self.sandbox_requested
            && self
                .manager
                .should_sandbox(self.permissions, SandboxablePreference::Auto)
    }

    pub fn env_for(
        &self,
        command: SandboxCommand,
        options: ExecOptions,
    ) -> Result<crate::sandboxing::ExecRequest, CodexErr> {
        let request = self
            .manager
            .transform(SandboxTransformRequest {
                command,
                permissions: self.permissions,
                sandbox: self.sandbox,
                sandbox_policy_cwd: self.sandbox_cwd,
                ahead_linux_sandbox_exe: self
                    .ahead_linux_sandbox_exe
                    .map(std::path::PathBuf::as_path),
                use_legacy_landlock: self.use_legacy_landlock,
                windows_sandbox_level: self.windows_sandbox_level,
                windows_sandbox_private_desktop: self.windows_sandbox_private_desktop,
            })
            .map_err(CodexErr::from)?;
        let workspace_roots = self
            .workspace_roots
            .iter()
            .map(PathUri::to_abs_path)
            .collect::<std::io::Result<Vec<_>>>()?;
        crate::sandboxing::ExecRequest::from_sandbox_exec_request(request, options, workspace_roots)
    }

    pub fn env_for_exec_server(
        &self,
        command: SandboxCommand,
        options: ExecOptions,
    ) -> Result<crate::sandboxing::ExecRequest, CodexErr> {
        let exec_server_permissions = effective_permission_profile(
            self.exec_server_permissions,
            command.additional_permissions.as_ref(),
        );
        let request = self
            .manager
            .transform(SandboxTransformRequest {
                command,
                permissions: self.permissions,
                // The exec-server must receive the native command, not this host's wrapper.
                sandbox: SandboxType::None,
                sandbox_policy_cwd: self.sandbox_cwd,
                ahead_linux_sandbox_exe: None,
                use_legacy_landlock: self.use_legacy_landlock,
                windows_sandbox_level: self.windows_sandbox_level,
                windows_sandbox_private_desktop: self.windows_sandbox_private_desktop,
            })
            .map_err(CodexErr::from)?;
        let mut exec_request = crate::sandboxing::ExecRequest::from_sandbox_exec_request(
            request,
            options,
            Vec::new(),
        )?;
        if self.sandbox_requested {
            exec_request.exec_server_sandbox = Some(FileSystemSandboxContext {
                permissions: exec_server_permissions.into(),
                cwd: Some(exec_request.windows_sandbox_policy_cwd.clone()),
                workspace_roots: self.workspace_roots.to_vec(),
                user_home_dir: None,
                temporary_directories: None,
                windows_sandbox_level: executor_windows_sandbox_level(
                    self.windows_sandbox_level,
                    self.sandbox_cwd,
                ),
                windows_sandbox_private_desktop: self.windows_sandbox_private_desktop,
                use_legacy_landlock: self.use_legacy_landlock,
            });
        }
        Ok(exec_request)
    }
}

#[cfg(test)]
#[path = "sandboxing_tests.rs"]
mod tests;
