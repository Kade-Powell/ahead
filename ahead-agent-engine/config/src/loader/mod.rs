mod layer_io;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(test)]
mod tests;

use crate::ConfigLayerSource;
use crate::RequirementsLayerEntry;
use crate::compose_requirements;
use crate::config_requirements::RequirementSource;
use crate::config_toml::ConfigToml;
use crate::diagnostics::config_error_from_toml;
use crate::diagnostics::first_layer_config_error_from_entries as typed_first_layer_config_error_from_entries;
use crate::diagnostics::io_error_from_config_error;
use crate::overrides::build_cli_overrides_layer;
use crate::shell_environment_policy::ShellEnvironmentPolicyFilterConfigToml;
use crate::state::ConfigLayerEntry;
use crate::state::ConfigLayerStack;
use crate::state::ConfigLoadOptions;
use crate::state::LoaderOverrides;
use crate::state::validate_enabled_config_layers;
use crate::strict_config::config_error_from_ignored_toml_value_fields;
use crate::strict_config::ignored_toml_value_field;
use crate::strict_config::unknown_feature_toml_value_field;
use codex_file_system::ExecutorFileSystem;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_absolute_path::AbsolutePathBufGuard;
use codex_utils_path_uri::PathUri;
use std::io;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use toml::Value as TomlValue;

#[cfg(windows)]
const DEFAULT_PROGRAM_DATA_DIR_WINDOWS: &str = r"C:\ProgramData";

