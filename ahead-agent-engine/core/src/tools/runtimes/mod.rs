/*
Module: runtimes

Concrete ToolRuntime implementations for specific tools. Each runtime stays
small and focused and reuses the orchestrator for approvals + sandbox + retry.
*/
use crate::exec_env::CODEX_PERMISSION_PROFILE_ENV_VAR;
use crate::exec_env::CODEX_SESSION_ID_ENV_VAR;
use crate::exec_env::CODEX_THREAD_ID_ENV_VAR;
use crate::shell::Shell;
use crate::shell::ShellType;
use codex_apply_patch::CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS_ENV_VAR;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::shell_environment::is_non_inheritable_env_var;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::collections::HashMap;
use std::path::Path;

pub(crate) mod apply_patch;
pub(crate) mod unified_exec;

pub(crate) fn prepare_powershell_command_for_elevated_windows_sandbox(
    command: &[String],
    shell_type: Option<&ShellType>,
    sandbox_requested: bool,
    windows_sandbox_level: WindowsSandboxLevel,
) -> Vec<String> {
    prepare_powershell_command_for_elevated_windows_sandbox_with_fallback(
        command,
        shell_type,
        sandbox_requested,
        windows_sandbox_level,
        |path| {
            codex_shell_command::shell_detect::fallback_powershell_shell_for_elevated_windows_sandbox(
                path,
            )
        },
    )
}

fn prepare_powershell_command_for_elevated_windows_sandbox_with_fallback(
    command: &[String],
    shell_type: Option<&ShellType>,
    sandbox_requested: bool,
    windows_sandbox_level: WindowsSandboxLevel,
    find_fallback: impl FnOnce(&Path) -> Option<codex_shell_command::shell_detect::DetectedShell>,
) -> Vec<String> {
    if shell_type != Some(&ShellType::PowerShell)
        || !sandbox_requested
        || windows_sandbox_level != WindowsSandboxLevel::Elevated
        || command.is_empty()
    {
        return command.to_vec();
    }

    let mut command = command.to_vec();
    if let Some(fallback) = find_fallback(Path::new(&command[0])) {
        command[0] = fallback.shell_path.to_string_lossy().to_string();
    }

    if command[1..]
        .iter()
        .any(|arg| arg.eq_ignore_ascii_case("-NoProfile"))
    {
        return command;
    }

    // The elevated Windows sandbox runs as a dedicated sandbox account while
    // HOME/USERPROFILE may still point at the real user profile. Loading
    // PowerShell profiles in that mixed context is not a valid login shell.
    command.insert(1, "-NoProfile".to_string());
    command
}

