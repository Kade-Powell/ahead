use std::{
    fs,
    io::{self, ErrorKind, Read},
    path::{Path, PathBuf},
};

#[cfg(any(unix, windows))]
use std::ffi::OsStr;

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// Creates the shareable workspace config on first use without replacing an
/// existing file or following project-owned `.ahead` symlinks.
pub fn ensure_workspace_config(root: &Path) -> io::Result<PathBuf> {
    let root = root.canonicalize()?;
    let ahead = root.join(".ahead");
    match fs::create_dir(&ahead) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }

    #[cfg(any(unix, windows))]
    {
        let workspace = crate::secure_fs::open_canonical_directory(&root)?;
        let ahead_directory = crate::secure_fs::open_relative_directory(
            &workspace,
            OsStr::new(".ahead"),
        )?;
        crate::secure_fs::open_or_create_relative_lock_file(
            &ahead_directory,
            OsStr::new("config.toml"),
        )?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        return Err(io::Error::new(
            ErrorKind::Unsupported,
            "secure workspace config creation is unsupported on this platform",
        ));
    }

    Ok(ahead.join("config.toml"))
}

/// Reads one AHEAD provider-settings layer without following a project-owned
/// `.ahead` directory or config-file symlink.
pub fn read_ahead_config(root: &Path, filename: &str) -> io::Result<Option<String>> {
    if !matches!(
        filename,
        "settings.toml" | "config.toml" | "config.local.toml" | "team.toml"
    ) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "unsupported AHEAD config filename",
        ));
    }
    match std::fs::symlink_metadata(root) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let root = root.canonicalize()?;
    let path = root.join(".ahead").join(filename);
    #[cfg(unix)]
    let mut file = match crate::secure_fs::open_canonical_regular_file(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    #[cfg(not(unix))]
    let mut file = {
        let ahead = root.join(".ahead");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !metadata.file_type().is_file()
            || !fs::symlink_metadata(&ahead)?.file_type().is_dir()
            || !ahead.canonicalize()?.starts_with(&root)
        {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "AHEAD config directory and file must not be symlinks",
            ));
        }
        fs::File::open(&path)?
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "AHEAD config must be a regular file of at most 1 MiB",
        ));
    }
    let mut content = String::new();
    (&mut file)
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut content)?;
    if content.len() as u64 > MAX_CONFIG_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "AHEAD config must be a regular file of at most 1 MiB",
        ));
    }
    if filename == "config.toml" {
        let table = content.parse::<toml::Table>().map_err(|_| {
            io::Error::new(
                ErrorKind::InvalidData,
                "invalid TOML in .ahead/config.toml",
            )
        })?;
        validate_tracked_config(&toml::Value::Table(table))?;
    } else if filename == "team.toml" {
        let table = content.parse::<toml::Table>().map_err(|_| {
            io::Error::new(
                ErrorKind::InvalidData,
                "invalid TOML in .ahead/team.toml",
            )
        })?;
        if has_secret_field(&toml::Value::Table(table)) {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "tracked .ahead/team.toml contains a credential field",
            ));
        }
    }
    Ok(Some(content))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanguageEditorSettings {
    pub tab_size: u32,
    pub hard_tabs: bool,
    pub preferred_line_length: u32,
}

impl Default for LanguageEditorSettings {
    fn default() -> Self {
        Self {
            tab_size: 4,
            hard_tabs: false,
            preferred_line_length: 80,
        }
    }
}

