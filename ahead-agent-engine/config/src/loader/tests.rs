use super::*;
use crate::{CONFIG_TOML_FILE, ConfigRequirementsToml};
use codex_file_system::CopyOptions;
use codex_file_system::CreateDirectoryOptions;
use codex_file_system::ExecutorFileSystemFuture;
use codex_file_system::FileMetadata;
use codex_file_system::FileSystemReadStream;
use codex_file_system::FileSystemSandboxContext;
use codex_file_system::GetMetadataOptions;
use codex_file_system::ReadDirectoryEntry;
use codex_file_system::ReadFileOptions;
use codex_file_system::RemoveOptions;
use codex_file_system::WalkOptions;
use codex_file_system::WalkOutcome;
use codex_file_system::WriteFileOptions;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

pub(super) struct TestFileSystem;

#[tokio::test]
async fn system_browser_import_denial_survives_user_and_session_config() {
    let tmp = tempdir().expect("tempdir");
    let allow = "[in_app_browser]\nallow_external_browser_settings_import = true";
    let deny = "[in_app_browser]\nallow_external_browser_settings_import = false";
    let requirements_path = tmp.path().join("requirements.toml");
    std::fs::write(&requirements_path, deny).expect("write system requirements");
    std::fs::write(tmp.path().join(CONFIG_TOML_FILE), allow).expect("write user config");
    let mut loader_overrides = LoaderOverrides::isolated_for_tests();
    loader_overrides.system_requirements_path = Some(requirements_path);

    let stack = load_config_layers_state(
        &TestFileSystem,
        tmp.path(),
        /*cwd*/ None,
        &[(
            "in_app_browser.allow_external_browser_settings_import".to_string(),
            TomlValue::Boolean(true),
        )],
        ConfigLoadOptions {
            loader_overrides,
            strict_config: false,
        },
    )
    .await
    .expect("load managed browser import requirement");

    assert_eq!(
        stack.requirements_toml(),
        &toml::from_str::<ConfigRequirementsToml>(deny).expect("expected requirement"),
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn managed_preferences_requirements_are_resolved_and_override_system() {
    use base64::Engine;
    use base64::prelude::BASE64_STANDARD;

    let temp = tempdir().expect("tempdir");
    let runtime_home = temp.path().join("ahead-home");
    std::fs::create_dir_all(&runtime_home).expect("create runtime home");
    let system_requirements_path = runtime_home.join("requirements.toml");
    std::fs::write(
        &system_requirements_path,
        "allowed_approval_policies = [\"on-request\"]\n",
    )
    .expect("write system requirements");
    let mut overrides = LoaderOverrides::isolated_for_tests();
    overrides.system_requirements_path = Some(system_requirements_path);
    overrides.ignore_user_config = true;
    overrides.managed_preferences_requirements_base64 = Some(
        BASE64_STANDARD.encode(
            r#"
allowed_approval_policies = ["never"]
allowed_sandbox_modes = ["read-only"]
log_dir = "~/.ahead/logs"
model_catalog_json = "models.json"
"#
            .as_bytes(),
        ),
    );

    let state = load_config_layers_state(
        &TestFileSystem,
        &runtime_home,
        /*cwd*/ None,
        &[],
        overrides,
    )
    .await
    .expect("load managed requirements");

    assert_eq!(
        state.requirements().approval_policy.value(),
        codex_protocol::protocol::AskForApproval::Never
    );
    assert_eq!(
        state.requirements().permission_profile.get(),
        &codex_protocol::models::PermissionProfile::read_only()
    );
    assert!(
        state
            .requirements()
            .approval_policy
            .can_set(&codex_protocol::protocol::AskForApproval::OnRequest)
            .is_err()
    );
    assert!(
        state
            .requirements()
            .permission_profile
            .can_set(&codex_protocol::models::PermissionProfile::workspace_write())
            .is_err()
    );
    let expected_log_dir =
        AbsolutePathBuf::resolve_path_against_base("~/.ahead/logs", &runtime_home);
    let requirements = state.requirements_toml();
    assert_eq!(
        requirements.log_dir.as_deref(),
        Some(expected_log_dir.as_path())
    );
    assert_eq!(
        requirements.model_catalog_json.as_deref(),
        Some(runtime_home.join("models.json").as_path())
    );
}

impl ExecutorFileSystem for TestFileSystem {
    fn canonicalize<'a>(
        &'a self,
        path: &'a PathUri,
        _sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, PathUri> {
        Box::pin(async move {
            let path = path.to_abs_path()?;
            let canonicalized = path.canonicalize()?;
            Ok(PathUri::from_abs_path(&canonicalized))
        })
    }

    fn read_file<'a>(
        &'a self,
        path: &'a PathUri,
        _options: ReadFileOptions,
        _sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let path = path.to_abs_path()?;
            tokio::fs::read(path.as_path()).await
        })
    }

    fn read_file_stream<'a>(
        &'a self,
        _path: &'a PathUri,
        _sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileSystemReadStream> {
        Box::pin(async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "test filesystem does not support streaming reads",
            ))
        })
    }

    fn write_file<'a>(
        &'a self,
        _path: &'a PathUri,
        _contents: Vec<u8>,
        _options: WriteFileOptions,
        _sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(async move { unimplemented!("test filesystem only supports reads") })
    }

    fn create_directory<'a>(
        &'a self,
        _path: &'a PathUri,
        _create_directory_options: CreateDirectoryOptions,
        _sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(async move { unimplemented!("test filesystem only supports reads") })
    }

    fn get_metadata<'a>(
        &'a self,
        path: &'a PathUri,
        _options: GetMetadataOptions,
        _sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileMetadata> {
        Box::pin(async move {
            let path = path.to_abs_path()?;
            let metadata = tokio::fs::symlink_metadata(path.as_path()).await?;
            Ok(FileMetadata {
                is_directory: metadata.is_dir(),
                is_file: metadata.is_file(),
                is_symlink: metadata.file_type().is_symlink(),
                size: metadata.len(),
                created_at_ms: 0,
                modified_at_ms: 0,
            })
        })
    }

    fn read_directory<'a>(
        &'a self,
        _path: &'a PathUri,
        _sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<ReadDirectoryEntry>> {
        Box::pin(async move { unimplemented!("test filesystem only supports reads") })
    }

    fn walk<'a>(
        &'a self,
        _path: &'a PathUri,
        _options: WalkOptions,
        _sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, WalkOutcome> {
        unimplemented!()
    }

    fn remove<'a>(
        &'a self,
        _path: &'a PathUri,
        _remove_options: RemoveOptions,
        _sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(async move { unimplemented!("test filesystem only supports reads") })
    }

    fn copy<'a>(
        &'a self,
        _source_path: &'a PathUri,
        _destination_path: &'a PathUri,
        _copy_options: CopyOptions,
        _sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(async move { unimplemented!("test filesystem only supports reads") })
    }
}

