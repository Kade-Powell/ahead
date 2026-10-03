use ahead_extension_host::{IconTheme, load_icon_theme};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

const DEFAULT_ICON_THEME: &str = "material-icon-theme";

fn preference_path() -> Option<PathBuf> {
    ahead_core::directory::Directory::config_directory()
        .map(|directory| directory.join("icon-theme"))
}

pub(crate) fn active_icon_theme_id() -> String {
    preference_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|id| id.trim().to_string())
        .filter(|id| valid_extension_id(id))
        .unwrap_or_else(|| DEFAULT_ICON_THEME.to_string())
}

fn valid_extension_id(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
        })
}

fn theme_from_extension(plugins: &Path, id: &str) -> Result<IconTheme, String> {
    if !valid_extension_id(id) {
        return Err("Invalid icon theme extension ID".into());
    }
    let directory = plugins.join(id);
    let metadata =
        std::fs::symlink_metadata(&directory).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Icon theme extension directory is invalid".into());
    }
    let manifest_path = directory.join("extension.toml");
    let manifest_metadata = std::fs::symlink_metadata(&manifest_path)
        .map_err(|error| error.to_string())?;
    if !manifest_metadata.is_file()
        || manifest_metadata.file_type().is_symlink()
        || manifest_metadata.len() > 256 * 1024
    {
        return Err("Icon theme extension manifest is invalid".into());
    }
    let manifest: toml::Table = std::fs::read_to_string(manifest_path)
        .map_err(|error| error.to_string())?
        .parse()
        .map_err(|error: toml::de::Error| error.to_string())?;
    if manifest.get("id").and_then(toml::Value::as_str) != Some(id) {
        return Err("Icon theme extension ID does not match its directory".into());
    }
    let path = manifest
        .get("icon_themes")
        .and_then(toml::Value::as_array)
        .and_then(|paths| paths.first())
        .and_then(toml::Value::as_str)
        .ok_or("Extension declares no icon themes")?;
    load_icon_theme(&directory, Path::new(path)).map_err(|error| error.to_string())
}

pub(crate) fn active_icon_theme() -> Result<Option<IconTheme>, String> {
    let plugins = ahead_core::directory::Directory::plugins_directory()
        .ok_or("AHEAD extension directory is unavailable")?;
    let id = active_icon_theme_id();
    if !plugins.join(&id).exists() {
        return Ok(None);
    }
    theme_from_extension(&plugins, &id).map(Some)
}

pub(crate) fn select_icon_theme(id: &str) -> Result<(), String> {
    let plugins = ahead_core::directory::Directory::plugins_directory()
        .ok_or("AHEAD extension directory is unavailable")?;
    theme_from_extension(&plugins, id)?;
    let path = preference_path().ok_or("AHEAD config directory is unavailable")?;
    let parent = path
        .parent()
        .ok_or("Icon theme preference has no directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| error.to_string())?;
    temporary
        .write_all(id.as_bytes())
        .map_err(|error| error.to_string())?;
    temporary
        .persist(path)
        .map_err(|error| error.error.to_string())?;
    Ok(())
}
