//! Direct policy comparisons, launch capture, and private approval text.

use super::*;
use crate::session::tests::make_session_and_context;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::PermissionProfileSnapshot;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;

fn terminal_permissions(profile: &PermissionProfile) -> TerminalPermissions {
    TerminalPermissions {
        policy: TerminalPolicy {
            sandbox: FileSystemSandboxContext::from_permission_profile(
                effective_permission_profile(profile, /*additional_permissions*/ None),
            ),
            network: profile.network_sandbox_policy(),
        },
        sandbox_source: TerminalSandboxSource::Native,
        launch_permissions: SandboxPermissions::UseDefault,
        additional_permissions: None,
    }
}

#[test]
fn reduced_permissions_require_review() -> anyhow::Result<()> {
    let permissions = terminal_permissions(&PermissionProfile::Disabled);
    let baseline = PermissionProfile::read_only();
    let current = terminal_permissions(&baseline);
    let expected = SandboxPermissions::RequireEscalated;
    assert_eq!(
        permissions.review_requirement(&current.policy, &baseline),
        Ok(expected)
    );
    insta::assert_snapshot!(
        "reduced_permissions",
        permissions.approval_reason(expected)?
    );
    Ok(())
}

#[test]
fn sandbox_bypass_requires_review_even_when_permissions_match() -> anyhow::Result<()> {
    let baseline = PermissionProfile::Disabled;
    let mut permissions = terminal_permissions(&baseline);
    permissions.launch_permissions = SandboxPermissions::RequireEscalated;
    let expected = SandboxPermissions::RequireEscalated;
    assert_eq!(
        permissions.review_requirement(&permissions.policy, &baseline),
        Ok(expected)
    );
    insta::assert_snapshot!("sandbox_bypass", permissions.approval_reason(expected)?);
    Ok(())
}

#[test]
fn denied_reads_reject_file_system_changes_but_only_review_network_changes() {
    let mut file_system = FileSystemSandboxPolicy::read_only();
    file_system.entries.push(FileSystemSandboxEntry::new(
        FileSystemPath::GlobPattern {
            pattern: "**/.env".into(),
        },
        FileSystemAccessMode::Deny,
    ));
    let baseline =
        PermissionProfile::from_runtime_permissions(&file_system, NetworkSandboxPolicy::Restricted);
    let permissions = terminal_permissions(&PermissionProfile::Disabled);
    let current = terminal_permissions(&baseline);
    assert_eq!(
        permissions.review_requirement(&current.policy, &baseline),
        Err(
            "this terminal cannot enforce the current denied-read restrictions; start a new terminal"
        )
    );

    let permissions = current;
    let baseline =
        PermissionProfile::from_runtime_permissions(&file_system, NetworkSandboxPolicy::Enabled);
    let current = terminal_permissions(&baseline);
    assert_eq!(
        permissions.review_requirement(&current.policy, &baseline),
        Ok(SandboxPermissions::RequireEscalated)
    );
}

#[tokio::test]
async fn captured_network_changes_require_review_or_a_new_terminal() -> anyhow::Result<()> {
    let (_session, turn) = make_session_and_context().await;
    let mut environment = turn.environments.primary().expect("environment").clone();
    let file_system_policy = FileSystemSandboxPolicy::read_only();
    let restricted = PermissionProfile::from_runtime_permissions(
        &file_system_policy,
        NetworkSandboxPolicy::Restricted,
    );
    environment.config_mut().permission_profile = PermissionProfileSnapshot::legacy(restricted);
    let permissions = TerminalPermissions::for_launch(
        &environment,
        TerminalSandboxSource::Native,
        SandboxPermissions::UseDefault,
        /*additional_permissions*/ None,
    );

    let enabled = PermissionProfile::from_runtime_permissions(
        &file_system_policy,
        NetworkSandboxPolicy::Enabled,
    );
    environment.config_mut().permission_profile = PermissionProfileSnapshot::legacy(enabled);
    let current = TerminalPolicy::capture(
        &environment,
        TerminalSandboxSource::Native,
        /*additional_permissions*/ None,
    );
    assert_eq!(
        permissions.review_requirement(&current, environment.permission_profile()),
        Err("this terminal cannot enforce the current network restrictions; start a new terminal")
    );
    Ok(())
}

#[tokio::test]
async fn enabling_windows_sandbox_respects_the_launch_backend() -> anyhow::Result<()> {
    for (source, expected) in [
        (
            TerminalSandboxSource::Native,
            SandboxPermissions::RequireEscalated,
        ),
        (
            TerminalSandboxSource::Executor,
            SandboxPermissions::UseDefault,
        ),
    ] {
        let (_session, turn) = make_session_and_context().await;
        let mut environment = turn.environments.primary().expect("environment").clone();
        environment.selection.cwd = PathUri::parse("file:///C:/workspace")?;
        environment.config_mut().permission_profile =
            PermissionProfileSnapshot::legacy(PermissionProfile::read_only());
        environment.config_mut().windows_sandbox_level = WindowsSandboxLevel::Disabled;
        let permissions = TerminalPermissions::for_launch(
            &environment,
            &turn,
            source,
            SandboxPermissions::UseDefault,
            /*additional_permissions*/ None,
        );
        let current = TerminalPolicy::capture(
            &environment,
            &turn,
            source,
            /*additional_permissions*/ None,
        );
        assert_eq!(
            permissions.review_requirement(&current, environment.permission_profile()),
            Ok(SandboxPermissions::UseDefault)
        );
        environment.config_mut().windows_sandbox_level = WindowsSandboxLevel::RestrictedToken;
        let current = TerminalPolicy::capture(
            &environment,
            &turn,
            source,
            /*additional_permissions*/ None,
        );
        assert_eq!(
            permissions.review_requirement(&current, environment.permission_profile()),
            Ok(expected)
        );
    }
    Ok(())
}
