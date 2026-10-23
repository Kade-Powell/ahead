use super::*;
use crate::shell::ShellType;
use codex_utils_absolute_path::AbsolutePathBuf;
use core_test_support::PathBufExt;
use pretty_assertions::assert_eq;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

fn shell_with_snapshot(
    shell_type: ShellType,
    shell_path: &str,
    snapshot_path: AbsolutePathBuf,
) -> (Shell, AbsolutePathBuf) {
    (
        Shell {
            shell_type,
            shell_path: PathBuf::from(shell_path),
        },
        snapshot_path,
    )
}

#[test]
fn explicit_escalation_preserves_user_ca_env() {
    let env = HashMap::from([(
        "SSL_CERT_FILE".to_string(),
        "/tmp/custom-ca.pem".to_string(),
    )]);

    let env = exec_env_for_sandbox_permissions(&env, SandboxPermissions::RequireEscalated);

    assert_eq!(
        env.get("SSL_CERT_FILE"),
        Some(&"/tmp/custom-ca.pem".to_string())
    );
}

#[test]
fn explicit_escalation_keeps_user_proxy_env_without_codex_marker() {
    let env = HashMap::from([
        (
            "HTTP_PROXY".to_string(),
            "http://user.proxy:8080".to_string(),
        ),
        ("CUSTOM_ENV".to_string(), "kept".to_string()),
    ]);

    let env = exec_env_for_sandbox_permissions(&env, SandboxPermissions::RequireEscalated);

    assert_eq!(
        env.get("HTTP_PROXY"),
        Some(&"http://user.proxy:8080".to_string())
    );
    assert_eq!(env.get("CUSTOM_ENV"), Some(&"kept".to_string()));
}

#[test]
fn maybe_wrap_shell_lc_with_snapshot_bootstraps_in_user_shell() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_path = dir.path().join("snapshot.sh");
    std::fs::write(&snapshot_path, "# Snapshot file\n").expect("write snapshot");
    let (session_shell, shell_snapshot) =
        shell_with_snapshot(ShellType::Zsh, "/bin/zsh", snapshot_path.abs());
    let command = vec![
        "/bin/bash".to_string(),
        "-lc".to_string(),
        "echo hello".to_string(),
    ];

    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &HashMap::new(),
        &HashMap::new(),
    );

    assert_eq!(rewritten[0], "/bin/zsh");
    assert_eq!(rewritten[1], "-c");
    assert!(rewritten[2].contains("if . '"));
    assert!(rewritten[2].contains("exec '/bin/bash' -c 'echo hello'"));
}

#[test]
fn maybe_wrap_shell_lc_with_snapshot_escapes_single_quotes() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_path = dir.path().join("snapshot.sh");
    std::fs::write(&snapshot_path, "# Snapshot file\n").expect("write snapshot");
    let (session_shell, shell_snapshot) =
        shell_with_snapshot(ShellType::Zsh, "/bin/zsh", snapshot_path.abs());
    let command = vec![
        "/bin/bash".to_string(),
        "-lc".to_string(),
        "echo 'hello'".to_string(),
    ];

    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &HashMap::new(),
        &HashMap::new(),
    );

    assert!(rewritten[2].contains(r#"exec '/bin/bash' -c 'echo '"'"'hello'"'"''"#));
}

#[test]
fn maybe_wrap_shell_lc_with_snapshot_uses_bash_bootstrap_shell() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_path = dir.path().join("snapshot.sh");
    std::fs::write(&snapshot_path, "# Snapshot file\n").expect("write snapshot");
    let (session_shell, shell_snapshot) =
        shell_with_snapshot(ShellType::Bash, "/bin/bash", snapshot_path.abs());
    let command = vec![
        "/bin/zsh".to_string(),
        "-lc".to_string(),
        "echo hello".to_string(),
    ];

    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &HashMap::new(),
        &HashMap::new(),
    );

    assert_eq!(rewritten[0], "/bin/bash");
    assert_eq!(rewritten[1], "-c");
    assert!(rewritten[2].contains("if . '"));
    assert!(rewritten[2].contains("exec '/bin/zsh' -c 'echo hello'"));
}

