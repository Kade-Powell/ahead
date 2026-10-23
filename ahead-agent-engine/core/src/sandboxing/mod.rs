/*
Module: sandboxing

Core-owned adapter types for exec/runtime plumbing. Policy selection and
command transformation live in the codex-sandboxing crate; this module keeps
the exec-only metadata and translates transformed sandbox commands back into
ExecRequest for execution.
*/

use crate::exec::ExecCapturePolicy;
use crate::exec::ExecExpiration;
use crate::exec::StdoutStream;
use crate::exec::execute_exec_request;
#[cfg(target_os = "macos")]
use crate::spawn::CODEX_SANDBOX_ENV_VAR;
use crate::spawn::CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR;
use codex_file_system::FileSystemSandboxContext;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::error::CodexErr;
use codex_protocol::exec_output::ExecToolCallOutput;
use codex_protocol::models::PermissionProfile;
pub use codex_protocol::models::SandboxPermissions;
use codex_sandboxing::SandboxExecRequest;
use codex_sandboxing::SandboxType;
use codex_sandboxing::WindowsSandboxFilesystemOverrides;
use codex_sandboxing::resolve_windows_elevated_filesystem_overrides;
use codex_sandboxing::resolve_windows_restricted_token_filesystem_overrides;
use codex_sandboxing::windows_sandbox_uses_elevated_backend;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use codex_utils_string::truncate_middle_with_token_budget;
use std::collections::HashMap;

#[derive(Debug)]
pub(crate) struct ExecOptions {
    pub(crate) expiration: ExecExpiration,
    pub(crate) capture_policy: ExecCapturePolicy,
}

#[derive(Clone, Debug)]
pub(crate) struct ExecServerEnvConfig {
    pub(crate) policy: codex_exec_server::ExecEnvPolicy,
    pub(crate) local_policy_env: HashMap<String, String>,
}

#[derive(Debug)]
pub struct ExecRequest {
    pub command: Vec<String>,
    pub cwd: PathUri,
    pub env: HashMap<String, String>,
    pub(crate) exec_server_env_config: Option<ExecServerEnvConfig>,
    pub(crate) exec_server_shell_snapshot: Option<codex_exec_server::ShellSnapshotRequest>,
    pub expiration: ExecExpiration,
    pub capture_policy: ExecCapturePolicy,
    pub sandbox: SandboxType,
    pub windows_sandbox_policy_cwd: PathUri,
    pub windows_sandbox_workspace_roots: Vec<AbsolutePathBuf>,
    // TODO(anp): Reconcile these backend copies with TurnEnvironment::sandbox_context
    // and exec_server_sandbox so local launches use the same settings.
    pub windows_sandbox_level: WindowsSandboxLevel,
    pub windows_sandbox_private_desktop: bool,
    pub permission_profile: PermissionProfile,
    pub(crate) windows_sandbox_filesystem_overrides: Option<WindowsSandboxFilesystemOverrides>,
    pub arg0: Option<String>,
    pub(crate) exec_server_sandbox: Option<FileSystemSandboxContext>,
}

impl ExecRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        command: Vec<String>,
        cwd: AbsolutePathBuf,
        env: HashMap<String, String>,
        expiration: ExecExpiration,
        capture_policy: ExecCapturePolicy,
        sandbox: SandboxType,
        windows_sandbox_workspace_roots: Vec<AbsolutePathBuf>,
        windows_sandbox_level: WindowsSandboxLevel,
        windows_sandbox_private_desktop: bool,
        permission_profile: PermissionProfile,
        arg0: Option<String>,
    ) -> Self {
        let cwd = PathUri::from_abs_path(&cwd);
        let windows_sandbox_policy_cwd = cwd.clone();
        Self {
            command,
            cwd,
            env,
            exec_server_env_config: None,
            exec_server_shell_snapshot: None,
            expiration,
            capture_policy,
            sandbox,
            windows_sandbox_policy_cwd,
            windows_sandbox_workspace_roots,
            windows_sandbox_level,
            windows_sandbox_private_desktop,
            permission_profile,
            windows_sandbox_filesystem_overrides: None,
            arg0,
            exec_server_sandbox: None,
        }
    }

    pub(crate) fn from_sandbox_exec_request(
        request: SandboxExecRequest,
        options: ExecOptions,
        windows_sandbox_workspace_roots: Vec<AbsolutePathBuf>,
    ) -> Result<Self, CodexErr> {
        let SandboxExecRequest {
            command,
            cwd,
            sandbox_policy_cwd: windows_sandbox_policy_cwd,
            mut env,
            sandbox,
            windows_sandbox_level,
            windows_sandbox_private_desktop,
            permission_profile,
            arg0,
            ..
        } = request;
        let ExecOptions {
            expiration,
            capture_policy,
        } = options;
        let windows_sandbox_filesystem_overrides = if sandbox == SandboxType::WindowsRestrictedToken
        {
            let sandbox_policy_cwd = windows_sandbox_policy_cwd
                .to_abs_path()
                .map_err(|err| CodexErr::InvalidRequest(format!("invalid sandbox cwd: {err}")))?;
            let use_windows_elevated_backend =
                windows_sandbox_uses_elevated_backend(windows_sandbox_level);
            if use_windows_elevated_backend {
                resolve_windows_elevated_filesystem_overrides(
                    sandbox,
                    &permission_profile,
                    &sandbox_policy_cwd,
                    use_windows_elevated_backend,
                )
            } else {
                resolve_windows_restricted_token_filesystem_overrides(
                    sandbox,
                    &permission_profile,
                    &sandbox_policy_cwd,
                    windows_sandbox_level,
                )
            }
            .map_err(|error| {
                CodexErr::UnsupportedOperation(
                    truncate_middle_with_token_budget(&error, /*max_tokens*/ 900).0,
                )
            })?
        } else {
            None
        };
        let network_sandbox_policy = permission_profile.network_sandbox_policy();
        if !network_sandbox_policy.is_enabled() {
            env.insert(
                CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR.to_string(),
                "1".to_string(),
            );
        }
        #[cfg(target_os = "macos")]
        if sandbox == SandboxType::MacosSeatbelt {
            env.insert(CODEX_SANDBOX_ENV_VAR.to_string(), "seatbelt".to_string());
        }
        Ok(Self {
            command,
            cwd,
            env,
            exec_server_env_config: None,
            exec_server_shell_snapshot: None,
            expiration,
            capture_policy,
            sandbox,
            windows_sandbox_policy_cwd,
            windows_sandbox_workspace_roots,
            windows_sandbox_level,
            windows_sandbox_private_desktop,
            permission_profile,
            windows_sandbox_filesystem_overrides,
            arg0,
            exec_server_sandbox: None,
        })
    }
}

pub async fn execute_env(
    exec_request: ExecRequest,
    stdout_stream: Option<StdoutStream>,
) -> codex_protocol::error::Result<ExecToolCallOutput> {
    execute_exec_request(exec_request, stdout_stream, /*after_spawn*/ None).await
}

pub async fn execute_exec_request_with_after_spawn(
    exec_request: ExecRequest,
    stdout_stream: Option<StdoutStream>,
    after_spawn: Option<Box<dyn FnOnce() + Send>>,
) -> codex_protocol::error::Result<ExecToolCallOutput> {
    execute_exec_request(exec_request, stdout_stream, after_spawn).await
}
