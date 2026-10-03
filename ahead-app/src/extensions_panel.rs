use std::{io::Read, path::Path, rc::Rc, sync::Arc};

use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent, PanelId, TabGroup,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, Selectable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit_assets::IconName;

const LANGUAGE_EXTENSION_CATALOG_URL: &str = "https://api.zed.dev/extensions";
const MAX_EXTENSION_CATALOG_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, serde::Deserialize)]
struct LanguageExtensionEntry {
    id: String,
    name: String,
    version: String,
    description: Option<String>,
    schema_version: Option<u32>,
    wasm_api_version: Option<String>,
    #[serde(default)]
    provides: Vec<String>,
    archive_sha256: Option<String>,
    #[serde(skip)]
    installed_version: Option<String>,
    #[serde(skip)]
    from_catalog: bool,
}

#[derive(serde::Deserialize)]
struct LanguageExtensionCatalog {
    data: Vec<LanguageExtensionEntry>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LanguageExtensionFilter {
    All,
    Installed,
    NotInstalled,
}

fn extension_download_url(entry: &LanguageExtensionEntry) -> Option<String> {
    let valid_id = !entry.id.is_empty()
        && entry.id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
        });
    let valid_version = !entry.version.is_empty()
        && entry.version != "."
        && entry.version != ".."
        && entry.version.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+')
        });
    (valid_id && valid_version).then(|| {
        format!(
            "https://api.zed.dev/extensions/{}/{}/download",
            entry.id, entry.version
        )
    })
}