/// To build up the set of admin-enforced constraints, requirements layers are
/// collected in ascending precedence order, matching config layers, and then
/// composed with config-style TOML merging plus field-specific handling for
/// rules, deny-read permissions, and remote sandbox config:
///
/// - system    `/etc/ahead/requirements.toml` (Unix) or
///   `%ProgramData%\AHEAD\requirements.toml` (Windows)
/// - admin:    managed preferences (*)
///
/// Configuration is built up from multiple layers in the following order:
///
/// - package:  optional default configuration supplied with AHEAD
/// - user      `${runtime_home}/config.toml`
/// - runtime   e.g., --config flags, model selector in UI
///
/// (*) Only available on macOS via managed device profiles.
///
/// When loading the config stack for a thread, there should be a `cwd`
/// associated with it such that `cwd` should be `Some(...)`. Only for
/// test-only layer inspection may pass `cwd` as `None`.
#[allow(clippy::too_many_arguments)]
pub async fn load_config_layers_state(
    fs: &dyn ExecutorFileSystem,
    codex_home: &Path,
    cwd: Option<AbsolutePathBuf>,
    cli_overrides: &[(String, TomlValue)],
    options: impl Into<ConfigLoadOptions>,
) -> io::Result<ConfigLayerStack> {
    let ConfigLoadOptions {
        loader_overrides: overrides,
        strict_config,
    } = options.into();
    let packaged_defaults_layer = if let Some(file) = &overrides.packaged_defaults_path {
        let config = layer_io::read_config_from_path(
            fs,
            file,
            /*log_missing_as_info*/ false,
            strict_config,
        )
        .await?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("packaged defaults config file {} not found", file.display()),
            )
        })?;
        let base_dir = file.as_path().parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "packaged defaults config file {} has no parent directory",
                    file.display()
                ),
            )
        })?;
        ConfigLayerEntry::new(
            ConfigLayerSource::PackagedDefaults { file: file.clone() },
            resolve_relative_paths_in_config_toml(config, base_dir)?,
        )
    } else {
        let file = AbsolutePathBuf::from_absolute_path(std::env::current_exe()?)?;
        let raw_toml = include_str!("../../defaults.toml");
        let config = toml::from_str(raw_toml).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid embedded AHEAD defaults: {error}"),
            )
        })?;
        ConfigLayerEntry::new_with_raw_toml(
            ConfigLayerSource::PackagedDefaults { file },
            config,
            raw_toml.to_owned(),
            AbsolutePathBuf::from_absolute_path(codex_home)?,
        )
    };
    let ignore_managed_requirements = overrides.ignore_managed_requirements;
    let ignore_user_config = overrides.ignore_user_config;
    let ignore_user_exec_policy_rules = overrides.ignore_user_exec_policy_rules;
    let mut requirements_layers = Vec::new();
    let mut system_requirements_layer = None;
    let managed_preferences_requirements_layer;

    if !ignore_managed_requirements {
        #[cfg(target_os = "macos")]
        {
            let managed_preferences_base_dir = AbsolutePathBuf::from_absolute_path(codex_home)?;
            managed_preferences_requirements_layer =
                macos::load_managed_preferences_requirements_layer(
                    overrides.managed_preferences_requirements_base64.as_deref(),
                )
                .await?
                .map(|layer| layer.with_base_dir(managed_preferences_base_dir));
        }
        #[cfg(not(target_os = "macos"))]
        {
            managed_preferences_requirements_layer = None;
        }

        // Honor the system requirements.toml location.
        let requirements_toml_file = system_requirements_toml_file_with_overrides(&overrides)?;
        system_requirements_layer = load_requirements_toml(fs, &requirements_toml_file).await?;
    } else {
        managed_preferences_requirements_layer = None;
    }

    if !ignore_managed_requirements {
        requirements_layers.extend(system_requirements_layer);
        requirements_layers.extend(managed_preferences_requirements_layer);
    }

    let config_requirements_toml = compose_requirements(requirements_layers)?.unwrap_or_default();

    let mut layers = Vec::<ConfigLayerEntry>::new();
    layers.push(packaged_defaults_layer);

    let cli_overrides_layer = if cli_overrides.is_empty() {
        None
    } else {
        let cli_overrides_layer = build_cli_overrides_layer(cli_overrides);
        let base_dir = cwd
            .as_ref()
            .map(AbsolutePathBuf::as_path)
            .unwrap_or(codex_home);
        if strict_config {
            validate_cli_overrides_strictly(&cli_overrides_layer, base_dir)?;
        }
        Some(resolve_relative_paths_in_config_toml(
            cli_overrides_layer,
            base_dir,
        )?)
    };

    let user_config_file = overrides.user_config_path(codex_home)?;
    let user_config_layer =
        load_user_config_layer(fs, &user_config_file, ignore_user_config, strict_config).await?;
    layers.push(user_config_layer);

    // Add a layer for runtime overrides from the CLI or UI, if any exist.
    if let Some(cli_overrides_layer) = cli_overrides_layer {
        layers.push(ConfigLayerEntry::new(
            ConfigLayerSource::SessionFlags,
            cli_overrides_layer,
        ));
    }

    if let Err(err) = validate_enabled_config_layers(&layers) {
        if let Some(config_error) = typed_first_layer_config_error_from_entries::<
            ShellEnvironmentPolicyFilterConfigToml,
        >(&layers)
        .await
        {
            return Err(io_error_from_config_error(
                io::ErrorKind::InvalidData,
                config_error,
                /*source*/ None,
            ));
        }
        return Err(err);
    }

    let config_layer_stack = ConfigLayerStack::new(
        layers,
        config_requirements_toml.clone().try_into()?,
        config_requirements_toml.into_toml(),
    )?
    .with_user_exec_policy_rules_ignored(ignore_user_exec_policy_rules);
    Ok(config_layer_stack)
}

async fn load_user_config_layer(
    fs: &dyn ExecutorFileSystem,
    user_file: &AbsolutePathBuf,
    ignore_user_config: bool,
    strict_config: bool,
) -> io::Result<ConfigLayerEntry> {
    if ignore_user_config {
        return Ok(ConfigLayerEntry::new(
            ConfigLayerSource::User {
                file: user_file.clone(),
            },
            TomlValue::Table(toml::map::Map::new()),
        ));
    }

    load_config_toml_for_required_layer(fs, user_file, strict_config, |config_toml| {
        ConfigLayerEntry::new(
            ConfigLayerSource::User {
                file: user_file.clone(),
            },
            config_toml,
        )
    })
    .await
}