/// Resolves language indentation settings from the same layered AHEAD config
/// used by language extensions.
pub fn language_editor_settings(
    workspace_root: &Path,
    user_home: Option<&Path>,
    language_name: Option<&str>,
) -> io::Result<LanguageEditorSettings> {
    let mut settings = LanguageEditorSettings::default();
    let Some(language_name) = language_name else {
        return Ok(settings);
    };

    let mut layers = Vec::with_capacity(4);
    if let Some(user_home) = user_home {
        layers.push((user_home, "settings.toml", "~/.ahead/settings.toml"));
    }
    layers.extend([
        (workspace_root, "config.toml", ".ahead/config.toml"),
        (
            workspace_root,
            "config.local.toml",
            ".ahead/config.local.toml",
        ),
        (workspace_root, "settings.toml", ".ahead/settings.toml"),
    ]);

    for (root, filename, source) in layers {
        let content = read_ahead_config(root, filename).map_err(|error| {
            io::Error::new(error.kind(), format!("reading {source}: {error}"))
        })?;
        let Some(content) = content else {
            continue;
        };
        let config: toml::Table =
            content.parse().map_err(|error: toml::de::Error| {
                io::Error::new(
                    ErrorKind::InvalidData,
                    format!("parsing {source}: {}", error.message()),
                )
            })?;
        let Some(languages) = config.get("languages") else {
            continue;
        };
        let languages = languages.as_table().ok_or_else(|| {
            io::Error::new(
                ErrorKind::InvalidData,
                format!("{source} [languages] must be a table"),
            )
        })?;
        let mut matching_languages = languages
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case(language_name));
        let Some((configured_name, configured)) = matching_languages.next() else {
            continue;
        };
        if matching_languages.next().is_some() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "{source} defines language `{language_name}` more than once"
                ),
            ));
        }
        let configured = configured.as_table().ok_or_else(|| {
            io::Error::new(
                ErrorKind::InvalidData,
                format!("{source} [languages.{configured_name}] must be a table"),
            )
        })?;

        for (key, value) in configured {
            match key.as_str() {
                "tab_size" => {
                    let Some(size) = value
                        .as_integer()
                        .filter(|size| (1..=i64::from(u32::MAX)).contains(size))
                    else {
                        return Err(io::Error::new(
                            ErrorKind::InvalidData,
                            format!(
                                "{source} [languages.{configured_name}].{key} has an invalid value"
                            ),
                        ));
                    };
                    settings.tab_size = size as u32;
                }
                "hard_tabs" => {
                    let Some(hard_tabs) = value.as_bool() else {
                        return Err(io::Error::new(
                            ErrorKind::InvalidData,
                            format!(
                                "{source} [languages.{configured_name}].{key} has an invalid value"
                            ),
                        ));
                    };
                    settings.hard_tabs = hard_tabs;
                }
                "preferred_line_length" => {
                    let Some(length) = value
                        .as_integer()
                        .filter(|length| (0..=i64::from(u32::MAX)).contains(length))
                    else {
                        return Err(io::Error::new(
                            ErrorKind::InvalidData,
                            format!(
                                "{source} [languages.{configured_name}].{key} has an invalid value"
                            ),
                        ));
                    };
                    settings.preferred_line_length = length as u32;
                }
                // These values route extensions and are not editor settings.
                "language_servers" | "requests" => {}
                _ => {}
            }
        }
    }

    Ok(settings)
}

pub fn validate_tracked_config(value: &toml::Value) -> io::Result<()> {
    if has_secret_field(value) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "tracked .ahead/config.toml contains a credential field; use ignored settings.toml or config.local.toml",
        ));
    }
    Ok(())
}

fn has_secret_field(value: &toml::Value) -> bool {
    match value {
        toml::Value::Table(table) => table.iter().any(|(key, value)| {
            credential_key(key, value.is_str()) || has_secret_field(value)
        }),
        toml::Value::Array(values) => values.iter().any(has_secret_field),
        toml::Value::String(text) => url::Url::parse(text).ok().is_some_and(|url| {
            !url.username().is_empty()
                || url.password().is_some()
                || url.query_pairs().any(|(key, _)| credential_key(&key, true))
        }),
        _ => false,
    }
}

fn credential_key(key: &str, string_value: bool) -> bool {
    let key = key.replace('-', "_").to_ascii_lowercase();
    matches!(
        key.as_str(),
        "api_key"
            | "apikey"
            | "token"
            | "password"
            | "private_key"
            | "secret_key"
            | "client_secret"
            | "authorization"
            | "bearer_token"
            | "auth_token"
            | "access_token"
            | "refresh_token"
            | "authtoken"
            | "accesstoken"
            | "bearertoken"
            | "clientsecret"
    ) || (string_value
        && (key.ends_with("_token")
            || key.ends_with("_secret")
            || key.ends_with("_password")))
}

#[cfg(test)]
mod tests {
    use super::{
        LanguageEditorSettings, ensure_workspace_config, language_editor_settings,
        read_ahead_config,
    };