#[test]
fn maybe_wrap_shell_lc_with_snapshot_uses_sh_bootstrap_shell() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_path = dir.path().join("snapshot.sh");
    std::fs::write(&snapshot_path, "# Snapshot file\n").expect("write snapshot");
    let (session_shell, shell_snapshot) =
        shell_with_snapshot(ShellType::Sh, "/bin/sh", snapshot_path.abs());
    let command = vec![
        "/bin/bash".to_string(),
        "-lc".to_string(),
        "echo hello".to_string(),
    ];

    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &HashMap::new(),
        &HashMap::new(),
    );

    assert_eq!(rewritten[0], "/bin/sh");
    assert_eq!(rewritten[1], "-c");
    assert!(rewritten[2].contains("if . '"));
    assert!(rewritten[2].contains("exec '/bin/bash' -c 'echo hello'"));
}

#[test]
fn maybe_wrap_shell_lc_with_snapshot_preserves_trailing_args() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_path = dir.path().join("snapshot.sh");
    std::fs::write(&snapshot_path, "# Snapshot file\n").expect("write snapshot");
    let (session_shell, shell_snapshot) =
        shell_with_snapshot(ShellType::Zsh, "/bin/zsh", snapshot_path.abs());
    let command = vec![
        "/bin/bash".to_string(),
        "-lc".to_string(),
        "printf '%s %s' \"$0\" \"$1\"".to_string(),
        "arg0".to_string(),
        "arg1".to_string(),
    ];

    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &HashMap::new(),
        &HashMap::new(),
    );

    assert!(
        rewritten[2]
            .contains(r#"exec '/bin/bash' -c 'printf '"'"'%s %s'"'"' "$0" "$1"' 'arg0' 'arg1'"#)
    );
}

#[test]
fn maybe_wrap_shell_lc_with_snapshot_restores_explicit_override_precedence() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_path = dir.path().join("snapshot.sh");
    std::fs::write(
        &snapshot_path,
        "# Snapshot file\nexport TEST_ENV_SNAPSHOT=global\nexport SNAPSHOT_ONLY=from_snapshot\n",
    )
    .expect("write snapshot");
    let (session_shell, shell_snapshot) =
        shell_with_snapshot(ShellType::Bash, "/bin/bash", snapshot_path.abs());
    let command = vec![
        "/bin/bash".to_string(),
        "-lc".to_string(),
        "printf '%s|%s' \"$TEST_ENV_SNAPSHOT\" \"${SNAPSHOT_ONLY-unset}\"".to_string(),
    ];
    let explicit_env_overrides =
        HashMap::from([("TEST_ENV_SNAPSHOT".to_string(), "worktree".to_string())]);
    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &explicit_env_overrides,
        &HashMap::from([("TEST_ENV_SNAPSHOT".to_string(), "worktree".to_string())]),
    );
    let output = Command::new(&rewritten[0])
        .args(&rewritten[1..])
        .env("TEST_ENV_SNAPSHOT", "worktree")
        .output()
        .expect("run rewritten command");

    assert!(output.status.success(), "command failed: {output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "worktree|from_snapshot"
    );
}

#[test]
fn maybe_wrap_shell_lc_with_snapshot_restores_codex_thread_id_from_env() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_path = dir.path().join("snapshot.sh");
    std::fs::write(
        &snapshot_path,
        "# Snapshot file\nexport CODEX_THREAD_ID='parent-thread'\n",
    )
    .expect("write snapshot");
    let (session_shell, shell_snapshot) =
        shell_with_snapshot(ShellType::Bash, "/bin/bash", snapshot_path.abs());
    let command = vec![
        "/bin/bash".to_string(),
        "-lc".to_string(),
        "printf '%s' \"$CODEX_THREAD_ID\"".to_string(),
    ];
    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &HashMap::new(),
        &HashMap::from([("CODEX_THREAD_ID".to_string(), "nested-thread".to_string())]),
    );
    let output = Command::new(&rewritten[0])
        .args(&rewritten[1..])
        .env("CODEX_THREAD_ID", "nested-thread")
        .output()
        .expect("run rewritten command");

    assert!(output.status.success(), "command failed: {output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "nested-thread");
}