/// Attempts to load a config.toml file from `config_toml`.
/// - If the file exists and is valid TOML, passes the parsed `toml::Value` to
///   `create_entry` and returns the resulting layer entry.
/// - If the file does not exist, uses an empty `Table` with `create_entry` and
///   returns the resulting layer entry.
/// - If there is an error reading the file or parsing the TOML, returns an
///   error.
async fn load_config_toml_for_required_layer(
    fs: &dyn ExecutorFileSystem,
    toml_file: &AbsolutePathBuf,
    strict_config: bool,
    create_entry: impl FnOnce(TomlValue) -> ConfigLayerEntry,
) -> io::Result<ConfigLayerEntry> {
    let loaded = load_config_toml_for_required_layer_raw(fs, toml_file, strict_config).await?;
    let toml_value = resolve_relative_paths_in_config_toml(loaded.toml, loaded.base_dir.as_path())?;

    Ok(create_entry(toml_value))
}

#[derive(Debug, Clone)]
struct LoadedTomlFile {
    toml: TomlValue,
    base_dir: AbsolutePathBuf,
}

async fn load_config_toml_for_required_layer_raw(
    fs: &dyn ExecutorFileSystem,
    toml_file: &AbsolutePathBuf,
    strict_config: bool,
) -> io::Result<LoadedTomlFile> {
    let config_parent = toml_file.as_path().parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Config file {} has no parent directory",
                toml_file.as_path().display()
            ),
        )
    })?;
    let base_dir = AbsolutePathBuf::from_absolute_path(config_parent)?;
    let toml_file_uri = PathUri::from_abs_path(toml_file);
    let toml_value = match fs
        .read_file_text(&toml_file_uri, Default::default(), /*sandbox*/ None)
        .await
    {
        Ok(contents) => {
            let config: TomlValue = toml::from_str(&contents).map_err(|err| {
                let config_error =
                    config_error_from_toml(toml_file.as_path(), &contents, err.clone());
                io_error_from_config_error(io::ErrorKind::InvalidData, config_error, Some(err))
            })?;
            if strict_config {
                validate_config_toml_strictly(
                    toml_file.as_path(),
                    &contents,
                    &config,
                    config_parent,
                )?;
            }
            Ok(config)
        }
        Err(e) => {
            if e.kind() == io::ErrorKind::NotFound {
                Ok(TomlValue::Table(toml::map::Map::new()))
            } else {
                Err(io::Error::new(
                    e.kind(),
                    format!(
                        "Failed to read config file {}: {e}",
                        toml_file.as_path().display()
                    ),
                ))
            }
        }
    }?;

    Ok(LoadedTomlFile {
        toml: toml_value,
        base_dir,
    })
}

fn validate_config_toml_strictly(
    toml_file: &Path,
    contents: &str,
    value: &TomlValue,
    base_dir: &Path,
) -> io::Result<()> {
    let _guard = AbsolutePathBufGuard::new(base_dir);
    if let Some(config_error) = config_error_from_ignored_toml_value_fields::<ConfigToml>(
        toml_file,
        contents,
        value.clone(),
    ) {
        Err(io_error_from_config_error(
            io::ErrorKind::InvalidData,
            config_error,
            /*source*/ None,
        ))
    } else {
        Ok(())
    }
}

fn validate_cli_overrides_strictly(
    cli_overrides_layer: &TomlValue,
    base_dir: &Path,
) -> io::Result<()> {
    let _guard = AbsolutePathBufGuard::new(base_dir);
    if let Some(ignored_path) = ignored_toml_value_field::<ConfigToml>(cli_overrides_layer.clone())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown configuration field `{ignored_path}` in -c/--config override"),
        ));
    }

    if let Some(ignored_path) = unknown_feature_toml_value_field(cli_overrides_layer) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown configuration field `{ignored_path}` in -c/--config override"),
        ));
    }

    Ok(())
}