    #[test]
    fn workspace_config_is_created_once_and_existing_content_is_preserved() {
        let workspace = tempfile::tempdir().expect("workspace");
        let config = ensure_workspace_config(workspace.path())
            .expect("create shareable config");
        assert_eq!(
            config,
            workspace
                .path()
                .canonicalize()
                .expect("canonical workspace")
                .join(".ahead/config.toml")
        );
        assert_eq!(
            read_ahead_config(workspace.path(), "config.toml")
                .expect("read new config")
                .as_deref(),
            Some("")
        );

        std::fs::write(&config, "[languages.Rust]\ntab_size = 2\n")
            .expect("edit config");
        ensure_workspace_config(workspace.path()).expect("open existing config");
        assert_eq!(
            std::fs::read_to_string(config).expect("preserved config"),
            "[languages.Rust]\ntab_size = 2\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn workspace_config_creation_rejects_symlinked_directory_and_file() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        symlink(outside.path(), workspace.path().join(".ahead"))
            .expect("symlinked config directory");
        assert!(ensure_workspace_config(workspace.path()).is_err());
        assert!(!outside.path().join("config.toml").exists());

        std::fs::remove_file(workspace.path().join(".ahead"))
            .expect("remove directory link");
        std::fs::create_dir(workspace.path().join(".ahead"))
            .expect("real config directory");
        let target = outside.path().join("config.toml");
        std::fs::write(&target, "[languages.Rust]\n").expect("outside config");
        symlink(&target, workspace.path().join(".ahead/config.toml"))
            .expect("symlinked config file");
        assert!(ensure_workspace_config(workspace.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(target).expect("outside file preserved"),
            "[languages.Rust]\n"
        );
    }

    #[test]
    fn language_editor_settings_merge_case_insensitive_layers() {
        let workspace = tempfile::tempdir().expect("workspace");
        let user_home = tempfile::tempdir().expect("user home");
        std::fs::create_dir(user_home.path().join(".ahead"))
            .expect("user settings directory");
        std::fs::write(
            user_home.path().join(".ahead/settings.toml"),
            "[languages.Python]\ntab_size = 2\nhard_tabs = true\n",
        )
        .expect("user settings");
        let ahead = workspace.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("workspace config directory");
        std::fs::write(
            ahead.join("config.toml"),
            "[languages.python]\npreferred_line_length = 100\n",
        )
        .expect("tracked config");
        std::fs::write(
            ahead.join("settings.toml"),
            "[languages.PYTHON]\ntab_size = 8\nhard_tabs = false\n",
        )
        .expect("workspace settings");

        assert_eq!(
            language_editor_settings(
                workspace.path(),
                Some(user_home.path()),
                Some("Python"),
            )
            .expect("resolved settings"),
            LanguageEditorSettings {
                tab_size: 8,
                hard_tabs: false,
                preferred_line_length: 100,
            }
        );
        assert_eq!(
            language_editor_settings(workspace.path(), None, None)
                .expect("defaults"),
            LanguageEditorSettings::default()
        );
    }

    #[test]
    fn language_editor_settings_reject_invalid_and_duplicate_values() {
        let workspace = tempfile::tempdir().expect("workspace");
        let ahead = workspace.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("workspace config directory");
        std::fs::write(
            ahead.join("config.toml"),
            "[languages.Rust]\ntab_size = 0\n",
        )
        .expect("invalid config");
        let error = language_editor_settings(workspace.path(), None, Some("Rust"))
            .expect_err("zero tab size is invalid");
        assert!(error.to_string().contains("tab_size has an invalid value"));

        std::fs::write(
            ahead.join("config.toml"),
            "[languages.Rust]\ntab_size = 2\n[languages.rust]\nhard_tabs = true\n",
        )
        .expect("duplicate config");
        let error = language_editor_settings(workspace.path(), None, Some("Rust"))
            .expect_err("case-insensitive duplicate names are invalid");
        assert!(
            error
                .to_string()
                .contains("defines language `Rust` more than once")
        );
    }

    #[test]
    fn reads_regular_config_and_rejects_links_and_large_files() {
        let root = tempfile::tempdir().expect("workspace");
        let ahead = root.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("settings directory");
        std::fs::write(ahead.join("settings.toml"), "[ai]\nmodel = 'safe'\n")
            .expect("write settings");
        assert_eq!(
            read_ahead_config(root.path(), "settings.toml")
                .expect("read settings")
                .as_deref(),
            Some("[ai]\nmodel = 'safe'\n")
        );
        assert!(read_ahead_config(root.path(), "../outside.toml").is_err());
        std::fs::write(ahead.join("config.toml"), "x".repeat(1024 * 1024 + 1))
            .expect("write oversized config");
        assert!(read_ahead_config(root.path(), "config.toml").is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let outside = root.path().join("outside.toml");
            std::fs::write(&outside, "[ai]\nmodel = 'outside'\n")
                .expect("write outside config");
            std::fs::remove_file(ahead.join("settings.toml"))
                .expect("remove settings");
            symlink(&outside, ahead.join("settings.toml")).expect("link settings");
            assert!(read_ahead_config(root.path(), "settings.toml").is_err());
            std::fs::remove_file(ahead.join("settings.toml"))
                .expect("remove settings link");
            std::fs::remove_file(ahead.join("config.toml"))
                .expect("remove oversized config");
            std::fs::remove_dir(&ahead).expect("remove settings directory");
            std::fs::write(root.path().join("config.toml"), "[ai]\n")
                .expect("write outside config");
            symlink(root.path(), &ahead).expect("link settings directory");
            assert!(read_ahead_config(root.path(), "config.toml").is_err());
        }
    }

    #[test]
    fn missing_optional_config_root_is_absent() {
        let workspace = tempfile::tempdir().expect("workspace");
        let missing_user_home = workspace.path().join("user-home");

        assert_eq!(
            language_editor_settings(
                workspace.path(),
                Some(&missing_user_home),
                Some("Rust"),
            )
            .expect("missing user config is optional"),
            LanguageEditorSettings::default()
        );
        assert_eq!(
            read_ahead_config(&missing_user_home, "settings.toml")
                .expect("missing root is optional"),
            None
        );
    }

    #[test]
    fn tracked_config_rejects_nested_credentials_but_private_settings_allow_them() {
        let root = tempfile::tempdir().expect("workspace");
        let ahead = root.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("settings directory");
        std::fs::write(
            ahead.join("config.toml"),
            "[ai]\nmax_tokens = 2048\n[[ai.connections]]\nname = 'Shared'\nauth_token = 'private'\n",
        )
        .expect("write tracked config");
        assert!(read_ahead_config(root.path(), "config.toml").is_err());

        for address in [
            "https://name:password@example.invalid/v1",
            "https://example.invalid/v1?api_key=private",
        ] {
            std::fs::write(
                ahead.join("config.toml"),
                format!("[ai]\nbase_url = '{address}'\n"),
            )
            .expect("write credential URL");
            assert!(read_ahead_config(root.path(), "config.toml").is_err());
        }

        std::fs::write(
            ahead.join("config.toml"),
            "[ai]\nmax_tokens = 2048\nsemantic_token = true\nbase_url = 'https://example.invalid/v1?max_tokens=2048'\n[[ai.connections]]\nname = 'Shared'\n",
        )
        .expect("write shareable config");
        assert!(
            read_ahead_config(root.path(), "config.toml")
                .expect("read shareable config")
                .is_some()
        );
        std::fs::write(ahead.join("settings.toml"), "[ai]\napi_key = 'private'\n")
            .expect("write private settings");
        assert!(
            read_ahead_config(root.path(), "settings.toml")
                .expect("read private settings")
                .is_some()
        );
    }

    #[test]
    fn team_manifest_is_readable_without_exposing_credentials() {
        let root = tempfile::tempdir().expect("workspace");
        let ahead = root.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("team directory");
        std::fs::write(ahead.join("team.toml"), "members = []\n")
            .expect("write team manifest");
        assert_eq!(
            read_ahead_config(root.path(), "team.toml")
                .expect("read team manifest")
                .as_deref(),
            Some("members = []\n")
        );
        std::fs::write(ahead.join("team.toml"), "token = 'private'\n")
            .expect("write credential");
        assert!(read_ahead_config(root.path(), "team.toml").is_err());
    }
}
