use super::*;
use codex_config::types::WindowsToml;
use pretty_assertions::assert_eq;

#[test]
fn resolve_windows_sandbox_mode_uses_current_windows_setting() {
    let cfg = ConfigToml {
        windows: Some(WindowsToml {
            sandbox: Some(WindowsSandboxModeToml::Unelevated),
            ..Default::default()
        }),
        ..Default::default()
    };

    assert_eq!(
        resolve_windows_sandbox_mode(&cfg),
        Some(WindowsSandboxModeToml::Unelevated)
    );
}

#[test]
fn resolve_windows_sandbox_mode_defaults_to_disabled() {
    assert_eq!(resolve_windows_sandbox_mode(&ConfigToml::default()), None);
}

#[test]
fn resolve_windows_sandbox_private_desktop_defaults_to_true() {
    assert!(resolve_windows_sandbox_private_desktop(
        &ConfigToml::default()
    ));
}

#[test]
fn resolve_windows_sandbox_private_desktop_respects_explicit_cfg_value() {
    let cfg = ConfigToml {
        windows: Some(WindowsToml {
            sandbox_private_desktop: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    };

    assert!(!resolve_windows_sandbox_private_desktop(&cfg));
}
