//! AHEAD Settings Panel - Appearance & BYOK AI Connection Configuration
//!
//! Allows users to configure multiple OpenAI-compatible model servers
//! and persist configurations securely in the workspace `.ahead` directory.

use std::rc::Rc;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use ahead_rpc::ahead::McpServerDeclaration;
use fs4::fs_std::FileExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent, PanelId, TabGroup,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::setting::{
    SettingGroup, SettingItem, SettingPage, Settings,
};
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

fn model_discovery_outcome(
    active_generation: u64,
    request_generation: u64,
    result: Result<Vec<String>, String>,
) -> Option<(Vec<String>, String)> {
    if active_generation != request_generation {
        return None;
    }
    Some(match result {
        Ok(models) => {
            let status = format!(
                "Discovered {} model(s) · choose one below or edit the catalog",
                models.len()
            );
            (models, status)
        }
        Err(error) => (Vec::new(), format!("Model discovery failed · {error}")),
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
) -> Result<[Option<SystemTime>; 3], String> {
    let path = SettingsPanel::config_path(workspace);
    let parent = path.parent().ok_or("settings path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let _settings_lock =
        lock_settings_file(&path).map_err(|error| error.to_string())?;
    let existing = ahead_core::config::read_ahead_config(workspace, "settings.toml")
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| DEFAULT_SETTINGS.to_string());
    let content = merge_ai_config(&existing, ai_config)?;
    write_user_config(&path, &content).map_err(|error| error.to_string())?;
    Ok(SettingsPanel::settings_modified(workspace))
}

struct PendingSettingsSave {
    workspace: PathBuf,
    generation: u64,
    ai_config: String,
}

struct LoadedSettings {
    connections: Vec<AiConnection>,
    selected_connection: usize,
    source: String,
    setup_required: bool,
    modified: [Option<SystemTime>; 3],
    status: String,
}

fn load_settings(
    workspace: &Path,
    previous_modified: Option<[Option<SystemTime>; 3]>,
) -> Option<LoadedSettings> {
    let path = SettingsPanel::config_path(workspace);
    let initial = previous_modified.is_none();
    let bootstrap = if initial {
        ensure_settings_file(&path)
    } else {
        Ok(false)
    };
    let modified = SettingsPanel::settings_modified(workspace);
    if previous_modified == Some(modified) {
        return None;
    }
    let (connections, source, errors) = SettingsPanel::load_saved_config(workspace);
    let selected_connection =
        SettingsPanel::active_connection_index(workspace, &connections);
    let setup_required = bootstrap.as_ref().is_ok_and(|created| *created)
        || !has_ai_settings(workspace);
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
    pub extension_id_input: Entity<InputState>,
    pub extension_url_input: Entity<InputState>,
    pub test_status: SharedString,
    pub extension_status: SharedString,
    mcp_declarations: Vec<McpServerDeclaration>,
    mcp_status: SharedString,
    mcp_busy: bool,
    pub status: SharedString,
    pub effective_source: SharedString,
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
    settings_modified: [Option<SystemTime>; 3],
    discovered_models: Vec<String>,
    model_discovery_generation: u64,
    close_handler: Option<Rc<dyn Fn(PanelId, &mut Window, &mut App)>>,
    save_handler: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
    proxy: Option<Arc<crate::proxy_client::ProxyClient>>,
}

impl SettingsPanel {
    pub fn new(
        workspace: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx));
        let base_url_input = cx.new(|cx| InputState::new(window, cx));
        let api_key_input = cx.new(|cx| InputState::new(window, cx));
        let model_input = cx.new(|cx| InputState::new(window, cx));
        let extension_id_input = cx.new(|cx| InputState::new(window, cx));
        let extension_url_input = cx.new(|cx| InputState::new(window, cx));

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
                            this.model_discovery_generation =
                                this.model_discovery_generation.wrapping_add(1);
                            this.discovered_models.clear();
                            this.test_status =
                                "Endpoint changed; discover models again".into();
                        }
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
            extension_id_input,
            extension_url_input,
            test_status: "Endpoint not tested".into(),
            extension_status: "No language extension install started".into(),
            mcp_declarations: Vec::new(),
            mcp_status: "MCP declarations not loaded".into(),
            mcp_busy: false,
            status: "Loading workspace settings…".into(),
            effective_source: "Loading…".into(),
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
            settings_modified: [None; 3],
            discovered_models: Vec::new(),
            model_discovery_generation: 0,
            close_handler: None,
            save_handler: None,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
            proxy: None,
        };
        panel.refresh_from_disk(window, cx);
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
        enabled: bool,
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
                    .set_mcp_server_approval(&workspace, &server_id, &fingerprint, enabled)
                    .map_err(|error| error.message);
                let current = proxy.mcp_server_declarations(&workspace).map_err(|error| error.message);
                (change, current)
            }).await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                this.mcp_busy = false;
                if let Ok(declarations) = current {
                    this.mcp_declarations = declarations;
                }
                this.mcp_status = match change {
                    Ok(()) if enabled => "MCP server approved for new managed sessions".into(),
                    Ok(()) => "MCP server disabled for new sessions; stop existing sessions separately".into(),
                    Err(error) => format!("MCP approval failed: {error}; refresh the declaration").into(),
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

    pub(crate) fn settings_modified(workspace: &Path) -> [Option<SystemTime>; 3] {
        ["settings.toml", "config.toml", "config.local.toml"].map(|filename| {
            std::fs::symlink_metadata(workspace.join(".ahead").join(filename))
                .and_then(|metadata| metadata.modified())
                .ok()
        })
    }

    fn active_connection_index(
        workspace: &Path,
        connections: &[AiConnection],
    ) -> usize {
        let active_name =
            ahead_core::config::read_ahead_config(workspace, "settings.toml")
                .ok()
                .flatten()
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

    fn load_saved_config(
        workspace: &Path,
    ) -> (Vec<AiConnection>, String, Vec<String>) {
        let mut configs = Vec::new();
        let mut errors = Vec::new();
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
                models: vec![
                    resolve_config_value(&configs, "model")
                        .unwrap_or_else(|| "deepseek-coder".to_string()),
                ],
            })
        }
        let source = configs
            .last()
            .map(|(label, _)| (*label).to_string())
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

        self.pending_save = Some(PendingSettingsSave {
            workspace: self.workspace.clone(),
            generation: self.save_generation,
            ai_config,
        });
        self.status = "Saving workspace settings…".into();
        self.start_pending_save(window, cx);
        cx.notify();
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
                    save_ai_config(&request.workspace, &request.ai_config)
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
                    this.refresh_from_disk(window, cx);
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
        self.discovered_models.clear();
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
        self.api_key_input
            .update(cx, |input, cx| input.set_value(&api_key, window, cx));
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
        self._config_watch = Some(cx.spawn_in(window, async move |this, cx| {
            while receiver.recv().await.is_ok() {
                if this
                    .update_in(cx, |this, window, cx| {
                        this.refresh_from_disk(window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn refresh_from_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let modified = self.loaded.then_some(self.settings_modified);
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
                        this.selected_connection = loaded.selected_connection;
                        this.effective_source = loaded.source.into();
                        this.setup_required = loaded.setup_required;
                        this.settings_modified = loaded.modified;
                        this.status = loaded.status.into();
                        this.load_connection_into_inputs(window, cx);
                        this.loaded = true;
                        if let Some(handler) = this.save_handler.clone() {
                            handler(window, cx);
                        }
                        cx.notify();
                    }
                }
                if this.reload_requested {
                    this.refresh_from_disk(window, cx);
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

    pub fn install_language_extension(&mut self, cx: &mut Context<Self>) {
        let Some(proxy) = self.proxy.clone() else {
            self.extension_status = "AHEAD proxy is unavailable".into();
            cx.notify();
            return;
        };
        let extension_id =
            self.extension_id_input.read(cx).value().trim().to_string();
        let url = self.extension_url_input.read(cx).value().trim().to_string();
        if extension_id.is_empty() || url.is_empty() {
            self.extension_status = "Enter an extension ID and package URL".into();
            cx.notify();
            return;
        }

        self.extension_status = format!("Installing {extension_id}…").into();
        cx.notify();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        proxy.install_language_extension(url, extension_id, move |result| {
            let _ = tx.send(result);
        });
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    rx.recv()
                        .map_err(|_| "AHEAD proxy connection closed".to_string())?
                })
                .await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                this.extension_status = match result {
                    Ok(()) => "Language extension installed; open a matching file to start its LSP".into(),
                    Err(error) => format!("Language extension install failed · {error}").into(),
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub fn discover_models(&mut self, cx: &mut Context<Self>) {
        if !self.loaded {
            return;
        }
        self.model_discovery_generation =
            self.model_discovery_generation.wrapping_add(1);
        let generation = self.model_discovery_generation;
        self.discovered_models.clear();
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
                    let response =
                        request.send().map_err(|error| error.to_string())?;
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
                let Some((models, status)) = model_discovery_outcome(
                    this.model_discovery_generation,
                    generation,
                    result,
                ) else {
                    return;
                };
                this.discovered_models = models;
                this.test_status = status.into();
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
        self.test_status =
            format!("Selected model {model} · save the connection to apply").into();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AiConnection, McpServerDeclaration, SettingsPanel, discovered_model_ids,
        ensure_settings_file, has_ai_settings, lock_settings_file, model_catalog,
        model_discovery_outcome, parse_toml_value, resolve_config_value,
        serialize_ai_config, write_user_config,
    };
    use fs4::fs_std::FileExt;
    use std::{cell::Cell, rc::Rc};

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
            SettingsPanel::load_saved_config(workspace.path());
        assert_eq!(connections[0].base_url, "http://127.0.0.1:1234/v1");
        assert!(connections[0].api_key.is_empty());
        assert!(errors.iter().any(|error| error.contains("settings.toml")));
        std::fs::write(ahead.join("config.toml"), "[ai\n")
            .expect("write malformed shared settings");
        let (_, _, errors) = SettingsPanel::load_saved_config(workspace.path());
        assert!(errors.iter().any(|error| error.contains("invalid TOML")));
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
            }];
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.update(cx, |panel, _| {
            assert_eq!(panel.mcp_declarations[0].id, "docs");
        });
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
        panel.update_in(cx, |panel, window, cx| panel.refresh_from_disk(window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.update(cx, |panel, _| {
            assert!(panel.status.to_string().contains("Settings reloaded"));
        });
        assert_eq!(reloads.get(), 1, "chat model picker should reload");
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
        let original =
            "[ai]\nmodel = 'existing-model'\napi_key = 'fixture-secret'\n";
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
            assert_eq!(
                panel.save_generation, 0,
                "loading must not trigger autosave"
            );
        });
        assert_eq!(
            std::fs::read_to_string(path).expect("unchanged settings"),
            original
        );
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
            panel.refresh_from_disk(window, cx);
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
            panel.refresh_from_disk(window, cx);
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
        panel.update_in(cx, |panel, window, cx| panel.refresh_from_disk(window, cx));
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
    fn merges_ai_values_without_discarding_workspace_defaults_or_mcp_permissions() {
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
        let existing = format!(
            "{}\n[mcp]\nenabled_servers = ['docs']\n[mcp.tool_permissions.docs]\nsearch = 'allow'\n",
            super::DEFAULT_SETTINGS
        );
        let merged = super::merge_ai_config(&existing, &ai).unwrap();
        let config = parse_toml_value(&merged).unwrap();

        assert!(config.get("core").is_some());
        assert!(config.get("editor").is_some());
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
            "data": [{"id": "qwen"}, {"id": "llama"}, {"id": "qwen"}]
        });
        assert_eq!(discovered_model_ids(response), vec!["qwen", "llama"]);
    }

    #[test]
    fn ignores_stale_model_discovery_results() {
        assert_eq!(
            model_discovery_outcome(4, 3, Ok(vec!["stale-model".to_string()]),),
            None
        );
        assert_eq!(
            model_discovery_outcome(4, 3, Err("stale failure".to_string()),),
            None
        );
        assert_eq!(
            model_discovery_outcome(4, 4, Ok(vec!["current-model".to_string()]),),
            Some((
                vec!["current-model".to_string()],
                "Discovered 1 model(s) · choose one below or edit the catalog"
                    .to_string(),
            ))
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
    fn render_content(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let text = cx.theme().sidebar_foreground;
        let card = cx.theme().sidebar;
        let _group = cx.theme().group_box;
        let is_dark = cx.theme().mode.is_dark();

        v_flex()
            .w_full()
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
                                Input::new(&self.api_key_input)
                                    .disabled(!self.loaded)
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
                                    .disabled(!self.loaded)
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
                                            !enabled,
                                            cx,
                                        );
                                    })),
                            )
                    })),
            )
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
                            .child(IconName::Code)
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(12.))
                                    .text_color(text)
                                    .child("LANGUAGE EXTENSIONS"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child("Install a Zed language extension package for additional languages. Rust Analyzer, Vtsls (TypeScript/JavaScript), and BasedPyright (Python) can also run from PATH. Matching extensions take precedence; manual LSP command settings are not supported."),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(11.))
                                    .text_color(text)
                                    .child("Package URL"),
                            )
                            .child(
                                Input::new(&self.extension_url_input)
                                    .aria_label("Language extension package URL"),
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
                                    .child("Extension ID"),
                            )
                            .child(
                                Input::new(&self.extension_id_input)
                                    .aria_label("Language extension ID"),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("install_language_extension")
                                    .primary()
                                    .icon(IconName::Download)
                                    .label("Install / Update")
                                    .tooltip("Install or update a language extension")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                        this.install_language_extension(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.extension_status.clone()),
                            ),
                    ),
            )
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
                        SettingItem::render(move |_, _, app| {
                            panel.update(app, |this, cx| {
                                this.render_content(cx).into_any_element()
                            })
                        }),
                    )),
            )
    }
}