/// If available, load requirements from the platform's system `requirements.toml`
/// location as a requirements layer.
pub async fn load_requirements_toml(
    fs: &dyn ExecutorFileSystem,
    requirements_toml_file: &AbsolutePathBuf,
) -> io::Result<Option<RequirementsLayerEntry>> {
    let requirements_toml_file_uri = PathUri::from_abs_path(requirements_toml_file);
    match fs
        .read_file_text(
            &requirements_toml_file_uri,
            Default::default(),
            /*sandbox*/ None,
        )
        .await
    {
        Ok(contents) => {
            let requirements_parent = requirements_toml_file.parent().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "Requirements file {} has no parent directory",
                        requirements_toml_file.as_ref().display()
                    ),
                )
            })?;
            let base_dir = AbsolutePathBuf::from_absolute_path(requirements_parent)?;
            Ok(Some(
                RequirementsLayerEntry::from_toml(
                    RequirementSource::SystemRequirementsToml {
                        file: requirements_toml_file.clone(),
                    },
                    contents,
                )
                .with_base_dir(base_dir),
            ))
        }
        Err(e) => {
            if e.kind() != io::ErrorKind::NotFound {
                Err(io::Error::new(
                    e.kind(),
                    format!(
                        "Failed to read requirements file {}: {e}",
                        requirements_toml_file.as_path().display(),
                    ),
                ))
            } else {
                Ok(None)
            }
        }
    }
}

#[cfg(unix)]
fn system_requirements_toml_file() -> io::Result<AbsolutePathBuf> {
    AbsolutePathBuf::from_absolute_path(Path::new("/etc/ahead/requirements.toml"))
}

#[cfg(windows)]
fn system_requirements_toml_file() -> io::Result<AbsolutePathBuf> {
    windows_system_requirements_toml_file()
}

fn system_requirements_toml_file_with_overrides(
    overrides: &LoaderOverrides,
) -> io::Result<AbsolutePathBuf> {
    match &overrides.system_requirements_path {
        Some(path) => AbsolutePathBuf::from_absolute_path(path),
        None => system_requirements_toml_file(),
    }
}

#[cfg(windows)]
fn windows_ahead_system_dir() -> PathBuf {
    let program_data = windows_program_data_dir_from_known_folder().unwrap_or_else(|err| {
        tracing::warn!(
            error = %err,
            "Failed to resolve ProgramData known folder; using default path"
        );
        PathBuf::from(DEFAULT_PROGRAM_DATA_DIR_WINDOWS)
    });
    program_data.join("AHEAD")
}

#[cfg(windows)]
fn windows_system_requirements_toml_file() -> io::Result<AbsolutePathBuf> {
    let requirements_toml_file = windows_ahead_system_dir().join("requirements.toml");
    AbsolutePathBuf::try_from(requirements_toml_file)
}

#[cfg(windows)]
fn windows_program_data_dir_from_known_folder() -> io::Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::FOLDERID_ProgramData;
    use windows_sys::Win32::UI::Shell::KF_FLAG_DEFAULT;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;

    let mut path_ptr = std::ptr::null_mut::<u16>();
    let known_folder_flags = u32::try_from(KF_FLAG_DEFAULT).map_err(|_| {
        io::Error::other(format!(
            "KF_FLAG_DEFAULT did not fit in u32: {KF_FLAG_DEFAULT}"
        ))
    })?;
    // Known folder IDs reference:
    // https://learn.microsoft.com/en-us/windows/win32/shell/knownfolderid
    // SAFETY: SHGetKnownFolderPath initializes path_ptr with a CoTaskMem-allocated,
    // null-terminated UTF-16 string on success.
    let hr = unsafe {
        SHGetKnownFolderPath(&FOLDERID_ProgramData, known_folder_flags, 0, &mut path_ptr)
    };
    if hr != 0 {
        return Err(io::Error::other(format!(
            "SHGetKnownFolderPath(FOLDERID_ProgramData) failed with HRESULT {hr:#010x}"
        )));
    }
    if path_ptr.is_null() {
        return Err(io::Error::other(
            "SHGetKnownFolderPath(FOLDERID_ProgramData) returned a null pointer",
        ));
    }

    // SAFETY: path_ptr is a valid null-terminated UTF-16 string allocated by
    // SHGetKnownFolderPath and must be freed with CoTaskMemFree.
    let path = unsafe {
        let mut len = 0usize;
        while *path_ptr.add(len) != 0 {
            len += 1;
        }
        let wide = std::slice::from_raw_parts(path_ptr, len);
        let path = PathBuf::from(OsString::from_wide(wide));
        CoTaskMemFree(path_ptr.cast());
        path
    };

    Ok(path)
}

