//! AHEAD Settings Panel - Appearance & BYOK AI Connection Configuration
//!
//! Allows users to configure multiple OpenAI-compatible model servers
//! and persist configurations securely in the workspace `.ahead` directory.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    rc::Rc,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use ahead_rpc::ahead::{McpServerApprovalAction, McpServerDeclaration};
use fs4::fs_std::FileExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent, PanelId, TabGroup,
};
use gpui_kit::component::input::{Input, InputContentType, InputEvent, InputState};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::component::setting::{
    SelectIndex, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use gpui_kit_assets::IconName;

const DEFAULT_SETTINGS: &str = include_str!("../../defaults/settings.toml");
#[derive(Clone, Debug, PartialEq, Eq)]
struct AiConnection {
    name: String,
    provider_id: String,
    base_url: String,
    api_key: String,
    model: String,
    models: Vec<String>,
    hidden_models: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiscoveredModel {
    pub provider_id: String,
    pub provider_name: String,
    pub model_id: String,
    pub display_name: Option<String>,
    pub context_window: Option<u32>,
    pub supports_tool_calls: Option<bool>,
}

pub(crate) struct ModelDiscovery {
    pub models: Vec<DiscoveredModel>,
    pub configured_provider_ids: Option<HashSet<String>>,
    pub hidden_model_ids_by_provider: HashMap<String, HashSet<String>>,
    pub refreshed_provider_ids: HashSet<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ModelCatalogEntry {
    model_id: String,
    display_name: Option<String>,
    context_window: Option<u32>,
    supports_tool_calls: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ModelVisibilityModel {
    model_id: String,
    visible: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ModelVisibilityProvider {
    connection_index: usize,
    provider_id: String,
    name: String,
    models: Vec<ModelVisibilityModel>,
    expanded: bool,
}

fn parse_toml_value(content: &str) -> Option<toml::Value> {
    content.parse::<toml::Table>().ok().map(toml::Value::Table)
}

const MODEL_REASONING_PREFERENCES_KEY: &str = "model_reasoning_preferences";

fn model_reasoning_preferences(
    value: &toml::Value,
) -> HashMap<(String, String), String> {
    value
        .get("agent")
        .and_then(|agent| agent.get(MODEL_REASONING_PREFERENCES_KEY))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let table = entry.as_table()?;
            Some((
                (
                    table.get("provider_id")?.as_str()?.to_string(),
                    table.get("model_id")?.as_str()?.to_string(),
                ),
                table.get("effort")?.as_str()?.to_string(),
            ))
        })
        .collect()
}

pub(crate) fn model_reasoning_preference(
    provider_id: &str,
    model_id: &str,
) -> Option<String> {
    let user_home = ahead_core::directory::Directory::home_dir()?;
    let content = ahead_core::config::read_ahead_config(&user_home, "settings.toml")
        .ok()??;
    let value = parse_toml_value(&content)?;
    model_reasoning_preferences(&value)
        .remove(&(provider_id.to_string(), model_id.to_string()))
}

pub(crate) fn save_model_reasoning_preference(
    provider_id: &str,
    model_id: &str,
    effort: Option<&str>,
) -> Result<(), String> {
    let user_home = ahead_core::directory::Directory::home_dir()
        .ok_or_else(|| "User settings directory is unavailable".to_string())?;
    save_model_reasoning_preference_with_home(
        &user_home,
        provider_id,
        model_id,
        effort,
    )
    .map_err(|error| error.to_string())
}

fn save_model_reasoning_preference_with_home(
    user_home: &Path,
    provider_id: &str,
    model_id: &str,
    effort: Option<&str>,
) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};

    let path = user_home.join(".ahead").join("settings.toml");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _settings_lock = lock_settings_file(&path)?;
    let mut config = match std::fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "settings path is not a regular file",
            ));
        }
        Ok(_) => {
            let content = std::fs::read_to_string(&path)?;
            parse_toml_value(&content).ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidData,
                    "user settings contain invalid TOML",
                )
            })?
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            toml::Value::Table(toml::map::Map::new())
        }
        Err(error) => return Err(error),
    };
    let root = config.as_table_mut().ok_or_else(|| {
        Error::new(ErrorKind::InvalidData, "user settings must be a TOML table")
    })?;
    let agent = root
        .entry("agent")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .ok_or_else(|| {
            Error::new(ErrorKind::InvalidData, "agent settings must be a table")
        })?;
    let remove_preferences = {
        let preferences = agent
            .entry(MODEL_REASONING_PREFERENCES_KEY)
            .or_insert_with(|| toml::Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidData,
                    "model reasoning preferences must be an array",
                )
            })?;
        preferences.retain(|entry| {
            let Some(table) = entry.as_table() else {
                return true;
            };
            table.get("provider_id").and_then(toml::Value::as_str)
                != Some(provider_id)
                || table.get("model_id").and_then(toml::Value::as_str)
                    != Some(model_id)
        });
        if let Some(effort) = effort {
            let mut preference = toml::map::Map::new();
            preference.insert(
                "provider_id".into(),
                toml::Value::String(provider_id.to_string()),
            );
            preference.insert(
                "model_id".into(),
                toml::Value::String(model_id.to_string()),
            );
            preference
                .insert("effort".into(), toml::Value::String(effort.to_string()));
            preferences.push(toml::Value::Table(preference));
        }
        preferences.is_empty()
    };
    if remove_preferences {
        agent.remove(MODEL_REASONING_PREFERENCES_KEY);
    }
    let content = toml::to_string(&config)
        .map_err(|error| Error::new(ErrorKind::InvalidData, error.to_string()))?;
    write_user_config(&path, &content)
}

pub(crate) fn inline_blame_enabled(workspace: &Path) -> bool {
    ahead_core::config::read_ahead_config(workspace, "settings.toml")
        .ok()
        .flatten()
        .and_then(|content| parse_toml_value(&content))
        .and_then(|config| config.get("editor")?.get("inline-blame")?.as_bool())
        .unwrap_or(true)
}

fn model_catalog(value: &str) -> Vec<String> {
    value
        .split([',', '\n'])
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .fold(Vec::new(), |mut models, model| {
            if !models.iter().any(|existing| existing == &model) {
                models.push(model);
            }
            models
        })
}

fn models_endpoint(base_url: &str) -> String {
    format!("{}/models", base_url.trim_end_matches('/'))
}

fn fetch_model_ids(
    client: &reqwest::blocking::Client,
    base_url: &str,
    api_key: &str,
) -> Result<Vec<String>, String> {
    fetch_model_catalog(client, base_url, api_key)
        .map(|models| models.into_iter().map(|model| model.model_id).collect())
}

fn fetch_model_catalog(
    client: &reqwest::blocking::Client,
    base_url: &str,
    api_key: &str,
) -> Result<Vec<ModelCatalogEntry>, String> {
    let mut request = client.get(models_endpoint(base_url));
    if !api_key.trim().is_empty() {
        request = request.bearer_auth(api_key);
    }
    let response = request.send().map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(model_endpoint_status(response.status()));
    }
    let value = response
        .json::<serde_json::Value>()
        .map_err(|_| "Invalid model list response".to_string())?;
    let models = discovered_model_catalog(value);
    if models.is_empty() {
        return Err("No model ids returned".into());
    }
    Ok(models)
}

pub(crate) fn discover_configured_models(workspace: &Path) -> ModelDiscovery {
    let (connections, source, mut warnings) =
        SettingsPanel::load_saved_config(workspace);
    if source == "built-in defaults" {
        return ModelDiscovery {
            models: Vec::new(),
            configured_provider_ids: warnings.is_empty().then(HashSet::new),
            hidden_model_ids_by_provider: HashMap::new(),
            refreshed_provider_ids: HashSet::new(),
            warnings,
        };
    }
    let configured_provider_ids = connections
        .iter()
        .map(|connection| connection.provider_id.clone())
        .collect::<HashSet<_>>();
    let hidden_model_ids_by_provider = connections
        .iter()
        .map(|connection| {
            (
                connection.provider_id.clone(),
                connection.hidden_models.iter().cloned().collect(),
            )
        })
        .collect();
    let configuration_is_complete = warnings.is_empty();
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            warnings.push("Could not initialize model discovery".into());
            return ModelDiscovery {
                models: Vec::new(),
                configured_provider_ids: configuration_is_complete
                    .then_some(configured_provider_ids),
                hidden_model_ids_by_provider,
                refreshed_provider_ids: HashSet::new(),
                warnings,
            };
        }
    };
    let results = std::thread::scope(|scope| {
        connections
            .into_iter()
            .map(|connection| {
                let client = client.clone();
                scope.spawn(move || {
                    let result = fetch_model_catalog(
                        &client,
                        &connection.base_url,
                        &connection.api_key,
                    );
                    (connection, result)
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|request| request.join())
            .collect::<Vec<_>>()
    });

    let mut models = Vec::new();
    let mut refreshed_provider_ids = HashSet::new();
    for result in results {
        let Ok((connection, result)) = result else {
            warnings.push("A model endpoint discovery task failed".into());
            continue;
        };
        match result {
            Ok(model_catalog) => {
                refreshed_provider_ids.insert(connection.provider_id.clone());
                models.extend(
                    model_catalog
                        .into_iter()
                        .filter(|model| {
                            !connection.hidden_models.contains(&model.model_id)
                        })
                        .map(|model| DiscoveredModel {
                            provider_id: connection.provider_id.clone(),
                            provider_name: connection.name.clone(),
                            model_id: model.model_id,
                            display_name: model.display_name,
                            context_window: model.context_window,
                            supports_tool_calls: model.supports_tool_calls,
                        }),
                );
            }
            Err(error) => {
                let reason = if error.starts_with("Model endpoint responded")
                    || error.starts_with("Model endpoint reachable")
                {
                    error
                } else {
                    "endpoint unavailable".into()
                };
                warnings.push(format!("{}: {reason}", connection.name));
            }
        }
    }
    ModelDiscovery {
        models,
        configured_provider_ids: configuration_is_complete
            .then_some(configured_provider_ids),
        hidden_model_ids_by_provider,
        refreshed_provider_ids,
        warnings,
    }
}

fn model_endpoint_status(status: reqwest::StatusCode) -> String {
    if status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
    {
        format!("Model endpoint reachable · credentials rejected (HTTP {status})")
    } else if status.is_success() {
        format!("Model endpoint reachable · HTTP {status}")
    } else {
        format!("Model endpoint responded with HTTP {status}")
    }
}

fn discovered_model_catalog(value: serde_json::Value) -> Vec<ModelCatalogEntry> {
    let entries = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .or_else(|| value.as_array());
    let mut models: Vec<ModelCatalogEntry> = Vec::new();
    for entry in entries.into_iter().flatten() {
        let Some(model_id) = entry
            .as_str()
            .or_else(|| entry.get("id").and_then(serde_json::Value::as_str))
            .map(str::trim)
            .filter(|model_id| !model_id.is_empty())
        else {
            continue;
        };
        let context_window = ["context_length", "context_window"]
            .into_iter()
            .find_map(|field| {
                entry
                    .get(field)
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|tokens| u32::try_from(tokens).ok())
                    .filter(|tokens| *tokens > 0)
            });
        let display_name = ["name", "display_name"].into_iter().find_map(|field| {
            entry
                .get(field)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        });
        let supports_tool_calls = entry
            .get("supports_tools")
            .or_else(|| entry.get("supports_tool_calls"))
            .and_then(serde_json::Value::as_bool)
            .or_else(|| {
                entry
                    .get("supported_parameters")
                    .and_then(serde_json::Value::as_array)
                    .map(|parameters| {
                        parameters
                            .iter()
                            .any(|parameter| parameter.as_str() == Some("tools"))
                    })
            });
        if let Some(existing) = models
            .iter_mut()
            .find(|existing| existing.model_id == model_id)
        {
            if existing.context_window.is_none() {
                existing.context_window = context_window;
            }
            if existing.display_name.is_none() {
                existing.display_name = display_name;
            }
            if existing.supports_tool_calls.is_none() {
                existing.supports_tool_calls = supports_tool_calls;
            }
        } else {
            models.push(ModelCatalogEntry {
                model_id: model_id.to_string(),
                display_name,
                context_window,
                supports_tool_calls,
            });
        }
    }
    models
}

fn model_discovery_outcome<T>(
    active_generation: u64,
    request_generation: u64,
    result: Result<T, String>,
) -> Option<Result<T, String>> {
    (active_generation == request_generation).then_some(result)
}

fn model_visibility_providers(
    connections: &[AiConnection],
    discovered_models: &[Vec<String>],
    expanded_providers: &HashSet<String>,
) -> Vec<ModelVisibilityProvider> {
    connections
        .iter()
        .enumerate()
        .map(|(connection_index, connection)| {
            let mut models = connection.models.clone();
            if let Some(discovered) = discovered_models.get(connection_index) {
                for model in discovered {
                    if !models.contains(model) {
                        models.push(model.clone());
                    }
                }
            }
            ModelVisibilityProvider {
                connection_index,
                provider_id: connection.provider_id.clone(),
                name: connection.name.clone(),
                models: models
                    .into_iter()
                    .map(|model_id| ModelVisibilityModel {
                        visible: !connection.hidden_models.contains(&model_id),
                        model_id,
                    })
                    .collect(),
                expanded: expanded_providers.contains(&connection.provider_id),
            }
        })
        .collect()
}

const MODEL_VISIBILITY_ROW_HEIGHT: f32 = 36.0;

fn model_visibility_scroll_area(
    scroll_handle: &UniformListScrollHandle,
    model_count: usize,
    model_list: impl IntoElement,
    scrollbar_track: gpui_kit::Hsla,
    scrollbar_thumb: gpui_kit::Hsla,
    scrollbar_thumb_hover: gpui_kit::Hsla,
) -> impl IntoElement {
    div()
        .relative()
        .w_full()
        .h(px(280.))
        .overflow_hidden()
        .child(model_list)
        // Paint above virtual rows, which otherwise cover the scrollbar thumb.
        .child(
            div().absolute().inset_0().child(
                Scrollbar::vertical(scroll_handle)
                    .mode(ScrollbarMode::Always)
                    .scroll_size(size(
                        px(0.),
                        px(model_count as f32 * MODEL_VISIBILITY_ROW_HEIGHT),
                    ))
                    .styles(|styles| {
                        styles
                            .track(|track| track.bg(scrollbar_track).width(px(14.)))
                            .track_hover(|track| {
                                track.bg(scrollbar_track).width(px(14.))
                            })
                            .thumb(|thumb| thumb.bg(scrollbar_thumb).width(px(8.)))
                            .thumb_hover(|thumb| {
                                thumb.bg(scrollbar_thumb_hover).width(px(10.))
                            })
                    }),
            ),
        )
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
}

fn provider_id_for(name: &str, fallback: &str) -> String {
    let mut id = String::from("ahead-");
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            id.push(character.to_ascii_lowercase());
        } else if !id.ends_with('-') {
            id.push('-');
        }
    }
    while id.ends_with('-') {
        id.pop();
    }
    if id == "ahead" {
        format!("ahead-{}", fallback.trim_matches('-'))
    } else {
        id
    }
}

fn configured_provider_id(
    connection: &toml::Value,
    fallback_provider: &str,
) -> Option<String> {
    let table = connection.as_table()?;
    let connection_name = table
        .get("name")
        .and_then(toml::Value::as_str)
        .unwrap_or("Configured model")
        .trim();
    Some(
        table
            .get("provider_id")
            .or_else(|| table.get("id"))
            .and_then(toml::Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| provider_id_for(connection_name, fallback_provider)),
    )
}

