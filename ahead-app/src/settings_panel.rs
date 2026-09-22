//! AHEAD Settings Panel - Appearance & BYOK AI Connection Configuration
//!
//! Allows users to configure multiple OpenAI-compatible model servers
//! and persist configurations securely in the workspace `.ahead` directory.

use std::rc::Rc;
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent, PanelId, TabGroup,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::setting::{
    SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme};
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
}

fn parse_toml_value(content: &str) -> Option<toml::Value> {
    content.parse::<toml::Table>().ok().map(toml::Value::Table)
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

fn discovered_model_ids(value: serde_json::Value) -> Vec<String> {
    let entries = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .or_else(|| value.as_array());
    entries
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry.as_str().map(str::to_string).or_else(|| {
                entry
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
        })
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .fold(Vec::new(), |mut models, model| {
            if !models.iter().any(|existing| existing == &model) {
                models.push(model);
            }
            models
        })
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
    value
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
            if name.is_empty() || base_url.is_empty() {
                return None;
            }
            Some(AiConnection {
                name: name.to_string(),
                provider_id: table
                    .get("provider_id")
                    .or_else(|| table.get("id"))
                    .and_then(toml::Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| provider_id_for(name, "openai-compatible")),
                base_url: base_url.to_string(),
                api_key: table
                    .get("api_key")
                    .and_then(toml::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                model,
                models,
            })
        })
        .collect()
}

fn write_user_config(path: &Path, content: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true).mode(0o600);
        let mut file = options.open(path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(content.as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, content)
    }
}

fn ensure_settings_file(path: &Path) -> std::io::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_user_config(path, DEFAULT_SETTINGS)?;
    Ok(true)
}

