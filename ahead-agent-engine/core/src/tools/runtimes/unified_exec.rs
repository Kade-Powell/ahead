/*
Runtime: unified exec

Handles approval + sandbox orchestration for unified exec requests, delegating to
the process manager to spawn PTYs once an ExecRequest is prepared.
*/
use crate::exec::ExecCapturePolicy;
use crate::exec::ExecExpiration;
use crate::sandboxing::ExecOptions;
use crate::sandboxing::ExecServerEnvConfig;
use crate::sandboxing::SandboxPermissions;
use crate::session::turn_context::TurnEnvironment;
use crate::shell::ShellType;
use crate::tools::runtimes::maybe_wrap_shell_lc_with_snapshot;
use crate::tools::runtimes::prepare_powershell_command_for_elevated_windows_sandbox;
use crate::tools::sandboxing::Approvable;
use crate::tools::sandboxing::ApprovalAction;
use crate::tools::sandboxing::ExecApprovalRequirement;
use crate::tools::sandboxing::SandboxAttempt;
use crate::tools::sandboxing::Sandboxable;
use crate::tools::sandboxing::ToolCtx;
use crate::tools::sandboxing::ToolError;
use crate::tools::sandboxing::ToolRuntime;
use crate::unified_exec::NoopSpawnLifecycle;
use crate::unified_exec::TerminalPermissions;
use crate::unified_exec::TerminalSandboxSource;
use crate::unified_exec::UnifiedExecProcess;
use crate::unified_exec::UnifiedExecProcessManager;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_sandboxing::SandboxCommand;
use codex_sandboxing::SandboxablePreference;
use codex_shell_command::powershell::prefix_powershell_script_with_utf8;
use codex_utils_path_uri::PathUri;
use std::collections::HashMap;

/// Request payload used by the unified-exec runtime after approvals and
/// sandbox preferences have been resolved for the current turn.
#[derive(Clone, Debug)]
pub struct UnifiedExecRequest {
    pub command: Vec<String>,
    pub shell_type: ShellType,
    pub process_id: i32,
    pub cwd: PathUri,
    pub sandbox_cwd: PathUri,
    pub turn_environment: TurnEnvironment,
    pub env: HashMap<String, String>,
    pub exec_server_env_config: Option<ExecServerEnvConfig>,
    pub shell_snapshot: Option<codex_exec_server::ShellSnapshotRequest>,
    pub explicit_env_overrides: HashMap<String, String>,
    pub tty: bool,
    pub sandbox_permissions: SandboxPermissions,
    pub additional_permissions: Option<AdditionalPermissionProfile>,
    pub justification: Option<String>,
    pub exec_approval_requirement: ExecApprovalRequirement,
}

/// Cache key for approval decisions that can be reused across equivalent
/// unified-exec launches.
#[derive(serde::Serialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct UnifiedExecApprovalKey {
    pub environment_id: String,
    pub executable: Option<String>,
    pub command: Vec<String>,
    pub cwd: PathUri,
    pub tty: bool,
    pub sandbox_permissions: SandboxPermissions,
    pub additional_permissions: Option<AdditionalPermissionProfile>,
}

/// Runtime adapter that keeps policy and sandbox orchestration on the
/// unified-exec side while delegating process startup to the manager.
pub struct UnifiedExecRuntime<'a> {
    manager: &'a UnifiedExecProcessManager,
}

pub(crate) struct UnifiedExecAttempt {
    pub(crate) process: UnifiedExecProcess,
    pub(crate) permissions: TerminalPermissions,
}

fn unified_exec_options() -> ExecOptions {
    ExecOptions {
        expiration: ExecExpiration::DefaultTimeout,
        capture_policy: ExecCapturePolicy::ShellTool,
    }
}

fn build_unified_exec_sandbox_command(
    command: &[String],
    cwd: &PathUri,
    env: &HashMap<String, String>,
    additional_permissions: Option<AdditionalPermissionProfile>,
) -> Result<SandboxCommand, ToolError> {
    let (program, args) = command
        .split_first()
        .ok_or_else(|| ToolError::Rejected("command args are empty".to_string()))?;
    Ok(SandboxCommand {
        program: program.clone().into(),
        args: args.to_vec(),
        cwd: cwd.clone(),
        env: env.clone(),
        additional_permissions,
    })
}