fn resolve_config_value(
    configs: &[(String, toml::Value)],
    key: &str,
) -> Option<String> {
    configs.iter().rev().find_map(|(_, config)| {
        config
            .get("ai")
            .and_then(|ai| ai.get(key))
            .and_then(toml::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn serialize_ai_config(
    connections: &[AiConnection],
    active: usize,
) -> Result<String, toml::ser::Error> {
    let active_connection = connections.get(active).or_else(|| connections.first());
    let mut ai = toml::map::Map::new();
    ai.insert(
        "provider".into(),
        toml::Value::String("openai-compatible".into()),
    );
    if let Some(active_connection) = active_connection {
        ai.insert(
            "active_connection".into(),
            toml::Value::String(active_connection.name.clone()),
        );
        ai.insert(
            "base_url".into(),
            toml::Value::String(active_connection.base_url.clone()),
        );
        ai.insert(
            "api_key".into(),
            toml::Value::String(active_connection.api_key.clone()),
        );
        ai.insert(
            "model".into(),
            toml::Value::String(active_connection.model.clone()),
        );
    }
    ai.insert(
        "connections".into(),
        toml::Value::Array(
            connections
                .iter()
                .map(|connection| {
                    let mut table = toml::map::Map::new();
                    table.insert(
                        "name".into(),
                        toml::Value::String(connection.name.clone()),
                    );
                    table.insert(
                        "provider_id".into(),
                        toml::Value::String(connection.provider_id.clone()),
                    );
                    table.insert(
                        "base_url".into(),
                        toml::Value::String(connection.base_url.clone()),
                    );
                    table.insert(
                        "api_key".into(),
                        toml::Value::String(connection.api_key.clone()),
                    );
                    table.insert(
                        "model".into(),
                        toml::Value::String(connection.model.clone()),
                    );
                    table.insert(
                        "models".into(),
                        toml::Value::Array(
                            connection
                                .models
                                .iter()
                                .map(|model| toml::Value::String(model.clone()))
                                .collect(),
                        ),
                    );
                    table.insert(
                        "hidden_models".into(),
                        toml::Value::Array(
                            connection
                                .hidden_models
                                .iter()
                                .map(|model| toml::Value::String(model.clone()))
                                .collect(),
                        ),
                    );
                    toml::Value::Table(table)
                })
                .collect(),
        ),
    );
    ai.insert("temperature".into(), toml::Value::Float(0.2));

    let mut root = toml::map::Map::new();
    root.insert("ai".into(), toml::Value::Table(ai));
    toml::to_string(&toml::Value::Table(root))
}

fn parse_connections(value: &toml::Value) -> Vec<AiConnection> {
    let connections = value
        .get("ai")
        .and_then(|ai| ai.get("connections"))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|connection| {
            let table = connection.as_table()?;
            let name = table.get("name")?.as_str()?.trim();
            let base_url = table.get("base_url")?.as_str()?.trim();
            let mut models = table
                .get("models")
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
                .flat_map(model_catalog)
                .collect::<Vec<_>>();
            if let Some(model) = table.get("model").and_then(toml::Value::as_str) {
                let active = model.trim();
                if !active.is_empty() && !models.iter().any(|item| item == active) {
                    models.insert(0, active.to_string());
                }
            }
            let model = models.first()?.clone();
            let hidden_models = table
                .get("hidden_models")
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
                .flat_map(model_catalog)
                .fold(Vec::new(), |mut hidden, model| {
                    if !hidden.contains(&model) {
                        hidden.push(model);
                    }
                    hidden
                });
            if name.is_empty() || base_url.is_empty() {
                return None;
            }
            let provider_id =
                configured_provider_id(connection, "openai-compatible")?;
            Some(AiConnection {
                name: name.to_string(),
                provider_id,
                base_url: base_url.to_string(),
                api_key: table
                    .get("api_key")
                    .and_then(toml::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                model,
                models,
                hidden_models,
            })
        })
        .collect::<Vec<_>>();
    let mut seen_provider_ids = HashSet::new();
    let mut connections = connections
        .into_iter()
        .rev()
        .filter(|connection| {
            seen_provider_ids.insert(connection.provider_id.clone())
        })
        .collect::<Vec<_>>();
    connections.reverse();
    connections
}

fn write_user_config(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind, Write};

    let parent = path.parent().ok_or_else(|| {
        Error::new(ErrorKind::InvalidInput, "settings path has no parent")
    })?;
    if !std::fs::symlink_metadata(parent)?.is_dir() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "settings directory is not a regular directory",
        ));
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "settings path is not a regular file",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(content.as_bytes())?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

struct SettingsFileLock(std::fs::File);

impl Drop for SettingsFileLock {
    fn drop(&mut self) {
        // A concurrent fork can retain the open-file description. Explicitly
        // unlock when our critical section ends, even if that copy is still open.
        if let Err(error) = FileExt::unlock(&self.0) {
            eprintln!("Unlocking AHEAD settings: {error}");
        }
    }
}

fn lock_settings_file(path: &Path) -> std::io::Result<SettingsFileLock> {
    use std::io::{Error, ErrorKind};

    let parent = path.parent().ok_or_else(|| {
        Error::new(ErrorKind::InvalidInput, "settings path has no parent")
    })?;
    if !std::fs::symlink_metadata(parent)?.is_dir() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "settings directory is not a regular directory",
        ));
    }
    let lock_path = parent.join("settings.toml.lock");
    match std::fs::symlink_metadata(&lock_path) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "settings lock is not a regular file",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    file.lock_exclusive()?;
    Ok(SettingsFileLock(file))
}

fn ensure_settings_file(path: &Path) -> std::io::Result<bool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _settings_lock = lock_settings_file(path)?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => return Ok(false),
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "settings path is not a regular file",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    write_user_config(path, DEFAULT_SETTINGS)?;
    Ok(true)
}

#[cfg(test)]
fn has_ai_settings(workspace: &Path) -> bool {
    ahead_core::config::read_ahead_config(workspace, "settings.toml")
        .ok()
        .flatten()
        .and_then(|content| parse_toml_value(&content))
        .and_then(|config| config.get("ai").cloned())
        .is_some()
}

fn merge_ai_config(existing: &str, ai_config: &str) -> Result<String, String> {
    let mut config = if existing.trim().is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        parse_toml_value(existing)
            .ok_or_else(|| "workspace settings contain invalid TOML".to_string())?
    };
    let ai = parse_toml_value(ai_config)
        .and_then(|value| value.get("ai").cloned())
        .ok_or_else(|| "generated AI settings are invalid TOML".to_string())?;
    config
        .as_table_mut()
        .ok_or_else(|| "workspace settings must contain a TOML table".to_string())?
        .insert("ai".into(), ai);
    toml::to_string(&config).map_err(|error| error.to_string())
}

fn save_ai_config(
    workspace: &Path,
    ai_config: &str,
    voice: &VoiceSettings,
    inline_blame: bool,
) -> Result<[Option<SystemTime>; 4], String> {
    let path = SettingsPanel::config_path(workspace);
    let parent = path.parent().ok_or("settings path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let _settings_lock =
        lock_settings_file(&path).map_err(|error| error.to_string())?;
    let existing = ahead_core::config::read_ahead_config(workspace, "settings.toml")
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| DEFAULT_SETTINGS.to_string());
    let mut content = parse_toml_value(&merge_ai_config(&existing, ai_config)?)
        .ok_or("merged settings are invalid TOML")?;
    let root = content.as_table_mut().ok_or("settings must be a table")?;
    let mut voice_table = toml::map::Map::new();
    voice_table.insert(
        "provider".into(),
        toml::Value::String(voice.provider.clone()),
    );
    voice_table.insert("profile".into(), toml::Value::String(voice.profile.clone()));
    if !voice.endpoint.trim().is_empty() {
        voice_table.insert(
            "endpoint".into(),
            toml::Value::String(voice.endpoint.trim().to_string()),
        );
    }
    if !voice.model.trim().is_empty() {
        voice_table.insert(
            "model".into(),
            toml::Value::String(voice.model.trim().to_string()),
        );
    }
    if !voice.api_key.trim().is_empty() {
        voice_table
            .insert("api_key".into(), toml::Value::String(voice.api_key.clone()));
    }
    root.insert("voice".into(), toml::Value::Table(voice_table));
    let editor = root
        .entry("editor")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .ok_or("editor settings must be a table")?;
    editor.insert("inline-blame".into(), toml::Value::Boolean(inline_blame));
    let content = toml::to_string(&content).map_err(|error| error.to_string())?;
    write_user_config(&path, &content).map_err(|error| error.to_string())?;
    Ok(SettingsPanel::settings_modified(workspace))
}

struct PendingSettingsSave {
    workspace: PathBuf,
    generation: u64,
    ai_config: String,
    voice: VoiceSettings,
    inline_blame: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct VoiceSettings {
    provider: String,
    profile: String,
    endpoint: String,
    model: String,
    api_key: String,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            provider: "openai-live".into(),
            profile: "realtime-venus".into(),
            endpoint: String::new(),
            model: String::new(),
            api_key: String::new(),
        }
    }
}

struct LoadedSettings {
    connections: Vec<AiConnection>,
    selected_connection: usize,
    source: String,
    setup_required: bool,
    modified: [Option<SystemTime>; 4],
    status: String,
    inline_blame: bool,
    voice: VoiceSettings,
}