/// Takes a `toml::Value` parsed from a config.toml file and walks through it,
/// resolving any `AbsolutePathBuf` fields against `base_dir`, returning a new
/// `toml::Value` with the same shape but with paths resolved.
///
/// This ensures that multiple config layers can be merged together correctly
/// even if they were loaded from different directories.
#[doc(hidden)]
pub fn resolve_relative_paths_in_config_toml(
    value_from_config_toml: TomlValue,
    base_dir: &Path,
) -> io::Result<TomlValue> {
    // Use the serialize/deserialize round-trip to convert the
    // `toml::Value` into a `ConfigToml` with `AbsolutePath
    let _guard = AbsolutePathBufGuard::new(base_dir);
    let Ok(resolved) = value_from_config_toml.clone().try_into::<ConfigToml>() else {
        return Ok(value_from_config_toml);
    };
    drop(_guard);

    let resolved_value = TomlValue::try_from(resolved).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Failed to serialize resolved config: {e}"),
        )
    })?;

    Ok(copy_shape_from_original(
        &value_from_config_toml,
        &resolved_value,
    ))
}

/// Ensure that every field in `original` is present in the returned
/// `toml::Value`, taking the value from `resolved` where possible. This ensures
/// the fields that we "removed" during the serialize/deserialize round-trip in
/// `resolve_config_paths` are preserved, out of an abundance of caution.
fn copy_shape_from_original(original: &TomlValue, resolved: &TomlValue) -> TomlValue {
    match (original, resolved) {
        (TomlValue::Table(original_table), TomlValue::Table(resolved_table)) => {
            let mut table = toml::map::Map::new();
            for (key, original_value) in original_table {
                let resolved_value = resolved_table.get(key).unwrap_or(original_value);
                table.insert(
                    key.clone(),
                    copy_shape_from_original(original_value, resolved_value),
                );
            }
            TomlValue::Table(table)
        }
        (TomlValue::Array(original_array), TomlValue::Array(resolved_array)) => {
            let mut items = Vec::new();
            for (index, original_value) in original_array.iter().enumerate() {
                let resolved_value = resolved_array.get(index).unwrap_or(original_value);
                items.push(copy_shape_from_original(original_value, resolved_value));
            }
            TomlValue::Array(items)
        }
        (_, resolved_value) => resolved_value.clone(),
    }
}

// Cannot name this `mod tests` because of tests.rs in this folder.
#[cfg(test)]
mod unit_tests {
    use super::*;
    #[cfg(windows)]
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn ensure_resolve_relative_paths_in_config_toml_preserves_all_fields() -> anyhow::Result<()> {
        let tmp = tempdir()?;
        let base_dir = tmp.path();
        let contents = r#"
# This is a field recognized by config.toml that is an AbsolutePathBuf in
# the ConfigToml struct.
model_instructions_file = "./some_file.md"

# This is a field recognized by config.toml.
model = "gpt-1000"

# This is a field not recognized by config.toml.
foo = "xyzzy"
"#;
        let user_config: TomlValue = toml::from_str(contents)?;

        let normalized_toml_value = resolve_relative_paths_in_config_toml(user_config, base_dir)?;
        let mut expected_toml_value = toml::map::Map::new();
        expected_toml_value.insert(
            "model_instructions_file".to_string(),
            TomlValue::String(
                AbsolutePathBuf::resolve_path_against_base("./some_file.md", base_dir)
                    .as_path()
                    .to_string_lossy()
                    .to_string(),
            ),
        );
        expected_toml_value.insert(
            "model".to_string(),
            TomlValue::String("gpt-1000".to_string()),
        );
        expected_toml_value.insert("foo".to_string(), TomlValue::String("xyzzy".to_string()));
        assert_eq!(normalized_toml_value, TomlValue::Table(expected_toml_value));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_system_requirements_toml_file_uses_expected_suffix() {
        let expected = windows_program_data_dir_from_known_folder()
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_PROGRAM_DATA_DIR_WINDOWS))
            .join("AHEAD")
            .join("requirements.toml");
        assert_eq!(
            windows_system_requirements_toml_file()
                .expect("requirements.toml path")
                .as_path(),
            expected.as_path()
        );
        assert!(
            windows_system_requirements_toml_file()
                .expect("requirements.toml path")
                .as_path()
                .ends_with(Path::new("AHEAD").join("requirements.toml"))
        );
    }
}
