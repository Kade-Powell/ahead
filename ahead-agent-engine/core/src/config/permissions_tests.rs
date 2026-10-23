use super::*;
use crate::config::Config;
use crate::config::ConfigOverrides;
use codex_config::config_toml::ConfigToml;
use codex_config::permissions_toml::FilesystemPermissionToml;
use codex_config::permissions_toml::FilesystemPermissionsToml;
use codex_config::permissions_toml::NetworkToml;
use codex_config::permissions_toml::PermissionProfileToml;
use codex_config::permissions_toml::PermissionsToml;
use codex_config::permissions_toml::WorkspaceRootsToml;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::FileSystemSpecialPath;
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use tempfile::TempDir;

#[test]
fn normalize_absolute_path_for_platform_simplifies_windows_verbatim_paths() {
    let parsed = normalize_absolute_path_for_platform(
        r"\\?\D:\c\x\worktrees\2508\swift-base",
        /*is_windows*/ true,
    );
    assert_eq!(parsed, PathBuf::from(r"D:\c\x\worktrees\2508\swift-base"));
}

#[test]
fn windows_verbatim_path_prefix_does_not_count_as_glob_syntax() {
    assert!(!contains_glob_chars_for_platform(
        r"\\?\D:\c\x\worktrees\2508\swift-base",
        /*is_windows*/ true,
    ));
    assert!(contains_glob_chars_for_platform(
        r"\\?\D:\c\x\worktrees\2508\**\*.env",
        /*is_windows*/ true,
    ));
}

#[test]
fn network_toml_accepts_only_the_binary_network_policy() {
    assert_eq!(
        toml::from_str::<NetworkToml>("enabled = true").expect("valid network policy"),
        NetworkToml {
            enabled: Some(true)
        }
    );
    assert!(
        toml::from_str::<NetworkToml>("allowed_domains = [\"openai.com\"]").is_err(),
        "AHEAD does not accept proxy allowlists"
    );
}

#[test]
fn profile_network_policy_selects_restricted_or_enabled_sandboxing() {
    assert_eq!(
        compile_network_sandbox_policy(
            Some(&NetworkToml {
                enabled: Some(true)
            }),
            NetworkSandboxPolicy::Restricted,
        ),
        NetworkSandboxPolicy::Enabled
    );
    assert_eq!(
        compile_network_sandbox_policy(
            Some(&NetworkToml {
                enabled: Some(false)
            }),
            NetworkSandboxPolicy::Enabled,
        ),
        NetworkSandboxPolicy::Restricted
    );
}

#[test]
fn permissions_profiles_resolve_extends_parent_first_with_child_overrides() {
    let permissions = toml::from_str::<PermissionsToml>(
        r#"
[base]
description = "Base profile"

[base.filesystem]
glob_scan_max_depth = 1
"/tmp/base" = "read"
"/tmp/shared" = "read"

[base.filesystem.":project_roots"]
"**/*.env" = "deny"
docs = "read"

[base.network]
enabled = true

[base.network.domains]
"base.example.com" = "allow"
"SHARED.EXAMPLE.COM." = "deny"

[base.network.unix_sockets]
"/tmp/base.sock" = "allow"
"/tmp/blocked.sock" = "deny"

[child]
extends = "base"

[child.filesystem]
glob_scan_max_depth = 3
"/tmp/shared" = "write"

[child.filesystem.":project_roots"]
docs = "write"
src = "read"

[child.network]
enabled = false
allow_local_binding = true

[child.network.domains]
"child.example.com" = "allow"
"shared.example.com" = "allow"

[child.network.unix_sockets]
"/tmp/child.sock" = "allow"
"#,
    )
    .expect("permissions should deserialize");

    let resolved = permissions
        .resolve_profile("child", |_| None)
        .expect("child profile should resolve");
    let expected_profile = toml::from_str::<PermissionProfileToml>(
        r#"
extends = "base"

[filesystem]
glob_scan_max_depth = 3
"/tmp/base" = "read"
"/tmp/shared" = "write"

[filesystem.":project_roots"]
"**/*.env" = "deny"
docs = "write"
src = "read"

[network]
enabled = false
allow_local_binding = true

[network.domains]
"base.example.com" = "allow"
"child.example.com" = "allow"
"shared.example.com" = "allow"

[network.unix_sockets]
"/tmp/base.sock" = "allow"
"/tmp/blocked.sock" = "deny"
"/tmp/child.sock" = "allow"
"#,
    )
    .expect("expected profile should deserialize");

    assert_eq!(resolved, expected_profile);
}