fn load_voice_settings(workspace: &Path) -> VoiceSettings {
    let Some(content) =
        ahead_core::config::read_ahead_config(workspace, "settings.toml")
            .ok()
            .flatten()
    else {
        return VoiceSettings::default();
    };
    let Some(voice) =
        parse_toml_value(&content).and_then(|config| config.get("voice").cloned())
    else {
        return VoiceSettings::default();
    };
    VoiceSettings {
        provider: voice
            .get("provider")
            .and_then(toml::Value::as_str)
            .unwrap_or("openai-live")
            .to_string(),
        profile: voice
            .get("profile")
            .and_then(toml::Value::as_str)
            .unwrap_or("realtime-venus")
            .to_string(),
        endpoint: voice
            .get("endpoint")
            .and_then(toml::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        model: voice
            .get("model")
            .and_then(toml::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        api_key: voice
            .get("api_key")
            .and_then(toml::Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

fn load_settings(
    workspace: &Path,
    previous_modified: Option<[Option<SystemTime>; 4]>,
) -> Option<LoadedSettings> {
    load_settings_with_user_home(
        workspace,
        previous_modified,
        ahead_core::directory::Directory::home_dir().as_deref(),
    )
}

fn load_settings_with_user_home(
    workspace: &Path,
    previous_modified: Option<[Option<SystemTime>; 4]>,
    user_home: Option<&Path>,
) -> Option<LoadedSettings> {
    let path = SettingsPanel::config_path(workspace);
    let initial = previous_modified.is_none();
    let bootstrap = if initial {
        ensure_settings_file(&path)
    } else {
        Ok(false)
    };
    let modified =
        SettingsPanel::settings_modified_with_user_home(workspace, user_home);
    if previous_modified == Some(modified) {
        return None;
    }
    let (connections, source, errors) =
        SettingsPanel::load_saved_config_with_user_home(workspace, user_home);
    let configs =
        SettingsPanel::read_ai_config_layers(workspace, user_home, &mut Vec::new());
    let selected_connection =
        SettingsPanel::active_connection_index_from_configs(&configs, &connections);
    let setup_required = source == "built-in defaults";
    let status = match bootstrap {
        Err(error) => format!("Unable to create workspace settings: {error}"),
        Ok(_) if !errors.is_empty() => {
            format!("Provider settings skipped: {}", errors.join("; "))
        }
        Ok(_) if initial && setup_required => {
            "Complete AHEAD setup to save workspace settings".into()
        }
        Ok(_) if initial => format!("Settings loaded from {}", path.display()),
        Ok(_) => format!("Settings reloaded from {}", path.display()),
    };
    Some(LoadedSettings {
        connections,
        selected_connection,
        source,
        setup_required,
        modified,
        status,
        inline_blame: inline_blame_enabled(workspace),
        voice: load_voice_settings(workspace),
    })
}

pub struct SettingsPanel {
    pub focus: FocusHandle,
    pub workspace: PathBuf,
    connections: Vec<AiConnection>,
    selected_connection: usize,
    pub name_input: Entity<InputState>,
    pub base_url_input: Entity<InputState>,
    pub api_key_input: Entity<InputState>,
    pub model_input: Entity<InputState>,
    voice_endpoint_input: Entity<InputState>,
    voice_model_input: Entity<InputState>,
    voice_api_key_input: Entity<InputState>,
    voice_provider: String,
    voice_profile: String,
    pub test_status: SharedString,
    mcp_declarations: Vec<McpServerDeclaration>,
    mcp_status: SharedString,
    mcp_busy: bool,
    pub status: SharedString,
    pub effective_source: SharedString,
    inline_blame: bool,
    setup_required: bool,
    suppress_autosave: bool,
    loaded: bool,
    load_in_flight: bool,
    reload_requested: bool,
    _config_watch: Option<Task<()>>,
    save_generation: u64,
    save_in_flight: bool,
    pending_save: Option<PendingSettingsSave>,
    save_error: Option<String>,
    settings_modified: [Option<SystemTime>; 4],
    discovered_models: Vec<Vec<String>>,
    expanded_model_providers: HashSet<String>,
    model_visibility_scrolls: HashMap<String, UniformListScrollHandle>,
    model_discovery_generation: u64,
    close_handler: Option<Rc<dyn Fn(PanelId, &mut Window, &mut App)>>,
    save_handler: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
    proxy: Option<Arc<crate::proxy_client::ProxyClient>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsSection {
    Appearance,
    Editor,
    AiIntegration,
    VoiceCalls,
    McpServers,
}

impl SettingsPanel {
    pub fn new(
        workspace: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx));
        let base_url_input = cx.new(|cx| InputState::new(window, cx));
        let api_key_input = cx.new(|cx| InputState::new(window, cx).masked(true));
        let model_input = cx.new(|cx| InputState::new(window, cx));
        let voice_endpoint_input = cx.new(|cx| InputState::new(window, cx));
        let voice_model_input = cx.new(|cx| InputState::new(window, cx));
        let voice_api_key_input =
            cx.new(|cx| InputState::new(window, cx).masked(true));
        for (input, invalidates_model_discovery) in [
            (name_input.clone(), false),
            (base_url_input.clone(), true),
            (api_key_input.clone(), true),
            (model_input.clone(), false),
        ] {
            cx.subscribe_in(
                &input,
                window,
                move |this: &mut Self, _state, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) && !this.suppress_autosave
                    {
                        if invalidates_model_discovery {
                            let matches_saved_connection = this
                                .connections
                                .get(this.selected_connection)
                                .is_some_and(|connection| {
                                    connection.base_url
                                        == this.base_url_input.read(cx).value()
                                        && connection.api_key
                                            == this.api_key_input.read(cx).value()
                                });
                            if matches_saved_connection {
                                return;
                            }
                            this.model_discovery_generation =
                                this.model_discovery_generation.wrapping_add(1);
                            if let Some(models) = this
                                .discovered_models
                                .get_mut(this.selected_connection)
                            {
                                models.clear();
                            }
                            this.test_status =
                                "Endpoint changed; discover models again".into();
                        }
                        this.save_config(window, cx);
                    }
                },
            )
            .detach();
        }

        for input in [
            voice_endpoint_input.clone(),
            voice_model_input.clone(),
            voice_api_key_input.clone(),
        ] {
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, _state, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) && !this.suppress_autosave
                    {
                        this.save_config(window, cx);
                    }
                },
            )
            .detach();
        }

        let mut panel = Self {
            focus: cx.focus_handle(),
            workspace,
            connections: Vec::new(),
            selected_connection: 0,
            name_input,
            base_url_input,
            api_key_input,
            model_input,
            voice_endpoint_input,
            voice_model_input,
            voice_api_key_input,
            voice_provider: "openai-live".into(),
            voice_profile: "realtime-venus".into(),
            test_status: "Endpoint not tested".into(),
            mcp_declarations: Vec::new(),
            mcp_status: "MCP declarations not loaded".into(),
            mcp_busy: false,
            status: "Loading workspace settings…".into(),
            effective_source: "Loading…".into(),
            inline_blame: true,
            setup_required: false,
            suppress_autosave: false,
            loaded: false,
            load_in_flight: false,
            reload_requested: false,
            _config_watch: None,
            save_generation: 0,
            save_in_flight: false,
            pending_save: None,
            save_error: None,
            settings_modified: [None; 4],
            discovered_models: Vec::new(),
            expanded_model_providers: HashSet::new(),
            model_visibility_scrolls: HashMap::new(),
            model_discovery_generation: 0,
            close_handler: None,
            save_handler: None,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
            proxy: None,
        };
        panel.refresh_from_disk(false, window, cx);
        panel
    }

    pub fn with_proxy(
        mut self,
        proxy: Arc<crate::proxy_client::ProxyClient>,
    ) -> Self {
        self.proxy = Some(proxy);
        self
    }

    pub fn load_mcp_server_declarations(&mut self, cx: &mut Context<Self>) {
        if self.mcp_busy {
            return;
        }
        let Some(proxy) = self.proxy.clone() else {
            self.mcp_status = "AHEAD proxy is unavailable".into();
            cx.notify();
            return;
        };
        let workspace = self.workspace.clone();
        self.mcp_busy = true;
        self.mcp_status = "Loading MCP declarations…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    proxy
                        .mcp_server_declarations(&workspace)
                        .map_err(|error| error.message)
                })
                .await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                this.mcp_busy = false;
                match result {
                    Ok(declarations) => {
                        this.mcp_status =
                            format!("{} tracked MCP server(s)", declarations.len())
                                .into();
                        this.mcp_declarations = declarations;
                    }
                    Err(error) => {
                        this.mcp_status =
                            format!("MCP discovery failed: {error}").into()
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_mcp_server_approval(
        &mut self,
        server_id: String,
        fingerprint: String,
        action: McpServerApprovalAction,
        cx: &mut Context<Self>,
    ) {
        if self.mcp_busy {
            return;
        }
        let Some(proxy) = self.proxy.clone() else {
            self.mcp_status = "AHEAD proxy is unavailable".into();
            cx.notify();
            return;
        };
        let workspace = self.workspace.clone();
        self.mcp_busy = true;
        self.mcp_status = format!("Updating MCP server {server_id}…").into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (change, current) = cx.background_spawn(async move {
                let change = proxy
                    .set_mcp_server_approval(&workspace, &server_id, &fingerprint, action)
                    .map_err(|error| error.message);
                let current = proxy.mcp_server_declarations(&workspace).map_err(|error| error.message);
                (change, current)
            }).await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                this.mcp_busy = false;
                if let Ok(declarations) = current {
                    this.mcp_declarations = declarations;
                }
                this.mcp_status = match (change, action) {
                    (Ok(()), McpServerApprovalAction::ApproveAndEnable) => "MCP server approved for new managed sessions".into(),
                    (Ok(()), McpServerApprovalAction::Disable) => "MCP server disabled for new sessions; stop existing sessions separately".into(),
                    (Ok(()), McpServerApprovalAction::AutoApproveAll) => "Permitted calls from this MCP server will be auto-approved in new managed sessions".into(),
                    (Ok(()), McpServerApprovalAction::ReviewEachCall) => "MCP calls will require review in new managed sessions".into(),
                    (Err(error), _) => format!("MCP approval failed: {error}; refresh the declaration").into(),
                };
                cx.notify();
            });
        }).detach();
    }

    pub fn set_close_handler<F>(&mut self, handler: F)
    where
        F: Fn(PanelId, &mut Window, &mut App) + 'static,
    {
        self.close_handler = Some(Rc::new(handler));
    }

    pub fn set_save_handler<F>(&mut self, handler: F)
    where
        F: Fn(&mut Window, &mut App) + 'static,
    {
        self.save_handler = Some(Rc::new(handler));
    }

    fn config_path(workspace: &Path) -> PathBuf {
        workspace.join(".ahead").join("settings.toml")
    }

    pub(crate) fn settings_modified(workspace: &Path) -> [Option<SystemTime>; 4] {
        Self::settings_modified_with_user_home(
            workspace,
            ahead_core::directory::Directory::home_dir().as_deref(),
        )
    }

    pub(crate) fn is_provider_settings_change(
        workspace: &Path,
        paths: &[PathBuf],
    ) -> bool {
        Self::is_provider_settings_change_with_user_home(
            workspace,
            paths,
            ahead_core::directory::Directory::home_dir().as_deref(),
        )
    }

    fn is_provider_settings_change_with_user_home(
        workspace: &Path,
        paths: &[PathBuf],
        user_home: Option<&Path>,
    ) -> bool {
        let workspace_ahead = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf())
            .join(".ahead");
        let user_ahead = user_home.map(|home| {
            home.canonicalize()
                .unwrap_or_else(|_| home.to_path_buf())
                .join(".ahead")
        });
        paths.iter().any(|path| {
            (path.parent() == Some(workspace_ahead.as_path())
                && matches!(
                    path.file_name().and_then(|name| name.to_str()),
                    Some("settings.toml" | "config.toml" | "config.local.toml")
                ))
                || user_ahead.as_ref().is_some_and(|directory| {
                    path == directory
                        || (path.parent() == Some(directory.as_path())
                            && path
                                .file_name()
                                .is_some_and(|name| name == "settings.toml"))
                })
        })
    }

    fn settings_modified_with_user_home(
        workspace: &Path,
        user_home: Option<&Path>,
    ) -> [Option<SystemTime>; 4] {
        let workspace_modified =
            ["settings.toml", "config.toml", "config.local.toml"].map(|filename| {
                std::fs::symlink_metadata(workspace.join(".ahead").join(filename))
                    .and_then(|metadata| metadata.modified())
                    .ok()
            });
        let user_modified = user_home.and_then(|home| {
            std::fs::symlink_metadata(home.join(".ahead/settings.toml"))
                .and_then(|metadata| metadata.modified())
                .ok()
        });
        [
            workspace_modified[0],
            workspace_modified[1],
            workspace_modified[2],
            user_modified,
        ]
    }

    #[cfg(test)]
    fn active_connection_index_with_user_home(
        workspace: &Path,
        connections: &[AiConnection],
        user_home: Option<&Path>,
    ) -> usize {
        let configs =
            Self::read_ai_config_layers(workspace, user_home, &mut Vec::new());
        Self::active_connection_index_from_configs(&configs, connections)
    }

    fn active_connection_index_from_configs(
        configs: &[(String, toml::Value)],
        connections: &[AiConnection],
    ) -> usize {
        let active_name = resolve_config_value(configs, "active_connection");
        let active_provider_id = active_name.as_deref().and_then(|active_name| {
            configs.iter().rev().find_map(|(_, config)| {
                let ai = config.get("ai")?;
                let fallback_provider = ai
                    .get("provider")
                    .and_then(toml::Value::as_str)
                    .unwrap_or("openai-compatible");
                ai.get("connections")
                    .and_then(toml::Value::as_array)?
                    .iter()
                    .find(|connection| {
                        connection
                            .as_table()
                            .and_then(|table| table.get("name"))
                            .and_then(toml::Value::as_str)
                            .is_some_and(|name| name.trim() == active_name)
                    })
                    .and_then(|connection| {
                        configured_provider_id(connection, fallback_provider)
                    })
            })
        });
        active_name
            .as_deref()
            .and_then(|name| {
                connections.iter().position(|connection| {
                    connection.name == name || connection.provider_id == name
                })
            })
            .or_else(|| {
                active_provider_id.as_deref().and_then(|provider_id| {
                    connections
                        .iter()
                        .position(|connection| connection.provider_id == provider_id)
                })
            })
            .unwrap_or(0)
    }

    fn load_saved_config(
        workspace: &Path,
    ) -> (Vec<AiConnection>, String, Vec<String>) {
        Self::load_saved_config_with_user_home(
            workspace,
            ahead_core::directory::Directory::home_dir().as_deref(),
        )
    }

    fn read_ai_config_layers(
        workspace: &Path,
        user_home: Option<&Path>,
        errors: &mut Vec<String>,
    ) -> Vec<(String, toml::Value)> {
        let mut configs = Vec::new();
        if let Some(user_home) = user_home {
            match ahead_core::config::read_ahead_config(user_home, "settings.toml") {
                Ok(Some(content)) => match parse_toml_value(&content) {
                    Some(value) => {
                        configs.push(("user ~/.ahead/settings.toml".into(), value))
                    }
                    None => errors
                        .push("user ~/.ahead/settings.toml: invalid TOML".into()),
                },
                Ok(None) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    errors.push(format!("user ~/.ahead/settings.toml: {error}"))
                }
            }
        }
        for (label, filename) in [
            ("workspace .ahead/settings.toml", "settings.toml"),
            ("shared .ahead/config.toml", "config.toml"),
            ("local .ahead/config.local.toml", "config.local.toml"),
        ] {
            match ahead_core::config::read_ahead_config(workspace, filename) {
                Ok(Some(content)) => {
                    if let Some(value) = parse_toml_value(&content) {
                        configs.push((label.to_string(), value));
                    } else {
                        errors.push(format!("{label}: invalid TOML"));
                    }
                }
                Ok(None) => {}
                Err(error) => errors.push(format!("{label}: {error}")),
            }
        }
        configs
    }

    fn load_saved_config_with_user_home(
        workspace: &Path,
        user_home: Option<&Path>,
    ) -> (Vec<AiConnection>, String, Vec<String>) {
        let mut errors = Vec::new();
        let configs = Self::read_ai_config_layers(workspace, user_home, &mut errors);

        let mut connections: Vec<AiConnection> = Vec::new();
        for (_, config) in &configs {
            for mut connection in parse_connections(config) {
                if let Some(existing) = connections
                    .iter_mut()
                    .find(|existing| existing.provider_id == connection.provider_id)
                {
                    if connection.api_key.is_empty()
                        && existing.base_url == connection.base_url
                    {
                        connection.api_key.clone_from(&existing.api_key);
                    }
                    *existing = connection;
                } else {
                    connections.push(connection);
                }
            }
        }
        if connections.is_empty() {
            let base_url = resolve_config_value(&configs, "base_url")
                .unwrap_or_else(|| "http://localhost:1234/v1".to_string());
            let api_key = configs
                .iter()
                .rev()
                .find_map(|(_, config)| {
                    let ai = config.get("ai")?;
                    let configured_url = ai.get("base_url")?.as_str()?;
                    (configured_url == base_url)
                        .then(|| ai.get("api_key")?.as_str().map(str::to_string))?
                })
                .unwrap_or_default();
            connections.push(AiConnection {
                name: "OpenAI-compatible server".into(),
                provider_id: resolve_config_value(&configs, "provider")
                    .map(|provider| {
                        provider_id_for("OpenAI-compatible server", &provider)
                    })
                    .unwrap_or_else(|| "openai-compatible".into()),
                base_url: base_url.clone(),
                api_key,
                model: resolve_config_value(&configs, "model")
                    .unwrap_or_else(|| "deepseek-coder".to_string()),
                models: vec![
                    resolve_config_value(&configs, "model")
                        .unwrap_or_else(|| "deepseek-coder".to_string()),
                ],
                hidden_models: Vec::new(),
            })
        }
        let source = configs
            .iter()
            .rev()
            .find(|(_, config)| config.get("ai").is_some())
            .map(|(label, _)| label.clone())
            .unwrap_or_else(|| "built-in defaults".to_string());
        (connections, source, errors)
    }

    pub fn save_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.loaded {
            return;
        }
        self.commit_selected_connection(cx);
        self.save_generation = self.save_generation.wrapping_add(1);
        self.pending_save = None;
        self.save_error = None;
        let ai_config =
            match serialize_ai_config(&self.connections, self.selected_connection) {
                Ok(content) => content,
                Err(error) => {
                    self.status = format!("Save failed: {error}").into();
                    self.save_error = Some(error.to_string());
                    cx.notify();
                    return;
                }
            };
        let voice = VoiceSettings {
            provider: self.voice_provider.clone(),
            profile: self.voice_profile.clone(),
            endpoint: self.voice_endpoint_input.read(cx).value().to_string(),
            model: self.voice_model_input.read(cx).value().to_string(),
            api_key: self.voice_api_key_input.read(cx).value().to_string(),
        };

        self.pending_save = Some(PendingSettingsSave {
            workspace: self.workspace.clone(),
            generation: self.save_generation,
            ai_config,
            voice,
            inline_blame: self.inline_blame,
        });
        self.status = "Saving workspace settings…".into();
        self.start_pending_save(window, cx);
        cx.notify();
    }

    fn select_voice_provider(
        &mut self,
        provider: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(provider, "openai-live" | "self-hosted") {
            return;
        }
        self.voice_provider = provider.to_string();
        if provider == "self-hosted"
            && self.voice_model_input.read(cx).value().trim().is_empty()
        {
            let default_model = match self.voice_profile.as_str() {
                "nemotron-voicechat" => "nemotron-voicechat",
                "realtime-venus" => {
                    "inclusionAI/Realtime-Venus/Realtime-Venus-Audio"
                }
                _ => "",
            };
            if !default_model.is_empty() {
                self.suppress_autosave = true;
                self.voice_model_input.update(cx, |input, cx| {
                    input.set_value(default_model, window, cx);
                });
                self.suppress_autosave = false;
            }
        }
        self.save_config(window, cx);
    }

    fn select_voice_profile(
        &mut self,
        profile: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(
            profile,
            "nemotron-voicechat" | "vllm-omni-pcm16" | "realtime-venus"
        ) {
            return;
        }
        self.voice_profile = profile.to_string();
        let default_model = match profile {
            "nemotron-voicechat" => "nemotron-voicechat",
            "realtime-venus" => "inclusionAI/Realtime-Venus/Realtime-Venus-Audio",
            _ => "",
        };
        if !default_model.is_empty()
            && self.voice_model_input.read(cx).value().trim().is_empty()
        {
            self.suppress_autosave = true;
            self.voice_model_input.update(cx, |input, cx| {
                input.set_value(default_model, window, cx);
            });
            self.suppress_autosave = false;
        }
        self.save_config(window, cx);
    }

    fn start_pending_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.save_in_flight {
            return;
        }
        let Some(request) = self.pending_save.take() else {
            return;
        };
        self.save_in_flight = true;
        let workspace = request.workspace.clone();
        let generation = request.generation;
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    save_ai_config(
                        &request.workspace,
                        &request.ai_config,
                        &request.voice,
                        request.inline_blame,
                    )
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.save_in_flight = false;
                if this.workspace == workspace && this.save_generation == generation
                {
                    match result {
                        Ok(modified) => {
                            this.setup_required = false;
                            // This write does not read the shared/local layers.
                            this.settings_modified[0] = modified[0];
                            this.status = format!(
                                "Saved workspace configuration to {}",
                                Self::config_path(&workspace).display()
                            )
                            .into();
                            if let Some(handler) = this.save_handler.clone() {
                                handler(window, cx);
                            }
                        }
                        Err(error) => {
                            this.status = format!("Save failed: {error}").into();
                            this.save_error = Some(error);
                        }
                    }
                }
                this.start_pending_save(window, cx);
                if this.reload_requested {
                    this.refresh_from_disk(true, window, cx);
                }
                cx.notify();
            })
        })
        .detach_and_log_err(cx);
    }

    pub(crate) fn save_generation(&self) -> u64 {
        self.save_generation
    }

    pub(crate) fn prepare_close(
        &self,
        cx: &mut Context<Self>,
    ) -> Task<Result<u64, String>> {
        let generation = self.save_generation;
        let workspace = self.workspace.clone();
        cx.spawn(async move |this, cx| {
            for poll in 0..=500 {
                let result = this.read_with(cx, |this, _| {
                    if this.save_generation != generation || this.workspace != workspace {
                        Some(Err("Settings changed while preparing to close. Review them and try again.".into()))
                    } else if this.save_in_flight || this.pending_save.is_some() {
                        None
                    } else {
                        Some(this.save_error.as_ref().map_or(Ok(generation), |error| {
                            Err(format!("Settings were not saved: {error}"))
                        }))
                    }
                }).map_err(|error| error.to_string())?;
                if let Some(result) = result {
                    return result;
                }
                if poll < 500 {
                    cx.background_executor().timer(std::time::Duration::from_millis(10)).await;
                }
            }
            Err("Settings save is still pending. Keep this window open and try again.".into())
        })
    }

    pub fn setup_required(&self) -> bool {
        self.setup_required
    }

    fn commit_selected_connection(&mut self, cx: &mut Context<Self>) {
        let Some(connection) = self.connections.get_mut(self.selected_connection)
        else {
            return;
        };
        connection.name = self.name_input.read(cx).value().to_string();
        connection.base_url = self.base_url_input.read(cx).value().to_string();
        connection.api_key = self.api_key_input.read(cx).value().to_string();
        connection.models = model_catalog(&self.model_input.read(cx).value());
        if let Some(model) = connection.models.first() {
            connection.model = model.clone();
        }
    }

    fn load_connection_into_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.model_discovery_generation =
            self.model_discovery_generation.wrapping_add(1);
        self.test_status = "Connection changed; discover models again".into();
        let Some(connection) = self.connections.get(self.selected_connection) else {
            return;
        };
        let name = connection.name.clone();
        let base_url = connection.base_url.clone();
        let api_key = connection.api_key.clone();
        let model = connection.models.join(", ");
        self.suppress_autosave = true;
        self.name_input
            .update(cx, |input, cx| input.set_value(&name, window, cx));
        self.base_url_input
            .update(cx, |input, cx| input.set_value(&base_url, window, cx));
        self.api_key_input.update(cx, |input, cx| {
            input.set_value(&api_key, window, cx);
            input.set_masked(true, window, cx);
        });
        self.model_input
            .update(cx, |input, cx| input.set_value(&model, window, cx));
        self.suppress_autosave = false;
    }

    pub fn watch_config_changes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(proxy) = self.proxy.as_ref() else {
            return;
        };
        let receiver = proxy.subscribe_workspace_file_changes();
        let workspace = self.workspace.clone();
        self._config_watch = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(paths) = receiver.recv().await {
                if !Self::is_provider_settings_change(&workspace, &paths) {
                    continue;
                }
                if this
                    .update_in(cx, |this, window, cx| {
                        this.refresh_from_disk(true, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn refresh_from_disk(
        &mut self,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reload_requested = true;
        if self.load_in_flight
            || self.save_in_flight
            || self.pending_save.is_some()
            || self.save_error.is_some()
        {
            return;
        }
        self.reload_requested = false;
        self.load_in_flight = true;
        let workspace = self.workspace.clone();
        let generation = self.save_generation;
        let modified = (self.loaded && !force).then_some(self.settings_modified);
        cx.spawn_in(window, async move |this, cx| {
            let load_workspace = workspace.clone();
            let loaded = cx
                .background_spawn(
                    async move { load_settings(&load_workspace, modified) },
                )
                .await;
            this.update_in(cx, |this, window, cx| {
                this.load_in_flight = false;
                if this.workspace == workspace && this.save_generation == generation
                {
                    if let Some(loaded) = loaded {
                        this.connections = loaded.connections;
                        this.discovered_models =
                            vec![Vec::new(); this.connections.len()];
                        let provider_ids = this
                            .connections
                            .iter()
                            .map(|connection| connection.provider_id.clone())
                            .collect::<HashSet<_>>();
                        this.expanded_model_providers.retain(|provider_id| {
                            provider_ids.contains(provider_id)
                        });
                        this.model_visibility_scrolls.retain(|provider_id, _| {
                            provider_ids.contains(provider_id)
                        });
                        this.selected_connection = loaded.selected_connection;
                        this.effective_source = loaded.source.into();
                        this.setup_required = loaded.setup_required;
                        this.settings_modified = loaded.modified;
                        this.status = loaded.status.into();
                        this.inline_blame = loaded.inline_blame;
                        this.load_connection_into_inputs(window, cx);
                        this.voice_provider = loaded.voice.provider;
                        this.voice_profile = loaded.voice.profile;
                        this.suppress_autosave = true;
                        this.voice_endpoint_input.update(cx, |input, cx| {
                            input.set_value(&loaded.voice.endpoint, window, cx);
                        });
                        this.voice_model_input.update(cx, |input, cx| {
                            input.set_value(&loaded.voice.model, window, cx);
                        });
                        this.voice_api_key_input.update(cx, |input, cx| {
                            input.set_value(&loaded.voice.api_key, window, cx);
                            input.set_masked(true, window, cx);
                        });
                        this.suppress_autosave = false;
                        this.loaded = true;
                        this.discover_models(cx);
                        if let Some(handler) = this.save_handler.clone() {
                            handler(window, cx);
                        }
                        cx.notify();
                    }
                }
                if this.reload_requested {
                    this.refresh_from_disk(true, window, cx);
                }
            })
        })
        .detach_and_log_err(cx);
    }

    pub fn select_connection(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.connections.len() {
            return;
        }
        self.commit_selected_connection(cx);
        self.selected_connection = index;
        self.load_connection_into_inputs(window, cx);
        self.test_status = "Connection selected".into();
        self.discover_models(cx);
        self.save_config(window, cx);
        cx.notify();
    }

    pub fn add_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.loaded {
            return;
        }
        self.commit_selected_connection(cx);
        let number = self.connections.len() + 1;
        self.connections.push(AiConnection {
            name: format!("Server {number}"),
            provider_id: provider_id_for(
                &format!("Server {number}"),
                "openai-compatible",
            ),
            base_url: "http://localhost:1234/v1".into(),
            api_key: String::new(),
            model: "deepseek-coder".into(),
            models: vec!["deepseek-coder".into()],
            hidden_models: Vec::new(),
        });
        self.discovered_models.push(Vec::new());
        self.selected_connection = self.connections.len() - 1;
        self.load_connection_into_inputs(window, cx);
        self.test_status = "New OpenAI-compatible server".into();
        self.save_config(window, cx);
        self.discover_models(cx);
        cx.notify();
    }

    pub fn remove_connection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.connections.len() <= 1 {
            return;
        }
        self.commit_selected_connection(cx);
        self.connections.remove(self.selected_connection);
        if self.selected_connection < self.discovered_models.len() {
            self.discovered_models.remove(self.selected_connection);
        }
        let provider_ids = self
            .connections
            .iter()
            .map(|connection| connection.provider_id.clone())
            .collect::<HashSet<_>>();
        self.expanded_model_providers
            .retain(|provider_id| provider_ids.contains(provider_id));
        self.model_visibility_scrolls
            .retain(|provider_id, _| provider_ids.contains(provider_id));
        self.selected_connection = self
            .selected_connection
            .min(self.connections.len().saturating_sub(1));
        self.load_connection_into_inputs(window, cx);
        self.test_status = "Connection removed".into();
        self.save_config(window, cx);
        self.discover_models(cx);
        cx.notify();
    }

    pub fn test_connection(&mut self, cx: &mut Context<Self>) {
        if !self.loaded {
            return;
        }
        let url = self.base_url_input.read(cx).value().to_string();
        let api_key = self.api_key_input.read(cx).value().to_string();
        if url.is_empty() {
            self.test_status = "Please enter an endpoint URL".into();
            cx.notify();
            return;
        }

        let endpoint = models_endpoint(&url);
        self.test_status = format!("Checking {endpoint}…").into();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result: Result<String, String> = cx
                .background_spawn(async move {
                    let client = reqwest::blocking::Client::builder()
                        .timeout(std::time::Duration::from_secs(5))
                        .build()
                        .map_err(|error| error.to_string())?;
                    let mut request = client.get(&endpoint);
                    if !api_key.trim().is_empty() {
                        request = request.bearer_auth(api_key);
                    }
                    let response =
                        request.send().map_err(|error| error.to_string())?;
                    Ok(model_endpoint_status(response.status()))
                })
                .await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                this.test_status = match result {
                    Ok(status) => status.into(),
                    Err(error) => format!("Endpoint unavailable · {error}").into(),
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub fn discover_models(&mut self, cx: &mut Context<Self>) {
        if !self.loaded || self.connections.is_empty() {
            return;
        }
        self.commit_selected_connection(cx);
        self.model_discovery_generation =
            self.model_discovery_generation.wrapping_add(1);
        let generation = self.model_discovery_generation;
        let connections = self.connections.clone();
        self.test_status =
            format!("Discovering models from {} provider(s)…", connections.len())
                .into();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result: Result<Vec<(usize, Result<Vec<String>, String>)>, String> = cx
                .background_spawn(async move {
                    let client = reqwest::blocking::Client::builder()
                        .timeout(std::time::Duration::from_secs(5))
                        .build()
                        .map_err(|error| error.to_string())?;
                    Ok(std::thread::scope(|scope| {
                        let requests = connections
                            .into_iter()
                            .enumerate()
                            .map(|(connection_index, connection)| {
                                let client = client.clone();
                                (
                                    connection_index,
                                    scope.spawn(move || {
                                        fetch_model_ids(
                                            &client,
                                            &connection.base_url,
                                            &connection.api_key,
                                        )
                                    }),
                                )
                            })
                            .collect::<Vec<_>>();
                        requests
                            .into_iter()
                            .map(|(connection_index, request)| {
                                (
                                    connection_index,
                                    request.join().unwrap_or_else(|_| {
                                        Err("Model discovery task failed".into())
                                    }),
                                )
                            })
                            .collect()
                    }))
                })
                .await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                let Some(result) = model_discovery_outcome(
                    this.model_discovery_generation,
                    generation,
                    result,
                ) else {
                    return;
                };
                match result {
                    Ok(provider_results) => {
                        let mut discovered_count = 0;
                        let mut unavailable_count = 0;
                        for (connection_index, result) in provider_results {
                            match result {
                                Ok(models) => {
                                    discovered_count += models.len();
                                    if let Some(catalog) =
                                        this.discovered_models.get_mut(connection_index)
                                    {
                                        *catalog = models;
                                    }
                                }
                                Err(_) => unavailable_count += 1,
                            }
                        }
                        this.test_status = if unavailable_count == 0 {
                            format!(
                                "Discovered {discovered_count} model(s) across {} provider(s) · all visible unless hidden below",
                                this.connections.len()
                            )
                        } else {
                            format!(
                                "Discovered {discovered_count} model(s); {unavailable_count} provider(s) unavailable"
                            )
                        }
                        .into();
                    }
                    Err(error) => {
                        this.test_status =
                            format!("Model discovery failed · {error}").into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_model_provider(
        &mut self,
        connection_index: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(provider_id) = self
            .connections
            .get(connection_index)
            .map(|connection| connection.provider_id.clone())
        else {
            return;
        };
        if !self.expanded_model_providers.insert(provider_id.clone()) {
            self.expanded_model_providers.remove(&provider_id);
        }
        cx.notify();
    }

    fn set_model_visibility(
        &mut self,
        connection_index: usize,
        model: String,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.loaded {
            return;
        }
        self.commit_selected_connection(cx);
        let Some(connection) = self.connections.get_mut(connection_index) else {
            return;
        };
        let was_hidden = connection
            .hidden_models
            .iter()
            .any(|hidden| hidden == &model);
        let changed = if visible {
            if was_hidden {
                connection.hidden_models.retain(|hidden| hidden != &model);
                true
            } else {
                false
            }
        } else if was_hidden {
            false
        } else {
            connection.hidden_models.push(model.clone());
            true
        };
        if changed {
            self.test_status = if visible {
                format!("{model} will appear in the chat picker").into()
            } else {
                format!("{model} hidden from the chat picker").into()
            };
            self.save_config(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActiveTheme, AiConnection, HashSet, McpServerDeclaration, ModelCatalogEntry,
        ModelVisibilityModel, ModelVisibilityProvider, SettingsPanel,
        discovered_model_catalog, ensure_settings_file, has_ai_settings,
        lock_settings_file, model_catalog, model_discovery_outcome,
        model_endpoint_status, model_visibility_providers,
        model_visibility_scroll_area, models_endpoint, parse_connections,
        parse_toml_value, resolve_config_value, serialize_ai_config,
        write_user_config,
    };
    use fs4::fs_std::FileExt;
    use gpui_kit::{
        AppContext, Element, InteractiveElement, IntoElement, ParentElement, Styled,
    };
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn inline_blame_defaults_on_and_persists_with_ai_settings() {
        let workspace = tempfile::tempdir().expect("workspace");
        let settings = workspace.path().join(".ahead/settings.toml");
        assert!(super::inline_blame_enabled(workspace.path()));
        std::fs::create_dir_all(settings.parent().expect("settings directory"))
            .expect("create settings directory");
        std::fs::write(&settings, "[editor]\nwrap-style = 'editor-width'\n")
            .expect("write settings");
        super::save_ai_config(
            workspace.path(),
            "[ai]\nmodel = 'test'\n",
            &super::VoiceSettings::default(),
            false,
        )
        .expect("save settings");
        assert!(!super::inline_blame_enabled(workspace.path()));
        let saved = std::fs::read_to_string(settings).expect("read settings");
        let config = parse_toml_value(&saved).expect("valid settings");
        assert_eq!(
            config["editor"]["wrap-style"].as_str(),
            Some("editor-width")
        );
        assert_eq!(config["ai"]["model"].as_str(), Some("test"));
    }

    #[test]
    fn reasoning_effort_preferences_are_user_scoped_and_replaceable() {
        let user_home = tempfile::tempdir().expect("user home");
        let settings_path = user_home.path().join(".ahead/settings.toml");
        std::fs::create_dir_all(settings_path.parent().expect("settings parent"))
            .expect("create settings directory");
        std::fs::write(&settings_path, "[core]\ntheme = 'dark'\n")
            .expect("write unrelated user setting");

        super::save_model_reasoning_preference_with_home(
            user_home.path(),
            "provider-a",
            "model-a",
            Some("high"),
        )
        .expect("save effort preference");
        super::save_model_reasoning_preference_with_home(
            user_home.path(),
            "provider-a",
            "model-b",
            Some("low"),
        )
        .expect("save second model preference");

        let stored =
            std::fs::read_to_string(&settings_path).expect("read user settings");
        let config = super::parse_toml_value(&stored).expect("parse user settings");
        let preferences = super::model_reasoning_preferences(&config);
        assert_eq!(config["core"]["theme"].as_str(), Some("dark"));
        assert_eq!(
            preferences
                .get(&("provider-a".into(), "model-a".into()))
                .map(String::as_str),
            Some("high")
        );
        assert_eq!(
            preferences
                .get(&("provider-a".into(), "model-b".into()))
                .map(String::as_str),
            Some("low")
        );

        super::save_model_reasoning_preference_with_home(
            user_home.path(),
            "provider-a",
            "model-a",
            None,
        )
        .expect("restore model default");
        let stored =
            std::fs::read_to_string(settings_path).expect("read updated settings");
        let config =
            super::parse_toml_value(&stored).expect("parse updated settings");
        let preferences = super::model_reasoning_preferences(&config);
        assert!(!preferences.contains_key(&("provider-a".into(), "model-a".into())));
        assert_eq!(
            preferences
                .get(&("provider-a".into(), "model-b".into()))
                .map(String::as_str),
            Some("low")
        );
    }

    #[test]
    fn first_settings_load_bootstraps_private_workspace_config_once() {
        let workspace = tempfile::tempdir().expect("workspace");
        let settings = SettingsPanel::config_path(workspace.path());
        assert!(!settings.exists());

        let loaded =
            super::load_settings_with_user_home(workspace.path(), None, None)
                .expect("initial settings load");

        assert!(settings.is_file());
        assert_eq!(
            std::fs::read_to_string(&settings).expect("bootstrapped settings"),
            super::DEFAULT_SETTINGS
        );
        assert!(loaded.setup_required);
        assert!(loaded.status.contains("Complete AHEAD setup"));
        assert!(
            super::load_settings_with_user_home(
                workspace.path(),
                Some(loaded.modified),
                None,
            )
            .is_none(),
            "an unchanged workspace should not bootstrap or reload again"
        );
    }

    #[test]
    fn voice_settings_round_trip_with_ai_settings() {
        let workspace = tempfile::tempdir().expect("workspace");
        let voice = super::VoiceSettings {
            provider: "self-hosted".into(),
            profile: "nemotron-voicechat".into(),
            endpoint: "https://voice.example.com/v1".into(),
            model: "voice-model-with-tools".into(),
            api_key: "private-voice-token".into(),
        };

        super::save_ai_config(
            workspace.path(),
            "[ai]\nmodel = 'test'\n",
            &voice,
            true,
        )
        .expect("save settings");

        assert_eq!(super::load_voice_settings(workspace.path()), voice);
    }

    #[test]
    fn voice_defaults_keep_openai_cloud_and_choose_venus_for_self_hosted_evaluation()
    {
        let default = super::VoiceSettings::default();
        assert_eq!(default.provider, "openai-live");
        assert_eq!(default.profile, "realtime-venus");

        let workspace = tempfile::tempdir().expect("workspace");
        let settings_directory = workspace.path().join(".ahead");
        std::fs::create_dir_all(&settings_directory)
            .expect("create settings directory");
        std::fs::write(
            settings_directory.join("settings.toml"),
            "[voice]\nprovider = 'self-hosted'\n",
        )
        .expect("write settings");

        let loaded = super::load_voice_settings(workspace.path());
        assert_eq!(loaded.provider, "self-hosted");
        assert_eq!(loaded.profile, "realtime-venus");
    }

    #[test]
    fn unsupported_voice_provider_is_preserved_for_error_reporting() {
        let workspace = tempfile::tempdir().expect("workspace");
        let voice = super::VoiceSettings {
            provider: "unknown-provider".into(),
            ..super::VoiceSettings::default()
        };
        super::save_ai_config(
            workspace.path(),
            "[ai]\nmodel = 'test'\n",
            &voice,
            true,
        )
        .expect("save settings");

        assert_eq!(
            super::load_voice_settings(workspace.path()).provider,
            "unknown-provider"
        );
    }

    #[test]
    fn settings_lock_serializes_independent_writers() {
        let directory = tempfile::tempdir().expect("settings directory");
        let path = directory.path().join("settings.toml");
        let first = lock_settings_file(&path).expect("first writer lock");
        // A concurrent fork can retain this open-file description until exec.
        let inherited_handle = first.0.try_clone().expect("inherited lock handle");
        let second = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(directory.path().join("settings.toml.lock"))
            .expect("second writer handle");
        assert!(!second.try_lock_exclusive().expect("try second lock"));
        drop(first);
        assert!(second.try_lock_exclusive().expect("retry second lock"));
        drop(inherited_handle);
        drop(second);
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let lock_path = directory.path().join("settings.toml.lock");
            let outside = directory.path().join("outside.lock");
            std::fs::write(&outside, "untouched").expect("outside lock target");
            std::fs::remove_file(&lock_path).expect("remove unlocked lock");
            symlink(&outside, lock_path).expect("symlink settings lock");
            assert!(lock_settings_file(&path).is_err());
            assert_eq!(
                std::fs::read_to_string(outside).expect("outside lock unchanged"),
                "untouched"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn settings_panel_does_not_load_symlinked_provider_settings() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().expect("workspace");
        let ahead = workspace.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("settings directory");
        std::fs::write(
            ahead.join("config.toml"),
            "[ai]\nbase_url = 'http://127.0.0.1:1234/v1'\nmodel = 'safe'\n",
        )
        .expect("shared settings");
        let outside = workspace.path().join("outside.toml");
        std::fs::write(
            &outside,
            "[ai]\nbase_url = 'https://outside.example/v1'\napi_key = 'outside-secret'\n",
        )
        .expect("outside settings");
        let settings = ahead.join("settings.toml");
        symlink(&outside, &settings).expect("symlink settings");

        assert!(ensure_settings_file(&settings).is_err());
        assert!(!has_ai_settings(workspace.path()));
        let (connections, _, errors) =
            SettingsPanel::load_saved_config_with_user_home(workspace.path(), None);
        assert_eq!(connections[0].base_url, "http://127.0.0.1:1234/v1");
        assert!(connections[0].api_key.is_empty());
        assert!(errors.iter().any(|error| error.contains("settings.toml")));
        std::fs::write(ahead.join("config.toml"), "[ai\n")
            .expect("write malformed shared settings");
        let (_, _, errors) =
            SettingsPanel::load_saved_config_with_user_home(workspace.path(), None);
        assert!(errors.iter().any(|error| error.contains("invalid TOML")));
    }

    #[test]
    fn scalar_endpoint_override_does_not_inherit_workspace_api_key() {
        let workspace = tempfile::tempdir().expect("workspace");
        let ahead = workspace.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("settings directory");
        std::fs::write(
            ahead.join("settings.toml"),
            "[ai]\nbase_url = 'https://private.example/v1'\napi_key = 'private-key'\nmodel = 'private-model'\n",
        )
        .expect("private settings");
        let shared_config = ahead.join("config.toml");
        std::fs::write(
            &shared_config,
            "[ai]\nbase_url = 'https://project.example/v1'\nmodel = 'project-model'\n",
        )
        .expect("shared settings");

        let (connections, _, errors) =
            SettingsPanel::load_saved_config_with_user_home(workspace.path(), None);
        assert!(errors.is_empty());
        assert_eq!(connections[0].base_url, "https://project.example/v1");
        assert!(connections[0].api_key.is_empty());

        std::fs::write(
            &shared_config,
            "[ai]\nbase_url = 'https://private.example/v1'\nmodel = 'project-model'\n",
        )
        .expect("matching endpoint override");
        let (connections, _, errors) =
            SettingsPanel::load_saved_config_with_user_home(workspace.path(), None);
        assert!(errors.is_empty());
        assert_eq!(connections[0].api_key, "private-key");
    }

    #[test]
    fn user_ai_defaults_are_visible_and_workspace_profiles_override_by_provider() {
        let workspace = tempfile::tempdir().expect("workspace");
        let user_home = tempfile::tempdir().expect("user home");
        let user_ahead = user_home.path().join(".ahead");
        std::fs::create_dir(&user_ahead).expect("user settings directory");
        std::fs::write(
            user_ahead.join("settings.toml"),
            "[ai]\nactive_connection = 'Shared user'\n[[ai.connections]]\nname = 'Shared user'\nprovider_id = 'shared'\nbase_url = 'https://provider.example/v1'\napi_key = 'private-token'\nmodel = 'old-model'\nmodels = ['old-model', 'hidden-model']\nhidden_models = ['hidden-model']\n[[ai.connections]]\nname = 'Personal'\nprovider_id = 'personal'\nbase_url = 'https://personal.example/v1'\napi_key = 'personal-token'\nmodels = ['personal-model']\n",
        )
        .expect("user defaults");
        let workspace_ahead = workspace.path().join(".ahead");
        std::fs::create_dir(&workspace_ahead).expect("workspace settings directory");
        std::fs::write(
            workspace_ahead.join("settings.toml"),
            "[ai]\nactive_connection = 'Shared workspace'\n[[ai.connections]]\nname = 'Shared workspace'\nprovider_id = 'shared'\nbase_url = 'https://provider.example/v1'\nmodels = ['new-model', 'hidden-model']\nhidden_models = []\n[[ai.connections]]\nname = 'Workspace'\nprovider_id = 'workspace'\nbase_url = 'https://workspace.example/v1'\nmodels = ['workspace-model']\n",
        )
        .expect("workspace overrides");

        let (connections, source, errors) =
            SettingsPanel::load_saved_config_with_user_home(
                workspace.path(),
                Some(user_home.path()),
            );

        assert!(errors.is_empty());
        assert_eq!(source, "workspace .ahead/settings.toml");
        assert_eq!(connections.len(), 3);
        assert_eq!(connections[0].provider_id, "shared");
        assert_eq!(connections[0].name, "Shared workspace");
        assert_eq!(connections[0].api_key, "private-token");
        assert_eq!(
            connections[0].models,
            vec!["new-model".to_string(), "hidden-model".to_string()]
        );
        assert_eq!(connections[1].provider_id, "personal");
        assert_eq!(connections[2].provider_id, "workspace");
        assert_eq!(
            SettingsPanel::active_connection_index_with_user_home(
                workspace.path(),
                &connections,
                Some(user_home.path()),
            ),
            0
        );
        assert!(
            SettingsPanel::settings_modified_with_user_home(
                workspace.path(),
                Some(user_home.path()),
            )[3]
            .is_some()
        );
    }

    #[test]
    fn absent_user_settings_home_is_an_optional_config_layer() {
        let workspace = tempfile::tempdir().expect("workspace");
        let user_home = workspace.path().join("not-created-yet");
        let mut errors = Vec::new();

        let configs = SettingsPanel::read_ai_config_layers(
            workspace.path(),
            Some(&user_home),
            &mut errors,
        );

        assert!(configs.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn provider_settings_change_filter_includes_user_and_workspace_files_only() {
        let workspace = tempfile::tempdir().expect("workspace");
        let user_home = tempfile::tempdir().expect("user home");
        let user_home_path =
            user_home.path().canonicalize().expect("canonical home");
        let settings_directory = workspace.path().join(".ahead");
        std::fs::create_dir(&settings_directory).expect("workspace settings");
        let user_settings_directory = user_home_path.join(".ahead");
        std::fs::create_dir(&user_settings_directory)
            .expect("user settings directory");
        let settings = settings_directory.join("settings.toml");
        let shared = settings_directory.join("config.toml");
        let user_settings = user_settings_directory.join("settings.toml");
        let unrelated = workspace.path().join("src/main.rs");

        assert!(SettingsPanel::is_provider_settings_change_with_user_home(
            workspace.path(),
            std::slice::from_ref(&settings),
            Some(&user_home_path),
        ));
        assert!(SettingsPanel::is_provider_settings_change_with_user_home(
            workspace.path(),
            std::slice::from_ref(&shared),
            Some(&user_home_path),
        ));
        assert!(SettingsPanel::is_provider_settings_change_with_user_home(
            workspace.path(),
            std::slice::from_ref(&user_settings),
            Some(&user_home_path),
        ));
        assert!(!SettingsPanel::is_provider_settings_change_with_user_home(
            workspace.path(),
            std::slice::from_ref(&unrelated),
            Some(&user_home_path),
        ));
    }

    #[gpui_kit::test]
    fn renders_tracked_mcp_declaration_without_starting_it(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("workspace");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            SettingsPanel::new(workspace.path().to_path_buf(), window, cx)
        });
        panel.update(cx, |panel, cx| {
            panel.mcp_declarations = vec![McpServerDeclaration {
                id: "docs".into(),
                declared: true,
                declaration_toml: "command = 'echo'\nargs = ['docs']\n".into(),
                fingerprint: format!("sha256:{}", "0".repeat(64)),
                enabled: false,
                approved: false,
                auto_approve_all: false,
            }];
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.update(cx, |panel, cx| {
            panel.mcp_declarations[0].enabled = true;
            panel.mcp_declarations[0].approved = true;
            panel.mcp_declarations[0].auto_approve_all = true;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.update(cx, |panel, _| {
            assert_eq!(panel.mcp_declarations[0].id, "docs");
        });
    }

    #[gpui_kit::test]
    fn model_list_scroll_does_not_scroll_settings_page(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let model_scroll = gpui_kit::UniformListScrollHandle::new();
        let page_scroll = gpui_kit::ListState::new(
            20,
            gpui_kit::ListAlignment::Top,
            gpui_kit::px(40.),
        );

        struct ModelListView {
            model_scroll: gpui_kit::UniformListScrollHandle,
        }

        impl gpui_kit::Render for ModelListView {
            fn render(
                &mut self,
                _window: &mut gpui_kit::Window,
                cx: &mut gpui_kit::Context<Self>,
            ) -> impl gpui_kit::IntoElement {
                model_visibility_scroll_area(
                    &self.model_scroll,
                    40,
                    gpui_kit::uniform_list(
                        "model-list-scroll-test",
                        40,
                        cx.processor(
                            |_this, range: std::ops::Range<usize>, _window, _cx| {
                                range
                                    .map(|index| {
                                        gpui_kit::component::h_flex()
                                            .w_full()
                                            .h(gpui_kit::px(super::MODEL_VISIBILITY_ROW_HEIGHT))
                                            .justify_between()
                                            .child(gpui_kit::div().child("model"))
                                            .child(
                                                gpui_kit::component::switch::Switch::new(
                                                    format!("model-row-switch-{index}"),
                                                ),
                                            )
                                            .into_any()
                                    })
                                    .collect::<Vec<_>>()
                            },
                        ),
                    )
                    .track_scroll(&self.model_scroll)
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .size_full(),
                    cx.theme().border.opacity(0.35),
                    cx.theme().scrollbar_thumb,
                    cx.theme().scrollbar_thumb_hover,
                )
            }
        }

        struct NestedScrollView {
            page_scroll: gpui_kit::ListState,
            model_list: gpui_kit::Entity<ModelListView>,
        }

        impl gpui_kit::Render for NestedScrollView {
            fn render(
                &mut self,
                _window: &mut gpui_kit::Window,
                _cx: &mut gpui_kit::Context<Self>,
            ) -> impl gpui_kit::IntoElement {
                let model_list = self.model_list.clone();
                gpui_kit::list(self.page_scroll.clone(), move |index, _, _| {
                    if index == 1 {
                        model_list.clone().into_any_element()
                    } else {
                        gpui_kit::div()
                            .w_full()
                            .h(gpui_kit::px(40.))
                            .into_any_element()
                    }
                })
                .w(gpui_kit::px(400.))
                .h(gpui_kit::px(400.))
            }
        }

        let (_, cx) = cx.add_window_view(|_, cx| {
            let model_list = cx.new(|_| ModelListView {
                model_scroll: model_scroll.clone(),
            });
            NestedScrollView {
                page_scroll: page_scroll.clone(),
                model_list,
            }
        });
        cx.update(|window, cx| _ = window.draw(cx));
        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: gpui_kit::point(gpui_kit::px(50.), gpui_kit::px(100.)),
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.),
                gpui_kit::px(-40.),
            )),
            ..Default::default()
        });

        assert_ne!(
            model_scroll.0.borrow().base_handle.offset().y,
            gpui_kit::px(0.)
        );
        let page_top = page_scroll.logical_scroll_top();
        assert_eq!(
            (page_top.item_ix, page_top.offset_in_item),
            (0, gpui_kit::px(0.))
        );

        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: gpui_kit::point(gpui_kit::px(50.), gpui_kit::px(350.)),
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.),
                gpui_kit::px(-40.),
            )),
            ..Default::default()
        });
        let page_top = page_scroll.logical_scroll_top();
        assert_ne!(
            (page_top.item_ix, page_top.offset_in_item),
            (0, gpui_kit::px(0.))
        );
        cx.run_until_parked();
        cx.update(|window, cx| _ = window.draw(cx));

        let page_top_before_model_scroll =
            (page_top.item_ix, page_top.offset_in_item);
        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: gpui_kit::point(gpui_kit::px(50.), gpui_kit::px(100.)),
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.),
                gpui_kit::px(40.),
            )),
            ..Default::default()
        });
        assert_eq!(
            model_scroll.0.borrow().base_handle.offset().y,
            gpui_kit::px(0.)
        );
        let page_top = page_scroll.logical_scroll_top();
        assert_eq!(
            (page_top.item_ix, page_top.offset_in_item),
            page_top_before_model_scroll
        );

        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: gpui_kit::point(gpui_kit::px(370.), gpui_kit::px(100.)),
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.),
                gpui_kit::px(-40.),
            )),
            ..Default::default()
        });
        assert_ne!(
            model_scroll.0.borrow().base_handle.offset().y,
            gpui_kit::px(0.)
        );
        let page_top = page_scroll.logical_scroll_top();
        assert_eq!(
            (page_top.item_ix, page_top.offset_in_item),
            page_top_before_model_scroll
        );

        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: gpui_kit::point(gpui_kit::px(370.), gpui_kit::px(100.)),
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.),
                gpui_kit::px(40.),
            )),
            ..Default::default()
        });
        assert_eq!(
            model_scroll.0.borrow().base_handle.offset().y,
            gpui_kit::px(0.)
        );
        let page_top = page_scroll.logical_scroll_top();
        assert_eq!(
            (page_top.item_ix, page_top.offset_in_item),
            page_top_before_model_scroll
        );

        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: gpui_kit::point(gpui_kit::px(50.), gpui_kit::px(100.)),
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.),
                gpui_kit::px(40.),
            )),
            ..Default::default()
        });
        let page_top = page_scroll.logical_scroll_top();
        assert_eq!(
            (page_top.item_ix, page_top.offset_in_item),
            page_top_before_model_scroll
        );

        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: gpui_kit::point(gpui_kit::px(50.), gpui_kit::px(100.)),
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.),
                gpui_kit::px(-2_000.),
            )),
            ..Default::default()
        });
        let model_bottom_offset = model_scroll.0.borrow().base_handle.offset().y;
        assert!(model_bottom_offset < gpui_kit::px(0.));
        let page_top = page_scroll.logical_scroll_top();
        assert_eq!(
            (page_top.item_ix, page_top.offset_in_item),
            page_top_before_model_scroll
        );

        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: gpui_kit::point(gpui_kit::px(50.), gpui_kit::px(100.)),
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.),
                gpui_kit::px(-40.),
            )),
            ..Default::default()
        });
        assert_eq!(
            model_scroll.0.borrow().base_handle.offset().y,
            model_bottom_offset
        );
        let page_top = page_scroll.logical_scroll_top();
        assert_eq!(
            (page_top.item_ix, page_top.offset_in_item),
            page_top_before_model_scroll
        );
    }

    #[gpui_kit::test]
    fn shows_invalid_provider_layer_in_settings(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("workspace");
        let ahead = workspace.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("settings directory");
        std::fs::write(ahead.join("settings.toml"), "[ai]\nmodel = 'safe'\n")
            .expect("private settings");
        let shared = ahead.join("config.toml");
        std::fs::write(&shared, "[ai\n").expect("invalid shared settings");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&shared)
            .expect("shared settings handle")
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
            .expect("fixed initial mtime");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            SettingsPanel::new(workspace.path().to_path_buf(), window, cx)
        });
        let reloads = Rc::new(Cell::new(0));
        cx.run_until_parked();
        panel.update(cx, |panel, _| {
            let reloads = reloads.clone();
            panel.set_save_handler(move |_, _| reloads.set(reloads.get() + 1));
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.update(cx, |panel, _| {
            assert!(panel.status.to_string().contains("config.toml"));
            assert!(panel.status.to_string().contains("invalid TOML"));
        });

        std::fs::write(&shared, "[ai]\nmodel = 'repaired'\n")
            .expect("repair shared settings");
        panel.update_in(cx, |panel, window, cx| {
            panel.refresh_from_disk(true, window, cx)
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.update(cx, |panel, _| {
            assert!(panel.status.to_string().contains("Settings reloaded"));
        });
        assert_eq!(reloads.get(), 1, "chat model picker should reload");

        std::fs::write(&shared, "[ai]\napi_key = 'tracked-secret'\n")
            .expect("credential in shared settings");
        panel.update_in(cx, |panel, window, cx| {
            panel.refresh_from_disk(true, window, cx)
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.update(cx, |panel, _| {
            assert!(panel.status.to_string().contains("credential field"));
        });
    }

    #[test]
    fn settings_replacement_is_private_and_rejects_symlink_targets() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        std::fs::write(&path, "old secret").unwrap();
        write_user_config(&path, "new secret").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new secret");

        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};

            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let link = directory.path().join("link.toml");
            symlink(&path, &link).unwrap();
            assert!(write_user_config(&link, "redirected").is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "new secret");
        }
    }

    #[gpui_kit::test(iterations = 5)]
    fn startup_renders_under_lock_contention_without_saving_loading_defaults(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let cx = cx.add_empty_window();
        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::create_dir(workspace.path().join(".ahead"))
            .expect("settings directory");
        let path = SettingsPanel::config_path(workspace.path());
        let original = "[ai]\nbase_url = 'http://localhost:1234/v1'\nmodel = 'existing-model'\napi_key = 'fixture-secret'\n";
        std::fs::write(&path, original).expect("existing settings");
        let lock = lock_settings_file(&path).expect("contended bootstrap lock");
        let (release, released) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let responsive = released
                .recv_timeout(std::time::Duration::from_secs(5))
                .is_ok();
            drop(lock);
            responsive
        });
        // add_window_view pumps background tasks before returning, so install
        // the view directly to inspect foreground progress before releasing IO.
        let panel = cx.update(|window, cx| {
            window.replace_root(cx, |window, cx| {
                SettingsPanel::new(workspace.path().to_path_buf(), window, cx)
            })
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.save_config(window, cx);
            panel.add_connection(window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let loading = panel.update(cx, |panel, _| {
            !panel.loaded
                && panel.load_in_flight
                && panel.connections.is_empty()
                && panel.save_generation == 0
                && !panel.save_in_flight
                && panel.status.contains("Loading")
        });
        let released_in_time = release.send(()).is_ok();
        assert!(
            holder.join().expect("lock holder"),
            "Settings construction blocked the UI"
        );
        assert!(released_in_time);
        assert!(
            loading,
            "loading values must not become an editable/saved profile"
        );
        cx.run_until_parked();
        panel.update(cx, |panel, cx| {
            assert!(panel.loaded);
            assert_eq!(panel.model_input.read(cx).value(), "existing-model");
            assert_eq!(panel.api_key_input.read(cx).value(), "fixture-secret");
            assert!(panel.api_key_input.read(cx).presentation().is_masked());
            assert_eq!(
                panel.save_generation, 0,
                "loading must not trigger autosave"
            );
        });
        assert_eq!(
            std::fs::read_to_string(path).expect("unchanged settings"),
            original
        );
        panel.update_in(cx, |panel, window, cx| {
            panel.api_key_input.update(cx, |input, cx| {
                input.set_masked(false, window, cx);
            });
            assert!(!panel.api_key_input.read(cx).presentation().is_masked());
            panel.load_connection_into_inputs(window, cx);
            assert!(panel.api_key_input.read(cx).presentation().is_masked());
        });
    }

    #[gpui_kit::test(iterations = 5)]
    fn workspace_notifications_reload_settings_without_render_time_io(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("workspace");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            workspace.path().to_owned(),
        );
        let (panel, cx) = cx.add_window_view(|window, cx| {
            SettingsPanel::new(workspace.path().to_owned(), window, cx)
                .with_proxy(proxy.clone())
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.watch_config_changes(window, cx)
        });
        cx.run_until_parked();
        let reloads = Rc::new(Cell::new(0));
        panel.update(cx, |panel, _| {
            let reloads = reloads.clone();
            panel.set_save_handler(move |_, _| reloads.set(reloads.get() + 1));
        });
        let shared = workspace.path().join(".ahead/config.toml");
        std::fs::write(&shared, "[ai]\nmodel = 'external-model'\n")
            .expect("external change");
        proxy.route_core(ahead_rpc::core::CoreNotification::WorkspaceFileChange {
            generation: 1,
            paths: Vec::new(),
        });
        cx.run_until_parked();
        panel.update(cx, |panel, cx| {
            assert_eq!(panel.model_input.read(cx).value(), "external-model");
        });
        assert_eq!(reloads.get(), 1);
        for _ in 0..3 {
            proxy.route_core(
                ahead_rpc::core::CoreNotification::WorkspaceFileChange {
                    generation: 1,
                    paths: Vec::new(),
                },
            );
        }
        cx.run_until_parked();
        assert_eq!(
            reloads.get(),
            1,
            "unchanged settings must not reload the picker"
        );
        std::fs::remove_file(shared).expect("delete disposable shared config");
        proxy.route_core(ahead_rpc::core::CoreNotification::WorkspaceFileChange {
            generation: 1,
            paths: Vec::new(),
        });
        cx.run_until_parked();
        panel.update(cx, |panel, cx| {
            assert_ne!(panel.model_input.read(cx).value(), "external-model");
        });
        assert_eq!(reloads.get(), 2);
    }

    #[gpui_kit::test(iterations = 20)]
    fn late_reload_cannot_replace_a_newer_settings_edit(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("workspace");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            SettingsPanel::new(workspace.path().to_owned(), window, cx)
        });
        cx.run_until_parked();
        let path = SettingsPanel::config_path(workspace.path());
        std::fs::write(&path, "[ai]\nmodel = 'older-external-model'\n")
            .expect("external change");
        let reloads = Rc::new(Cell::new(0));
        panel.update_in(cx, |panel, window, cx| {
            let reloads = reloads.clone();
            panel.set_save_handler(move |_, _| reloads.set(reloads.get() + 1));
            panel.refresh_from_disk(true, window, cx);
            panel
                .model_input
                .update(cx, |input, cx| input.set_value("newer-edit", window, cx));
            panel.save_config(window, cx);
        });
        cx.run_until_parked();
        panel.update(cx, |panel, cx| {
            assert_eq!(panel.model_input.read(cx).value(), "newer-edit");
            assert!(panel.status.contains("Saved workspace"));
            assert!(!panel.load_in_flight && !panel.save_in_flight);
        });
        assert_eq!(
            reloads.get(),
            1,
            "only the current save may refresh the picker"
        );
        let saved = std::fs::read_to_string(path).expect("saved settings");
        assert_eq!(
            parse_toml_value(&saved).expect("valid settings")["ai"]["model"]
                .as_str(),
            Some("newer-edit")
        );
    }

    #[gpui_kit::test(iterations = 5)]
    fn saves_coalesce_without_blocking_render_or_losing_mcp_changes(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("workspace");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            SettingsPanel::new(workspace.path().to_path_buf(), window, cx)
        });
        let path = SettingsPanel::config_path(workspace.path());
        cx.run_until_parked();
        let lock = lock_settings_file(&path).expect("concurrent MCP writer");
        let (release, released) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            // A synchronous regression must fail instead of hanging the suite.
            let responsive = released
                .recv_timeout(std::time::Duration::from_secs(5))
                .is_ok();
            let current = std::fs::read_to_string(&path).expect("settings");
            write_user_config(
                &path,
                &format!("{current}\n[mcp]\nenabled_servers = ['docs']\n"),
            )
            .expect("concurrent MCP change");
            drop(lock);
            responsive
        });
        let reloads = Rc::new(Cell::new(0));
        panel.update_in(cx, |panel, window, cx| {
            let reloads = reloads.clone();
            panel.set_save_handler(move |_, _| reloads.set(reloads.get() + 1));
            for model in ["first", "intermediate", "latest"] {
                panel.model_input.update(cx, |input, cx| {
                    input.set_value(model, window, cx);
                });
                panel.save_config(window, cx);
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let draft_preserved = panel.update(cx, |panel, cx| {
            panel.save_in_flight
                && panel.pending_save.as_ref().is_some_and(|save| {
                    save.generation == panel.save_generation
                        && save.ai_config.contains("latest")
                        && !save.ai_config.contains("intermediate")
                })
                && panel.model_input.read(cx).value() == "latest"
                && panel.status.contains("Saving")
        });
        let released_in_time = release.send(()).is_ok();
        assert!(writer.join().expect("writer"), "UI blocked on the lock");
        assert!(released_in_time, "writer hit its safety timeout");
        assert!(draft_preserved, "render must keep the newest pending edit");
        cx.run_until_parked();
        panel.update(cx, |panel, _| {
            assert!(!panel.save_in_flight);
            assert!(panel.pending_save.is_none());
            assert!(panel.save_error.is_none());
            assert!(panel.status.contains("Saved workspace"));
        });
        assert_eq!(reloads.get(), 1, "ignore superseded save completions");
        let saved =
            std::fs::read_to_string(SettingsPanel::config_path(workspace.path()))
                .expect("saved settings");
        let saved = parse_toml_value(&saved).expect("valid settings");
        assert_eq!(saved["ai"]["model"].as_str(), Some("latest"));
        assert_eq!(saved["mcp"]["enabled_servers"][0].as_str(), Some("docs"));
    }

    #[gpui_kit::test(iterations = 5)]
    fn shared_config_change_during_save_is_not_marked_as_already_loaded(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("workspace");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            SettingsPanel::new(workspace.path().to_owned(), window, cx)
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.save_config(window, cx);
            std::fs::write(workspace.path().join(".ahead/config.toml"), "[ai\n")
                .expect("shared settings change while save is in flight");
            panel.refresh_from_disk(true, window, cx);
            assert!(panel.reload_requested);
        });
        cx.run_until_parked();
        panel.update(cx, |panel, _| {
            assert!(
                !panel.save_in_flight
                    && !panel.load_in_flight
                    && !panel.reload_requested
            );
            assert!(panel.status.contains("invalid TOML"));
            assert!(panel.status.contains("config.toml"));
            assert!(panel.settings_modified[1].is_some());
        });
    }

    #[gpui_kit::test(iterations = 5)]
    async fn failed_save_preserves_draft_blocks_close_and_can_retry(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("workspace");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            SettingsPanel::new(workspace.path().to_path_buf(), window, cx)
        });
        let path = SettingsPanel::config_path(workspace.path());
        cx.run_until_parked();
        std::fs::write(&path, "[invalid\n").expect("malformed file");
        panel.update_in(cx, |panel, window, cx| {
            panel.model_input.update(cx, |input, cx| {
                input.set_value("unsaved-model", window, cx);
            });
            panel.save_config(window, cx);
        });
        cx.run_until_parked();
        panel.update_in(cx, |panel, window, cx| {
            panel.refresh_from_disk(true, window, cx)
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.update(cx, |panel, cx| {
            assert!(panel.save_error.is_some());
            assert!(panel.reload_requested, "failed draft defers the reload");
            assert!(panel.status.contains("Save failed"));
            assert_eq!(panel.model_input.read(cx).value(), "unsaved-model");
        });
        let close = panel.update(cx, |panel, cx| panel.prepare_close(cx));
        assert!(
            close
                .await
                .expect_err("keep window open")
                .contains("not saved")
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("untouched file"),
            "[invalid\n"
        );

        std::fs::write(&path, super::DEFAULT_SETTINGS).expect("repair file");
        panel.update_in(cx, |panel, window, cx| panel.save_config(window, cx));
        let close = panel.update(cx, |panel, cx| panel.prepare_close(cx));
        assert_eq!(
            close.await.expect("saved before close"),
            panel.update(cx, |panel, _| panel.save_generation)
        );
        let saved = std::fs::read_to_string(path).expect("saved retry");
        assert_eq!(
            parse_toml_value(&saved).expect("valid settings")["ai"]["model"]
                .as_str(),
            Some("unsaved-model")
        );
    }

    #[gpui_kit::test]
    async fn pending_save_timeout_and_new_edits_cancel_close(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("workspace");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            SettingsPanel::new(workspace.path().to_path_buf(), window, cx)
        });
        let close = panel.update(cx, |panel, cx| {
            panel.save_in_flight = true;
            panel.prepare_close(cx)
        });
        cx.run_until_parked();
        for _ in 0..500 {
            cx.background_executor
                .advance_clock(std::time::Duration::from_millis(10));
            cx.run_until_parked();
        }
        assert!(
            close
                .await
                .expect_err("pending save keeps window open")
                .contains("still pending")
        );
        assert!(panel.update(cx, |panel, _| panel.save_in_flight));

        let close = panel.update(cx, |panel, cx| panel.prepare_close(cx));
        cx.run_until_parked();
        panel.update(cx, |panel, _| panel.save_generation += 1);
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(10));
        assert!(
            close
                .await
                .expect_err("new edit cancels close")
                .contains("Settings changed")
        );
        panel.update(cx, |panel, _| panel.save_in_flight = false);
    }

    #[test]
    fn serializes_user_values_as_valid_toml() {
        let serialized = serialize_ai_config(
            &[AiConnection {
                name: "Quoted server".into(),
                provider_id: "ahead-quoted-server".into(),
                base_url: "https://example.test/v1?name=\"quoted\"".into(),
                api_key: "key-with-\n-newline".into(),
                model: "model\"name".into(),
                models: vec!["model\"name".into(), "second".into()],
                hidden_models: vec!["second".into()],
            }],
            0,
        )
        .unwrap();
        let value = parse_toml_value(&serialized).unwrap();
        let ai = value.get("ai").unwrap();
        assert_eq!(
            ai.get("base_url").and_then(toml::Value::as_str),
            Some("https://example.test/v1?name=\"quoted\"")
        );
        assert_eq!(
            ai.get("api_key").and_then(toml::Value::as_str),
            Some("key-with-\n-newline")
        );
        assert_eq!(
            ai.get("model").and_then(toml::Value::as_str),
            Some("model\"name")
        );
        assert_eq!(
            ai.get("connections")
                .and_then(toml::Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("models"))
                .and_then(toml::Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(
            ai["connections"][0]["hidden_models"][0].as_str(),
            Some("second")
        );
    }

    #[test]
    fn parses_hidden_models_per_connection() {
        let config = parse_toml_value(
            "[ai]\n[[ai.connections]]\nname = 'OpenRouter'\nbase_url = 'https://openrouter.ai/api/v1'\nmodels = ['a', 'b']\nhidden_models = ['b', 'b']\n",
        )
        .expect("settings config");
        let connections = super::parse_connections(&config);
        assert_eq!(connections[0].hidden_models, vec!["b"]);
    }

    #[test]
    fn merges_ai_values_without_discarding_workspace_defaults_or_mcp_permissions() {
        let ai = serialize_ai_config(
            &[AiConnection {
                name: "Local".into(),
                provider_id: "ahead-local".into(),
                base_url: "http://localhost:1234/v1".into(),
                api_key: String::new(),
                model: "deepseek-coder".into(),
                models: vec!["deepseek-coder".into()],
                hidden_models: Vec::new(),
            }],
            0,
        )
        .unwrap();
        let existing = format!(
            "{}\n[mcp]\nenabled_servers = ['docs']\n[mcp.server_permissions]\ndocs = 'allow'\n[mcp.tool_permissions.docs]\nsearch = 'allow'\n",
            super::DEFAULT_SETTINGS
        );
        let merged = super::merge_ai_config(&existing, &ai).unwrap();
        let config = parse_toml_value(&merged).unwrap();

        assert!(config.get("core").is_some());
        assert!(config.get("editor").is_some());
        assert_eq!(
            config["mcp"]["server_permissions"]["docs"].as_str(),
            Some("allow")
        );
        assert_eq!(
            config
                .get("mcp")
                .and_then(|mcp| mcp.get("tool_permissions"))
                .and_then(|permissions| permissions.get("docs"))
                .and_then(|docs| docs.get("search"))
                .and_then(toml::Value::as_str),
            Some("allow")
        );
        assert_eq!(
            config
                .get("ai")
                .and_then(|ai| ai.get("model"))
                .and_then(toml::Value::as_str),
            Some("deepseek-coder")
        );
    }

    #[test]
    fn project_overrides_non_secret_user_settings() {
        let configs = vec![
            (
                "user".to_string(),
                parse_toml_value("[ai]\nbase_url = 'user'\napi_key = 'secret'")
                    .unwrap(),
            ),
            (
                "shared".to_string(),
                parse_toml_value("[ai]\nbase_url = 'shared'\nmodel = 'repo-model'")
                    .unwrap(),
            ),
            (
                "local".to_string(),
                parse_toml_value("[ai]\nbase_url = 'local'").unwrap(),
            ),
        ];
        assert_eq!(
            resolve_config_value(&configs, "base_url").as_deref(),
            Some("local")
        );
        assert_eq!(
            resolve_config_value(&configs, "model").as_deref(),
            Some("repo-model")
        );
        assert_eq!(
            resolve_config_value(&configs, "api_key").as_deref(),
            Some("secret")
        );
    }

    #[test]
    fn normalizes_model_catalogs_and_openai_model_responses() {
        assert_eq!(model_catalog("qwen, llama\nqwen"), vec!["qwen", "llama"]);
        let response = serde_json::json!({
            "data": [
                {"id": "qwen"},
                {"id": "llama", "context_window": 65_536, "supports_tools": false},
                {"id": "qwen", "name": "Qwen 3", "context_length": 131_072, "supported_parameters": ["tools"]},
                {"id": "unknown", "context_length": 0}
            ]
        });
        assert_eq!(
            discovered_model_catalog(response),
            vec![
                ModelCatalogEntry {
                    model_id: "qwen".into(),
                    display_name: Some("Qwen 3".into()),
                    context_window: Some(131_072),
                    supports_tool_calls: Some(true),
                },
                ModelCatalogEntry {
                    model_id: "llama".into(),
                    display_name: None,
                    context_window: Some(65_536),
                    supports_tool_calls: Some(false),
                },
                ModelCatalogEntry {
                    model_id: "unknown".into(),
                    display_name: None,
                    context_window: None,
                    supports_tool_calls: None,
                },
            ]
        );
        assert_eq!(
            discovered_model_catalog(serde_json::json!(["plain-model"])),
            vec![ModelCatalogEntry {
                model_id: "plain-model".into(),
                display_name: None,
                context_window: None,
                supports_tool_calls: None,
            }]
        );
    }

    #[test]
    fn models_endpoint_appends_to_provider_base_url() {
        assert_eq!(
            models_endpoint("https://openrouter.ai/api/v1/"),
            "https://openrouter.ai/api/v1/models"
        );
    }

    #[test]
    fn model_endpoint_status_does_not_treat_http_failures_as_success() {
        assert!(
            model_endpoint_status(reqwest::StatusCode::NOT_FOUND)
                .starts_with("Model endpoint responded with HTTP")
        );
        assert!(
            model_endpoint_status(reqwest::StatusCode::UNAUTHORIZED)
                .contains("credentials rejected")
        );
        assert!(
            model_endpoint_status(reqwest::StatusCode::OK)
                .starts_with("Model endpoint reachable")
        );
    }

    #[test]
    fn ignores_stale_model_discovery_results() {
        assert_eq!(
            model_discovery_outcome(4, 3, Ok(vec!["stale-model".to_string()])),
            None
        );
        assert_eq!(
            model_discovery_outcome::<Vec<String>>(
                4,
                3,
                Err("stale failure".to_string()),
            ),
            None
        );
        assert_eq!(
            model_discovery_outcome(4, 4, Ok(vec!["current-model".to_string()])),
            Some(Ok(vec!["current-model".to_string()]))
        );
    }

    #[test]
    fn model_visibility_models_stay_scoped_to_collapsible_providers() {
        let connections = vec![
            AiConnection {
                name: "OpenRouter".into(),
                provider_id: "openrouter".into(),
                base_url: "https://openrouter.ai/api/v1".into(),
                api_key: String::new(),
                model: "shared".into(),
                models: vec!["shared".into(), "fallback".into()],
                hidden_models: vec!["new-model".into()],
            },
            AiConnection {
                name: "Local".into(),
                provider_id: "local".into(),
                base_url: "http://localhost:1234/v1".into(),
                api_key: String::new(),
                model: "shared".into(),
                models: vec!["shared".into()],
                hidden_models: Vec::new(),
            },
        ];
        let discovered = vec![
            vec!["shared".into(), "new-model".into()],
            vec!["local-model".into()],
        ];

        assert_eq!(
            model_visibility_providers(&connections, &discovered, &HashSet::new()),
            vec![
                ModelVisibilityProvider {
                    connection_index: 0,
                    provider_id: "openrouter".into(),
                    name: "OpenRouter".into(),
                    models: vec![
                        ModelVisibilityModel {
                            model_id: "shared".into(),
                            visible: true,
                        },
                        ModelVisibilityModel {
                            model_id: "fallback".into(),
                            visible: true,
                        },
                        ModelVisibilityModel {
                            model_id: "new-model".into(),
                            visible: false,
                        },
                    ],
                    expanded: false,
                },
                ModelVisibilityProvider {
                    connection_index: 1,
                    provider_id: "local".into(),
                    name: "Local".into(),
                    models: vec![
                        ModelVisibilityModel {
                            model_id: "shared".into(),
                            visible: true,
                        },
                        ModelVisibilityModel {
                            model_id: "local-model".into(),
                            visible: true,
                        },
                    ],
                    expanded: false,
                },
            ]
        );

        let expanded = model_visibility_providers(
            &connections,
            &discovered,
            &HashSet::from(["openrouter".to_string()]),
        );
        assert!(expanded[0].expanded);
        assert!(!expanded[1].expanded);
        assert_eq!(expanded[0].models.len(), 3);
        assert_eq!(expanded[1].models.len(), 2);
        assert!(!expanded[0].models[2].visible);

        let reordered = model_visibility_providers(
            &[connections[1].clone(), connections[0].clone()],
            &[discovered[1].clone(), discovered[0].clone()],
            &HashSet::from(["openrouter".to_string()]),
        );
        assert_eq!(reordered[0].provider_id, "local");
        assert!(!reordered[0].expanded);
        assert_eq!(reordered[1].provider_id, "openrouter");
        assert!(reordered[1].expanded);
    }

    #[test]
    fn duplicate_provider_ids_use_the_last_profile_in_settings() {
        let config = parse_toml_value(
            "[ai]\nactive_connection = 'First'\n[[ai.connections]]\nname = 'Other'\nprovider_id = 'other'\nbase_url = 'http://other/v1'\nmodel = 'other-model'\nmodels = ['other-model']\n[[ai.connections]]\nname = 'First'\nprovider_id = 'shared'\nbase_url = 'http://first/v1'\nmodel = 'first-model'\nmodels = ['first-model']\n[[ai.connections]]\nname = 'Second'\nprovider_id = 'shared'\nbase_url = 'http://second/v1'\nmodel = 'second-model'\nmodels = ['second-model']\nhidden_models = ['second-model']\n",
        )
        .expect("valid provider settings");

        let connections = parse_connections(&config);
        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::create_dir_all(workspace.path().join(".ahead"))
            .expect("settings directory");
        std::fs::write(
            workspace.path().join(".ahead/settings.toml"),
            toml::to_string(&config).expect("serialize settings"),
        )
        .expect("write settings");

        assert_eq!(connections.len(), 2);
        assert_eq!(connections[1].name, "Second");
        assert_eq!(connections[1].base_url, "http://second/v1");
        assert_eq!(connections[1].models, vec!["second-model".to_string()]);
        assert_eq!(
            connections[1].hidden_models,
            vec!["second-model".to_string()]
        );
        assert_eq!(
            SettingsPanel::active_connection_index_with_user_home(
                workspace.path(),
                &connections,
                None,
            ),
            1
        );
    }
}

impl BasePanel for SettingsPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_settings"
    }

    fn on_added_to(
        &mut self,
        group: WeakEntity<TabGroup>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.tab_group = Some(group);
    }
}