fn has_ai_settings(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
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

pub struct SettingsPanel {
    pub focus: FocusHandle,
    pub workspace: PathBuf,
    connections: Vec<AiConnection>,
    selected_connection: usize,
    pub name_input: Entity<InputState>,
    pub base_url_input: Entity<InputState>,
    pub api_key_input: Entity<InputState>,
    pub model_input: Entity<InputState>,
    pub test_status: SharedString,
    pub status: SharedString,
    pub effective_source: SharedString,
    setup_required: bool,
    suppress_autosave: bool,
    settings_modified: Option<SystemTime>,
    discovered_models: Vec<String>,
    close_handler: Option<Rc<dyn Fn(PanelId, &mut Window, &mut App)>>,
    save_handler: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
}

impl SettingsPanel {
    pub fn new(
        workspace: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings_path = Self::config_path(&workspace);
        let (settings_created, settings_error) =
            match ensure_settings_file(&settings_path) {
                Ok(created) => (created, None),
                Err(error) => (false, Some(error.to_string())),
            };
        let setup_required = settings_created || !has_ai_settings(&settings_path);
        let settings_modified = Self::settings_modified(&settings_path);
        let (connections, source) = Self::load_saved_config(&workspace);
        let selected_connection =
            Self::active_connection_index(&workspace, &connections);
        let saved_connection = connections
            .get(selected_connection)
            .cloned()
            .or_else(|| connections.first().cloned())
            .unwrap_or(AiConnection {
                name: "Local server".into(),
                provider_id: "ahead-local-server".into(),
                base_url: "http://localhost:1234/v1".into(),
                api_key: String::new(),
                model: "deepseek-coder".into(),
                models: vec!["deepseek-coder".into()],
            });

        let name_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(&saved_connection.name, window, cx);
            input
        });

        let base_url_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(&saved_connection.base_url, window, cx);
            input
        });

        let api_key_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(&saved_connection.api_key, window, cx);
            input
        });

        let model_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(&saved_connection.model, window, cx);
            input
        });

        for input in [
            name_input.clone(),
            base_url_input.clone(),
            api_key_input.clone(),
            model_input.clone(),
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

        Self {
            focus: cx.focus_handle(),
            workspace,
            connections: if connections.is_empty() {
                vec![saved_connection]
            } else {
                connections
            },
            selected_connection,
            name_input,
            base_url_input,
            api_key_input,
            model_input,
            test_status: "Endpoint not tested".into(),
            status: if let Some(error) = settings_error {
                format!("Unable to create workspace settings: {error}").into()
            } else if setup_required {
                "Complete AHEAD setup to save workspace settings".into()
            } else {
                format!("Settings loaded from {}", settings_path.display()).into()
            },
            effective_source: source.into(),
            setup_required,
            suppress_autosave: false,
            settings_modified,
            discovered_models: Vec::new(),
            close_handler: None,
            save_handler: None,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
        }
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

    fn settings_modified(path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    }

    fn active_connection_index(
        workspace: &Path,
        connections: &[AiConnection],
    ) -> usize {
        let active_name = std::fs::read_to_string(Self::config_path(workspace))
            .ok()
            .and_then(|content| parse_toml_value(&content))
            .and_then(|config| {
                config
                    .get("ai")
                    .and_then(|ai| ai.get("active_connection"))
                    .and_then(toml::Value::as_str)
                    .map(str::to_string)
            });
        active_name
            .and_then(|name| {
                connections.iter().position(|connection| {
                    connection.name == name || connection.provider_id == name
                })
            })
            .unwrap_or(0)
    }

    fn load_saved_config(workspace: &Path) -> (Vec<AiConnection>, String) {
        let mut configs = Vec::new();
        let settings_path = Self::config_path(workspace);
        if let Ok(content) = std::fs::read_to_string(&settings_path) {
            if let Some(value) = parse_toml_value(&content) {
                configs.push(("workspace .ahead/settings.toml".to_string(), value));
            }
        }
        for (label, path) in [
            (
                "shared .ahead/config.toml",
                workspace.join(".ahead/config.toml"),
            ),
            (
                "local .ahead/config.local.toml",
                workspace.join(".ahead/config.local.toml"),
            ),
        ] {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Some(value) = parse_toml_value(&content) {
                    configs.push((label.to_string(), value));
                }
            }
        }

        let mut connections = configs
            .iter()
            .filter(|(label, _)| label.starts_with("workspace "))
            .find_map(|(_, config)| {
                let connections = parse_connections(config);
                (!connections.is_empty()).then_some(connections)
            })
            .unwrap_or_default();
        if connections.is_empty() {
            let key = configs
                .iter()
                .find(|(label, _)| label.starts_with("workspace "))
                .and_then(|(_, config)| {
                    config
                        .get("ai")
                        .and_then(|ai| ai.get("api_key"))
                        .and_then(toml::Value::as_str)
                })
                .unwrap_or_default()
                .to_string();
            connections.push(AiConnection {
                name: "OpenAI-compatible server".into(),
                provider_id: resolve_config_value(&configs, "provider")
                    .map(|provider| {
                        provider_id_for("OpenAI-compatible server", &provider)
                    })
                    .unwrap_or_else(|| "openai-compatible".into()),
                base_url: resolve_config_value(&configs, "base_url")
                    .unwrap_or_else(|| "http://localhost:1234/v1".to_string()),
                api_key: key,
                model: resolve_config_value(&configs, "model")
                    .unwrap_or_else(|| "deepseek-coder".to_string()),
                models: vec![resolve_config_value(&configs, "model")
                    .unwrap_or_else(|| "deepseek-coder".to_string())],
            })
        }
        let source = configs
            .last()
            .map(|(label, _)| (*label).to_string())
            .unwrap_or_else(|| "built-in defaults".to_string());
        (connections, source)
    }

    pub fn save_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_selected_connection(cx);
        let ai_config =
            match serialize_ai_config(&self.connections, self.selected_connection) {
                Ok(content) => content,
                Err(error) => {
                    self.status = format!("Save failed: {error}").into();
                    cx.notify();
                    return;
                }
            };

        let path = Self::config_path(&self.workspace);
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                self.status = format!("Save failed: {error}").into();
                cx.notify();
                return;
            }
        }
        let existing_config = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                DEFAULT_SETTINGS.to_string()
            }
            Err(error) => {
                self.status = format!("Save failed: {error}").into();
                cx.notify();
                return;
            }
        };
        let toml_str = match merge_ai_config(&existing_config, &ai_config) {
            Ok(content) => content,
            Err(error) => {
                self.status = format!("Save failed: {error}").into();
                cx.notify();
                return;
            }
        };
        let save_handler = self.save_handler.clone();
        match write_user_config(&path, &toml_str) {
            Ok(()) => {
                self.setup_required = false;
                self.settings_modified = Self::settings_modified(&path);
                self.status =
                    format!("Saved workspace configuration to {}", path.display())
                        .into();
                if let Some(handler) = save_handler {
                    handler(window, cx);
                }
            }
            Err(e) => {
                self.status = format!("Save failed: {e}").into();
            }
        }
        cx.notify();
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
        self.api_key_input
            .update(cx, |input, cx| input.set_value(&api_key, window, cx));
        self.model_input
            .update(cx, |input, cx| input.set_value(&model, window, cx));
        self.suppress_autosave = false;
    }

    fn refresh_from_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = Self::config_path(&self.workspace);
        let modified = Self::settings_modified(&path);
        if modified == self.settings_modified {
            return;
        }

        let (connections, source) = Self::load_saved_config(&self.workspace);
        self.connections = connections;
        self.selected_connection =
            Self::active_connection_index(&self.workspace, &self.connections);
        self.effective_source = source.into();
        self.setup_required = !path.exists();
        self.settings_modified = modified;
        self.load_connection_into_inputs(window, cx);
        self.status = format!("Settings reloaded from {}", path.display()).into();
        cx.notify();
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
        self.save_config(window, cx);
        cx.notify();
    }

    pub fn add_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        });
        self.selected_connection = self.connections.len() - 1;
        self.load_connection_into_inputs(window, cx);
        self.test_status = "New OpenAI-compatible server".into();
        self.save_config(window, cx);
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
        self.selected_connection = self
            .selected_connection
            .min(self.connections.len().saturating_sub(1));
        self.load_connection_into_inputs(window, cx);
        self.test_status = "Connection removed".into();
        self.save_config(window, cx);
        cx.notify();
    }

    pub fn test_connection(&mut self, cx: &mut Context<Self>) {
        let url = self.base_url_input.read(cx).value().to_string();
        let api_key = self.api_key_input.read(cx).value().to_string();
        if url.is_empty() {
            self.test_status = "Please enter an endpoint URL".into();
            cx.notify();
            return;
        }

        self.test_status = format!("Checking {url}…").into();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result: Result<String, String> = cx
                .background_spawn(async move {
                    let client = reqwest::blocking::Client::builder()
                        .timeout(std::time::Duration::from_secs(5))
                        .build()
                        .map_err(|error| error.to_string())?;
                    let mut request = client.get(&url);
                    if !api_key.trim().is_empty() {
                        request = request.bearer_auth(api_key);
                    }
                    let response = request.send().map_err(|error| error.to_string())?;
                    let status = response.status();
                    if status == reqwest::StatusCode::UNAUTHORIZED
                        || status == reqwest::StatusCode::FORBIDDEN
                    {
                        Ok(format!(
                            "Endpoint reachable · credentials rejected (HTTP {status})"
                        ))
                    } else {
                        Ok(format!("Endpoint reachable · HTTP {status}"))
                    }
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
        let url = self.base_url_input.read(cx).value().to_string();
        let api_key = self.api_key_input.read(cx).value().to_string();
        if url.trim().is_empty() {
            self.test_status = "Please enter an endpoint URL".into();
            cx.notify();
            return;
        }
        let endpoint = format!("{}/models", url.trim_end_matches('/'));
        self.test_status = format!("Discovering models from {endpoint}…").into();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result: Result<Vec<String>, String> = cx
                .background_spawn(async move {
                    let client = reqwest::blocking::Client::builder()
                        .timeout(std::time::Duration::from_secs(5))
                        .build()
                        .map_err(|error| error.to_string())?;
                    let mut request = client.get(endpoint);
                    if !api_key.trim().is_empty() {
                        request = request.bearer_auth(api_key);
                    }
                    let response = request.send().map_err(|error| error.to_string())?;
                    if !response.status().is_success() {
                        return Err(format!("HTTP {}", response.status()));
                    }
                    let value = response
                        .json::<serde_json::Value>()
                        .map_err(|error| error.to_string())?;
                    let models = discovered_model_ids(value);
                    if models.is_empty() {
                        return Err("No model ids returned".into());
                    }
                    Ok(models)
                })
                .await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                match result {
                    Ok(models) => {
                        this.discovered_models = models;
                        this.test_status = format!(
                            "Discovered {} model(s) · choose one below or edit the catalog",
                            this.discovered_models.len()
                        )
                        .into();
                    }
                    Err(error) => {
                        this.test_status = format!("Model discovery failed · {error}").into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select_discovered_model(
        &mut self,
        model: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.model_input
            .update(cx, |input, cx| input.set_value(&model, window, cx));
        self.test_status = format!("Selected model {model} · saved").into();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        discovered_model_ids, model_catalog, parse_toml_value, resolve_config_value,
        serialize_ai_config, AiConnection,
    };

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
    }

    #[test]
    fn merges_ai_values_without_discarding_workspace_defaults() {
        let ai = serialize_ai_config(
            &[AiConnection {
                name: "Local".into(),
                provider_id: "ahead-local".into(),
                base_url: "http://localhost:1234/v1".into(),
                api_key: String::new(),
                model: "deepseek-coder".into(),
                models: vec!["deepseek-coder".into()],
            }],
            0,
        )
        .unwrap();
        let merged = super::merge_ai_config(super::DEFAULT_SETTINGS, &ai).unwrap();
        let config = parse_toml_value(&merged).unwrap();

        assert!(config.get("core").is_some());
        assert!(config.get("editor").is_some());
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
            "data": [{"id": "qwen"}, {"id": "llama"}, {"id": "qwen"}]
        });
        assert_eq!(discovered_model_ids(response), vec!["qwen", "llama"]);
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
    fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.refresh_from_disk(window, cx);
        let border = cx.theme().border;
        let text = cx.theme().sidebar_foreground;
        let card = cx.theme().sidebar;
        let _group = cx.theme().group_box;
        let is_dark = cx.theme().mode.is_dark();

        v_flex()
            .w_full()
            .min_h_0()
            .overflow_y_scrollbar()
            .p_4()
            .gap_4()
            .track_focus(&self.focus)
            // Header
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(IconName::Settings)
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_size(px(14.))
                            .text_color(text)
                            .child("AHEAD Settings"),
                    ),
            )
            .when(self.setup_required, |page| {
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
            .child(
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
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "Current theme: {} (instant app-wide update)",
                                if is_dark { "Dark" } else { "Light" }
                            )),
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
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        gpui_kit::component::Theme::change(
                                            gpui_kit::component::ThemeMode::Dark,
                                            Some(window),
                                            cx,
                                        );
                                        this.status = "Theme: Dark".into();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("settings_light_btn")
                                    .icon(IconName::Sun)
                                    .label("Light Mode")
                                    .tooltip("Switch to Light Mode")
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        gpui_kit::component::Theme::change(
                                            gpui_kit::component::ThemeMode::Light,
                                            Some(window),
                                            cx,
                                        );
                                        this.status = "Theme: Light".into();
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            // Section 2: AI Connections & BYOK (Bring Your Own Key)
            .child(
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
                                    .child("AI CONNECTIONS (BRING YOUR OWN KEY / MODEL)"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child("Configure multiple OpenAI-compatible servers. Changes are saved automatically; credentials stay in the workspace .ahead/settings.toml."),
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
                                Input::new(&self.api_key_input)
                                    .aria_label("API Key"),
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
                                    .child("Model Catalog (comma-separated; first is active)"),
                            )
                            .child(
                                Input::new(&self.model_input)
                                    .aria_label("Model Catalog"),
                            ),
                    )
                    // Test and Save buttons
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Button::new("test_conn_btn")
                                    .icon(IconName::Activity)
                                    .label("Test Endpoint")
                                    .tooltip("Check reachability")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                        this.test_connection(cx);
                                    })),
                            )
                            .child(
                                Button::new("discover_models_btn")
                                    .icon(IconName::Search)
                                    .label("Discover Models")
                                    .tooltip("Discover model ids from the endpoint")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                        this.discover_models(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.test_status.clone()),
                            ),
                    ),
            )
            .when(!self.discovered_models.is_empty(), |section| {
                section.child(
                    v_flex()
                        .p_3()
                        .gap_2()
                        .bg(card)
                        .border_1()
                        .border_color(border)
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::BOLD)
                                .text_size(px(11.))
                                .text_color(text)
                                .child("DISCOVERED MODELS"),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(cx.theme().muted_foreground)
                                .child("Choose a model to replace the catalog, then save the connection."),
                        )
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_1()
                                .children(self.discovered_models.iter().map(|model| {
                                    let model = model.clone();
                                    Button::new(SharedString::from(format!("discovered_{model}")))
                                        .label(model.clone())
                                        .tooltip(format!("Use {model}"))
                                        .on_click(cx.listener(move |this: &mut Self, _, window, cx| {
                                            this.select_discovered_model(model.clone(), window, cx);
                                        }))
                                })),
                        ),
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
            .page(
                SettingPage::new("AHEAD")
                    .icon(IconName::Settings)
                    .description("Editor, appearance, and BYOK agent connections")
                    .default_open(true)
                    .group(SettingGroup::new().title("Configuration").item(
                        SettingItem::render(move |_, window, app| {
                            panel.update(app, |this, cx| {
                                this.render_content(window, cx).into_any_element()
                            })
                        }),
                    )),
            )
    }
}