fn parse_language_extension_catalog(
    body: &[u8],
) -> Result<Vec<LanguageExtensionEntry>, String> {
    let response: LanguageExtensionCatalog =
        serde_json::from_slice(body).map_err(|error| error.to_string())?;
    Ok(response
        .data
        .into_iter()
        .filter(|entry| {
            entry.schema_version.is_some_and(|version| version <= 1)
                && semver::Version::parse(&entry.version).is_ok()
                && (entry.provides.iter().any(|item| item == "icon-themes")
                    || entry.provides.iter().any(|item| item == "language-servers"))
                && (!entry.provides.iter().any(|item| item == "language-servers")
                    || matches!(
                        entry.wasm_api_version.as_deref(),
                        Some("0.6.0" | "0.7.0" | "0.8.0")
                    ))
                && entry.archive_sha256.as_deref().is_none_or(|digest| {
                    digest.len() == 64
                        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                && extension_download_url(entry).is_some()
        })
        .map(|mut entry| {
            entry.from_catalog = true;
            entry
        })
        .collect())
}

fn extension_update_available(entry: &LanguageExtensionEntry) -> bool {
    entry
        .installed_version
        .as_deref()
        .and_then(|installed| {
            let installed = semver::Version::parse(installed).ok()?;
            let available = semver::Version::parse(&entry.version).ok()?;
            Some(available > installed)
        })
        .unwrap_or(false)
}

fn installed_language_extension(
    directory: &Path,
    id: &str,
) -> Option<LanguageExtensionEntry> {
    let metadata = std::fs::symlink_metadata(directory).ok()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return None;
    }
    let manifest_path = directory.join("extension.toml");
    let manifest_metadata = std::fs::symlink_metadata(&manifest_path).ok()?;
    if !manifest_metadata.is_file()
        || manifest_metadata.file_type().is_symlink()
        || manifest_metadata.len() > 256 * 1024
    {
        return None;
    }
    let manifest: toml::Table =
        std::fs::read_to_string(manifest_path).ok()?.parse().ok()?;
    if manifest.get("id")?.as_str()? != id
        || !matches!(manifest.get("schema_version")?.as_integer()?, 0 | 1)
    {
        return None;
    }
    let has_language_servers = manifest
        .get("language_servers")
        .and_then(toml::Value::as_table)
        .is_some_and(|servers| !servers.is_empty());
    let has_icon_themes = manifest
        .get("icon_themes")
        .and_then(toml::Value::as_array)
        .is_some_and(|themes| {
            themes.iter().filter_map(toml::Value::as_str).any(|theme| {
                ahead_extension_host::load_icon_theme(directory, Path::new(theme))
                    .is_ok()
            })
        });
    let api_version = manifest
        .get("lib")
        .and_then(toml::Value::as_table)
        .and_then(|lib| lib.get("version"))
        .and_then(toml::Value::as_str);
    let has_wasm = std::fs::symlink_metadata(directory.join("extension.wasm"))
        .is_ok_and(|metadata| {
            metadata.is_file() && !metadata.file_type().is_symlink()
        });
    if (!has_language_servers && !has_icon_themes)
        || (has_language_servers
            && (!has_wasm
                || !matches!(api_version, Some("0.6.0" | "0.7.0" | "0.8.0"))))
    {
        return None;
    }
    let version = manifest.get("version")?.as_str()?.to_string();
    let entry = LanguageExtensionEntry {
        id: id.to_string(),
        name: manifest.get("name")?.as_str()?.to_string(),
        version: version.clone(),
        description: manifest
            .get("description")
            .and_then(toml::Value::as_str)
            .map(str::to_string),
        schema_version: Some(1),
        wasm_api_version: api_version.map(str::to_string),
        provides: [
            has_language_servers.then_some("language-servers"),
            has_icon_themes.then_some("icon-themes"),
        ]
        .into_iter()
        .flatten()
        .map(str::to_string)
        .collect(),
        archive_sha256: None,
        installed_version: Some(version),
        from_catalog: false,
    };
    extension_download_url(&entry)?;
    Some(entry)
}

fn installed_language_extensions_in(root: &Path) -> Vec<LanguageExtensionEntry> {
    if !std::fs::symlink_metadata(root).is_ok_and(|metadata| metadata.is_dir()) {
        return Vec::new();
    }
    let Ok(directories) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut entries = directories
        .flatten()
        .filter_map(|directory| {
            let id = directory.file_name().into_string().ok()?;
            installed_language_extension(&directory.path(), &id)
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries
}

fn installed_language_extensions() -> Vec<LanguageExtensionEntry> {
    ahead_core::directory::Directory::plugins_directory()
        .map(|root| installed_language_extensions_in(&root))
        .unwrap_or_default()
}

fn merge_language_extension_catalog(
    mut online: Vec<LanguageExtensionEntry>,
    installed: Vec<LanguageExtensionEntry>,
) -> Vec<LanguageExtensionEntry> {
    for local in installed {
        if let Some(remote) = online.iter_mut().find(|entry| entry.id == local.id) {
            remote.installed_version = local.installed_version;
        } else {
            online.push(local);
        }
    }
    online
}

fn language_extension_catalog_url(query: &str, provides: &str) -> reqwest::Url {
    let mut url = reqwest::Url::parse(LANGUAGE_EXTENSION_CATALOG_URL)
        .expect("fixed extension catalog URL is valid");
    url.query_pairs_mut()
        .append_pair("max_schema_version", "1")
        .append_pair("provides", provides);
    if !query.trim().is_empty() {
        url.query_pairs_mut().append_pair("filter", query.trim());
    }
    url
}

fn fetch_language_extension_catalog(
    query: String,
) -> Result<Vec<LanguageExtensionEntry>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|error| error.to_string())?;
    let mut online = Vec::new();
    for provides in ["language-servers", "icon-themes"] {
        let mut response = client
            .get(language_extension_catalog_url(&query, provides))
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|error| error.to_string())?;
        let mut body = Vec::new();
        response
            .by_ref()
            .take(MAX_EXTENSION_CATALOG_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|error| error.to_string())?;
        if body.len() as u64 > MAX_EXTENSION_CATALOG_BYTES {
            return Err("extension catalog exceeds the 4 MiB limit".into());
        }
        for entry in parse_language_extension_catalog(&body)? {
            if !online
                .iter()
                .any(|existing: &LanguageExtensionEntry| existing.id == entry.id)
            {
                online.push(entry);
            }
        }
    }
    let installed = installed_language_extensions()
        .into_iter()
        .filter(|entry| {
            online.iter().any(|remote| remote.id == entry.id)
                || !matching_language_extensions(
                    std::slice::from_ref(entry),
                    &query,
                    LanguageExtensionFilter::All,
                )
                .is_empty()
        })
        .collect();
    Ok(merge_language_extension_catalog(online, installed))
}

fn matching_language_extensions<'a>(
    entries: &'a [LanguageExtensionEntry],
    query: &str,
    filter: LanguageExtensionFilter,
) -> Vec<&'a LanguageExtensionEntry> {
    let query = query.trim().to_lowercase();
    entries
        .iter()
        .filter(|entry| {
            let matches_filter = match filter {
                LanguageExtensionFilter::All => true,
                LanguageExtensionFilter::Installed => {
                    entry.installed_version.is_some()
                }
                LanguageExtensionFilter::NotInstalled => {
                    entry.installed_version.is_none()
                }
            };
            matches_filter
                && (query.is_empty()
                    || entry.name.to_lowercase().contains(&query)
                    || entry.id.to_lowercase().contains(&query)
                    || entry.description.as_deref().is_some_and(|description| {
                        description.to_lowercase().contains(&query)
                    }))
        })
        .collect()
}

