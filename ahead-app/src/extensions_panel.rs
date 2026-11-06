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

const AHEAD_EXTENSION_CATALOG_URL: &str = "https://raw.githubusercontent.com/Kade-Powell/ahead/main/extensions/registry.json";
const ZED_EXTENSION_CATALOG_URL: &str = "https://api.zed.dev/extensions";
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
    #[serde(default)]
    runtime_dependencies: Vec<String>,
    #[serde(default)]
    requested_capabilities: Vec<ExtensionRequestedCapability>,
    archive_sha256: Option<String>,
    #[serde(default)]
    download_url: Option<String>,
    #[serde(skip)]
    installed_version: Option<String>,
    #[serde(skip)]
    from_catalog: bool,
    #[serde(skip)]
    from_ahead_registry: bool,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct ExtensionRequestedCapability {
    kind: String,
    package: Option<String>,
}

#[derive(serde::Deserialize)]
struct LanguageExtensionCatalog {
    data: Vec<LanguageExtensionEntry>,
}

struct LanguageExtensionCatalogResult {
    entries: Vec<LanguageExtensionEntry>,
    warning: Option<String>,
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
    if !valid_id || !valid_version {
        return None;
    }
    let url = entry.download_url.clone().unwrap_or_else(|| {
        format!(
            "https://api.zed.dev/extensions/{}/{}/download",
            entry.id, entry.version
        )
    });
    let parsed = reqwest::Url::parse(&url).ok()?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    match (entry.from_ahead_registry, parsed.host_str(), parsed.path()) {
        (true, Some("github.com"), path)
            if path.starts_with("/Kade-Powell/ahead/releases/download/")
                && path.ends_with(&format!(
                    "/{}-{}.tar.gz",
                    entry.id, entry.version
                )) =>
        {
            Some(parsed.to_string())
        }
        (false, Some("api.zed.dev"), path)
            if path
                == format!(
                    "/extensions/{}/{}/download",
                    entry.id, entry.version
                ) =>
        {
            Some(parsed.to_string())
        }
        _ => None,
    }
}

fn parse_language_extension_catalog(
    body: &[u8],
) -> Result<Vec<LanguageExtensionEntry>, String> {
    parse_language_extension_catalog_from(body, false)
}

fn parse_ahead_extension_catalog(
    body: &[u8],
) -> Result<Vec<LanguageExtensionEntry>, String> {
    parse_language_extension_catalog_from(body, true)
}

