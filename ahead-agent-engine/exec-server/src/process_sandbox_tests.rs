use std::collections::HashMap;

#[cfg(windows)]
use codex_protocol::config_types::WindowsSandboxLevel;
#[cfg(any(unix, windows))]
use codex_protocol::models::PermissionProfile;
#[cfg(target_os = "linux")]
use codex_sandboxing::landlock::CODEX_LINUX_SANDBOX_ARG0;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;

use super::prepare_exec_request;
#[cfg(unix)]
use crate::AHEAD_ARG0_EXEC_HELPER_ARG1;
use crate::ExecParams;
#[cfg(any(unix, windows))]
use crate::FileSystemSandboxContext;
#[cfg(any(unix, windows))]
use crate::LocalAgentRuntimePaths;
use crate::ProcessId;

#[cfg(unix)]
#[test]
fn sandbox_request_wraps_native_argv_on_executor() {
    let cwd: AbsolutePathBuf = std::env::current_dir()
        .expect("current directory")
        .try_into()
        .expect("absolute cwd");
    let cwd_uri = PathUri::from_abs_path(&cwd);
    let self_exe = std::env::current_exe().expect("current executable");
    let runtime_paths =
        LocalAgentRuntimePaths::new(self_exe.clone(), Some(self_exe)).expect("runtime paths");
    let sandbox = FileSystemSandboxContext::from_permission_profile_with_cwd(
        PermissionProfile::workspace_write(),
        cwd_uri.clone(),
    );
    let params = ExecParams {
        process_id: ProcessId::from("process-1"),
        argv: vec![
            "/bin/bash".to_string(),
            "-lc".to_string(),
            "pwd".to_string(),
        ],
        cwd: cwd_uri,
        shell_snapshot: None,
        env_policy: None,
        env: HashMap::new(),
        tty: false,
        pipe_stdin: false,
        arg0: None,
        sandbox: Some(sandbox),
    };

    let prepared = prepare_exec_request(&params, HashMap::new(), Some(&runtime_paths))
        .expect("prepare sandboxed request");

    assert_ne!(prepared.command, params.argv);
    assert_eq!(prepared.cwd, cwd);
    #[cfg(target_os = "linux")]
    {
        assert_eq!(
            prepared.command.first(),
            Some(&runtime_paths.ahead_self_exe.to_string_lossy().into_owned())
        );
        let permission_profile_json = prepared
            .command
            .iter()
            .position(|arg| arg == "--permission-profile")
            .and_then(|index| prepared.command.get(index + 1))
            .expect("sandbox wrapper permission profile");
        let permission_profile: PermissionProfile =
            serde_json::from_str(permission_profile_json).expect("permission profile JSON");
        assert_eq!(
            permission_profile,
            PermissionProfile::workspace_write()
                .materialize_project_roots_with_workspace_roots(std::slice::from_ref(&cwd))
        );
    }
    #[cfg(target_os = "macos")]
    assert_eq!(
        prepared.command.first().map(String::as_str),
        Some("/usr/bin/sandbox-exec")
    );
}

#[cfg(unix)]
#[test]
fn sandbox_request_routes_custom_arg0_to_inner_helper() {
    let cwd: AbsolutePathBuf = std::env::current_dir()
        .expect("current directory")
        .try_into()
        .expect("absolute cwd");
    let cwd_uri = PathUri::from_abs_path(&cwd);
    let self_exe = std::env::current_exe().expect("current executable");
    let runtime_paths =
        LocalAgentRuntimePaths::new(self_exe.clone(), Some(self_exe)).expect("runtime paths");
    let sandbox = FileSystemSandboxContext::from_permission_profile_with_cwd(
        PermissionProfile::workspace_write(),
        cwd_uri.clone(),
    );
    let params = ExecParams {
        process_id: ProcessId::from("process-custom-arg0"),
        argv: vec!["/bin/sh".to_string(), "-c".to_string(), "true".to_string()],
        cwd: cwd_uri,
        shell_snapshot: None,
        env_policy: None,
        env: HashMap::new(),
        tty: false,
        pipe_stdin: false,
        arg0: Some("custom-arg0".to_string()),
        sandbox: Some(sandbox),
    };

    let prepared = prepare_exec_request(&params, HashMap::new(), Some(&runtime_paths))
        .expect("prepare sandboxed request");
    let helper_mode = prepared
        .command
        .iter()
        .position(|arg| arg == AHEAD_ARG0_EXEC_HELPER_ARG1)
        .expect("sandboxed command should invoke arg0 helper");

    assert_eq!(
        prepared.command[helper_mode..],
        [
            AHEAD_ARG0_EXEC_HELPER_ARG1,
            "custom-arg0",
            "/bin/sh",
            "-c",
            "true",
        ]
    );
    #[cfg(target_os = "linux")]
    assert_eq!(prepared.arg0, Some(CODEX_LINUX_SANDBOX_ARG0.to_string()));
    #[cfg(target_os = "macos")]
    assert_eq!(prepared.arg0, None);
}

#[test]
fn native_request_preserves_native_launch_fields() {
    let cwd: AbsolutePathBuf = std::env::current_dir()
        .expect("current directory")
        .try_into()
        .expect("absolute cwd");
    let cwd_uri = PathUri::from_abs_path(&cwd);
    let env = HashMap::from([("TEST_ENV".to_string(), "value".to_string())]);
    let params = ExecParams {
        process_id: ProcessId::from("process-1"),
        argv: vec!["echo".to_string(), "hello".to_string()],
        cwd: cwd_uri,
        shell_snapshot: None,
        env_policy: None,
        env: HashMap::new(),
        tty: false,
        pipe_stdin: false,
        arg0: Some("custom-arg0".to_string()),
        sandbox: None,
    };

    let prepared = prepare_exec_request(&params, env.clone(), /*runtime_paths*/ None)
        .expect("prepare native request");

    assert_eq!(prepared.command, params.argv);
    assert_eq!(prepared.cwd, cwd);
    assert_eq!(prepared.env, env);
    assert_eq!(prepared.arg0, params.arg0);
}