fn builtin_setup_notices(
    servers: Vec<crate::proxy_client::LspServerStatus>,
) -> Vec<(&'static str, crate::proxy_client::LspServerStatus)> {
    servers
        .into_iter()
        .filter_map(|server| {
            let language = match server.name.as_str() {
                "basedpyright-langserver" => "Python",
                "vtsls" => "JavaScript / TypeScript",
                _ => return None,
            };
            (server.is_error()
                || (!server.is_ready()
                    && server.message.as_deref().is_some_and(|message| {
                        message.starts_with("Installing from npm")
                    })))
            .then_some((language, server))
        })
        .collect()
}

pub struct ExtensionsPanel {
    focus: FocusHandle,
    pub search_input: Entity<InputState>,
    catalog: Vec<LanguageExtensionEntry>,
    filter: LanguageExtensionFilter,
    visible_limit: usize,
    pub(crate) catalog_busy: bool,
    catalog_loaded: bool,
    catalog_query: String,
    catalog_revision: u64,
    installing: Option<String>,
    install_failed: Option<String>,
    status: SharedString,
    proxy: Arc<crate::proxy_client::ProxyClient>,
    _server_updates: Task<()>,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
    close_handler: Option<Rc<dyn Fn(PanelId, &mut Window, &mut App)>>,
    icon_theme_changed_handler: Option<Rc<dyn Fn(&mut App)>>,
}