impl Panel for SettingsPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let group = self.tab_group.clone();
        let panel_id = self.panel_id;
        let close_handler = self.close_handler.clone();
        h_flex().items_center().gap_1().child("Settings").child(
            Button::new("close_settings")
                .icon(IconName::X)
                .ghost()
                .tooltip("Close Settings")
                .on_click(move |_, window, cx| {
                    if let Some(handler) = close_handler.as_ref() {
                        handler(panel_id, window, cx);
                    } else if let Some(group) = group.as_ref() {
                        _ = group
                            .update(cx, |group, cx| group.close_panel(panel_id, cx));
                    }
                }),
        )
    }

    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}

impl EventEmitter<PanelEvent> for SettingsPanel {}

impl Focusable for SettingsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SettingsPanel {
    fn settings_page(
        panel: Entity<Self>,
        section: SettingsSection,
        title: &'static str,
        icon: IconName,
        keywords: &[&str],
    ) -> SettingPage {
        let hidden_page_header = StyleRefinement::default().hidden();
        SettingPage::new(title)
            .icon(icon)
            .header_style(&hidden_page_header)
            .default_open(true)
            .group(
                SettingGroup::new().item(
                    SettingItem::render(move |_, _, app| {
                        panel.update(app, |this, cx| {
                            this.render_content(section, cx).into_any_element()
                        })
                    })
                    .keywords(keywords.iter().copied()),
                ),
            )
    }