#[test]
fn maybe_wrap_shell_lc_with_snapshot_restores_permission_profile_from_env() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_path = dir.path().join("snapshot.sh");
    std::fs::write(
        &snapshot_path,
        "# Snapshot file\nexport CODEX_PERMISSION_PROFILE='parent-profile'\n",
    )
    .expect("write snapshot");
    let (session_shell, shell_snapshot) =
        shell_with_snapshot(ShellType::Bash, "/bin/bash", snapshot_path.abs());
    let command = vec![
        "/bin/bash".to_string(),
        "-lc".to_string(),
        "printenv CODEX_PERMISSION_PROFILE".to_string(),
    ];
    let env = HashMap::from([(
        CODEX_PERMISSION_PROFILE_ENV_VAR.to_string(),
        "current-profile".to_string(),
    )]);
    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &HashMap::new(),
        &env,
    );
    let output = Command::new(&rewritten[0])
        .args(&rewritten[1..])
        .env(CODEX_PERMISSION_PROFILE_ENV_VAR, "current-profile")
        .output()
        .expect("run rewritten command");

    assert!(output.status.success(), "command failed: {output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "current-profile\n");
}

#[test]
fn maybe_wrap_shell_lc_with_snapshot_unsets_absent_permission_profile() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_path = dir.path().join("snapshot.sh");
    std::fs::write(
        &snapshot_path,
        "# Snapshot file\nexport CODEX_PERMISSION_PROFILE='stale-profile'\n",
    )
    .expect("write snapshot");
    let (session_shell, shell_snapshot) =
        shell_with_snapshot(ShellType::Bash, "/bin/bash", snapshot_path.abs());
    let command = vec![
        "/bin/bash".to_string(),
        "-lc".to_string(),
        "printenv CODEX_PERMISSION_PROFILE".to_string(),
    ];
    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &HashMap::new(),
        &HashMap::new(),
    );
    let output = Command::new(&rewritten[0])
        .args(&rewritten[1..])
        .env_remove(CODEX_PERMISSION_PROFILE_ENV_VAR)
        .output()
        .expect("run rewritten command");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"");
}

#[test]
fn maybe_wrap_shell_lc_with_snapshot_restores_apply_patch_rollout_state() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_path = dir.path().join("snapshot.sh");
    std::fs::write(
        &snapshot_path,
        "# Snapshot file\nexport CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS='stale'\n",
    )
    .expect("write snapshot");
    let (session_shell, shell_snapshot) =
        shell_with_snapshot(ShellType::Bash, "/bin/bash", snapshot_path.abs());
    let command = vec![
        "/bin/bash".to_string(),
        "-lc".to_string(),
        "printenv CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS".to_string(),
    ];
    let env = HashMap::from([(
        CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS_ENV_VAR.to_string(),
        "1".to_string(),
    )]);
    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &HashMap::new(),
        &env,
    );
    let output = Command::new(&rewritten[0])
        .args(&rewritten[1..])
        .env(CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS_ENV_VAR, "1")
        .output()
        .expect("run rewritten command");

    assert!(output.status.success(), "command failed: {output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "1\n");

    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &HashMap::new(),
        &HashMap::new(),
    );
    let output = Command::new(&rewritten[0])
        .args(&rewritten[1..])
        .env_remove(CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS_ENV_VAR)
        .output()
        .expect("run rewritten command");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"");
}

#[test]
fn maybe_wrap_shell_lc_with_snapshot_keeps_user_proxy_env() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_path = dir.path().join("snapshot.sh");
    std::fs::write(
        &snapshot_path,
        "# Snapshot file\nexport HTTP_PROXY='http://user.proxy:8080'\n",
    )
    .expect("write snapshot");
    let (session_shell, shell_snapshot) =
        shell_with_snapshot(ShellType::Bash, "/bin/bash", snapshot_path.abs());
    let command = vec![
        "/bin/bash".to_string(),
        "-lc".to_string(),
        "printf '%s' \"$HTTP_PROXY\"".to_string(),
    ];
    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &HashMap::new(),
        &HashMap::new(),
    );
    let mut command = Command::new(&rewritten[0]);
    command.args(&rewritten[1..]);
    command.env_remove("HTTP_PROXY");
    let output = command.output().expect("run rewritten command");

    assert!(output.status.success(), "command failed: {output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "http://user.proxy:8080"
    );
}

#[test]
fn maybe_wrap_shell_lc_with_snapshot_keeps_snapshot_path_without_override() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_path = dir.path().join("snapshot.sh");
    std::fs::write(
        &snapshot_path,
        "# Snapshot file\nexport PATH='/snapshot/bin'\n",
    )
    .expect("write snapshot");
    let (session_shell, shell_snapshot) =
        shell_with_snapshot(ShellType::Bash, "/bin/bash", snapshot_path.abs());
    let command = vec![
        "/bin/bash".to_string(),
        "-lc".to_string(),
        "printf '%s' \"$PATH\"".to_string(),
    ];
    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &HashMap::new(),
        &HashMap::new(),
    );
    let output = Command::new(&rewritten[0])
        .args(&rewritten[1..])
        .output()
        .expect("run rewritten command");

    assert!(output.status.success(), "command failed: {output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "/snapshot/bin");
}