impl ExtensionsPanel {
    pub fn new(
        proxy: Arc<crate::proxy_client::ProxyClient>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe_in(
            &search_input,
            window,
            |this: &mut Self, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.visible_limit = 20;
                    this.catalog_revision = this.catalog_revision.wrapping_add(1);
                    if this.catalog_loaded || this.catalog_busy {
                        if this.search_input.read(cx).value().trim().is_empty() {
                            this.refresh_language_extensions(cx);
                        } else {
                            let revision = this.catalog_revision;
                            cx.spawn(async move |this, cx| {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(250))
                                    .await;
                                let _ = this.update(cx, |this: &mut Self, cx| {
                                    if this.catalog_revision == revision {
                                        this.refresh_language_extensions(cx);
                                    }
                                });
                            })
                            .detach();
                        }
                    }
                    cx.notify();
                }
            },
        )
        .detach();
        let updates = proxy.subscribe_diagnostics();
        let server_updates = cx.spawn(async move |this, cx| {
            while updates.recv().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let installed = installed_language_extensions();
        Self {
            focus: cx.focus_handle(),
            search_input,
            catalog: installed.clone(),
            filter: LanguageExtensionFilter::All,
            visible_limit: 20,
            catalog_busy: false,
            catalog_loaded: false,
            catalog_query: String::new(),
            catalog_revision: 0,
            installing: None,
            install_failed: None,
            status: format!(
                "{} installed extensions · Load catalog to browse online",
                installed.len()
            )
            .into(),
            proxy,
            _server_updates: server_updates,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
            close_handler: None,
            icon_theme_changed_handler: None,
        }
    }

    pub fn refresh_language_extensions(&mut self, cx: &mut Context<Self>) {
        self.catalog_revision = self.catalog_revision.wrapping_add(1);
        let revision = self.catalog_revision;
        let query = self.search_input.read(cx).value().trim().to_string();
        self.catalog_busy = true;
        self.status = "Loading extensions…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let requested_query = query.clone();
            let result = cx
                .background_spawn(async move {
                    fetch_language_extension_catalog(query)
                })
                .await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                if this.catalog_revision != revision {
                    return;
                }
                this.catalog_busy = false;
                match result {
                    Ok(entries) => {
                        this.status =
                            format!("{} extension candidates", entries.len()).into();
                        this.catalog = entries;
                        this.catalog_loaded = true;
                        this.catalog_query = requested_query;
                    }
                    Err(error) => {
                        this.status =
                            format!("Extension catalog unavailable · {error}. Retry to refresh.")
                                .into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn load_catalog_if_needed(&mut self, cx: &mut Context<Self>) {
        if !self.catalog_loaded && !self.catalog_busy {
            self.refresh_language_extensions(cx);
        }
    }

    pub fn install_language_extension(
        &mut self,
        extension_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.installing.is_some() {
            return;
        }
        let proxy = self.proxy.clone();
        let Some(entry) = self.catalog.iter().find(|entry| entry.id == extension_id)
        else {
            self.status = "Select an extension from the gallery".into();
            cx.notify();
            return;
        };
        if entry.installed_version.is_some() && !extension_update_available(entry) {
            self.status = format!(
                "{} is already installed at this version or newer",
                entry.name
            )
            .into();
            cx.notify();
            return;
        }
        let Some(url) = extension_download_url(entry) else {
            self.status = "Invalid extension package address".into();
            cx.notify();
            return;
        };
        let extension_id = entry.id.clone();
        let version = entry.version.clone();
        let expected_sha256 = entry.archive_sha256.clone();
        let icon_theme = entry.provides.iter().any(|item| item == "icon-themes");
        let restart_language_servers =
            entry.provides.iter().any(|item| item == "language-servers");

        self.installing = Some(extension_id.clone());
        self.install_failed = None;
        self.status = format!("Installing {extension_id}…").into();
        cx.notify();
        let (tx, rx) = async_channel::bounded(1);
        proxy.install_extension(
            url,
            extension_id.clone(),
            version.clone(),
            expected_sha256,
            restart_language_servers,
            move |result| {
                if let Err(error) = tx.try_send(result) {
                    eprintln!("Extension install result dropped: {error}");
                }
            },
        );
        cx.spawn(async move |this, cx| {
            let result = rx
                .recv()
                .await
                .unwrap_or_else(|_| Err("AHEAD proxy connection closed".to_string()));
            let _ = this.update(cx, |this: &mut Self, cx| {
                this.installing = None;
                this.status = match result {
                    Ok(()) => {
                        if let Some(entry) = this
                            .catalog
                            .iter_mut()
                            .find(|entry| entry.id == extension_id)
                        {
                            entry.installed_version = Some(version);
                        }
                        if icon_theme {
                            match crate::icon_theme::select_icon_theme(&extension_id) {
                                Ok(()) => {
                                    if let Some(handler) = &this.icon_theme_changed_handler {
                                        handler(cx);
                                    }
                                    format!("{extension_id} installed and in use").into()
                                }
                                Err(error) => format!("{extension_id} installed · icon theme could not be activated: {error}").into(),
                            }
                        } else {
                            format!("{extension_id} installed · reloading language servers").into()
                        }
                    }
                    Err(error) => {
                        this.install_failed = Some(extension_id.clone());
                        format!("Extension install failed · {error}").into()
                    }
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn use_icon_theme(&mut self, extension_id: &str, cx: &mut Context<Self>) {
        self.status = match crate::icon_theme::select_icon_theme(extension_id) {
            Ok(()) => {
                if let Some(handler) = &self.icon_theme_changed_handler {
                    handler(cx);
                }
                format!("{extension_id} is now the file icon theme").into()
            }
            Err(error) => format!("Could not use icon theme: {error}").into(),
        };
        cx.notify();
    }

    pub fn set_icon_theme_changed_handler(
        &mut self,
        handler: impl Fn(&mut App) + 'static,
    ) {
        self.icon_theme_changed_handler = Some(Rc::new(handler));
    }

    fn render_extension_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let text = cx.theme().sidebar_foreground;
        let extension_query = self.search_input.read(cx).value().to_string();
        let local_query =
            if self.catalog_loaded && self.catalog_query == extension_query.trim() {
                ""
            } else {
                &extension_query
            };
        let matching_extensions =
            matching_language_extensions(&self.catalog, local_query, self.filter);
        let matching_count = matching_extensions.len();
        let visible_count = matching_count.min(self.visible_limit);
        let visible_extensions = matching_extensions
            .into_iter()
            .take(visible_count)
            .cloned()
            .collect::<Vec<_>>();
        let extension_install_busy = self.installing.is_some();
        let builtin_setup = builtin_setup_notices(self.proxy.lsp_servers());
        let setup_failed = builtin_setup.iter().any(|(_, server)| server.is_error());

        v_flex()
            .w_full()
            .p_4()
            .gap_4()
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                    .child("Browse language servers and icon themes from Zed's catalog. Language server compatibility remains a preview; a matching API version does not verify startup in AHEAD."),
                    )
                    .when(!builtin_setup.is_empty(), |content| {
                        content.child(
                            v_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .font_weight(FontWeight::BOLD)
                                        .text_size(px(11.))
                                        .text_color(text)
                                        .child("Built-in language support"),
                                )
                                .children(builtin_setup.into_iter().map(|(language, server)| {
                                    let failed = server.is_error();
                                    let message = if failed {
                                        format!(
                                            "{language} setup failed: {}",
                                            server.message.as_deref().unwrap_or("Unknown error")
                                        )
                                    } else {
                                        format!("Installing {language} language server…")
                                    };
                                    h_flex()
                                        .gap_2()
                                        .items_start()
                                        .child(if failed {
                                            IconName::TriangleAlert
                                        } else {
                                            IconName::CircleDashed
                                        })
                                        .child(div().min_w_0().text_color(if failed {
                                            cx.theme().danger
                                        } else {
                                            cx.theme().muted_foreground
                                        }).child(message))
                                }))
                                .when(setup_failed, |notices| {
                                    notices.child(
                                        Button::new("retry_builtin_language_support")
                                            .icon(IconName::RefreshCw)
                                            .label("Retry setup")
                                            .ghost()
                                            .tooltip("Restart language servers and retry built-in setup")
                                            .on_click(cx.listener(|this: &mut Self, _, _, _| {
                                                this.proxy.restart_language_servers();
                                            })),
                                    )
                                }),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .items_end()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        div()
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .text_size(px(11.))
                                            .text_color(text)
                                            .child("Search extensions"),
                                    )
                                    .child(
                                        Input::new(&self.search_input)
                                            .aria_label("Search extensions"),
                                    ),
                            )
                            .child(
                                Button::new("refresh_language_extensions")
                                    .icon(IconName::RefreshCw)
                                    .label(if !self.catalog_loaded {
                                        "Load catalog"
                                    } else {
                                        "Refresh"
                                    })
                                    .tooltip("Load extensions from Zed's catalog")
                                    .disabled(self.catalog_busy)
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                        this.refresh_language_extensions(cx);
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .child(
                                Button::new("extension_filter_all")
                                    .label("All")
                                    .ghost()
                                    .selected(self.filter == LanguageExtensionFilter::All)
                                    .tooltip("Show all extensions")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                        this.filter = LanguageExtensionFilter::All;
                                        this.visible_limit = 20;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("extension_filter_installed")
                                    .label("Installed")
                                    .ghost()
                                    .selected(self.filter == LanguageExtensionFilter::Installed)
                                    .tooltip("Show installed extensions")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                        this.filter = LanguageExtensionFilter::Installed;
                                        this.visible_limit = 20;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("extension_filter_not_installed")
                                    .label("Not Installed")
                                    .ghost()
                                    .selected(self.filter == LanguageExtensionFilter::NotInstalled)
                                    .tooltip("Show extensions available to install")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                        this.filter = LanguageExtensionFilter::NotInstalled;
                                        this.visible_limit = 20;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .children(visible_extensions.into_iter().map(|entry| {
                                let extension_id = entry.id.clone();
                                let update_available = extension_update_available(&entry);
                                let installed = entry.installed_version.is_some() && !update_available;
                                let icon_theme = entry.provides.iter().any(|item| item == "icon-themes");
                                let active_icon_theme = icon_theme && crate::icon_theme::active_icon_theme_id() == extension_id;
                                let action = if self.installing.as_deref()
                                    == Some(extension_id.as_str())
                                {
                                    "Installing…"
                                } else if self.install_failed.as_deref()
                                    == Some(extension_id.as_str())
                                {
                                    "Retry"
                                } else if installed && icon_theme {
                                    if active_icon_theme { "In Use" } else { "Use" }
                                } else if installed {
                                    "Installed"
                                } else if update_available {
                                    "Update"
                                } else {
                                    "Install"
                                };
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .min_w_0()
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .min_w_0()
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .truncate()
                                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                                            .text_size(px(12.))
                                                            .text_color(text)
                                                            .child(entry.name),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(10.))
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(format!("v{}", if installed {
                                                                entry.installed_version.as_deref().unwrap_or(&entry.version)
                                                            } else {
                                                                &entry.version
                                                            })),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_size(px(11.))
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(entry.description.unwrap_or_default()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.))
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(format!(
                                                        "{} · {}",
                                                        if entry.from_catalog {
                                                            "Zed catalog"
                                                        } else {
                                                            "Installed locally"
                                                        },
                                                        if icon_theme { "Icon theme".to_string() } else { format!("API {} · compatibility candidate", entry.wasm_api_version.as_deref().unwrap_or("unknown")) }
                                                    )),
                                            ),
                                    )
                                    .child(
                                        Button::new(format!("install_extension_{extension_id}"))
                                            .label(action)
                                            .tooltip(format!("{action} {extension_id}"))
                                            .disabled((installed && (!icon_theme || active_icon_theme)) || extension_install_busy)
                                            .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                if installed && icon_theme {
                                                    this.use_icon_theme(&extension_id, cx);
                                                } else {
                                                    this.install_language_extension(&extension_id, cx);
                                                }
                                            })),
                                    )
                            })),
                    )
                    .when(matching_count == 0 && !self.catalog_busy, |card| {
                        card.child(
                            div()
                                .text_size(px(11.))
                                .text_color(cx.theme().muted_foreground)
                                .child(if self.catalog.is_empty() {
                                    "No extensions installed. Load the online catalog to browse."
                                } else if self.filter == LanguageExtensionFilter::Installed {
                                    "No installed extensions match this search."
                                } else if self.filter == LanguageExtensionFilter::NotInstalled {
                                    "No uninstalled extensions match this search."
                                } else {
                                    "No extensions match this search."
                                }),
                        )
                    })
                    .when(matching_count > visible_count, |card| {
                        card.child(
                            Button::new("show_more_language_extensions")
                                .label(format!(
                                    "Show more ({visible_count} of {matching_count})"
                                ))
                                .tooltip("Show more matching extensions")
                                .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                    this.visible_limit += 20;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(self.status.clone()),
                    ),
            )
    }

    pub fn set_close_handler<F>(&mut self, handler: F)
    where
        F: Fn(PanelId, &mut Window, &mut App) + 'static,
    {
        self.close_handler = Some(Rc::new(handler));
    }
}

impl BasePanel for ExtensionsPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_extensions"
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

impl Panel for ExtensionsPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let group = self.tab_group.clone();
        let panel_id = self.panel_id;
        let close_handler = self.close_handler.clone();
        h_flex().items_center().gap_1().child("Extensions").child(
            Button::new("close_extensions")
                .icon(IconName::X)
                .ghost()
                .tooltip("Close Extensions")
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

impl EventEmitter<PanelEvent> for ExtensionsPanel {}

impl Focusable for ExtensionsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ExtensionsPanel {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let gallery = self.render_extension_content(cx).into_any_element();
        v_flex()
            .size_full()
            .min_h_0()
            .track_focus(&self.focus)
            .child(
                div()
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(18.))
                    .text_color(cx.theme().sidebar_foreground)
                    .child("Extensions"),
            )
            .child(
                div()
                    .id("extensions-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(gallery),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ExtensionsPanel, LanguageExtensionFilter, builtin_setup_notices,
        extension_download_url, extension_update_available,
        installed_language_extensions_in, language_extension_catalog_url,
        matching_language_extensions, merge_language_extension_catalog,
        parse_language_extension_catalog,
    };
    use crate::proxy_client::LspServerStatus;

    #[test]
    fn builtin_setup_notices_only_show_installing_and_failed_managed_servers() {
        let statuses = [
            (
                "basedpyright-langserver",
                false,
                false,
                Some("Installing from npm into AHEAD's cache…"),
            ),
            ("vtsls", false, true, Some("npm could not install vtsls")),
            ("rust-analyzer", false, true, Some("failed")),
            ("basedpyright-langserver", true, true, None),
            ("vtsls", false, false, None),
        ]
        .into_iter()
        .map(|(name, ready, quiescent, message)| LspServerStatus {
            name: name.into(),
            ready,
            quiescent,
            message: message.map(str::to_string),
        })
        .collect();
        let notices = builtin_setup_notices(statuses);
        assert_eq!(notices.len(), 2);
        assert_eq!(notices[0].0, "Python");
        assert_eq!(notices[1].0, "JavaScript / TypeScript");
    }

    #[test]
    fn gallery_search_uses_zeds_filtered_catalog_endpoint() {
        let url =
            language_extension_catalog_url("  C++ & Rust  ", "language-servers");
        assert_eq!(url.path(), "/extensions");
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![
                ("max_schema_version".into(), "1".into()),
                ("provides".into(), "language-servers".into()),
                ("filter".into(), "C++ & Rust".into()),
            ]
        );
        assert_eq!(
            language_extension_catalog_url(" ", "icon-themes")
                .query_pairs()
                .collect::<Vec<_>>(),
            vec![
                ("max_schema_version".into(), "1".into()),
                ("provides".into(), "icon-themes".into()),
            ]
        );
    }

    #[test]
    fn catalog_accepts_icon_only_extension_without_wasm_api() {
        let catalog = serde_json::json!({"data": [{
            "id": "material-icon-theme", "name": "Material Icon Theme", "version": "1.3.1",
            "schema_version": 1, "wasm_api_version": null, "provides": ["icon-themes"]
        }]});
        let entries =
            parse_language_extension_catalog(catalog.to_string().as_bytes())
                .expect("catalog");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "material-icon-theme");
        assert_eq!(entries[0].wasm_api_version, None);
    }

    #[test]
    fn installed_icon_only_extension_appears_offline() {
        let root = tempfile::tempdir().expect("extension root");
        let extension = root.path().join("material-icon-theme");
        std::fs::create_dir_all(extension.join("icon_themes")).expect("themes");
        std::fs::create_dir(extension.join("icons")).expect("icons");
        std::fs::write(extension.join("extension.toml"),
            "id = 'material-icon-theme'\nname = 'Material Icon Theme'\nversion = '1.3.1'\nschema_version = 1\nicon_themes = ['icon_themes/material.json']\n[lib]\n")
            .expect("manifest");
        std::fs::write(extension.join("icon_themes/material.json"),
            r#"{"themes":[{"name":"Material Icon Theme","file_icons":{"rust":{"path":"./icons/rust.svg"}},"file_suffixes":{"rs":"rust"}}]}"#)
            .expect("theme");
        std::fs::write(extension.join("icons/rust.svg"), "<svg/>").expect("icon");
        let installed = installed_language_extensions_in(root.path());
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].id, "material-icon-theme");
        assert_eq!(installed[0].provides, ["icon-themes"]);
        assert_eq!(installed[0].wasm_api_version, None);
    }

    #[test]
    fn language_extension_gallery_keeps_only_installable_compatible_packages() {
        let catalog = serde_json::json!({
            "data": [
                {"id":"html","name":"HTML","version":"0.3.2","description":"HTML support", "schema_version":1,"wasm_api_version":"0.7.0","provides":["languages","language-servers"],"archive_sha256":"a".repeat(64)},
                {"id":"future","name":"Future","version":"1.0.0","schema_version":1,"wasm_api_version":"0.9.0","provides":["language-servers"]},
                {"id":"theme","name":"Theme","version":"1.0.0","schema_version":1,"wasm_api_version":"0.7.0","provides":["themes"]},
                {"id":"bad-version","name":"Bad version","version":"not-semver","schema_version":1,"wasm_api_version":"0.7.0","provides":["language-servers"]},
                {"id":"../escape","name":"Escape","version":"1.0.0","schema_version":1,"wasm_api_version":"0.7.0","provides":["language-servers"]},
                {"id":"new-schema","name":"New schema","version":"1.0.0","schema_version":2,"wasm_api_version":"0.7.0","provides":["language-servers"]},
                {"id":"bad-digest","name":"Bad digest","version":"1.0.0","schema_version":1,"wasm_api_version":"0.7.0","provides":["language-servers"],"archive_sha256":"wrong"}
            ]
        });
        let entries =
            parse_language_extension_catalog(catalog.to_string().as_bytes())
                .expect("catalog parses");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "html");
        assert_eq!(
            entries[0].archive_sha256.as_deref(),
            Some("a".repeat(64).as_str())
        );
        assert!(entries[0].from_catalog);
        let mut installed_entry = entries[0].clone();
        installed_entry.installed_version = Some("0.4.0".into());
        assert!(!extension_update_available(&installed_entry));
        installed_entry.installed_version = Some("0.3.2".into());
        assert!(!extension_update_available(&installed_entry));
        installed_entry.installed_version = Some("0.3.2-alpha.1".into());
        assert!(extension_update_available(&installed_entry));
        installed_entry.installed_version = Some("unknown".into());
        assert!(!extension_update_available(&installed_entry));
        assert_eq!(
            extension_download_url(&entries[0]).as_deref(),
            Some("https://api.zed.dev/extensions/html/0.3.2/download")
        );
        assert_eq!(
            matching_language_extensions(
                &entries,
                "html",
                LanguageExtensionFilter::All
            )
            .len(),
            1
        );
        assert_eq!(
            matching_language_extensions(
                &entries,
                "support",
                LanguageExtensionFilter::All
            )
            .len(),
            1
        );
        assert!(
            matching_language_extensions(
                &entries,
                "python",
                LanguageExtensionFilter::All
            )
            .is_empty()
        );
        assert!(
            matching_language_extensions(
                &entries,
                "",
                LanguageExtensionFilter::Installed
            )
            .is_empty()
        );
        assert_eq!(
            matching_language_extensions(
                &entries,
                "",
                LanguageExtensionFilter::NotInstalled
            )
            .len(),
            1
        );
        let mut installed = entries.clone();
        installed[0].installed_version = Some(installed[0].version.clone());
        assert_eq!(
            matching_language_extensions(
                &installed,
                "",
                LanguageExtensionFilter::Installed
            )
            .len(),
            1
        );
        assert!(
            matching_language_extensions(
                &installed,
                "",
                LanguageExtensionFilter::NotInstalled
            )
            .is_empty()
        );
    }

    #[gpui_kit::test]
    fn gallery_install_sends_pinned_package_and_reloads_language_servers(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::proxy::{
            ProxyNotification, ProxyRequest, ProxyResponse, ProxyRpc,
        };

        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("workspace");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            workspace.path().to_owned(),
        );
        let (panel, cx) = cx.add_window_view(|window, cx| {
            ExtensionsPanel::new(proxy.clone(), window, cx)
        });
        let catalog = serde_json::json!({"data": [{
            "id": "html", "name": "HTML", "version": "0.3.2",
            "schema_version": 1, "wasm_api_version": "0.7.0",
            "provides": ["language-servers"], "archive_sha256": "a".repeat(64)
        }]});
        let entries =
            parse_language_extension_catalog(catalog.to_string().as_bytes())
                .expect("catalog");
        panel.update(cx, |panel, cx| {
            panel.catalog = entries;
            panel.install_language_extension("html", cx);
        });
        let rpc = proxy.rpc_for_test();
        let ProxyRpc::Request(
            request_id,
            ProxyRequest::InstallLanguageExtension {
                url,
                extension_id,
                version,
                expected_sha256,
            },
        ) = rpc.rx().try_recv().expect("install request")
        else {
            panic!("expected language-extension install request");
        };
        assert_eq!(extension_id, "html");
        assert_eq!(version, "0.3.2");
        assert_eq!(url, "https://api.zed.dev/extensions/html/0.3.2/download");
        assert_eq!(expected_sha256.as_deref(), Some("a".repeat(64).as_str()));
        rpc.handle_response(
            request_id,
            Err(ahead_rpc::RpcError {
                code: 0,
                message: "offline".into(),
            }),
        );
        cx.run_until_parked();
        assert!(
            rpc.rx().try_recv().is_err(),
            "failed install must not restart LSPs"
        );
        panel.update(cx, |panel, cx| {
            assert_eq!(panel.install_failed.as_deref(), Some("html"));
            assert!(panel.installing.is_none());
            panel.install_language_extension("html", cx);
        });
        let ProxyRpc::Request(
            retry_id,
            ProxyRequest::InstallLanguageExtension { extension_id, .. },
        ) = rpc.rx().try_recv().expect("retry request")
        else {
            panic!("expected language-extension retry request");
        };
        assert_eq!(extension_id, "html");
        rpc.handle_response(retry_id, Ok(ProxyResponse::Success {}));
        cx.run_until_parked();
        assert!(matches!(
            rpc.rx().try_recv(),
            Ok(ProxyRpc::Notification(
                ProxyNotification::RestartLanguageServers {}
            ))
        ));
        panel.update(cx, |panel, cx| {
            assert_eq!(panel.catalog[0].installed_version.as_deref(), Some("0.3.2"));
            assert!(panel.installing.is_none());
            assert!(panel.install_failed.is_none());
            panel.catalog[0].installed_version = Some("0.4.0".into());
            panel.install_failed = Some("html".into());
            panel.install_language_extension("html", cx);
            assert!(panel.status.contains("or newer"));
        });
        assert!(
            rpc.rx().try_recv().is_err(),
            "an older catalog version must not downgrade an installed extension"
        );
    }

    #[test]
    fn language_extension_gallery_keeps_installed_packages_offline() {
        let root = tempfile::tempdir().expect("extension root");
        let html = root.path().join("html");
        std::fs::create_dir(&html).expect("HTML extension directory");
        std::fs::write(
            html.join("extension.toml"),
            "id = 'html'\nname = 'HTML'\nversion = '0.2.0'\nschema_version = 1\ndescription = 'HTML support'\n[lib]\nversion = '0.7.0'\n[language_servers.html]\nlanguages = ['HTML']\n",
        )
        .expect("HTML extension manifest");
        std::fs::write(html.join("extension.wasm"), b"test")
            .expect("HTML extension module");
        let installed = installed_language_extensions_in(root.path());
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].installed_version.as_deref(), Some("0.2.0"));
        assert!(!installed[0].from_catalog);
        #[cfg(unix)]
        {
            let linked_root = root.path().join("linked-root");
            std::os::unix::fs::symlink(root.path(), &linked_root)
                .expect("symlink extension root");
            assert!(installed_language_extensions_in(&linked_root).is_empty());
        }
        let offline =
            merge_language_extension_catalog(Vec::new(), installed.clone());
        assert_eq!(offline[0].name, "HTML");

        let remote = serde_json::json!({"data": [{
            "id": "html", "name": "HTML", "version": "0.3.0",
            "schema_version": 1, "wasm_api_version": "0.7.0",
            "provides": ["language-servers"]
        }]});
        let online = parse_language_extension_catalog(remote.to_string().as_bytes())
            .expect("online catalog");
        let merged = merge_language_extension_catalog(online, installed);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].version, "0.3.0");
        assert_eq!(merged[0].installed_version.as_deref(), Some("0.2.0"));
        assert!(merged[0].from_catalog);
    }
}