/// POSIX-only helper: for commands produced by `Shell::derive_exec_args`
/// for Bash/Zsh/sh of the form `[shell_path, "-lc", "<script>"]`, and
/// when a snapshot is configured on the session shell, rewrite the argv
/// to a single non-login shell that sources the snapshot before running
/// the original script:
///
///   shell -lc "<script>"
///   => user_shell -c ". SNAPSHOT (best effort); exec shell -c <script>"
///
/// This wrapper script uses POSIX constructs (`if`, `.`, `exec`) so it can
/// be run by Bash/Zsh/sh. On non-matching commands, or when command cwd does
/// not match the snapshot cwd, this is a no-op.
///
/// `explicit_env_overrides` and `env` are intentionally separate inputs.
/// `explicit_env_overrides` contains policy-driven shell env overrides that
/// should win after the snapshot is sourced, while `env` is the full live exec
/// environment. We need access to both so snapshot restore logic can preserve
/// runtime-only vars like `CODEX_THREAD_ID` without pretending they came from
/// the explicit override policy.
///
pub(crate) fn maybe_wrap_shell_lc_with_snapshot(
    command: &[String],
    session_shell: &Shell,
    shell_snapshot: Option<&AbsolutePathBuf>,
    explicit_env_overrides: &HashMap<String, String>,
    env: &HashMap<String, String>,
) -> Vec<String> {
    if cfg!(windows) {
        return command.to_vec();
    }

    let Some(snapshot) = shell_snapshot else {
        return command.to_vec();
    };

    if !snapshot.exists() {
        return command.to_vec();
    }

    if command.len() < 3 {
        return command.to_vec();
    }

    let flag = command[1].as_str();
    if flag != "-lc" {
        return command.to_vec();
    }

    let snapshot_path = snapshot.to_string_lossy();
    let shell_path = session_shell.shell_path.to_string_lossy();
    let original_shell = shell_single_quote(&command[0]);
    let original_script = shell_single_quote(&command[2]);
    let snapshot_path = shell_single_quote(snapshot_path.as_ref());
    let trailing_args = command[3..]
        .iter()
        .map(|arg| format!(" '{}'", shell_single_quote(arg)))
        .collect::<String>();
    let mut override_env = explicit_env_overrides.clone();
    for key in [
        CODEX_SESSION_ID_ENV_VAR,
        CODEX_THREAD_ID_ENV_VAR,
        CODEX_PERMISSION_PROFILE_ENV_VAR,
        CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS_ENV_VAR,
    ] {
        if let Some(value) = env.get(key) {
            override_env.insert(key.to_string(), value.clone());
        }
    }
    // Do not let a snapshot resurrect stale runtime state when it is inactive.
    let (override_captures, override_exports) = build_override_exports(
        &override_env,
        &[
            CODEX_PERMISSION_PROFILE_ENV_VAR,
            CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS_ENV_VAR,
        ],
    );
    let rewritten_script = if override_exports.is_empty() {
        format!(
            "if . '{snapshot_path}' >/dev/null 2>&1; then :; fi\n\nexec '{original_shell}' -c '{original_script}'{trailing_args}"
        )
    } else {
        format!(
            "{override_captures}\n\nif . '{snapshot_path}' >/dev/null 2>&1; then :; fi\n\n{override_exports}\n\nexec '{original_shell}' -c '{original_script}'{trailing_args}"
        )
    };

    vec![shell_path.to_string(), "-c".to_string(), rewritten_script]
}

fn build_override_exports(
    explicit_env_overrides: &HashMap<String, String>,
    restore_even_when_absent: &[&str],
) -> (String, String) {
    let mut keys = explicit_env_overrides
        .keys()
        .map(String::as_str)
        .chain(restore_even_when_absent.iter().copied())
        .filter(|key| !is_non_inheritable_env_var(key))
        .filter(|key| is_valid_shell_variable_name(key))
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();

    build_override_exports_for_keys("__CODEX_SNAPSHOT_OVERRIDE", &keys)
}