    fn render_content(
        &mut self,
        section: SettingsSection,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_appearance = section == SettingsSection::Appearance;
        let is_editor = section == SettingsSection::Editor;
        let is_ai_integration = section == SettingsSection::AiIntegration;
        let is_voice_calls = section == SettingsSection::VoiceCalls;
        let is_mcp_servers = section == SettingsSection::McpServers;
        let border = cx.theme().border;
        let text = cx.theme().sidebar_foreground;
        let card = cx.theme().sidebar;
        let _group = cx.theme().group_box;
        let api_key_masked = self.api_key_input.read(cx).presentation().is_masked();
        let voice_api_key_masked =
            self.voice_api_key_input.read(cx).presentation().is_masked();
        let settings_panel = cx.entity();
        let model_providers = model_visibility_providers(
            &self.connections,
            &self.discovered_models,
            &self.expanded_model_providers,
        );
        let model_visibility_scrolls = model_providers
            .iter()
            .map(|provider| {
                let scroll_handle = self
                    .model_visibility_scrolls
                    .entry(provider.provider_id.clone())
                    .or_insert_with(UniformListScrollHandle::new);
                (provider.provider_id.clone(), scroll_handle.clone())
            })
            .collect::<HashMap<_, _>>();
        v_flex()
            .w_full()
            .p_4()
            .gap_4()
            .track_focus(&self.focus)
            .when(self.setup_required && is_ai_integration, |page| {
                page.child(
                    v_flex()
                        .p_3()
                        .gap_2()
                        .bg(cx.theme().group_box)
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(IconName::Sparkles)
                                .child(
                                    div()
                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                        .child("First-time setup"),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(cx.theme().muted_foreground)
                                .child("Choose the connection and model for AHEAD's internal agent. Changes are saved automatically to this workspace's .ahead folder."),
                        ),
                )
            })
            // Section 1: Appearance
            .when(is_appearance, |page| {
                page.child(
                v_flex()
                    .p_3()
                    .gap_2()
                    .bg(card)
                    .border_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(IconName::Sun)
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(12.))
                                    .text_color(text)
                                    .child("APPEARANCE"),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("settings_dark_btn")
                                    .primary()
                                    .icon(IconName::Moon)
                                    .label("Dark Mode")
                                    .tooltip("Switch to Dark Mode")
                                    .on_click(cx.listener(|_this: &mut Self, _, window, cx| {
                                        gpui_kit::component::Theme::change(
                                            gpui_kit::component::ThemeMode::Dark,
                                            Some(window),
                                            cx,
                                        );
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("settings_light_btn")
                                    .icon(IconName::Sun)
                                    .label("Light Mode")
                                    .tooltip("Switch to Light Mode")
                                    .on_click(cx.listener(|_this: &mut Self, _, window, cx| {
                                        gpui_kit::component::Theme::change(
                                            gpui_kit::component::ThemeMode::Light,
                                            Some(window),
                                            cx,
                                        );
                                        cx.notify();
                                    })),
                            ),
                    ),
                )
            })
            .when(is_editor, |page| {
                page.child(
                v_flex()
                    .p_3()
                    .gap_2()
                    .bg(card)
                    .border_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(IconName::Code)
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(12.))
                                    .text_color(text)
                                    .child("EDITOR"),
                            ),
                    )
                    .child(
                        Switch::new("settings_inline_blame")
                            .checked(self.inline_blame)
                            .label("Show author on active line")
                            .tooltip("Show or hide the Git author and time beside the active line")
                            .on_change(move |enabled, window, cx| {
                                settings_panel.update(cx, |panel, cx| {
                                    panel.inline_blame = *enabled;
                                    panel.save_config(window, cx);
                                });
                            }),
                    ),
                )
            })
            // Section 2: AI integration, provider setup and model visibility.
            .when(is_ai_integration, |page| {
                page.child(
                v_flex()
                    .p_3()
                    .gap_3()
                    .bg(card)
                    .border_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(IconName::Bot)
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(12.))
                                    .text_color(text)
                                    .child("AI INTEGRATION"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child("Configure model providers and discover their models. User defaults live in ~/.ahead/settings.toml; workspace overrides save to ignored .ahead/settings.toml."),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child("New chats use saved connections. Open built-in chats reconnect on the next turn after other managed turns finish."),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "Effective non-secret AI settings: {} · shared config is .ahead/config.toml; local config overrides it",
                                self.effective_source
                            )),
                    )
                    // Named server profiles
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .children(self.connections.iter().enumerate().map(|(index, connection)| {
                                let button = Button::new(format!("connection_{index}"))
                                    .label(connection.name.clone())
                                    .tooltip(format!("Use {}", connection.name));
                                let button = if index == self.selected_connection {
                                    button.primary()
                                } else {
                                    button
                                };
                                button.on_click(cx.listener(move |this: &mut Self, _, window, cx| {
                                    this.select_connection(index, window, cx);
                                }))
                            }))
                            .child(
                                Button::new("add_connection")
                                    .disabled(!self.loaded)
                                    .icon(IconName::Plus)
                                    .tooltip("Add OpenAI-compatible server")
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.add_connection(window, cx);
                                    })),
                            )
                            .when(self.connections.len() > 1, |this| {
                                this.child(
                                    Button::new("remove_connection")
                                        .label("Remove")
                                        .tooltip("Remove selected server")
                                        .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                            this.remove_connection(window, cx);
                                        })),
                                )
                            }),
                    )
                    // Form fields
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(11.))
                                    .text_color(text)
                                    .child("Connection Name"),
                            )
                            .child(
                                Input::new(&self.name_input)
                                    .disabled(!self.loaded)
                                    .aria_label("Connection Name"),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(11.))
                                    .text_color(text)
                                    .child("Base URL"),
                            )
                            .child(
                                Input::new(&self.base_url_input)
                                    .disabled(!self.loaded)
                                    .aria_label("AI Base URL"),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(11.))
                                    .text_color(text)
                                    .child("API Key (optional)"),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div().flex_1().min_w_0().child({
                                            let input = Input::new(&self.api_key_input)
                                                .disabled(!self.loaded)
                                                .aria_label("API Key");
                                            if api_key_masked {
                                                input.content_type(InputContentType::Password)
                                            } else {
                                                input
                                            }
                                        }),
                                    )
                                    .child(
                                        Button::new("toggle_api_key")
                                            .disabled(!self.loaded)
                                            .icon(if api_key_masked {
                                                IconName::Eye
                                            } else {
                                                IconName::EyeOff
                                            })
                                            .label(if api_key_masked { "Show" } else { "Hide" })
                                            .tooltip(if api_key_masked {
                                                "Show API key"
                                            } else {
                                                "Hide API key"
                                            })
                                            .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                                this.api_key_input.update(cx, |input, cx| {
                                                    input.toggle_masked(window, cx);
                                                });
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(11.))
                                    .text_color(text)
                                    .child("Fallback models (comma-separated; first is default)"),
                            )
                            .child(
                                Input::new(&self.model_input)
                                    .disabled(!self.loaded)
                                    .aria_label("Fallback models"),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Button::new("test_conn_btn")
                                            .disabled(!self.loaded)
                                            .icon(IconName::Activity)
                                            .label("Test Endpoint")
                                            .tooltip("Check reachability")
                                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                                this.test_connection(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("discover_models_btn")
                                            .disabled(!self.loaded)
                                            .icon(IconName::Search)
                                            .label("Discover Models")
                                            .tooltip("Refresh model ids from every configured provider")
                                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                                this.discover_models(cx);
                                            })),
                                    )
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.test_status.clone()),
                            ),
                    ),
                )
            })
            .when(is_ai_integration && !self.connections.is_empty(), |page| {
                page.child(
                    v_flex()
                        .p_3()
                        .gap_2()
                        .bg(card)
                        .border_1()
                        .border_color(border)
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(cx.theme().muted_foreground)
                                .child("Models are grouped by provider. All are shown in chat by default; hide a model only for its provider."),
                        )
                        .child(
                            v_flex()
                                .w_full()
                                .gap_1()
                                .children(model_providers.into_iter().map(|provider| {
                                    let connection_index = provider.connection_index;
                                    let provider_id = provider.provider_id;
                                    let expanded = provider.expanded;
                                    let model_count = provider.models.len();
                                    let scroll_handle = model_visibility_scrolls
                                        .get(&provider_id)
                                        .cloned()
                                        .unwrap_or_else(UniformListScrollHandle::new);
                                    let row_provider_id = provider_id.clone();
                                    let models = provider.models;

                                    v_flex()
                                        .w_full()
                                        .child(
                                            h_flex()
                                                .w_full()
                                                .h(px(36.))
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    Button::new(format!(
                                                        "model-provider-{provider_id}"
                                                    ))
                                                    .ghost()
                                                    .icon(if expanded {
                                                        IconName::ChevronDown
                                                    } else {
                                                        IconName::ChevronRight
                                                    })
                                                    .label(provider.name)
                                                    .tooltip(if expanded {
                                                        "Collapse this provider's models"
                                                    } else {
                                                        "Expand this provider's models"
                                                    })
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.toggle_model_provider(
                                                                connection_index,
                                                                cx,
                                                            );
                                                        },
                                                    )),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(10.))
                                                        .text_color(
                                                            cx.theme().muted_foreground,
                                                        )
                                                        .child(model_count.to_string()),
                                                ),
                                        )
                                        .when(expanded && model_count > 0, |provider| {
                                            provider.child(
                                                model_visibility_scroll_area(
                                                    &scroll_handle,
                                                    model_count,
                                                    uniform_list(
                                                        format!(
                                                            "provider-model-visibility-{provider_id}"
                                                        ),
                                                        model_count,
                                                        cx.processor(
                                                            move |_this,
                                                                  range: Range<usize>,
                                                                  _window,
                                                                  cx| {
                                                                range
                                                                    .filter_map(|model_index| {
                                                                        let model = models.get(model_index)?;
                                                                        let model_id = model.model_id.clone();
                                                                        let visible = model.visible;
                                                                        Some(
                                                                            h_flex()
                                                                                .w_full()
                                                                                .h(px(MODEL_VISIBILITY_ROW_HEIGHT))
                                                                                .min_w_0()
                                                                                .pr(px(16.))
                                                                                .items_center()
                                                                                .justify_between()
                                                                                .gap_2()
                                                                                .child(
                                                                                    div()
                                                                                        .ml_6()
                                                                                        .flex_1()
                                                                                        .min_w_0()
                                                                                        .truncate()
                                                                                        .child(model_id.clone()),
                                                                                )
                                                                                .child(
                                                                                    Switch::new(format!(
                                                                                        "model-visible-{row_provider_id}-{model_id}"
                                                                                    ))
                                                                                    .checked(visible)
                                                                                    .accessibility_label(
                                                                                        format!(
                                                                                            "Show {model_id} in this provider's chat model picker"
                                                                                        ),
                                                                                    )
                                                                                    .tooltip(
                                                                                        "Show this model in the chat picker",
                                                                                    )
                                                                                    .on_change(cx.listener(
                                                                                        move |this,
                                                                                              visible,
                                                                                              window,
                                                                                              cx| {
                                                                                            this.set_model_visibility(
                                                                                                connection_index,
                                                                                                model_id.clone(),
                                                                                                *visible,
                                                                                                window,
                                                                                                cx,
                                                                                            );
                                                                                        },
                                                                                    )),
                                                                                ),
                                                                        )
                                                                    })
                                                                    .collect::<Vec<_>>()
                                                            },
                                                        ),
                                                    )
                                                    .track_scroll(&scroll_handle)
                                                    // Stop scroll chaining only after the virtual
                                                    // list has processed its own wheel event.
                                                    .on_scroll_wheel(|_, _, cx| {
                                                        cx.stop_propagation()
                                                    })
                                                    .size_full(),
                                                    cx.theme().border.opacity(0.35),
                                                    cx.theme().scrollbar_thumb,
                                                    cx.theme().scrollbar_thumb_hover,
                                                ),
                                            )
                                        })
                                })),
                        ),
                )
            })
            .when(is_ai_integration && self.connections.is_empty(), |page| {
                page.child(
                    v_flex()
                        .p_3()
                        .gap_2()
                        .bg(card)
                        .border_1()
                        .border_color(border)
                        .child("Add an AI connection first to configure model visibility.")
                )
            })
            .when(is_voice_calls, |page| {
                page.child(
                v_flex()
                    .p_3()
                    .gap_3()
                    .bg(card)
                    .border_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(IconName::Mic)
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(12.))
                                    .text_color(text)
                                    .child("VOICE CALLS"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child("Voice uses a dedicated provider and does not change your coding model. Audio is streamed only during an active call. OpenAI bills by voice duration; self-hosted usage is billed by your GPU provider, and agent reasoning is charged separately. AHEAD shows the latest reported OpenAI call duration. The voice provider also receives current editor context (file, caret or selection, and a short excerpt); private-file contents are withheld."),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child({
                                let button = Button::new("voice_provider_openai")
                                    .label("OpenAI Cloud")
                                    .tooltip("Use GPT-Live for speech-to-speech calls");
                                let button = if self.voice_provider == "openai-live" {
                                    button.primary()
                                } else {
                                    button
                                };
                                button.on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                    this.select_voice_provider("openai-live", window, cx);
                                }))
                            })
                            .child({
                                let button = Button::new("voice_provider_self_hosted")
                                    .label("Self-hosted GPU")
                                    .tooltip("Connect a self-hosted realtime server; model capabilities vary");
                                let button = if self.voice_provider == "self-hosted" {
                                    button.primary()
                                } else {
                                    button
                                };
                                button.on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                    this.select_voice_provider("self-hosted", window, cx);
                                }))
                            }),
                    )
                    .when(self.voice_provider == "self-hosted", |section| {
                        section.child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child({
                                    let button = Button::new("voice_profile_nemotron")
                                        .label("Nemotron VoiceChat")
                                        .tooltip("Use the vLLM-Omni Nemotron audio profile");
                                    let button = if self.voice_profile == "nemotron-voicechat" {
                                        button.primary()
                                    } else {
                                        button
                                    };
                                    button.on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.select_voice_profile("nemotron-voicechat", window, cx);
                                    }))
                                })
                                .child({
                                    let button = Button::new("voice_profile_custom")
                                        .label("Custom PCM16")
                                        .tooltip("Use 16 kHz PCM16 chunks; verify model tool support");
                                    let button = if self.voice_profile == "vllm-omni-pcm16" {
                                        button.primary()
                                    } else {
                                        button
                                    };
                                    button.on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.select_voice_profile("vllm-omni-pcm16", window, cx);
                                    }))
                                })
                                .child({
                                    let button = Button::new("voice_profile_realtime_venus")
                                        .label("Realtime-Venus · evaluation")
                                        .tooltip("Use the experimental Realtime-Venus HTTP serving adapter");
                                    let button = if self.voice_profile == "realtime-venus" {
                                        button.primary()
                                    } else {
                                        button
                                    };
                                    button.on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.select_voice_profile("realtime-venus", window, cx);
                                    }))
                                }),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                        .text_size(px(11.))
                                        .text_color(text)
                                        .child(if self.voice_profile == "realtime-venus" {
                                            "Realtime-Venus server URL"
                                        } else {
                                            "vLLM-Omni server URL"
                                        }),
                                )
                                .child(
                                    Input::new(&self.voice_endpoint_input)
                                        .disabled(!self.loaded)
                                        .aria_label("Self-hosted voice server base URL"),
                                ),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                        .text_size(px(11.))
                                        .text_color(text)
                                        .child(if self.voice_profile == "realtime-venus" {
                                            "Hugging Face checkpoint ID"
                                        } else {
                                            "Realtime model ID"
                                        }),
                                )
                                .child(
                                    Input::new(&self.voice_model_input)
                                        .disabled(!self.loaded)
                                        .aria_label("Self-hosted realtime model ID"),
                                ),
                        )
                    })
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(11.))
                                    .text_color(text)
                                    .child("Voice API key"),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div().flex_1().min_w_0().child({
                                            let input = Input::new(&self.voice_api_key_input)
                                                .disabled(!self.loaded)
                                                .aria_label("Voice API key");
                                            if voice_api_key_masked {
                                                input.content_type(InputContentType::Password)
                                            } else {
                                                input
                                            }
                                        }),
                                    )
                                    .child(
                                        Button::new("toggle_voice_api_key")
                                            .disabled(!self.loaded)
                                            .icon(if voice_api_key_masked {
                                                IconName::Eye
                                            } else {
                                                IconName::EyeOff
                                            })
                                            .label(if voice_api_key_masked { "Show" } else { "Hide" })
                                            .tooltip(if voice_api_key_masked {
                                                "Show voice API key"
                                            } else {
                                                "Hide voice API key"
                                            })
                                            .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                                this.voice_api_key_input.update(cx, |input, cx| {
                                                    input.toggle_masked(window, cx);
                                                });
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    )
                    .when(self.voice_provider == "self-hosted", |section| {
                        section.child(
                            div()
                                .text_size(px(10.))
                                .text_color(cx.theme().muted_foreground)
                                .child(if self.voice_profile == "nemotron-voicechat" {
                                    "Nemotron uses 16 kHz float32 input in 80 ms frames and 22.05 kHz output. Current vLLM-Omni docs list its tool-call integration as a follow-up, so stock servers may not delegate code requests to AHEAD. Use a deployment with that integration enabled. AHEAD stops local playback on interruption; the model profile still needs GPU acceptance."
                                } else if self.voice_profile == "realtime-venus" {
                                    "Experimental adapter for the Realtime-Venus HTTP serving API. It currently allows one active call per server and downloads the matching tokenizer. Put the server behind an authenticated TLS proxy; upstream does not authenticate requests. Published model benchmarks do not qualify an AHEAD deployment."
                                } else {
                                    "Custom profile sends 16 kHz PCM16 input and expects a vLLM-Omni duplex endpoint. AHEAD requires negotiated continuous-input support, but vLLM-Omni does not negotiate tool-call support. Configure only a model that can emit the delegate_to_ahead function call; otherwise code and teaching requests cannot reach AHEAD. The voice server never receives editor tools."
                            }),
                        )
                    }),
                )
            })
            .when(is_mcp_servers, |page| {
                page.child(
                v_flex()
                    .p_3()
                    .gap_3()
                    .bg(card)
                    .border_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(IconName::ShieldCheck)
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(12.))
                                    .text_color(text)
                                    .child("MCP SERVERS"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child("Review the exact tracked declaration before enabling a local MCP process. Approval is private to this workspace and takes effect in new managed sessions."),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Button::new("refresh_mcp_declarations")
                                    .icon(IconName::RefreshCw)
                                    .label("Refresh")
                                    .tooltip("Reload tracked MCP server declarations")
                                    .disabled(self.mcp_busy)
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                        this.load_mcp_server_declarations(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.mcp_status.clone()),
                            ),
                    )
                    .children(self.mcp_declarations.iter().map(|declaration| {
                        let id = declaration.id.clone();
                        let fingerprint = declaration.fingerprint.clone();
                        let enabled = declaration.enabled
                            && (declaration.approved || !declaration.declared);
                        let auto_approve_all = declaration.auto_approve_all;
                        // ponytail: render at most 16 KiB inline; add a paged inspector if real declarations exceed this.
                        let reviewable = declaration.declared
                            && declaration.declaration_toml.len() <= 16 * 1024;
                        let status = if !declaration.declared {
                            "Tracked declaration missing"
                        } else if enabled {
                            "Enabled"
                        } else if declaration.enabled {
                            "Changed since approval"
                        } else {
                            "Not enabled"
                        };
                        v_flex()
                            .p_2()
                            .gap_2()
                            .bg(cx.theme().group_box)
                            .border_1()
                            .border_color(border)
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(id.clone())
                                    .child(status),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("Declaration fingerprint: {fingerprint}")),
                            )
                            .when(reviewable, |card| {
                                card.child(
                                    v_flex()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(10.))
                                        .children(declaration.declaration_toml.lines().map(|line| {
                                            div().child(line.to_string())
                                        })),
                                )
                            })
                            .when(!reviewable, |card| {
                                card.child(if declaration.declared {
                                    "Declaration exceeds 16 KiB; inspect .ahead/config.toml and approve it manually."
                                } else {
                                    "Disable this orphaned local opt-in before starting another managed session."
                                })
                            })
                            .child(
                                Button::new(format!("toggle_mcp_{id}"))
                                    .label(if enabled { "Disable" } else { "Approve & Enable" })
                                    .tooltip(if enabled {
                                        "Disable this MCP server for new managed sessions"
                                    } else {
                                        "Approve this exact declaration and enable it for new managed sessions"
                                    })
                                    .disabled(self.mcp_busy || (!enabled && !reviewable))
                                    .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                        this.set_mcp_server_approval(
                                            id.clone(),
                                            fingerprint.clone(),
                                            if enabled {
                                                McpServerApprovalAction::Disable
                                            } else {
                                                McpServerApprovalAction::ApproveAndEnable
                                            },
                                            cx,
                                        );
                                    })),
                            )
                            .when(enabled && declaration.declared, |card| {
                                let id = declaration.id.clone();
                                let fingerprint = declaration.fingerprint.clone();
                                card.child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            Button::new(format!("mcp_call_policy_{id}"))
                                                .label(if auto_approve_all {
                                                    "Require review for each call"
                                                } else {
                                                    "Auto-approve permitted calls"
                                                })
                                                .tooltip(if auto_approve_all {
                                                    "Review each tool and resource call in new managed sessions"
                                                } else {
                                                    "Trust this server's tools and resources without per-call review in new managed sessions"
                                                })
                                                .disabled(self.mcp_busy)
                                                .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                    this.set_mcp_server_approval(
                                                        id.clone(),
                                                        fingerprint.clone(),
                                                        if auto_approve_all {
                                                            McpServerApprovalAction::ReviewEachCall
                                                        } else {
                                                            McpServerApprovalAction::AutoApproveAll
                                                        },
                                                        cx,
                                                    );
                                                })),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(cx.theme().muted_foreground)
                                                .child("Auto-approval skips prompts for this server's permitted tools and resources. The server runs with your OS permissions; choose it only if you trust it."),
                                        ),
                                )
                            })
                    })),
                )
            })
            // Footer status
            .child(
                div()
                    .pt_2()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child(self.status.clone()),
            )
    }
}