#[test]
fn permissions_profiles_reject_undefined_extends_parent() {
    let permissions = toml::from_str::<PermissionsToml>(
        r#"
[child]
extends = "base"
"#,
    )
    .expect("permissions should deserialize");

    let err = permissions
        .resolve_profile("child", |_| None)
        .expect_err("missing parent should be rejected");

    assert_eq!(
        err.to_string(),
        "permissions profile `child` extends undefined profile `base`"
    );
}

#[test]
fn permissions_profiles_reject_unsupported_builtin_extends_parent() {
    let permissions = toml::from_str::<PermissionsToml>(
        r#"
[child]
extends = ":danger-full-access"
"#,
    )
    .expect("permissions should deserialize");

    let err = permissions
        .resolve_profile("child", |_| None)
        .expect_err("unsupported built-in parent should be rejected");

    assert_eq!(
        err.to_string(),
        "permissions profile `child` cannot extend unsupported built-in profile `:danger-full-access`"
    );
}

#[test]
fn permissions_profiles_reject_extends_cycles() {
    let permissions = toml::from_str::<PermissionsToml>(
        r#"
[alpha]
extends = "beta"

[beta]
extends = "alpha"
"#,
    )
    .expect("permissions should deserialize");

    let err = permissions
        .resolve_profile("alpha", |_| None)
        .expect_err("cycle should be rejected");

    assert_eq!(
        err.to_string(),
        "permissions profile inheritance cycle detected: alpha -> beta -> alpha"
    );
}

#[test]
fn compile_permission_profile_workspace_roots_resolves_enabled_entries() -> std::io::Result<()> {
    let cwd = TempDir::new()?;
    let workspace_roots = compile_permission_profile_workspace_roots(
        Some(&PermissionsToml {
            entries: BTreeMap::from([(
                "workspace".to_string(),
                PermissionProfileToml {
                    description: None,
                    extends: None,
                    workspace_roots: Some(WorkspaceRootsToml {
                        entries: BTreeMap::from([
                            ("backend".to_string(), true),
                            ("disabled".to_string(), false),
                        ]),
                    }),
                    filesystem: None,
                    network: None,
                },
            )]),
        }),
        "workspace",
        cwd.path(),
    )?;

    assert_eq!(
        workspace_roots,
        vec![AbsolutePathBuf::resolve_path_against_base(
            "backend",
            cwd.path()
        )]
    );
    Ok(())
}

#[test]
fn legacy_project_roots_restrictions_do_not_fail_open() -> std::io::Result<()> {
    let permissions = toml::from_str::<PermissionsToml>(
        r#"
[read_deny.filesystem]
":root" = "read"
":project_roots" = "none"

[write_deny.filesystem]
":root" = "write"
":project_roots" = "none"

[write_read.filesystem]
":root" = "write"

[write_read.filesystem.":project_roots"]
docs = "read"
"#,
    )
    .expect("legacy project roots profiles should deserialize");
    let cwd = TempDir::new()?;
    let docs = cwd.path().join("docs");
    let mut startup_warnings = Vec::new();

    let (read_deny_policy, _) =
        compile_permission_profile(&permissions, "read_deny", &mut startup_warnings)?;
    assert_eq!(
        read_deny_policy.resolve_access_with_cwd(cwd.path(), cwd.path()),
        FileSystemAccessMode::Deny
    );

    let (write_deny_policy, _) =
        compile_permission_profile(&permissions, "write_deny", &mut startup_warnings)?;
    assert!(!write_deny_policy.has_full_disk_write_access());
    assert_eq!(
        write_deny_policy.resolve_access_with_cwd(cwd.path(), cwd.path()),
        FileSystemAccessMode::Deny
    );

    let (write_read_policy, _) =
        compile_permission_profile(&permissions, "write_read", &mut startup_warnings)?;
    assert!(!write_read_policy.has_full_disk_write_access());
    assert_eq!(
        write_read_policy.resolve_access_with_cwd(&docs, cwd.path()),
        FileSystemAccessMode::Read
    );

    Ok(())
}