impl<'a> UnifiedExecRuntime<'a> {
    /// Creates a runtime bound to the shared unified-exec process manager.
    pub fn new(manager: &'a UnifiedExecProcessManager) -> Self {
        Self { manager }
    }
}

impl Sandboxable for UnifiedExecRuntime<'_> {
    fn sandbox_preference(&self) -> SandboxablePreference {
        SandboxablePreference::Auto
    }

    fn escalate_on_failure(&self) -> bool {
        true
    }
}

impl Approvable<UnifiedExecRequest> for UnifiedExecRuntime<'_> {
    fn approval_action(
        &self,
        req: &UnifiedExecRequest,
        call_id: &str,
    ) -> std::io::Result<ApprovalAction> {
        Ok(ApprovalAction::ExecCommand {
            id: call_id.to_string(),
            environment_id: req.turn_environment.selection.environment_id.clone(),
            command: req.command.clone(),
            cwd: req.cwd.clone(),
            sandbox_permissions: req.sandbox_permissions,
            additional_permissions: req.additional_permissions.clone(),
            justification: req.justification.clone(),
            tty: req.tty,
            proposed_execpolicy_amendment: req
                .exec_approval_requirement
                .proposed_execpolicy_amendment()
                .cloned(),
        })
    }

    fn exec_approval_requirement(
        &self,
        req: &UnifiedExecRequest,
    ) -> Option<ExecApprovalRequirement> {
        Some(req.exec_approval_requirement.clone())
    }

    fn sandbox_permissions(&self, req: &UnifiedExecRequest) -> SandboxPermissions {
        req.sandbox_permissions
    }
}