#[tokio::test]
async fn packaged_defaults_have_lower_precedence_than_user_and_session_layers() {
    let tmp = tempdir().expect("tempdir");
    let packaged_defaults_path =
        AbsolutePathBuf::resolve_path_against_base("packaged-defaults.toml", tmp.path());
    let user_config_path = tmp.path().join(CONFIG_TOML_FILE);

    std::fs::write(
        packaged_defaults_path.as_path(),
        r#"
model = "packaged-model"
model_provider = "packaged-provider"
model_context_window = 120000
"#,
    )
    .expect("write packaged defaults");
    std::fs::write(&user_config_path, r#"model = "user-model""#).expect("write user config");

    let mut overrides = LoaderOverrides::isolated_for_tests();
    overrides.packaged_defaults_path = Some(packaged_defaults_path.clone());

    let stack = load_config_layers_state(
        &TestFileSystem,
        tmp.path(),
        /*cwd*/ None,
        &[(
            "model".to_string(),
            TomlValue::String("session-model".to_string()),
        )],
        overrides,
    )
    .await
    .expect("load config layers");

    assert_eq!(
        stack
            .all_layers_low_to_high()
            .map(|layer| layer.name.clone())
            .collect::<Vec<_>>(),
        vec![
            ConfigLayerSource::PackagedDefaults {
                file: packaged_defaults_path,
            },
            ConfigLayerSource::User {
                file: AbsolutePathBuf::from_absolute_path(user_config_path)
                    .expect("absolute user config path"),
            },
            ConfigLayerSource::SessionFlags,
        ]
    );
    assert_eq!(
        stack.effective_config(),
        toml::toml! {
            model = "session-model"
            model_provider = "packaged-provider"
            model_context_window = 120000
        }
        .into()
    );
}

#[tokio::test]
async fn workspace_codex_config_is_not_discovered() {
    let tmp = tempdir().expect("tempdir");
    let workspace_codex_config = tmp.path().join(".codex").join(CONFIG_TOML_FILE);
    tokio::fs::create_dir_all(workspace_codex_config.parent().expect("parent"))
        .await
        .expect("create legacy config directory");
    tokio::fs::write(
        &workspace_codex_config,
        r#"model = "legacy-workspace-model""#,
    )
    .await
    .expect("write legacy config");

    let mut overrides = LoaderOverrides::isolated_for_tests();
    overrides.ignore_managed_requirements = true;
    overrides.ignore_user_config = true;
    let cwd = AbsolutePathBuf::from_absolute_path(tmp.path()).expect("absolute cwd");
    let stack = load_config_layers_state(&TestFileSystem, tmp.path(), Some(cwd), &[], overrides)
        .await
        .expect("load config without project-local Codex config");

    assert_ne!(
        stack.effective_config().get("model"),
        Some(&TomlValue::String("legacy-workspace-model".to_string()))
    );
}

#[tokio::test]
async fn missing_packaged_defaults_file_returns_an_error() {
    let tmp = tempdir().expect("tempdir");
    let packaged_defaults_path =
        AbsolutePathBuf::resolve_path_against_base("packaged-defaults.toml", tmp.path());
    let mut overrides = LoaderOverrides::isolated_for_tests();
    overrides.packaged_defaults_path = Some(packaged_defaults_path.clone());

    let err = load_config_layers_state(
        &TestFileSystem,
        tmp.path(),
        /*cwd*/ None,
        &[],
        overrides,
    )
    .await
    .expect_err("an explicitly configured packaged defaults file must exist");

    assert_eq!(err.kind(), io::ErrorKind::NotFound);
    assert_eq!(
        err.to_string(),
        format!(
            "packaged defaults config file {} not found",
            packaged_defaults_path.display()
        )
    );
}

#[tokio::test]
async fn explicitly_selected_user_config_path_is_loaded() {
    let temp = tempdir().expect("tempdir");
    let user_config_path = temp.path().join("agent.toml");
    std::fs::write(&user_config_path, "model = \"ahead-test-model\"\n")
        .expect("write selected user config");

    let mut overrides = LoaderOverrides::isolated_for_tests();
    overrides.user_config_path = Some(
        AbsolutePathBuf::from_absolute_path(&user_config_path)
            .expect("absolute selected user config path"),
    );
    let stack = load_config_layers_state(
        &TestFileSystem,
        temp.path(),
        /*cwd*/ None,
        &[],
        overrides,
    )
    .await
    .expect("load explicitly selected user config");

    assert_eq!(
        stack
            .effective_config()
            .get("model")
            .and_then(TomlValue::as_str),
        Some("ahead-test-model")
    );
    assert_eq!(
        stack.get_user_config_file().map(AbsolutePathBuf::as_path),
        Some(user_config_path.as_path())
    );
}