impl Render for SettingsPanel {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let panel = cx.entity();
        Settings::new("ahead-settings")
            .sidebar_width(px(190.))
            .default_selected_index(SelectIndex {
                page_ix: if self.setup_required { 2 } else { 0 },
                group_ix: None,
            })
            .pages([
                Self::settings_page(
                    panel.clone(),
                    SettingsSection::Appearance,
                    "Appearance",
                    IconName::Sun,
                    &["appearance", "theme", "dark mode", "light mode"],
                ),
                Self::settings_page(
                    panel.clone(),
                    SettingsSection::Editor,
                    "Editor",
                    IconName::Code,
                    &["editor", "active line", "git author", "blame"],
                ),
                Self::settings_page(
                    panel.clone(),
                    SettingsSection::AiIntegration,
                    "AI Integration",
                    IconName::Bot,
                    &[
                        "ai integration",
                        "ai connections",
                        "model visibility",
                        "provider",
                        "endpoint",
                        "base url",
                        "api key",
                        "openai",
                        "model",
                    ],
                ),
                Self::settings_page(
                    panel.clone(),
                    SettingsSection::VoiceCalls,
                    "Voice Calls",
                    IconName::Mic,
                    &["voice", "calls", "realtime", "self-hosted"],
                ),
                Self::settings_page(
                    panel,
                    SettingsSection::McpServers,
                    "MCP Servers",
                    IconName::ShieldCheck,
                    &["mcp", "servers", "tool", "approval", "declaration"],
                ),
            ])
    }
}