impl<'a> ToolRuntime<UnifiedExecRequest, UnifiedExecAttempt> for UnifiedExecRuntime<'a> {
    fn turn_environment<'b>(&self, req: &'b UnifiedExecRequest) -> &'b TurnEnvironment {
        &req.turn_environment
    }

    fn uses_executor_managed_process_sandbox(&self, req: &UnifiedExecRequest) -> bool {
        req.shell_snapshot.is_some()
    }

    fn sandbox_cwd<'b>(&self, req: &'b UnifiedExecRequest) -> Option<&'b PathUri> {
        Some(&req.sandbox_cwd)
    }

    async fn run(
        &mut self,
        req: &UnifiedExecRequest,
        attempt: &SandboxAttempt<'_>,
        ctx: &ToolCtx,
    ) -> Result<UnifiedExecAttempt, ToolError> {
        let base_command = &req.command;
        let session_shell = ctx.session.user_shell();
        let shell = req
            .turn_environment
            .shell
            .as_ref()
            .unwrap_or(session_shell.as_ref());
        // TODO(anp): Make shell snapshot lookup accept PathUri.
        let native_cwd = req
            .cwd
            .to_abs_path()
            .map_err(|err| ToolError::Rejected(err.to_string()))?;
        let shell_snapshot_location = req.turn_environment.shell_snapshot(&native_cwd);
        let env = req.env.clone();
        let explicit_env_overrides = req.explicit_env_overrides.clone();
        let command = maybe_wrap_shell_lc_with_snapshot(
            base_command,
            shell,
            shell_snapshot_location.as_ref(),
            &explicit_env_overrides,
            &env,
        );
        let command = prepare_powershell_command_for_elevated_windows_sandbox(
            &command,
            Some(&req.shell_type),
            attempt.sandbox_requested,
            attempt.windows_sandbox_level,
        );
        let command = if matches!(req.shell_type, ShellType::PowerShell) {
            prefix_powershell_script_with_utf8(&command)
        } else {
            command
        };
        let permissions = TerminalPermissions::for_launch(
            &req.turn_environment,
            if self.uses_executor_managed_process_sandbox(req) {
                TerminalSandboxSource::Executor
            } else {
                TerminalSandboxSource::Native
            },
            if attempt.is_escalated() {
                SandboxPermissions::RequireEscalated
            } else {
                SandboxPermissions::UseDefault
            },
            req.additional_permissions.as_ref(),
        );

        let command = build_unified_exec_sandbox_command(
            &command,
            &req.cwd,
            &env,
            req.additional_permissions.clone(),
        )
        .map_err(|error| match error {
            ToolError::Rejected(_) => {
                ToolError::Rejected("missing command line for PTY".to_string())
            }
            error @ ToolError::Codex(_) => error,
        })?;
        let options = unified_exec_options();
        let process = self
            .manager
            .open_session_with_exec_env(
                req.process_id,
                command,
                options,
                attempt,
                req.exec_server_env_config.clone(),
                req.shell_snapshot.clone(),
                req.tty,
                Box::new(NoopSpawnLifecycle),
                req.turn_environment.environment.as_ref(),
            )
            .await?;
        Ok(UnifiedExecAttempt {
            process,
            permissions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PermissionProfileSnapshot;
    use crate::environment_selection::EnvironmentConfigOrigin;
    use crate::tools::sandboxing::ToolRuntime;
    use codex_exec_server::Environment;
    use codex_exec_server::LOCAL_ENVIRONMENT_ID;
    use codex_protocol::config_types::WindowsSandboxLevel;
    use codex_protocol::models::PermissionProfile;
    use codex_protocol::protocol::EnvironmentConfig;
    use codex_protocol::protocol::EnvironmentConfigState;
    use codex_protocol::protocol::TurnEnvironmentSelection;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use codex_utils_path_uri::PathUri;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn test_turn_environment(cwd: PathUri) -> TurnEnvironment {
        TurnEnvironment::new(
            TurnEnvironmentSelection {
                environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
                cwd,
                workspace_roots: Vec::new(),
                config: EnvironmentConfigState::Ready(EnvironmentConfig {
                    allow_login_shell: true,
                    workspace_roots: Vec::new(),
                    windows_sandbox_level: WindowsSandboxLevel::Disabled,
                    windows_sandbox_private_desktop: true,
                    use_legacy_landlock: false,
                    permission_profile: PermissionProfileSnapshot::legacy(
                        PermissionProfile::read_only(),
                    ),
                    shell_environment_policy: Default::default(),
                    exec_policy: None,
                    mcp_policy: None,
                    network_policy: None,
                }),
            },
            EnvironmentConfigOrigin::Thread,
            Arc::new(Environment::default_for_tests()),
            /*shell*/ None,
        )
    }

    #[test]
    fn unified_exec_options_uses_default_timeout() {
        let options = unified_exec_options();

        assert_eq!(options.capture_policy, ExecCapturePolicy::ShellTool);
        assert!(matches!(options.expiration, ExecExpiration::DefaultTimeout));
    }

    #[tokio::test]
    async fn approval_key_includes_environment_id() {
        let manager = UnifiedExecProcessManager::default();
        let runtime = UnifiedExecRuntime::new(&manager);
        let mut request = test_request(
            SandboxPermissions::UseDefault,
            ExecApprovalRequirement::Skip {
                bypass_sandbox: false,
                proposed_execpolicy_amendment: None,
            },
        );
        request.turn_environment.selection.environment_id = "remote".to_string();
        let original_key = runtime
            .approval_action(&request, "call-1")
            .expect("build approval action")
            .cache_keys();
        request.turn_environment.selection.environment_id = "other".to_string();
        let other_key = runtime
            .approval_action(&request, "call-1")
            .expect("build approval action")
            .cache_keys();

        assert_ne!(original_key, other_key);
    }

    #[tokio::test]
    async fn unified_exec_uses_the_trusted_sandbox_cwd() {
        let cwd_dir = tempdir().expect("create process temp dir");
        let sandbox_dir = tempdir().expect("create sandbox temp dir");
        let cwd =
            AbsolutePathBuf::try_from(cwd_dir.path().to_path_buf()).expect("absolute temp dir");
        let sandbox_cwd = AbsolutePathBuf::try_from(sandbox_dir.path().to_path_buf())
            .expect("absolute sandbox temp dir");
        let manager = UnifiedExecProcessManager::default();
        let runtime = UnifiedExecRuntime::new(&manager);
        let request = UnifiedExecRequest {
            command: vec!["pwd".to_string()],
            shell_type: ShellType::Sh,
            process_id: 1000,
            cwd: cwd.into(),
            sandbox_cwd: sandbox_cwd.clone().into(),
            turn_environment: test_turn_environment(sandbox_cwd.clone().into()),
            env: HashMap::new(),
            exec_server_env_config: None,
            shell_snapshot: None,
            explicit_env_overrides: HashMap::new(),
            network: None,
            tty: false,
            sandbox_permissions: SandboxPermissions::UseDefault,
            additional_permissions: None,
            justification: None,
            exec_approval_requirement: ExecApprovalRequirement::Skip {
                bypass_sandbox: false,
                proposed_execpolicy_amendment: None,
            },
        };

        assert_eq!(
            runtime.sandbox_cwd(&request),
            Some(&PathUri::from_abs_path(&sandbox_cwd))
        );
    }

    #[tokio::test]
    async fn first_attempt_preserves_parent_sandbox_override() {
        let manager = UnifiedExecProcessManager::default();
        let request = test_request(
            SandboxPermissions::RequireEscalated,
            ExecApprovalRequirement::NeedsApproval {
                reason: None,
                proposed_execpolicy_amendment: None,
            },
        );
        let runtime = UnifiedExecRuntime::new(&manager);

        assert_eq!(
            runtime.sandbox_permissions(&request),
            SandboxPermissions::RequireEscalated,
            "unified exec should preserve a parent require_escalated request"
        );
    }

    #[tokio::test]
    async fn first_attempt_preserves_additional_permissions_request() {
        let manager = UnifiedExecProcessManager::default();
        let request = test_request(
            SandboxPermissions::WithAdditionalPermissions,
            ExecApprovalRequirement::NeedsApproval {
                reason: None,
                proposed_execpolicy_amendment: None,
            },
        );
        let runtime = UnifiedExecRuntime::new(&manager);

        assert_eq!(
            runtime.sandbox_permissions(&request),
            SandboxPermissions::WithAdditionalPermissions,
            "unified exec should keep bounded additional-permissions requests sandboxed"
        );
    }

    #[tokio::test]
    async fn execpolicy_allow_preserves_parent_sandbox_override() {
        let manager = UnifiedExecProcessManager::default();
        let request = test_request(
            SandboxPermissions::UseDefault,
            ExecApprovalRequirement::Skip {
                bypass_sandbox: true,
                proposed_execpolicy_amendment: None,
            },
        );
        let runtime = UnifiedExecRuntime::new(&manager);

        assert_eq!(
            runtime.exec_approval_requirement(&request),
            Some(ExecApprovalRequirement::Skip {
                bypass_sandbox: true,
                proposed_execpolicy_amendment: None,
            }),
            "unified exec should preserve exec-policy allow decisions that bypass the sandbox"
        );
    }

    fn test_request(
        sandbox_permissions: SandboxPermissions,
        exec_approval_requirement: ExecApprovalRequirement,
    ) -> UnifiedExecRequest {
        let cwd = AbsolutePathBuf::try_from(std::env::current_dir().unwrap())
            .expect("current dir is absolute");
        UnifiedExecRequest {
            command: vec!["zsh".to_string(), "-c".to_string(), "echo hi".to_string()],
            shell_type: ShellType::Zsh,
            process_id: 1000,
            cwd: cwd.clone().into(),
            sandbox_cwd: cwd.clone().into(),
            turn_environment: test_turn_environment(cwd.into()),
            env: HashMap::new(),
            exec_server_env_config: None,
            shell_snapshot: None,
            explicit_env_overrides: HashMap::new(),
            network: None,
            tty: false,
            sandbox_permissions,
            additional_permissions: None,
            justification: None,
            exec_approval_requirement,
        }
    }
}