fn build_override_exports_for_keys(variable_prefix: &str, keys: &[&str]) -> (String, String) {
    if keys.is_empty() {
        return (String::new(), String::new());
    }

    let captures = keys
        .iter()
        .enumerate()
        .map(|(idx, key)| {
            let set_var = format!("{variable_prefix}_SET_{idx}");
            let value_var = format!("{variable_prefix}_{idx}");
            format!("{set_var}=\"${{{key}+x}}\"\n{value_var}=\"${{{key}-}}\"")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let restores = keys
        .iter()
        .enumerate()
        .map(|(idx, key)| {
            let set_var = format!("{variable_prefix}_SET_{idx}");
            let value_var = format!("{variable_prefix}_{idx}");
            format!(
                "if [ -n \"${{{set_var}}}\" ]; then export {key}=\"${{{value_var}}}\"; else unset {key}; fi"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    (captures, restores)
}

fn is_valid_shell_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn shell_single_quote(input: &str) -> String {
    input.replace('\'', r#"'"'"'"#)
}

#[cfg(test)]
mod prepare_powershell_command_tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn inserts_no_profile_for_elevated_windows_sandbox() {
        let command = vec![
            "powershell.exe".to_string(),
            "-Command".to_string(),
            "Write-Output ok".to_string(),
        ];

        let rewritten = prepare_powershell_command_for_elevated_windows_sandbox(
            &command,
            Some(&ShellType::PowerShell),
            /*sandbox_requested*/ true,
            WindowsSandboxLevel::Elevated,
        );

        assert_eq!(
            rewritten,
            vec![
                "powershell.exe".to_string(),
                "-NoProfile".to_string(),
                "-Command".to_string(),
                "Write-Output ok".to_string(),
            ]
        );
    }

    #[test]
    fn inserts_no_profile_before_encoded_command() {
        let command = vec![
            "powershell.exe".to_string(),
            "-EncodedCommand".to_string(),
            "VwByAGkAdABlAC0ATwB1AHQAcAB1AHQAIABvAGsA".to_string(),
        ];

        let rewritten = prepare_powershell_command_for_elevated_windows_sandbox(
            &command,
            Some(&ShellType::PowerShell),
            /*sandbox_requested*/ true,
            WindowsSandboxLevel::Elevated,
        );

        assert_eq!(
            rewritten,
            vec![
                "powershell.exe".to_string(),
                "-NoProfile".to_string(),
                "-EncodedCommand".to_string(),
                "VwByAGkAdABlAC0ATwB1AHQAcAB1AHQAIABvAGsA".to_string(),
            ]
        );
    }

    #[test]
    fn preserves_existing_no_profile() {
        let command = vec![
            "pwsh.exe".to_string(),
            "-NoProfile".to_string(),
            "-Command".to_string(),
            "Write-Output ok".to_string(),
        ];

        let rewritten = prepare_powershell_command_for_elevated_windows_sandbox(
            &command,
            Some(&ShellType::PowerShell),
            /*sandbox_requested*/ true,
            WindowsSandboxLevel::Elevated,
        );

        assert_eq!(rewritten, command);
    }

    #[test]
    fn leaves_legacy_restricted_token_backend_alone() {
        let command = vec![
            r"C:\Program Files\WindowsApps\Microsoft.PowerShell_7.6.4.0_x64__8wekyb3d8bbwe\pwsh.exe"
                .to_string(),
            "-Command".to_string(),
            "Write-Output ok".to_string(),
        ];

        let rewritten = prepare_powershell_command_for_elevated_windows_sandbox(
            &command,
            Some(&ShellType::PowerShell),
            /*sandbox_requested*/ true,
            WindowsSandboxLevel::RestrictedToken,
        );

        assert_eq!(rewritten, command);
    }

    #[test]
    fn leaves_unsandboxed_attempts_alone() {
        let command = vec![
            r"C:\Program Files\WindowsApps\Microsoft.PowerShell_7.6.4.0_x64__8wekyb3d8bbwe\pwsh.exe"
                .to_string(),
            "-Command".to_string(),
            "Write-Output ok".to_string(),
        ];

        let rewritten = prepare_powershell_command_for_elevated_windows_sandbox(
            &command,
            Some(&ShellType::PowerShell),
            /*sandbox_requested*/ false,
            WindowsSandboxLevel::Elevated,
        );

        assert_eq!(rewritten, command);
    }

    #[test]
    fn leaves_non_powershell_alone() {
        let command = vec![
            "/bin/bash".to_string(),
            "-lc".to_string(),
            "echo ok".to_string(),
        ];

        let rewritten = prepare_powershell_command_for_elevated_windows_sandbox(
            &command,
            Some(&ShellType::Bash),
            /*sandbox_requested*/ true,
            WindowsSandboxLevel::Elevated,
        );

        assert_eq!(rewritten, command);
    }

    #[test]
    fn local_elevated_powershell_uses_discovered_fallback() {
        let command = vec![
            r"C:\Program Files\WindowsApps\Microsoft.PowerShell_7.6.4.0_x64__8wekyb3d8bbwe\pwsh.exe"
                .to_string(),
            "-Command".to_string(),
            "Write-Output ok".to_string(),
        ];
        let fallback_path = std::path::PathBuf::from(r"C:\Program Files\PowerShell\7\pwsh.exe");

        let rewritten = prepare_powershell_command_for_elevated_windows_sandbox_with_fallback(
            &command,
            Some(&ShellType::PowerShell),
            /*sandbox_requested*/ true,
            WindowsSandboxLevel::Elevated,
            |_| {
                Some(codex_shell_command::shell_detect::DetectedShell {
                    shell_type: ShellType::PowerShell,
                    shell_path: fallback_path.clone(),
                })
            },
        );

        assert_eq!(
            rewritten,
            vec![
                fallback_path.to_string_lossy().to_string(),
                "-NoProfile".to_string(),
                "-Command".to_string(),
                "Write-Output ok".to_string(),
            ]
        );
    }
}

#[cfg(all(test, unix))]
#[path = "mod_tests.rs"]
mod tests;