fn parse_language_extension_catalog_from(
    body: &[u8],
    from_ahead_registry: bool,
) -> Result<Vec<LanguageExtensionEntry>, String> {
    let response: LanguageExtensionCatalog =
        serde_json::from_slice(body).map_err(|error| error.to_string())?;
    Ok(response
        .data
        .into_iter()
        .map(|mut entry| {
            entry.from_ahead_registry = from_ahead_registry;
            entry
        })
        .filter(|entry| {
            entry.schema_version.is_some_and(|version| version <= 1)
                && semver::Version::parse(&entry.version).is_ok()
                && entry.provides.iter().any(|item| item == "language-servers")
                && matches!(
                    entry.wasm_api_version.as_deref(),
                    Some("0.6.0" | "0.7.0" | "0.8.0")
                )
                && entry.archive_sha256.as_deref().is_none_or(|digest| {
                    digest.len() == 64
                        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                && (!from_ahead_registry
                    || (entry.archive_sha256.as_deref().is_some_and(|digest| {
                        digest.len() == 64
                            && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                    }) && entry.download_url.is_some()))
                && extension_download_url(entry).is_some()
        })
        .map(|mut entry| {
            entry.from_catalog = true;
            entry.from_ahead_registry = from_ahead_registry;
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

fn extension_runtime_summary(entry: &LanguageExtensionEntry) -> Option<String> {
    let mut details = Vec::new();
    if !entry.runtime_dependencies.is_empty() {
        details.push(format!(
            "Runtime: {}",
            entry.runtime_dependencies.join(", ")
        ));
    }
    if !entry.requested_capabilities.is_empty() {
        let capabilities = entry
            .requested_capabilities
            .iter()
            .map(|capability| match capability.package.as_deref() {
                Some(package) => format!("{} ({package})", capability.kind),
                None => capability.kind.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        details.push(format!("Host access: {capabilities}"));
    }
    (!details.is_empty()).then(|| details.join(" · "))
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
    let api_version = manifest
        .get("lib")
        .and_then(toml::Value::as_table)
        .and_then(|lib| lib.get("version"))
        .and_then(toml::Value::as_str);
    let has_wasm = std::fs::symlink_metadata(directory.join("extension.wasm"))
        .is_ok_and(|metadata| {
            metadata.is_file() && !metadata.file_type().is_symlink()
        });
    // Current Zed packages embed their API version in the WASM custom section;
    // the extension host validates it when loading the package.
    if !has_language_servers || !has_wasm {
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
        provides: vec!["language-servers".to_string()],
        runtime_dependencies: Vec::new(),
        requested_capabilities: Vec::new(),
        archive_sha256: None,
        download_url: None,
        installed_version: Some(version),
        from_catalog: false,
        from_ahead_registry: false,
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

fn merge_extension_catalog_sources(
    mut zed_catalog: Vec<LanguageExtensionEntry>,
    ahead_catalog: Vec<LanguageExtensionEntry>,
) -> Vec<LanguageExtensionEntry> {
    for entry in ahead_catalog {
        if !zed_catalog.iter().any(|existing| existing.id == entry.id) {
            zed_catalog.push(entry);
        }
    }
    zed_catalog
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

fn extension_catalog_status(
    candidate_count: usize,
    installed_count: usize,
    warning: Option<&str>,
) -> String {
    match warning {
        Some(warning) if candidate_count > 0 => format!(
            "{candidate_count} extension candidates · Some catalog sources failed: {warning}"
        ),
        Some(warning) if installed_count > 0 => format!(
            "Showing {installed_count} installed extensions · online registry unavailable: {warning}"
        ),
        Some(warning) => format!("Extension catalog unavailable · {warning}"),
        None => format!("{candidate_count} extension candidates"),
    }
}

fn zed_extension_catalog_url(query: &str) -> reqwest::Url {
    let mut url = reqwest::Url::parse(ZED_EXTENSION_CATALOG_URL)
        .expect("fixed Zed catalog URL is valid");
    url.query_pairs_mut()
        .append_pair("max_schema_version", "1")
        .append_pair("provides", "language-servers");
    if !query.trim().is_empty() {
        url.query_pairs_mut().append_pair("filter", query.trim());
    }
    url
}

fn fetch_ahead_extension_catalog(
    client: &reqwest::blocking::Client,
) -> Result<Vec<u8>, String> {
    #[cfg(debug_assertions)]
    {
        let local_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../extensions/registry.json");
        match std::fs::read(local_path) {
            Ok(body) => return Ok(body),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    fetch_extension_catalog(client, AHEAD_EXTENSION_CATALOG_URL)
}

fn fetch_language_extension_catalog(
    query: String,
) -> Result<LanguageExtensionCatalogResult, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|error| error.to_string())?;
    let mut errors = Vec::new();
    let zed_catalog =
        fetch_extension_catalog(&client, zed_extension_catalog_url(&query).as_str())
            .and_then(|body| parse_language_extension_catalog(&body));
    let zed_catalog = match zed_catalog {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(format!("Zed catalog: {error}"));
            Vec::new()
        }
    };
    let ahead_catalog = match fetch_ahead_extension_catalog(&client)
        .and_then(|body| parse_ahead_extension_catalog(&body))
    {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(format!("AHEAD registry: {error}"));
            Vec::new()
        }
    };
    let online = merge_extension_catalog_sources(zed_catalog, ahead_catalog);
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
    Ok(LanguageExtensionCatalogResult {
        entries: merge_language_extension_catalog(online, installed),
        warning: (!errors.is_empty()).then(|| errors.join("; ")),
    })
}

fn fetch_extension_catalog(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
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
    Ok(body)
}

fn matching_language_extensions<'a>(
    entries: &'a [LanguageExtensionEntry],
    query: &str,
    filter: LanguageExtensionFilter,
) -> Vec<&'a LanguageExtensionEntry> {
    let query = query.trim().to_lowercase();
    entries
        .iter()
        .filter(|entry| language_extension_matches(entry, &query, filter))
        .collect()
}

fn matching_catalog_extensions<'a>(
    entries: &'a [LanguageExtensionEntry],
    query: &str,
    catalog_query: &str,
    filter: LanguageExtensionFilter,
) -> Vec<&'a LanguageExtensionEntry> {
    let query = query.trim().to_lowercase();
    let catalog_query = catalog_query.trim().to_lowercase();
    entries
        .iter()
        .filter(|entry| {
            let entry_query = if entry.from_ahead_registry || catalog_query != query
            {
                query.as_str()
            } else {
                ""
            };
            language_extension_matches(entry, entry_query, filter)
        })
        .collect()
}

fn language_extension_matches(
    entry: &LanguageExtensionEntry,
    query: &str,
    filter: LanguageExtensionFilter,
) -> bool {
    let matches_filter = match filter {
        LanguageExtensionFilter::All => true,
        LanguageExtensionFilter::Installed => entry.installed_version.is_some(),
        LanguageExtensionFilter::NotInstalled => entry.installed_version.is_none(),
    };
    matches_filter
        && (query.is_empty()
            || entry.name.to_lowercase().contains(query)
            || entry.id.to_lowercase().contains(query)
            || entry.description.as_deref().is_some_and(|description| {
                description.to_lowercase().contains(query)
            }))
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
                            || message.starts_with("Repairing AHEAD's cached")
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
    uninstalling: Option<String>,
    status: SharedString,
    proxy: Arc<crate::proxy_client::ProxyClient>,
    _server_updates: Task<()>,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
    close_handler: Option<Rc<dyn Fn(PanelId, &mut Window, &mut App)>>,
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
            uninstalling: None,
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
                    Ok(result) => {
                        let candidate_count = result
                            .entries
                            .iter()
                            .filter(|entry| entry.from_catalog)
                            .count();
                        let installed_count = result
                            .entries
                            .iter()
                            .filter(|entry| entry.installed_version.is_some())
                            .count();
                        this.status = extension_catalog_status(
                            candidate_count,
                            installed_count,
                            result.warning.as_deref(),
                        )
                        .into();
                        this.catalog = result.entries;
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
            move |result| {
                if let Err(error) = tx.try_send(result) {
                    eprintln!("Extension install result dropped: {error}");
                }
            },
        );
        cx.spawn(async move |this, cx| {
            let result = rx.recv().await.unwrap_or_else(|_| {
                Err("AHEAD proxy connection closed".to_string())
            });
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
                        format!(
                            "{extension_id} installed · reloading language servers"
                        )
                        .into()
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

    fn uninstall_language_extension(
        &mut self,
        extension_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.installing.is_some() || self.uninstalling.is_some() {
            return;
        }
        if !self.catalog.iter().any(|entry| {
            entry.id == extension_id && entry.installed_version.is_some()
        }) {
            self.status = "Select an installed extension to uninstall".into();
            cx.notify();
            return;
        }

        let extension_id = extension_id.to_string();
        self.uninstalling = Some(extension_id.clone());
        self.status = format!("Uninstalling {extension_id}…").into();
        cx.notify();
        let (tx, rx) = async_channel::bounded(1);
        self.proxy
            .uninstall_extension(extension_id.clone(), move |result| {
                if let Err(error) = tx.try_send(result) {
                    eprintln!("Extension uninstall result dropped: {error}");
                }
            });
        cx.spawn(async move |this, cx| {
            let result = rx.recv().await.unwrap_or_else(|_| {
                Err("AHEAD proxy connection closed".to_string())
            });
            let _ = this.update(cx, |this: &mut Self, cx| {
                this.uninstalling = None;
                match result {
                    Ok(()) => {
                        this.catalog.retain(|entry| {
                            entry.id != extension_id || entry.from_catalog
                        });
                        if let Some(entry) = this
                            .catalog
                            .iter_mut()
                            .find(|entry| entry.id == extension_id)
                        {
                            entry.installed_version = None;
                        }
                        this.status = format!(
                            "{extension_id} uninstalled · reloading language servers"
                        )
                        .into();
                    }
                    Err(error) => {
                        this.status =
                            format!("Extension uninstall failed · {error}").into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_extension_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let text = cx.theme().sidebar_foreground;
        let extension_query = self.search_input.read(cx).value().to_string();
        let matching_extensions = matching_catalog_extensions(
            &self.catalog,
            &extension_query,
            &self.catalog_query,
            self.filter,
        );
        let matching_count = matching_extensions.len();
        let visible_count = matching_count.min(self.visible_limit);
        let visible_extensions = matching_extensions
            .into_iter()
            .take(visible_count)
            .cloned()
            .collect::<Vec<_>>();
        let extension_install_busy = self.installing.is_some();
        let extension_uninstall_busy = self.uninstalling.is_some();
        let builtin_setup = builtin_setup_notices(self.proxy.lsp_servers());

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
                    .child("Live catalog and downloads are served by Zed. AHEAD is independent and not endorsed by Zed; language-server extensions are compatibility candidates, not guaranteed to work."),
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
                                    let repairing = server.message.as_deref().is_some_and(|message| {
                                        message.starts_with("Repairing AHEAD's cached")
                                    });
                                    let message = if failed {
                                        format!(
                                            "{language} language server failed: {}",
                                            server.message.as_deref().unwrap_or("Unknown error")
                                        )
                                    } else if repairing {
                                        format!("Repairing {language} language-server cache…")
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
                                        .when(failed, |row| {
                                            let server_id = server.name.clone();
                                            row.child(
                                                Button::new(format!("repair_{}", server.name))
                                                    .icon(IconName::RefreshCw)
                                                    .label("Repair cache")
                                                    .ghost()
                                                    .tooltip("Reinstall the AHEAD-managed npm package; system-installed servers are left untouched")
                                                    .on_click(cx.listener(move |this: &mut Self, _, _, _| {
                                                        this.proxy.repair_language_server(server_id.clone());
                                                    })),
                                            )
                                        })
                                }))
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
                                    .tooltip("Load the live Zed catalog and AHEAD-reviewed additions")
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
                                let runtime_summary = extension_runtime_summary(&entry);
                                let update_available = extension_update_available(&entry);
                                let installed = entry.installed_version.is_some() && !update_available;
                                let can_uninstall = entry.installed_version.is_some();
                                let uninstalling = self.uninstalling.as_deref()
                                    == Some(extension_id.as_str());
                                let action = if self.installing.as_deref()
                                    == Some(extension_id.as_str())
                                {
                                    "Installing…"
                                } else if self.install_failed.as_deref()
                                    == Some(extension_id.as_str())
                                {
                                    "Retry"
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
                                                            if entry.from_ahead_registry {
                                                                "AHEAD registry"
                                                            } else {
                                                                "Zed catalog"
                                                            }
                                                        } else {
                                                            "Installed locally"
                                                        },
                                                        format!("API {} · compatibility candidate", entry.wasm_api_version.as_deref().unwrap_or("unknown"))
                                                    )),
                                            )
                                            .when_some(
                                                runtime_summary,
                                                |column, summary| {
                                                    column.child(
                                                        div()
                                                            .min_w_0()
                                                            .truncate()
                                                            .text_size(px(10.))
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(summary),
                                                    )
                                                },
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .items_center()
                                            .child(
                                                Button::new(format!("install_extension_{extension_id}"))
                                                    .label(action)
                                                    .tooltip(format!("{action} {extension_id}"))
                                                    .disabled(
                                                        installed
                                                            || extension_install_busy
                                                            || extension_uninstall_busy,
                                                    )
                                                    .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                        this.install_language_extension(&extension_id, cx);
                                                    })),
                                            )
                                            .when(can_uninstall, |actions| {
                                                let uninstall_extension_id = entry.id.clone();
                                                actions.child(
                                                    Button::new(format!(
                                                        "uninstall_extension_{}",
                                                        uninstall_extension_id
                                                    ))
                                                    .label(if uninstalling {
                                                        "Uninstalling…"
                                                    } else {
                                                        "Uninstall"
                                                    })
                                                    .tooltip(format!(
                                                        "Uninstall {}",
                                                        uninstall_extension_id
                                                    ))
                                                    .disabled(
                                                        extension_install_busy
                                                            || extension_uninstall_busy,
                                                    )
                                                    .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                        this.uninstall_language_extension(
                                                            &uninstall_extension_id,
                                                            cx,
                                                        );
                                                    })),
                                                )
                                            }),
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
        extension_catalog_status, extension_download_url, extension_runtime_summary,
        extension_update_available, fetch_ahead_extension_catalog,
        installed_language_extensions_in, matching_catalog_extensions,
        matching_language_extensions, merge_extension_catalog_sources,
        merge_language_extension_catalog, parse_ahead_extension_catalog,
        parse_language_extension_catalog, zed_extension_catalog_url,
    };
    use crate::proxy_client::LspServerStatus;

    #[test]
    fn partial_catalog_failure_reports_available_candidates() {
        assert_eq!(
            extension_catalog_status(7, 2, Some("Zed catalog: timeout")),
            "7 extension candidates · Some catalog sources failed: Zed catalog: timeout"
        );
        assert_eq!(
            extension_catalog_status(0, 0, Some("AHEAD registry: DNS error")),
            "Extension catalog unavailable · AHEAD registry: DNS error"
        );
    }

    #[test]
    fn builtin_setup_notices_show_installing_repairing_and_failed_managed_servers() {
        let statuses = [
            (
                "basedpyright-langserver",
                false,
                false,
                Some("Installing from npm into AHEAD's cache…"),
            ),
            (
                "vtsls",
                false,
                false,
                Some("Repairing AHEAD's cached language-server package…"),
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
            on_type_formatting_triggers: Vec::new(),
        })
        .collect();
        let notices = builtin_setup_notices(statuses);
        assert_eq!(notices.len(), 3);
        assert_eq!(notices[0].0, "Python");
        assert_eq!(notices[1].0, "JavaScript / TypeScript");
        assert_eq!(notices[2].0, "JavaScript / TypeScript");
    }

    #[test]
    fn gallery_search_uses_zeds_filtered_catalog_endpoint() {
        let url = zed_extension_catalog_url("  C++ & Rust  ");
        assert_eq!(url.path(), "/extensions");
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![
                ("max_schema_version".into(), "1".into()),
                ("provides".into(), "language-servers".into()),
                ("filter".into(), "C++ & Rust".into()),
            ]
        );
    }

    #[test]
    fn gallery_search_locally_filters_ahead_feed_and_keeps_zed_matches() {
        let ahead_catalog = serde_json::json!({"data": [
            {
                "id": "html-ahead", "name": "HTML tools", "version": "1.0.0",
                "description": "HTML support", "schema_version": 1,
                "wasm_api_version": "0.7.0", "provides": ["language-servers"],
                "archive_sha256": "a".repeat(64),
                "download_url": "https://github.com/Kade-Powell/ahead/releases/download/v1/html-ahead-1.0.0.tar.gz"
            },
            {
                "id": "rust-ahead", "name": "Rust tools", "version": "1.0.0",
                "description": "Rust support", "schema_version": 1,
                "wasm_api_version": "0.7.0", "provides": ["language-servers"],
                "archive_sha256": "b".repeat(64),
                "download_url": "https://github.com/Kade-Powell/ahead/releases/download/v1/rust-ahead-1.0.0.tar.gz"
            }
        ]});
        let zed_catalog = serde_json::json!({"data": [{
            "id": "html-preview", "name": "HTML preview", "version": "0.3.2",
            "description": "HTML support", "schema_version": 1,
            "wasm_api_version": "0.7.0", "provides": ["language-servers"]
        }]});
        let ahead_entries =
            parse_ahead_extension_catalog(ahead_catalog.to_string().as_bytes())
                .expect("AHEAD catalog");
        let zed_entries =
            parse_language_extension_catalog(zed_catalog.to_string().as_bytes())
                .expect("Zed catalog");
        let entries = merge_extension_catalog_sources(zed_entries, ahead_entries);

        let html_matches = matching_catalog_extensions(
            &entries,
            "html",
            "html",
            LanguageExtensionFilter::All,
        );
        assert_eq!(
            html_matches
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["html-preview", "html-ahead"]
        );

        let rust_matches = matching_catalog_extensions(
            &entries,
            "rust",
            "html",
            LanguageExtensionFilter::All,
        );
        assert_eq!(
            rust_matches
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["rust-ahead"]
        );
    }

    #[test]
    fn live_zed_catalog_wins_duplicate_ids_and_ahead_feed_is_fallback() {
        let zed_catalog = serde_json::json!({"data": [{
            "id": "shared", "name": "Shared from Zed", "version": "2.0.0",
            "schema_version": 1, "wasm_api_version": "0.7.0",
            "provides": ["language-servers"]
        }]});
        let ahead_catalog = serde_json::json!({"data": [{
            "id": "shared", "name": "Shared from AHEAD", "version": "1.0.0",
            "schema_version": 1, "wasm_api_version": "0.7.0",
            "provides": ["language-servers"], "archive_sha256": "a".repeat(64),
            "download_url": "https://github.com/Kade-Powell/ahead/releases/download/v1/shared-1.0.0.tar.gz"
        }, {
            "id": "ahead-only", "name": "AHEAD only", "version": "1.0.0",
            "schema_version": 1, "wasm_api_version": "0.7.0",
            "provides": ["language-servers"], "archive_sha256": "b".repeat(64),
            "download_url": "https://github.com/Kade-Powell/ahead/releases/download/v1/ahead-only-1.0.0.tar.gz"
        }]});
        let zed_entries =
            parse_language_extension_catalog(zed_catalog.to_string().as_bytes())
                .expect("Zed catalog");
        let ahead_entries =
            parse_ahead_extension_catalog(ahead_catalog.to_string().as_bytes())
                .expect("AHEAD catalog");

        let merged =
            merge_extension_catalog_sources(zed_entries, ahead_entries.clone());
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name, "Shared from Zed");
        assert_eq!(merged[0].version, "2.0.0");
        assert!(!merged[0].from_ahead_registry);
        assert_eq!(merged[1].id, "ahead-only");
        assert!(merged[1].from_ahead_registry);

        let fallback = merge_extension_catalog_sources(Vec::new(), ahead_entries);
        assert_eq!(fallback.len(), 2);
        assert!(fallback.iter().all(|entry| entry.from_ahead_registry));
    }

    #[test]
    fn catalog_filters_out_icon_theme_only_extensions() {
        let catalog = serde_json::json!({"data": [{
            "id": "material-icon-theme", "name": "Material Icon Theme", "version": "1.3.1",
            "schema_version": 1, "wasm_api_version": null, "provides": ["icon-themes"]
        }]});
        let entries =
            parse_language_extension_catalog(catalog.to_string().as_bytes())
                .expect("catalog");
        assert!(entries.is_empty());
    }

    #[test]
    fn language_extension_gallery_keeps_only_installable_compatible_packages() {
        let catalog = serde_json::json!({
            "data": [
                {"id":"html","name":"HTML","version":"0.3.2","description":"HTML support", "schema_version":1,"wasm_api_version":"0.7.0","provides":["languages","language-servers"],"archive_sha256":"a".repeat(64)},
                {"id":"no-digest","name":"No digest","version":"1.0.0","schema_version":1,"wasm_api_version":"0.7.0","provides":["language-servers"]},
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
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "html");
        assert_eq!(
            entries[0].archive_sha256.as_deref(),
            Some("a".repeat(64).as_str())
        );
        assert!(entries[0].from_catalog);
        assert_eq!(entries[1].id, "no-digest");
        assert!(entries[1].archive_sha256.is_none());
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
            2
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
        let not_installed = matching_language_extensions(
            &installed,
            "",
            LanguageExtensionFilter::NotInstalled,
        );
        assert_eq!(not_installed.len(), 1);
        assert_eq!(not_installed[0].id, "no-digest");
    }

    #[test]
    fn ahead_registry_requires_a_hashed_asset_from_its_github_releases() {
        let catalog = serde_json::json!({
            "data": [
                {
                    "id": "html",
                    "name": "HTML",
                    "version": "0.3.2",
                    "schema_version": 1,
                    "wasm_api_version": "0.7.0",
                    "provides": ["language-servers"],
                    "archive_sha256": "a".repeat(64),
                    "download_url": "https://github.com/Kade-Powell/ahead/releases/download/extensions-v1/html-0.3.2.tar.gz"
                },
                {
                    "id": "missing-hash",
                    "name": "Missing hash",
                    "version": "1.0.0",
                    "schema_version": 1,
                    "wasm_api_version": "0.7.0",
                    "provides": ["language-servers"],
                    "download_url": "https://github.com/Kade-Powell/ahead/releases/download/extensions-v1/missing-hash-1.0.0.tar.gz"
                },
                {
                    "id": "other-host",
                    "name": "Other host",
                    "version": "1.0.0",
                    "schema_version": 1,
                    "wasm_api_version": "0.7.0",
                    "provides": ["language-servers"],
                    "archive_sha256": "b".repeat(64),
                    "download_url": "https://example.com/other-host-1.0.0.tar.gz"
                },
                {
                    "id": "zed-release",
                    "name": "Zed release",
                    "version": "1.0.0",
                    "schema_version": 1,
                    "wasm_api_version": "0.7.0",
                    "provides": ["language-servers"],
                    "archive_sha256": "c".repeat(64),
                    "download_url": "https://github.com/zed-industries/extensions/releases/download/v1/zed-release-1.0.0.tar.gz"
                }
            ]
        });
        let entries = parse_ahead_extension_catalog(catalog.to_string().as_bytes())
            .expect("AHEAD registry parses");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "html");
        assert!(entries[0].from_ahead_registry);
        assert_eq!(
            extension_download_url(&entries[0]).as_deref(),
            Some(
                "https://github.com/Kade-Powell/ahead/releases/download/extensions-v1/html-0.3.2.tar.gz"
            )
        );
    }

    #[test]
    fn ahead_registry_keeps_extension_runtime_and_host_requirements_visible() {
        let catalog = serde_json::json!({
            "data": [{
                "id": "html",
                "name": "HTML",
                "version": "0.3.2",
                "schema_version": 1,
                "wasm_api_version": "0.7.0",
                "provides": ["language-servers"],
                "archive_sha256": "a".repeat(64),
                "download_url": "https://github.com/Kade-Powell/ahead/releases/download/extensions-v1/html-0.3.2.tar.gz",
                "runtime_dependencies": ["@zed-industries/vscode-langservers-extracted"],
                "requested_capabilities": [{
                    "kind": "npm:install",
                    "package": "@zed-industries/vscode-langservers-extracted"
                }]
            }]
        });
        let entries = parse_ahead_extension_catalog(catalog.to_string().as_bytes())
            .expect("AHEAD registry parses");

        assert_eq!(
            extension_runtime_summary(&entries[0]).as_deref(),
            Some(
                "Runtime: @zed-industries/vscode-langservers-extracted · Host access: npm:install (@zed-industries/vscode-langservers-extracted)"
            )
        );
    }

    #[test]
    #[cfg(debug_assertions)]
    fn checked_in_ahead_registry_is_a_valid_empty_feed_until_published() {
        let client = reqwest::blocking::Client::builder()
            .build()
            .expect("catalog client");
        let body =
            fetch_ahead_extension_catalog(&client).expect("local checkout registry");
        let entries = parse_ahead_extension_catalog(&body)
            .expect("checked-in registry format");
        assert!(entries.is_empty());
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
        let ProxyRpc::Notification(ProxyNotification::RestartLanguageServers {
            extension_id,
        }) = rpc
            .rx()
            .try_recv()
            .expect("restart after successful install")
        else {
            panic!("expected extension-scoped restart");
        };
        assert_eq!(extension_id.as_deref(), Some("html"));
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

    #[gpui_kit::test]
    fn gallery_uninstall_updates_installed_state_through_the_proxy(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::proxy::{ProxyRequest, ProxyResponse, ProxyRpc};

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
            "provides": ["language-servers"]
        }]});
        let mut entries =
            parse_language_extension_catalog(catalog.to_string().as_bytes())
                .expect("catalog");
        entries[0].installed_version = Some("0.3.2".into());
        panel.update(cx, |panel, cx| {
            panel.catalog = entries;
            panel.uninstall_language_extension("html", cx);
        });

        let rpc = proxy.rpc_for_test();
        let ProxyRpc::Request(
            request_id,
            ProxyRequest::UninstallLanguageExtension { extension_id },
        ) = rpc.rx().try_recv().expect("uninstall request")
        else {
            panic!("expected language-extension uninstall request");
        };
        assert_eq!(extension_id, "html");
        rpc.handle_response(request_id, Ok(ProxyResponse::Success {}));
        cx.run_until_parked();

        panel.update(cx, |panel, _| {
            assert!(panel.uninstalling.is_none());
            assert_eq!(panel.catalog[0].installed_version, None);
            assert!(panel.status.contains("html uninstalled"));
        });
        assert!(
            rpc.rx().try_recv().is_err(),
            "the catalog owns the server restart; the app must not send a duplicate"
        );
    }

    #[test]
    fn language_extension_gallery_keeps_installed_packages_offline() {
        let root = tempfile::tempdir().expect("extension root");
        let html = root.path().join("html");
        std::fs::create_dir(&html).expect("HTML extension directory");
        std::fs::write(
            html.join("extension.toml"),
            "id = 'html'\nname = 'HTML'\nversion = '0.2.0'\nschema_version = 1\ndescription = 'HTML support'\n[language_servers.html]\nlanguages = ['HTML']\n",
        )
        .expect("HTML extension manifest");
        std::fs::write(html.join("extension.wasm"), b"test")
            .expect("HTML extension module");
        let installed = installed_language_extensions_in(root.path());
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].installed_version.as_deref(), Some("0.2.0"));
        assert_eq!(installed[0].wasm_api_version, None);
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