#[test]
fn read_write_glob_warnings_skip_supported_deny_read_globs_and_trailing_subpaths() {
    let filesystem = FilesystemPermissionsToml {
        glob_scan_max_depth: None,
        entries: BTreeMap::from([
            (
                "/tmp/**/*.log".to_string(),
                FilesystemPermissionToml::Access(FileSystemAccessMode::Read),
            ),
            (
                "/tmp/cache/**".to_string(),
                FilesystemPermissionToml::Access(FileSystemAccessMode::Write),
            ),
            (
                ":workspace_roots".to_string(),
                FilesystemPermissionToml::Scoped(BTreeMap::from([
                    ("**/*.env".to_string(), FileSystemAccessMode::Deny),
                    ("docs/**".to_string(), FileSystemAccessMode::Read),
                    ("src/**/*.rs".to_string(), FileSystemAccessMode::Write),
                ])),
            ),
        ]),
    };

    assert_eq!(
        unsupported_read_write_glob_paths(&filesystem),
        vec![
            "/tmp/**/*.log".to_string(),
            ":workspace_roots/src/**/*.rs".to_string()
        ],
        "`deny` glob patterns are supported as deny-read rules; only `read`/`write` globs should warn"
    );
}

#[test]
fn unreadable_globstar_warning_is_suppressed_when_scan_depth_is_configured() {
    let filesystem = FilesystemPermissionsToml {
        glob_scan_max_depth: None,
        entries: BTreeMap::from([(
            ":workspace_roots".to_string(),
            FilesystemPermissionToml::Scoped(BTreeMap::from([
                ("**/*.env".to_string(), FileSystemAccessMode::Deny),
                ("*.pem".to_string(), FileSystemAccessMode::Deny),
            ])),
        )]),
    };

    assert_eq!(
        unbounded_unreadable_globstar_paths(&filesystem),
        vec![":workspace_roots/**/*.env".to_string()]
    );

    let configured_filesystem = FilesystemPermissionsToml {
        glob_scan_max_depth: Some(2),
        ..filesystem
    };
    assert_eq!(
        unbounded_unreadable_globstar_paths(&configured_filesystem),
        Vec::<String>::new()
    );
}

#[test]
fn glob_scan_max_depth_must_be_positive() {
    let err = validate_glob_scan_max_depth(Some(0))
        .expect_err("zero depth would silently skip deny-read glob expansion");

    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(err.to_string(), "glob_scan_max_depth must be at least 1");
    assert_eq!(
        validate_glob_scan_max_depth(Some(2)).expect("depth should be valid"),
        Some(2)
    );
}

#[test]
fn read_write_trailing_glob_suffix_compiles_as_subpath() -> std::io::Result<()> {
    let mut startup_warnings = Vec::new();
    let (file_system_policy, _) = compile_permission_profile(
        &PermissionsToml {
            entries: BTreeMap::from([(
                "workspace".to_string(),
                PermissionProfileToml {
                    description: None,
                    extends: None,
                    workspace_roots: None,
                    filesystem: Some(FilesystemPermissionsToml {
                        glob_scan_max_depth: None,
                        entries: BTreeMap::from([(
                            ":workspace_roots".to_string(),
                            FilesystemPermissionToml::Scoped(BTreeMap::from([(
                                "docs/**".to_string(),
                                FileSystemAccessMode::Read,
                            )])),
                        )]),
                    }),
                    network: None,
                },
            )]),
        },
        "workspace",
        &mut startup_warnings,
    )?;

    assert_eq!(
        file_system_policy,
        FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
            path: FileSystemPath::Special {
                value: FileSystemSpecialPath::project_roots(Some("docs".into())),
            },
            access: FileSystemAccessMode::Read,
            missing_path_behavior: None,
        }]),
        "trailing /** should compile as a subtree path instead of a glob pattern"
    );
    Ok(())
}

#[test]
fn read_write_glob_patterns_still_reject_non_subpath_globs() {
    let err = compile_read_write_glob_path("src/**/*.rs", FileSystemAccessMode::Read)
        .expect_err("non-subpath read/write glob should be rejected");

    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        err.to_string()
            .contains("filesystem glob path `src/**/*.rs` only supports `deny` access"),
        "{err}"
    );
}