#[test]
fn maybe_wrap_shell_lc_with_snapshot_applies_explicit_path_override() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_path = dir.path().join("snapshot.sh");
    std::fs::write(
        &snapshot_path,
        "# Snapshot file\nexport PATH='/snapshot/bin'\n",
    )
    .expect("write snapshot");
    let (session_shell, shell_snapshot) =
        shell_with_snapshot(ShellType::Bash, "/bin/bash", snapshot_path.abs());
    let command = vec![
        "/bin/bash".to_string(),
        "-lc".to_string(),
        "printf '%s' \"$PATH\"".to_string(),
    ];
    let explicit_env_overrides = HashMap::from([("PATH".to_string(), "/worktree/bin".to_string())]);
    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &explicit_env_overrides,
        &HashMap::from([("PATH".to_string(), "/worktree/bin".to_string())]),
    );
    let output = Command::new(&rewritten[0])
        .args(&rewritten[1..])
        .env("PATH", "/worktree/bin")
        .output()
        .expect("run rewritten command");

    assert!(output.status.success(), "command failed: {output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "/worktree/bin");
}

#[test]
fn maybe_wrap_shell_lc_with_snapshot_does_not_embed_override_values_in_argv() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_path = dir.path().join("snapshot.sh");
    std::fs::write(
        &snapshot_path,
        "# Snapshot file\nexport OPENAI_API_KEY='snapshot-value'\n",
    )
    .expect("write snapshot");
    let (session_shell, shell_snapshot) =
        shell_with_snapshot(ShellType::Bash, "/bin/bash", snapshot_path.abs());
    let command = vec![
        "/bin/bash".to_string(),
        "-lc".to_string(),
        "printf '%s' \"$OPENAI_API_KEY\"".to_string(),
    ];
    let explicit_env_overrides = HashMap::from([
        (
            "OPENAI_API_KEY".to_string(),
            "super-secret-value".to_string(),
        ),
        (
            "openai_identity_token_file".to_string(),
            "/run/identity-token".to_string(),
        ),
    ]);
    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &explicit_env_overrides,
        &HashMap::from([(
            "OPENAI_API_KEY".to_string(),
            "super-secret-value".to_string(),
        )]),
    );

    assert!(!rewritten[2].contains("super-secret-value"));
    assert!(!rewritten[2].contains("openai_identity_token_file"));
    let output = Command::new(&rewritten[0])
        .args(&rewritten[1..])
        .env("OPENAI_API_KEY", "super-secret-value")
        .output()
        .expect("run rewritten command");
    assert!(output.status.success(), "command failed: {output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "super-secret-value"
    );
}

#[test]
fn maybe_wrap_shell_lc_with_snapshot_preserves_unset_override_variables() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_path = dir.path().join("snapshot.sh");
    std::fs::write(
        &snapshot_path,
        "# Snapshot file\nexport CODEX_TEST_UNSET_OVERRIDE='snapshot-value'\n",
    )
    .expect("write snapshot");
    let (session_shell, shell_snapshot) =
        shell_with_snapshot(ShellType::Bash, "/bin/bash", snapshot_path.abs());
    let command = vec![
            "/bin/bash".to_string(),
            "-lc".to_string(),
            "if [ \"${CODEX_TEST_UNSET_OVERRIDE+x}\" = x ]; then printf 'set:%s' \"$CODEX_TEST_UNSET_OVERRIDE\"; else printf 'unset'; fi".to_string(),
        ];
    let explicit_env_overrides = HashMap::from([(
        "CODEX_TEST_UNSET_OVERRIDE".to_string(),
        "worktree-value".to_string(),
    )]);
    let rewritten = maybe_wrap_shell_lc_with_snapshot(
        &command,
        &session_shell,
        Some(&shell_snapshot),
        &explicit_env_overrides,
        &HashMap::new(),
    );

    let output = Command::new(&rewritten[0])
        .args(&rewritten[1..])
        .env_remove("CODEX_TEST_UNSET_OVERRIDE")
        .output()
        .expect("run rewritten command");
    assert!(output.status.success(), "command failed: {output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "unset");
}
