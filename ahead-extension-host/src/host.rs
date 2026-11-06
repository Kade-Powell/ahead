use crate::node_runtime::NodeRuntime;
use anyhow::{Context, Result, anyhow};
use fs4::fs_std::FileExt;
use regex::Regex;
use serde::Deserialize;
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    collections::{BTreeMap, HashMap},
    env,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command as ProcessCommand},
    sync::oneshot,
};
use wasmtime::{
    Config, Engine, Store,
    component::{Component, HasSelf, Linker, Resource, ResourceTable},
};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use zed::extension::{
    common, context_server, dap, github,
    http_client::{self, HostHttpResponseStream},
    lsp, nodejs, platform, process, slash_command,
};

const MAX_EXTENSION_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXTENSION_UNPACKED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_EXTENSION_ARCHIVE_ENTRIES: usize = 10_000;
const MAX_EXTENSION_GRAMMAR_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXTENSION_QUERY_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CACHED_LANGUAGE_SYNTAX: usize = 32;
const MAX_CACHED_LANGUAGE_SOURCE_ROOTS: usize = 8;
const MAX_EXTENSION_METADATA_BYTES: u64 = 256 * 1024;
const MAX_EXTENSION_HTTP_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const EXTENSION_DOWNLOAD_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(60);
const EXTENSION_COMMAND_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(120);

async fn run_extension_command(
    mut command: ProcessCommand,
    timeout: std::time::Duration,
) -> Result<std::process::Output, String> {
    command
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "extension command stdout was not piped".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "extension command stderr was not piped".to_string())?;
    let (cancel_sender, mut cancel_receiver) = oneshot::channel::<()>();
    let (result_sender, result_receiver) = oneshot::channel();
    let deadline = tokio::time::Instant::now() + timeout;

    tokio::spawn(async move {
        let mut stdout_task = tokio::spawn(async move {
            let mut output = Vec::new();
            let mut stdout = stdout;
            stdout
                .read_to_end(&mut output)
                .await
                .map(|_| output)
                .map_err(|error| error.to_string())
        });
        let mut stderr_task = tokio::spawn(async move {
            let mut output = Vec::new();
            let mut stderr = stderr;
            stderr
                .read_to_end(&mut output)
                .await
                .map(|_| output)
                .map_err(|error| error.to_string())
        });

        let status = tokio::select! {
            status = child.wait() => status.map_err(|error| error.to_string()),
            _ = tokio::time::sleep_until(deadline) => {
                let error = format!("extension command timed out after {timeout:?}");
                let reap_error = stop_and_reap_extension_child(&mut child).await.err();
                stdout_task.abort();
                stderr_task.abort();
                let error = reap_error.map_or(error.clone(), |reap_error| {
                    format!("{error}; failed to reap extension command: {reap_error}")
                });
                if result_sender.send(Err(error)).is_err() {
                    return;
                }
                return;
            }
            _ = &mut cancel_receiver => {
                if let Err(error) = stop_and_reap_extension_child(&mut child).await {
                    eprintln!("failed to reap cancelled extension command: {error}");
                }
                stdout_task.abort();
                stderr_task.abort();
                return;
            }
        };

        let output = match status {
            Ok(status) => {
                let remaining =
                    deadline.saturating_duration_since(tokio::time::Instant::now());
                tokio::select! {
                    output = tokio::time::timeout(remaining, async {
                        let stdout = (&mut stdout_task)
                            .await
                            .map_err(|error| error.to_string())??;
                        let stderr = (&mut stderr_task)
                            .await
                            .map_err(|error| error.to_string())??;
                        Ok::<_, String>(std::process::Output { status, stdout, stderr })
                    }) => match output {
                        Ok(output) => output,
                        Err(_) => {
                            stdout_task.abort();
                            stderr_task.abort();
                            Err(format!("extension command timed out after {timeout:?}"))
                        }
                    },
                    _ = &mut cancel_receiver => {
                        stdout_task.abort();
                        stderr_task.abort();
                        return;
                    }
                }
            }
            Err(error) => {
                stdout_task.abort();
                stderr_task.abort();
                match stop_and_reap_extension_child(&mut child).await {
                    Ok(()) => Err(error),
                    Err(reap_error) => Err(format!(
                        "{error}; failed to reap extension command: {reap_error}"
                    )),
                }
            }
        };
        if result_sender.send(output).is_err() {
            return;
        }
    });

    let result = result_receiver
        .await
        .map_err(|_| "extension command worker stopped unexpectedly".to_string());
    drop(cancel_sender);
    result?
}

async fn stop_and_reap_extension_child(child: &mut Child) -> Result<(), String> {
    if let Err(error) = child.start_kill() {
        if child
            .try_wait()
            .map_err(|wait_error| wait_error.to_string())?
            .is_some()
        {
            return Ok(());
        }
        return Err(error.to_string());
    }
    tokio::time::timeout(std::time::Duration::from_secs(2), child.wait())
        .await
        .map_err(|_| "timed out waiting for killed extension command".to_string())?
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[derive(Debug, Deserialize)]
struct GithubReleaseResponse {
    tag_name: String,
    prerelease: bool,
    assets: Vec<GithubAssetResponse>,
}

#[derive(Debug, Deserialize)]
struct GithubAssetResponse {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

wasmtime::component::bindgen!({
    imports: {
        default: async | trappable,
    },
    exports: {
        default: async,
    },
    path: "wit/since_v0.8.0",
    world: "extension",
    with: {
        "worktree": WorktreeContext,
        "project": ProjectContext,
        "key-value-store": KeyValueStoreContext,
        "zed:extension/http-client.http-response-stream": HttpResponseStreamContext,
    },
});

#[allow(dead_code)]
mod v0_6 {
    wasmtime::component::bindgen!({
        imports: {
            default: async | trappable,
        },
        exports: {
            default: async,
        },
        path: "wit/since_v0.6.0",
        world: "extension",
        with: {
            "worktree": crate::host::WorktreeContext,
            "project": crate::host::ProjectContext,
            "key-value-store": crate::host::KeyValueStoreContext,
            "zed:extension/common": crate::host::zed::extension::common,
            "zed:extension/context-server": crate::host::zed::extension::context_server,
            "zed:extension/http-client": crate::host::zed::extension::http_client,
            "zed:extension/nodejs": crate::host::zed::extension::nodejs,
            "zed:extension/process": crate::host::zed::extension::process,
            "zed:extension/slash-command": crate::host::zed::extension::slash_command,
        },
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageServerCommand {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub initialization_options: Option<serde_json::Value>,
    pub initialization_options_schema: Option<serde_json::Value>,
    pub workspace_configuration: Option<serde_json::Value>,
    pub workspace_configuration_schema: Option<serde_json::Value>,
}

fn parse_extension_json_schema(schema: Option<String>) -> Option<serde_json::Value> {
    schema.and_then(|schema| serde_json::from_str(&schema).ok())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeContext {
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageServerExtension {
    pub id: String,
    pub server_id: String,
    pub directory: PathBuf,
    pub wasm_path: PathBuf,
    api_version: ExtensionApiVersion,
    languages: Vec<String>,
    opt_in_languages: Vec<String>,
    language_ids: BTreeMap<String, String>,
    capabilities: Vec<ProcessCapability>,
}

#[derive(Default)]
pub struct LanguageServerDiscovery {
    pub extensions: Vec<LanguageServerExtension>,
    pub errors: Vec<(PathBuf, anyhow::Error)>,
}

impl LanguageServerExtension {
    pub fn supports_language(&self, language_id: &str) -> bool {
        self.languages
            .iter()
            .any(|language| language.eq_ignore_ascii_case(language_id))
            || self
                .language_ids
                .values()
                .any(|protocol_id| protocol_id.eq_ignore_ascii_case(language_id))
    }

    pub fn is_opt_in_for(&self, language_id: &str) -> bool {
        self.opt_in_languages.iter().any(|opt_in_language| {
            opt_in_language.eq_ignore_ascii_case(language_id)
                || self.language_ids.iter().any(|(language, protocol_id)| {
                    opt_in_language.eq_ignore_ascii_case(language)
                        && (language_id.eq_ignore_ascii_case(language)
                            || language_id.eq_ignore_ascii_case(protocol_id))
                })
        })
    }

    pub fn language_ids(&self) -> Vec<String> {
        let mut languages = self.languages.clone();
        languages.extend(self.language_ids.values().cloned());
        languages.sort();
        languages.dedup();
        languages
    }

    pub fn protocol_language_id(&self, language_id: &str) -> String {
        self.language_ids
            .iter()
            .find(|(editor_id, protocol_id)| {
                editor_id.eq_ignore_ascii_case(language_id)
                    || protocol_id.eq_ignore_ascii_case(language_id)
            })
            .map(|(_, protocol_id)| protocol_id.clone())
            .unwrap_or_else(|| language_id.to_string())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct LanguageConfig {
    name: String,
    #[serde(default)]
    grammar: Option<String>,
    #[serde(default)]
    line_comments: Vec<String>,
    #[serde(default)]
    block_comment: Option<BlockCommentDefinition>,
    #[serde(default)]
    path_suffixes: Vec<String>,
    #[serde(default)]
    first_line_pattern: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum BlockCommentDefinition {
    Pair([String; 2]),
    Structured { start: String, end: String },
}

impl BlockCommentDefinition {
    fn into_delimiters(self) -> (String, String) {
        match self {
            Self::Pair([start, end]) => (start, end),
            Self::Structured { start, end } => (start, end),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageDefinition {
    pub name: String,
    pub grammar: Option<String>,
    pub comments: LanguageComments,
    pub path_suffixes: Vec<String>,
    pub first_line_pattern: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LanguageComments {
    pub line_comment: Option<String>,
    pub block_comment: Option<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtensionSyntax {
    pub language_name: String,
    pub captures: Vec<super::syntax::SyntaxCapture>,
    pub comments: LanguageComments,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageSyntax {
    pub extension_directory: PathBuf,
    pub language_name: String,
    pub grammar_name: String,
    pub wasm: Vec<u8>,
    pub highlights: String,
    pub injections: String,
    pub(crate) asset_digest: [u8; 32],
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct LanguageSyntaxCacheKey {
    extension_directory: PathBuf,
    language_path: PathBuf,
    grammar_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExtensionAssetStamp {
    length: u64,
    modified: SystemTime,
}

struct CachedLanguageSyntax {
    wasm_stamp: ExtensionAssetStamp,
    highlights_stamp: Option<ExtensionAssetStamp>,
    injections_stamp: Option<ExtensionAssetStamp>,
    syntax: Arc<LanguageSyntax>,
}

struct CachedLanguageSources {
    definitions: Vec<LanguageSource>,
    first_error: Option<String>,
}

static LANGUAGE_SYNTAX_CACHE: OnceLock<
    Mutex<HashMap<LanguageSyntaxCacheKey, CachedLanguageSyntax>>,
> = OnceLock::new();
static LANGUAGE_SOURCES_CACHE: OnceLock<
    Mutex<HashMap<PathBuf, Arc<CachedLanguageSources>>>,
> = OnceLock::new();

#[derive(Clone)]
pub(crate) struct LanguageSource {
    definition: LanguageDefinition,
    extension_directory: PathBuf,
    language_path: PathBuf,
    grammar_names: Vec<String>,
}

impl LanguageDefinition {
    fn matches(&self, path: &Path, content: Option<&str>) -> bool {
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        let path_match = self.path_suffixes.iter().any(|suffix| {
            file_name.eq_ignore_ascii_case(suffix)
                || file_name
                    .to_ascii_lowercase()
                    .ends_with(&format!(".{}", suffix.to_ascii_lowercase()))
        });
        path_match
            || self
                .first_line_pattern
                .as_deref()
                .and_then(|pattern| Regex::new(pattern).ok())
                .is_some_and(|pattern| {
                    content
                        .and_then(|content| content.lines().next())
                        .is_some_and(|line| pattern.is_match(line))
                })
    }
}

#[derive(Debug, Deserialize)]
struct ExtensionManifest {
    id: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    schema_version: Option<u32>,
    #[serde(default)]
    languages: Vec<PathBuf>,
    #[serde(default)]
    grammars: BTreeMap<String, toml::Table>,
    #[serde(default)]
    language_servers: BTreeMap<String, LanguageServerManifestEntry>,
    #[serde(default)]
    capabilities: Vec<CapabilityManifestEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionApiVersion {
    V0_6,
    V0_8,
}

fn extension_api_version(
    extension_id: &str,
    wasm_bytes: &[u8],
) -> Result<ExtensionApiVersion> {
    let mut version = None;
    for payload in wasmparser::Parser::new(0).parse_all(wasm_bytes) {
        if let wasmparser::Payload::CustomSection(section) =
            payload.context("error parsing extension WASM")?
            && section.name() == "zed:api-version"
        {
            let data = section.data();
            anyhow::ensure!(
                data.len() == 6,
                "extension {extension_id} has an invalid zed:api-version section"
            );
            version = Some((
                u16::from_be_bytes([data[0], data[1]]),
                u16::from_be_bytes([data[2], data[3]]),
                u16::from_be_bytes([data[4], data[5]]),
            ));
        }
    }

    let (major, minor, patch) = version.with_context(|| {
        format!("extension {extension_id} has no zed:api-version section")
    })?;
    match (major, minor, patch) {
        (0, 6 | 7, 0) => Ok(ExtensionApiVersion::V0_6),
        (0, 8, 0) => Ok(ExtensionApiVersion::V0_8),
        _ => Err(anyhow!(
            "unsupported Zed extension API version {major}.{minor}.{patch}"
        )),
    }
}

#[derive(Debug, Clone, Deserialize)]
struct CapabilityManifestEntry {
    kind: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    path: Vec<String>,
    #[serde(default)]
    package: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessCapability {
    command: String,
    args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DownloadCapability {
    host: String,
    path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NpmCapability {
    package: String,
}

impl NpmCapability {
    fn allows(&self, package: &str) -> bool {
        self.package == "*" || self.package == package
    }
}

impl ProcessCapability {
    fn allows(&self, command: &str, args: &[String]) -> bool {
        if self.command != command && self.command != "*" {
            return false;
        }
        for (index, allowed) in self.args.iter().enumerate() {
            if allowed == "**" {
                return true;
            }
            if index >= args.len() || (allowed != "*" && allowed != &args[index]) {
                return false;
            }
        }
        self.args.len() == args.len()
    }
}

impl DownloadCapability {
    fn allows(&self, url: &url::Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        let path = url
            .path_segments()
            .map(|segments| segments.collect::<Vec<_>>())
            .unwrap_or_default();
        if self.host != host && self.host != "*" {
            return false;
        }
        for (index, allowed) in self.path.iter().enumerate() {
            if allowed == "**" {
                return true;
            }
            if index >= path.len() || (allowed != "*" && allowed != path[index]) {
                return false;
            }
        }
        self.path.len() == path.len()
    }
}

// Zed keeps host grants separate from an extension's requested process commands.
#[derive(Clone, Debug, Default)]
struct ExtensionCapabilityGrants {
    process: Vec<ProcessCapability>,
    downloads: Vec<DownloadCapability>,
    npm: Vec<NpmCapability>,
}

impl ExtensionCapabilityGrants {
    fn zed_defaults() -> Self {
        Self {
            process: vec![ProcessCapability {
                command: "*".to_string(),
                args: vec!["**".to_string()],
            }],
            downloads: vec![DownloadCapability {
                host: "*".to_string(),
                path: vec!["**".to_string()],
            }],
            npm: vec![NpmCapability {
                package: "*".to_string(),
            }],
        }
    }

    fn from_user_settings(settings: Option<&str>) -> Result<Self> {
        let Some(settings) = settings else {
            return Ok(Self::zed_defaults());
        };
        let settings = settings
            .parse::<toml::Table>()
            .context("parsing AHEAD user settings")?;
        let Some(value) = settings.get("granted_extension_capabilities") else {
            return Ok(Self::zed_defaults());
        };
        let entries: Vec<CapabilityManifestEntry> = value
            .clone()
            .try_into()
            .context("parsing granted_extension_capabilities")?;
        let mut grants = Self::default();
        for entry in entries {
            match entry.kind.as_str() {
                "process:exec" => grants.process.push(ProcessCapability {
                    command: entry
                        .command
                        .context("process:exec grant is missing command")?,
                    args: entry.args,
                }),
                "download_file" => {
                    grants.downloads.push(DownloadCapability {
                        host: entry
                            .host
                            .context("download_file grant is missing host")?,
                        path: entry.path,
                    });
                }
                "npm:install" => grants.npm.push(NpmCapability {
                    package: entry
                        .package
                        .context("npm:install grant is missing package")?,
                }),
                kind => {
                    return Err(anyhow!("unsupported extension capability {kind}"));
                }
            }
        }
        Ok(grants)
    }

    fn load_for_user() -> Result<Self> {
        let Some(home) = ahead_core::directory::Directory::home_dir() else {
            return Ok(Self::zed_defaults());
        };
        Self::load_for_home(&home)
    }

    fn load_for_home(home: &Path) -> Result<Self> {
        let settings = ahead_core::config::read_ahead_config(home, "settings.toml")
            .with_context(|| {
                format!("reading extension grants from {}", home.display())
            })?;
        Self::from_user_settings(settings.as_deref())
    }

    fn allows_process_exec(
        &self,
        requested: &[ProcessCapability],
        command: &str,
        args: &[String],
    ) -> bool {
        requested
            .iter()
            .any(|capability| capability.allows(command, args))
            && self
                .process
                .iter()
                .any(|capability| capability.allows(command, args))
    }

    fn allows_download(&self, url: &url::Url) -> bool {
        self.downloads
            .iter()
            .any(|capability| capability.allows(url))
    }

    fn allows_npm_install(&self, package: &str) -> bool {
        self.npm.iter().any(|capability| capability.allows(package))
    }
}

#[derive(Debug, Deserialize, Default)]
struct LanguageServerManifestEntry {
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    language_ids: BTreeMap<String, String>,
    #[serde(default)]
    opt_in_languages: Vec<String>,
}

pub fn discover_language_server_extensions(
    extensions_root: &Path,
) -> Result<LanguageServerDiscovery> {
    let mut discovery = LanguageServerDiscovery::default();
    if !extensions_root.exists() {
        return Ok(discovery);
    }
    let _install_lock = lock_extension_installs(extensions_root)?;
    discovery
        .errors
        .extend(restore_interrupted_extension_updates(extensions_root)?);
    invalidate_language_sources_cache(extensions_root);
    for entry in std::fs::read_dir(extensions_root)? {
        let entry = entry?;
        let directory = entry.path();
        if !entry.file_type()?.is_dir()
            || entry
                .file_name()
                .to_str()
                .is_none_or(|name| name.starts_with('.'))
        {
            continue;
        }
        let manifest_path = directory.join("extension.toml");
        let wasm_path = directory.join("extension.wasm");
        if !manifest_path.is_file() || !wasm_path.is_file() {
            continue;
        }
        let manifest: Result<ExtensionManifest> =
            std::fs::read_to_string(&manifest_path)
                .with_context(|| format!("reading {}", manifest_path.display()))
                .and_then(|content| {
                    toml::from_str(&content).with_context(|| {
                        format!("parsing {}", manifest_path.display())
                    })
                });
        let manifest = match manifest {
            Ok(manifest) => manifest,
            Err(error) => {
                discovery.errors.push((directory, error));
                continue;
            }
        };
        if directory.file_name().and_then(|name| name.to_str())
            != Some(manifest.id.as_str())
        {
            discovery.errors.push((
                directory,
                anyhow!("extension directory name does not match manifest id"),
            ));
            continue;
        }
        let api_version = std::fs::read(&wasm_path)
            .with_context(|| format!("reading {}", wasm_path.display()))
            .and_then(|wasm| extension_api_version(&manifest.id, &wasm));
        let api_version = match api_version {
            Ok(api_version) => api_version,
            Err(error) => {
                discovery.errors.push((directory, error));
                continue;
            }
        };
        for (server_id, server) in manifest.language_servers {
            let mut languages = server.languages.clone();
            if let Some(language) = server.language {
                languages.push(language);
            }
            let language_ids = server.language_ids;
            let opt_in_languages = server.opt_in_languages;
            languages.extend(language_ids.keys().cloned());
            languages.sort_unstable_by_key(|language| language.to_ascii_lowercase());
            languages.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
            discovery.extensions.push(LanguageServerExtension {
                id: manifest.id.clone(),
                server_id,
                directory: directory.clone(),
                wasm_path: wasm_path.clone(),
                api_version,
                languages,
                opt_in_languages,
                language_ids,
                capabilities: manifest
                    .capabilities
                    .iter()
                    .filter(|capability| capability.kind == "process:exec")
                    .filter_map(|capability| {
                        Some(ProcessCapability {
                            command: capability.command.clone()?,
                            args: capability.args.clone(),
                        })
                    })
                    .collect(),
            });
        }
    }
    discovery.extensions.sort_by(|left, right| {
        (&left.id, &left.server_id).cmp(&(&right.id, &right.server_id))
    });
    Ok(discovery)
}

pub fn language_id_for_path(
    extensions_root: &Path,
    path: &Path,
    content: Option<&str>,
) -> Result<Option<String>> {
    Ok(language_source_for_path(extensions_root, path, content)?
        .map(|source| source.definition.name))
}

/// Reads the installed grammar and query assets for a matching file.
///
/// This performs filesystem I/O; callers should run it off the UI thread.
pub fn language_syntax_for_path(
    extensions_root: &Path,
    path: &Path,
    content: Option<&str>,
) -> Result<Option<LanguageSyntax>> {
    let Some(source) = language_source_for_path(extensions_root, path, content)?
    else {
        return Ok(None);
    };
    let Some(grammar_name) = source.definition.grammar.clone() else {
        return Ok(None);
    };
    if !source.grammar_names.contains(&grammar_name) {
        return Ok(None);
    }
    Ok(Some(read_language_syntax(source, grammar_name)?))
}

pub fn syntax_captures_for_path(
    extensions_root: &Path,
    path: &Path,
    content: &str,
) -> Result<Option<Vec<super::syntax::SyntaxCapture>>> {
    Ok(extension_syntax_for_path(extensions_root, path, content)?
        .map(|syntax| syntax.captures))
}

pub fn extension_syntax_for_path(
    extensions_root: &Path,
    path: &Path,
    content: &str,
) -> Result<Option<ExtensionSyntax>> {
    let sources = language_sources_for_root(extensions_root)?;
    let Some(source) =
        language_source_from_path_sources(&sources.definitions, path, Some(content))
    else {
        return match &sources.first_error {
            Some(error) => Err(anyhow!("{error}")),
            None => Ok(None),
        };
    };
    let language_name = source.definition.name.clone();
    let comments = source.definition.comments.clone();
    let captures = match source.definition.grammar.clone() {
        Some(grammar_name) if source.grammar_names.contains(&grammar_name) => {
            let syntax = cached_language_syntax(source, grammar_name)?;
            super::syntax::captures(&sources.definitions, &syntax, content)?
        }
        Some(_) | None => Vec::new(),
    };
    Ok(Some(ExtensionSyntax {
        language_name,
        captures,
        comments,
    }))
}

pub(crate) fn language_syntax_for_name(
    sources: &[LanguageSource],
    name: &str,
) -> Result<Option<Arc<LanguageSyntax>>> {
    let language_name = name.rsplit('.').next().unwrap_or(name);
    let source = sources
        .iter()
        .filter(|source| {
            source.definition.grammar.as_deref() == Some(language_name)
                || source.definition.name.eq_ignore_ascii_case(language_name)
                || source
                    .definition
                    .path_suffixes
                    .iter()
                    .any(|suffix| suffix.eq_ignore_ascii_case(language_name))
        })
        .max_by_key(|source| {
            if source.definition.grammar.as_deref() == Some(language_name) {
                1
            } else {
                0
            }
        })
        .cloned();
    let Some(source) = source else {
        return Ok(None);
    };
    let Some(grammar_name) = source.definition.grammar.clone() else {
        return Ok(None);
    };
    if !source.grammar_names.contains(&grammar_name) {
        return Ok(None);
    }
    Ok(Some(cached_language_syntax(source, grammar_name)?))
}

fn cached_language_syntax(
    source: LanguageSource,
    grammar_name: String,
) -> Result<Arc<LanguageSyntax>> {
    let wasm_relative_path =
        Path::new("grammars").join(format!("{grammar_name}.wasm"));
    let wasm_stamp = extension_asset_stamp(
        &source.extension_directory,
        &wasm_relative_path,
        MAX_EXTENSION_GRAMMAR_BYTES,
    )?
    .ok_or_else(|| anyhow!("extension grammar {grammar_name} is missing"))?;
    let highlights_stamp = extension_asset_stamp(
        &source.extension_directory,
        &source.language_path.join("highlights.scm"),
        MAX_EXTENSION_QUERY_BYTES,
    )?;
    let injections_stamp = extension_asset_stamp(
        &source.extension_directory,
        &source.language_path.join("injections.scm"),
        MAX_EXTENSION_QUERY_BYTES,
    )?;
    let key = LanguageSyntaxCacheKey {
        extension_directory: source.extension_directory.clone(),
        language_path: source.language_path.clone(),
        grammar_name: grammar_name.clone(),
    };
    let cache = LANGUAGE_SYNTAX_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .map_err(|_| anyhow!("extension syntax assets are unavailable"))?;
    if let Some(cached) = cache.get(&key)
        && cached.wasm_stamp == wasm_stamp
        && cached.highlights_stamp == highlights_stamp
        && cached.injections_stamp == injections_stamp
    {
        return Ok(cached.syntax.clone());
    }
    let syntax = Arc::new(read_language_syntax(source, grammar_name)?);
    cache.retain(|cached_key, _| {
        cached_key.extension_directory != key.extension_directory
            || cached_key.language_path != key.language_path
            || cached_key.grammar_name != key.grammar_name
    });
    if cache.len() >= MAX_CACHED_LANGUAGE_SYNTAX
        && let Some(oldest_key) = cache.keys().next().cloned()
    {
        cache.remove(&oldest_key);
    }
    cache.insert(
        key,
        CachedLanguageSyntax {
            wasm_stamp,
            highlights_stamp,
            injections_stamp,
            syntax: syntax.clone(),
        },
    );
    Ok(syntax)
}

fn read_language_syntax(
    source: LanguageSource,
    grammar_name: String,
) -> Result<LanguageSyntax> {
    let wasm_path = Path::new("grammars").join(format!("{grammar_name}.wasm"));
    let wasm = read_extension_asset(
        &source.extension_directory,
        &wasm_path,
        MAX_EXTENSION_GRAMMAR_BYTES,
    )?
    .ok_or_else(|| anyhow!("extension grammar {grammar_name} is missing"))?;
    let read_query = |filename: &str| -> Result<String> {
        let query_path = source.language_path.join(filename);
        let Some(bytes) = read_extension_asset(
            &source.extension_directory,
            &query_path,
            MAX_EXTENSION_QUERY_BYTES,
        )?
        else {
            return Ok(String::new());
        };
        String::from_utf8(bytes).context("extension query is not valid UTF-8")
    };
    let highlights = read_query("highlights.scm")?;
    let injections = read_query("injections.scm")?;
    let mut digest = Sha256::new();
    digest.update(&wasm);
    digest.update(highlights.as_bytes());
    digest.update(injections.as_bytes());
    Ok(LanguageSyntax {
        extension_directory: source.extension_directory,
        language_name: source.definition.name,
        grammar_name,
        wasm,
        highlights,
        injections,
        asset_digest: digest.finalize().into(),
    })
}

fn language_source_for_path(
    extensions_root: &Path,
    path: &Path,
    content: Option<&str>,
) -> Result<Option<LanguageSource>> {
    let sources = language_sources_for_root(extensions_root)?;
    let source =
        language_source_from_path_sources(&sources.definitions, path, content);
    match (source, &sources.first_error) {
        (Some(source), _) => Ok(Some(source)),
        (None, Some(error)) => Err(anyhow!("{error}")),
        (None, None) => Ok(None),
    }
}

fn language_source_from_path_sources(
    definitions: &[LanguageSource],
    path: &Path,
    content: Option<&str>,
) -> Option<LanguageSource> {
    definitions
        .iter()
        .filter(|source| source.definition.matches(path, content))
        .max_by_key(|source| {
            source
                .definition
                .path_suffixes
                .iter()
                .filter(|suffix| {
                    path.file_name().and_then(|name| name.to_str()).is_some_and(
                        |name| {
                            name.eq_ignore_ascii_case(suffix)
                                || name.to_ascii_lowercase().ends_with(&format!(
                                    ".{}",
                                    suffix.to_ascii_lowercase()
                                ))
                        },
                    )
                })
                .map(String::len)
                .max()
                .unwrap_or_default()
        })
        .cloned()
}

fn language_sources_for_root(
    extensions_root: &Path,
) -> Result<Arc<CachedLanguageSources>> {
    let cache = LANGUAGE_SOURCES_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .map_err(|_| anyhow!("extension language definitions are unavailable"))?;
    if let Some(sources) = cache.get(extensions_root) {
        return Ok(sources.clone());
    }

    let mut definitions = Vec::new();
    let mut first_error = None;
    if extensions_root.exists() {
        for entry in std::fs::read_dir(extensions_root)? {
            let entry = entry?;
            let directory = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    first_error.get_or_insert(error.into());
                    continue;
                }
            };
            if !file_type.is_dir()
                || entry
                    .file_name()
                    .to_str()
                    .is_none_or(|name| name.starts_with('.'))
            {
                continue;
            }
            let manifest_path = directory.join("extension.toml");
            if !manifest_path.is_file() {
                continue;
            }
            match language_definitions_for_extension(&directory) {
                Ok(extension_definitions) => {
                    definitions.extend(extension_definitions)
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
    }
    let sources = Arc::new(CachedLanguageSources {
        definitions,
        first_error: first_error.map(|error| format!("{error:#}")),
    });
    if cache.len() >= MAX_CACHED_LANGUAGE_SOURCE_ROOTS
        && let Some(evicted_root) = cache.keys().next().cloned()
    {
        cache.remove(&evicted_root);
    }
    cache.insert(extensions_root.to_path_buf(), sources.clone());
    Ok(sources)
}

fn invalidate_language_sources_cache(extensions_root: &Path) {
    let Some(cache) = LANGUAGE_SOURCES_CACHE.get() else {
        return;
    };
    cache
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(extensions_root);
}

fn language_definitions_for_extension(
    directory: &Path,
) -> Result<Vec<LanguageSource>> {
    let manifest_bytes = read_extension_asset(
        directory,
        Path::new("extension.toml"),
        MAX_EXTENSION_METADATA_BYTES,
    )?
    .ok_or_else(|| anyhow!("extension manifest is missing"))?;
    let manifest_source = String::from_utf8(manifest_bytes)
        .context("extension manifest is not valid UTF-8")?;
    let manifest: ExtensionManifest =
        toml::from_str(&manifest_source).context("parsing extension manifest")?;
    anyhow::ensure!(
        directory.file_name().and_then(|name| name.to_str())
            == Some(manifest.id.as_str()),
        "extension directory name does not match manifest id"
    );
    let language_paths = if manifest.languages.is_empty() {
        let languages_root = directory.join("languages");
        std::fs::read_dir(languages_root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| PathBuf::from("languages").join(entry.file_name()))
            .collect()
    } else {
        manifest.languages
    };
    let mut definitions = Vec::new();
    for language_path in language_paths {
        anyhow::ensure!(
            language_path.is_relative()
                && !language_path.components().any(|component| {
                    matches!(component, std::path::Component::ParentDir)
                }),
            "language config path escapes the extension directory"
        );
        let config_relative_path = language_path.join("config.toml");
        let config_path = directory.join(&config_relative_path);
        let Some(config_bytes) = read_extension_asset(
            directory,
            &config_relative_path,
            MAX_EXTENSION_METADATA_BYTES,
        )?
        else {
            continue;
        };
        let config_source = String::from_utf8(config_bytes).with_context(|| {
            format!("{} is not valid UTF-8", config_path.display())
        })?;
        let config: LanguageConfig = toml::from_str(&config_source)
            .with_context(|| format!("parsing {}", config_path.display()))?;
        if let Some(grammar_name) = &config.grammar {
            anyhow::ensure!(
                valid_grammar_name(grammar_name),
                "invalid grammar name in {}",
                config_path.display()
            );
        }
        definitions.push(LanguageSource {
            definition: LanguageDefinition {
                name: config.name,
                grammar: config.grammar,
                comments: LanguageComments {
                    line_comment: config
                        .line_comments
                        .into_iter()
                        .find(|line_comment| !line_comment.trim().is_empty()),
                    block_comment: config
                        .block_comment
                        .map(BlockCommentDefinition::into_delimiters),
                },
                path_suffixes: config.path_suffixes,
                first_line_pattern: config.first_line_pattern,
            },
            extension_directory: directory.to_path_buf(),
            language_path,
            grammar_names: manifest.grammars.keys().cloned().collect(),
        });
    }
    Ok(definitions)
}

fn valid_grammar_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn read_extension_asset(
    extension_directory: &Path,
    relative_path: &Path,
    max_bytes: u64,
) -> Result<Option<Vec<u8>>> {
    anyhow::ensure!(
        relative_path.is_relative()
            && relative_path.components().all(|component| matches!(
                component,
                std::path::Component::Normal(_)
            )),
        "extension asset path escapes the extension directory"
    );
    let root_metadata = std::fs::symlink_metadata(extension_directory)?;
    anyhow::ensure!(
        root_metadata.is_dir() && !root_metadata.file_type().is_symlink(),
        "extension directory is not a regular directory"
    );
    let mut asset_path = extension_directory.to_path_buf();
    let mut components = relative_path.components().peekable();
    while let Some(std::path::Component::Normal(component)) = components.next() {
        asset_path.push(component);
        let metadata = match std::fs::symlink_metadata(&asset_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "extension asset path contains a symlink"
        );
        if components.peek().is_some() {
            anyhow::ensure!(
                metadata.is_dir(),
                "extension asset parent is not a directory"
            );
        } else {
            anyhow::ensure!(
                metadata.is_file(),
                "extension asset is not a regular file"
            );
            anyhow::ensure!(
                metadata.len() <= max_bytes,
                "extension asset exceeds the size limit"
            );
            let mut bytes = Vec::new();
            std::fs::File::open(asset_path)?
                .take(max_bytes + 1)
                .read_to_end(&mut bytes)?;
            anyhow::ensure!(
                bytes.len() as u64 <= max_bytes,
                "extension asset exceeds the size limit"
            );
            return Ok(Some(bytes));
        }
    }
    Err(anyhow!("extension asset path is empty"))
}

fn extension_asset_stamp(
    extension_directory: &Path,
    relative_path: &Path,
    max_bytes: u64,
) -> Result<Option<ExtensionAssetStamp>> {
    anyhow::ensure!(
        relative_path.is_relative()
            && relative_path.components().all(|component| matches!(
                component,
                std::path::Component::Normal(_)
            )),
        "extension asset path escapes the extension directory"
    );
    let root_metadata = std::fs::symlink_metadata(extension_directory)?;
    anyhow::ensure!(
        root_metadata.is_dir() && !root_metadata.file_type().is_symlink(),
        "extension directory is not a regular directory"
    );
    let mut asset_path = extension_directory.to_path_buf();
    let mut components = relative_path.components().peekable();
    while let Some(std::path::Component::Normal(component)) = components.next() {
        asset_path.push(component);
        let metadata = match std::fs::symlink_metadata(&asset_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "extension asset path contains a symlink"
        );
        if components.peek().is_some() {
            anyhow::ensure!(
                metadata.is_dir(),
                "extension asset parent is not a directory"
            );
        } else {
            anyhow::ensure!(
                metadata.is_file(),
                "extension asset is not a regular file"
            );
            anyhow::ensure!(
                metadata.len() <= max_bytes,
                "extension asset exceeds the size limit"
            );
            return Ok(Some(ExtensionAssetStamp {
                length: metadata.len(),
                modified: metadata.modified()?,
            }));
        }
    }
    Err(anyhow!("extension asset path is empty"))
}

fn validate_extension_grammars(
    extension_directory: &Path,
    manifest: &ExtensionManifest,
) -> Result<()> {
    for grammar_name in manifest.grammars.keys() {
        anyhow::ensure!(
            valid_grammar_name(grammar_name),
            "invalid extension grammar name {grammar_name}"
        );
        let wasm_path = Path::new("grammars").join(format!("{grammar_name}.wasm"));
        anyhow::ensure!(
            read_extension_asset(
                extension_directory,
                &wasm_path,
                MAX_EXTENSION_GRAMMAR_BYTES,
            )?
            .is_some(),
            "extension package is missing grammar {grammar_name}.wasm"
        );
    }
    Ok(())
}

pub fn install_extension_from_url(
    url: &str,
    extensions_root: &Path,
    extension_id: &str,
    expected_version: &str,
    expected_sha256: Option<&str>,
) -> Result<PathBuf> {
    let archive = download_bytes(url, extension_id, expected_version)?;
    if let Some(expected_sha256) = expected_sha256 {
        verify_extension_archive_sha256(&archive, expected_sha256)?;
    }
    install_extension_package(
        &archive,
        extensions_root,
        extension_id,
        Some(expected_version),
    )
}

fn verify_extension_archive_sha256(archive: &[u8], expected: &str) -> Result<()> {
    anyhow::ensure!(
        expected.len() == 64
            && expected.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid extension archive SHA-256"
    );
    anyhow::ensure!(
        format!("{:x}", Sha256::digest(archive)).eq_ignore_ascii_case(expected),
        "extension archive SHA-256 mismatch"
    );
    Ok(())
}

pub fn install_extension_package(
    archive: &[u8],
    extensions_root: &Path,
    extension_id: &str,
    expected_version: Option<&str>,
) -> Result<PathBuf> {
    if !valid_extension_id(extension_id) {
        return Err(anyhow!("invalid extension id"));
    }
    std::fs::create_dir_all(extensions_root)?;
    let _install_lock = lock_extension_installs(extensions_root)?;
    let target = extensions_root.join(extension_id);
    let backup = extensions_root.join(format!(".{extension_id}.previous"));
    restore_extension_backup(extensions_root, extension_id)?;
    let staging = tempfile::tempdir_in(extensions_root)?;
    extract_tar_gz(archive, staging.path())?;
    let package_root = extension_package_root(staging.path())?;
    let manifest: ExtensionManifest = toml::from_str(&std::fs::read_to_string(
        package_root.join("extension.toml"),
    )?)?;
    anyhow::ensure!(
        manifest.id == extension_id,
        "extension manifest id does not match requested id"
    );
    if let Some(expected_version) = expected_version {
        anyhow::ensure!(
            manifest.version.as_deref() == Some(expected_version),
            "extension manifest version does not match selected version"
        );
    }
    if let Some(schema_version) = manifest.schema_version {
        anyhow::ensure!(
            schema_version <= 1,
            "unsupported language extension manifest schema version {schema_version}"
        );
    }
    anyhow::ensure!(
        !manifest.language_servers.is_empty() || !manifest.grammars.is_empty(),
        "extension package declares no supported language servers or grammar assets"
    );
    validate_extension_grammars(&package_root, &manifest)?;
    if !manifest.language_servers.is_empty() {
        anyhow::ensure!(
            package_root.join("extension.wasm").is_file(),
            "extension package is missing extension.wasm"
        );
        let wasm = std::fs::read(package_root.join("extension.wasm"))?;
        extension_api_version(&manifest.id, &wasm)?;
        ExtensionHost::new()?.validate_component(&wasm)?;
    }
    if backup.exists() {
        std::fs::remove_dir_all(&backup)?;
    }
    if target.exists() {
        std::fs::rename(&target, &backup)?;
    }
    if let Err(error) = std::fs::rename(package_root, &target) {
        if backup.exists() {
            std::fs::rename(&backup, &target).with_context(|| {
                format!(
                    "extension install failed: {error}; restoring previous version also failed"
                )
            })?;
        }
        return Err(error.into());
    }
    if backup.exists() {
        std::fs::remove_dir_all(backup)?;
    }
    invalidate_language_sources_cache(extensions_root);
    Ok(target)
}

pub fn uninstall_extension_package(
    extensions_root: &Path,
    extension_id: &str,
) -> Result<()> {
    anyhow::ensure!(valid_extension_id(extension_id), "invalid extension id");
    let root_metadata = std::fs::symlink_metadata(extensions_root)?;
    anyhow::ensure!(
        root_metadata.is_dir() && !root_metadata.file_type().is_symlink(),
        "extension root is not a regular directory"
    );
    let _install_lock = lock_extension_installs(extensions_root)?;
    let target = extensions_root.join(extension_id);
    let backup = extensions_root.join(format!(".{extension_id}.previous"));
    let target_exists =
        validate_installed_extension_directory(&target, extension_id)?;
    let backup_exists =
        validate_installed_extension_directory(&backup, extension_id)?;
    anyhow::ensure!(
        target_exists || backup_exists,
        "language extension {extension_id} is not installed"
    );

    // Remove a previous version first so a partial uninstall can never be
    // mistaken for an interrupted update and restored by discovery.
    if backup_exists {
        std::fs::remove_dir_all(&backup)?;
    }
    if target_exists {
        std::fs::remove_dir_all(&target)?;
    }
    if let Some(cache) = LANGUAGE_SYNTAX_CACHE.get() {
        cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|key, _| {
                key.extension_directory != target
                    && key.extension_directory != backup
            });
    }
    invalidate_language_sources_cache(extensions_root);
    Ok(())
}

fn validate_installed_extension_directory(
    directory: &Path,
    extension_id: &str,
) -> Result<bool> {
    let metadata = match std::fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(false);
        }
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "installed extension path is not a regular directory"
    );
    let manifest = read_extension_asset(
        directory,
        Path::new("extension.toml"),
        MAX_EXTENSION_METADATA_BYTES,
    )?
    .ok_or_else(|| anyhow!("installed extension is missing extension.toml"))?;
    let manifest = std::str::from_utf8(&manifest)
        .context("installed extension manifest is not valid UTF-8")?;
    let manifest: ExtensionManifest =
        toml::from_str(manifest).context("parsing installed extension manifest")?;
    anyhow::ensure!(
        manifest.id == extension_id,
        "installed extension manifest ID does not match its directory"
    );
    Ok(true)
}

fn valid_extension_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && !id.starts_with('.')
        && !id.contains('/')
        && !id.contains('\\')
}

fn restore_extension_backup(root: &Path, id: &str) -> Result<()> {
    let backup = root.join(format!(".{id}.previous"));
    let backup_metadata = match std::fs::symlink_metadata(&backup) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        backup_metadata.is_dir(),
        "extension backup is not a directory"
    );
    let target = root.join(id);
    match std::fs::symlink_metadata(&target) {
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let manifest_path = backup.join("extension.toml");
    let manifest_metadata = std::fs::symlink_metadata(&manifest_path)?;
    anyhow::ensure!(
        manifest_metadata.is_file() && manifest_metadata.len() <= 256 * 1024,
        "extension backup manifest is invalid"
    );
    let manifest: ExtensionManifest =
        toml::from_str(&std::fs::read_to_string(&manifest_path)?)?;
    anyhow::ensure!(
        manifest.id == id,
        "extension backup manifest ID does not match"
    );
    anyhow::ensure!(
        !manifest.language_servers.is_empty() || !manifest.grammars.is_empty(),
        "extension backup has no supported content"
    );
    validate_extension_grammars(&backup, &manifest)?;
    if !manifest.language_servers.is_empty() {
        let wasm_path = backup.join("extension.wasm");
        anyhow::ensure!(
            std::fs::symlink_metadata(&wasm_path)?.is_file(),
            "extension backup has no language server component"
        );
        let wasm = std::fs::read(wasm_path)?;
        extension_api_version(&manifest.id, &wasm)?;
    }
    std::fs::rename(&backup, &target)
        .context("restoring an interrupted extension update")?;
    Ok(())
}

fn restore_interrupted_extension_updates(
    root: &Path,
) -> Result<Vec<(PathBuf, anyhow::Error)>> {
    let mut errors = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_prefix('.'))
            .and_then(|name| name.strip_suffix(".previous"))
            .filter(|id| valid_extension_id(id))
        else {
            continue;
        };
        if let Err(error) = restore_extension_backup(root, id) {
            errors.push((entry.path(), error));
        }
    }
    Ok(errors)
}

struct ExtensionInstallLock(std::fs::File);

impl Drop for ExtensionInstallLock {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.0) {
            eprintln!("unlocking AHEAD extension installs: {error}");
        }
    }
}

fn lock_extension_installs(root: &Path) -> Result<ExtensionInstallLock> {
    anyhow::ensure!(
        std::fs::symlink_metadata(root)?.is_dir(),
        "extension root is not a regular directory"
    );
    let lock_path = root.join(".ahead-extension-install.lock");
    match std::fs::symlink_metadata(&lock_path) {
        Ok(metadata) => anyhow::ensure!(
            metadata.is_file(),
            "extension install lock is not a regular file"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    // ponytail: serialize the whole extension root; use per-ID locks only if installs become a bottleneck.
    file.lock_exclusive()?;
    Ok(ExtensionInstallLock(file))
}

fn download_bytes(
    url: &str,
    extension_id: &str,
    expected_version: &str,
) -> Result<Vec<u8>> {
    let url = reqwest::Url::parse(url).context("invalid extension download URL")?;
    anyhow::ensure!(
        allowed_extension_download_source_url(&url, extension_id, expected_version,),
        "extension downloads must use an approved registry URL over HTTPS"
    );
    let extension_id = extension_id.to_owned();
    let expected_version = expected_version.to_owned();
    let client = reqwest::blocking::Client::builder()
        .timeout(EXTENSION_DOWNLOAD_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() < 10
                && allowed_extension_download_redirect_url(
                    attempt.url(),
                    &extension_id,
                    &expected_version,
                )
            {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()?;
    let mut response = client
        .get(url.as_str())
        .send()
        .with_context(|| format!("downloading {url}"))?;
    if !response.status().is_success() {
        return Err(anyhow!("download failed with status {}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_EXTENSION_DOWNLOAD_BYTES)
    {
        return Err(anyhow!("extension download exceeds the 512 MiB limit"));
    }
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(MAX_EXTENSION_DOWNLOAD_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_EXTENSION_DOWNLOAD_BYTES,
        "extension download exceeds the 512 MiB limit"
    );
    Ok(bytes)
}

fn allowed_extension_download_source_url(
    url: &reqwest::Url,
    extension_id: &str,
    expected_version: &str,
) -> bool {
    if is_test_loopback_url(url) {
        return true;
    }
    if !is_credential_free_https_url(url)
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    match url.host_str() {
        Some("api.zed.dev") => {
            is_zed_extension_download_path(url, extension_id, expected_version)
        }
        Some("github.com") => {
            is_ahead_release_download_path(url, extension_id, expected_version)
        }
        _ => false,
    }
}

fn allowed_extension_download_redirect_url(
    url: &reqwest::Url,
    extension_id: &str,
    expected_version: &str,
) -> bool {
    if is_test_loopback_url(url) {
        return true;
    }
    if !is_credential_free_https_url(url) || url.fragment().is_some() {
        return false;
    }
    match url.host_str() {
        Some("api.zed.dev") => {
            url.query().is_none()
                && is_zed_extension_download_path(
                    url,
                    extension_id,
                    expected_version,
                )
        }
        Some("github.com") => {
            url.query().is_none()
                && is_ahead_release_download_path(
                    url,
                    extension_id,
                    expected_version,
                )
        }
        Some(
            "zed-extensions.nyc3.digitaloceanspaces.com"
            | "release-assets.githubusercontent.com"
            | "objects.githubusercontent.com",
        ) => true,
        _ => false,
    }
}

fn is_credential_free_https_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
}

fn is_test_loopback_url(url: &reqwest::Url) -> bool {
    cfg!(test)
        && url.scheme() == "http"
        && url.username().is_empty()
        && url.password().is_none()
        && match url.host() {
            Some(url::Host::Ipv4(address)) => address.is_loopback(),
            Some(url::Host::Ipv6(address)) => address.is_loopback(),
            Some(url::Host::Domain("localhost")) => true,
            _ => false,
        }
}

fn is_zed_extension_download_path(
    url: &reqwest::Url,
    extension_id: &str,
    expected_version: &str,
) -> bool {
    let Some(segments) = url
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
    else {
        return false;
    };
    segments.len() == 4
        && segments[0] == "extensions"
        && segments[1] == extension_id
        && segments[2] == expected_version
        && segments[3] == "download"
        && is_safe_extension_path_segment(extension_id)
        && is_safe_extension_path_segment(expected_version)
}

fn is_ahead_release_download_path(
    url: &reqwest::Url,
    extension_id: &str,
    expected_version: &str,
) -> bool {
    let Some(segments) = url
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
    else {
        return false;
    };
    segments.len() == 6
        && segments[0] == "Kade-Powell"
        && segments[1] == "ahead"
        && segments[2] == "releases"
        && segments[3] == "download"
        && is_safe_extension_path_segment(segments[4])
        && segments[5] == format!("{extension_id}-{expected_version}.tar.gz")
        && is_safe_extension_path_segment(extension_id)
        && is_safe_extension_path_segment(expected_version)
}

fn is_safe_extension_path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+')
        })
}

async fn latest_npm_package_version(url: &str) -> Result<String> {
    let response = reqwest::Client::builder()
        .timeout(EXTENSION_DOWNLOAD_TIMEOUT)
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    let package: serde_json::Value = response.json().await?;
    package["dist-tags"]["latest"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("NPM response has no latest version"))
}

async fn github_release(
    repo: &str,
    tag: Option<&str>,
    require_assets: bool,
    include_prerelease: bool,
) -> Result<github::GithubRelease, String> {
    let result: Result<github::GithubRelease> = async {
        let endpoint = match tag {
            Some(tag) => {
                format!("https://api.github.com/repos/{repo}/releases/tags/{tag}")
            }
            None => {
                format!("https://api.github.com/repos/{repo}/releases?per_page=30")
            }
        };
        let response = reqwest::Client::builder()
            .timeout(EXTENSION_DOWNLOAD_TIMEOUT)
            .user_agent("ahead-language-extension-host")
            .build()?
            .get(endpoint)
            .send()
            .await?
            .error_for_status()?;
        let releases: Vec<GithubReleaseResponse> = if tag.is_some() {
            vec![response.json().await?]
        } else {
            response.json().await?
        };
        let release = releases
            .into_iter()
            .find(|release| {
                (include_prerelease || !release.prerelease)
                    && (!require_assets || !release.assets.is_empty())
            })
            .ok_or_else(|| anyhow!("no matching GitHub release found"))?;
        Ok(github::GithubRelease {
            version: release.tag_name,
            assets: release
                .assets
                .into_iter()
                .map(|asset| github::GithubReleaseAsset {
                    name: asset.name,
                    download_url: asset.browser_download_url,
                    digest: asset.digest.and_then(|digest| {
                        digest.strip_prefix("sha256:").map(str::to_string)
                    }),
                })
                .collect(),
        })
    }
    .await;
    result.map_err(|error| error.to_string())
}

fn extract_tar_gz(archive_bytes: &[u8], destination_root: &Path) -> Result<()> {
    extract_tar_gz_with_limits(
        archive_bytes,
        destination_root,
        MAX_EXTENSION_UNPACKED_BYTES,
        MAX_EXTENSION_ARCHIVE_ENTRIES,
    )
}

fn extract_tar_gz_with_limits(
    archive_bytes: &[u8],
    destination_root: &Path,
    max_unpacked_bytes: u64,
    max_entries: usize,
) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(Cursor::new(archive_bytes));
    let mut archive = tar::Archive::new(decoder);
    let mut unpacked_bytes = 0_u64;
    for (index, entry) in archive.entries()?.enumerate() {
        anyhow::ensure!(
            index < max_entries,
            "extension archive exceeds the entry limit"
        );
        let mut entry = entry?;
        unpacked_bytes = unpacked_bytes
            .checked_add(entry.size())
            .filter(|total| *total <= max_unpacked_bytes)
            .ok_or_else(|| {
                anyhow!("extension archive exceeds the unpacked size limit")
            })?;
        let relative = entry.path()?.into_owned();
        if relative == Path::new(".") && entry.header().entry_type().is_dir() {
            continue;
        }
        let destination = checked_extension_path(
            destination_root,
            relative
                .to_str()
                .ok_or_else(|| anyhow!("archive path is not UTF-8"))?,
        )?;
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(destination)?;
        } else if entry.header().entry_type().is_file() {
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            entry.unpack(destination)?;
        } else {
            return Err(anyhow!("extension archive contains an unsupported entry"));
        }
    }
    Ok(())
}

fn extension_package_root(staging_root: &Path) -> Result<PathBuf> {
    if staging_root.join("extension.toml").is_file() {
        return Ok(staging_root.to_path_buf());
    }
    let directories = std::fs::read_dir(staging_root)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| entry.path())
        })
        .collect::<Vec<_>>();
    if directories.len() == 1 && directories[0].join("extension.toml").is_file() {
        return Ok(directories[0].clone());
    }
    Err(anyhow!(
        "extension archive has no extension.toml at its root"
    ))
}

#[derive(Debug, Clone)]
pub struct ProjectContext;

#[derive(Debug, Clone)]
pub struct KeyValueStoreContext;

#[derive(Debug, Clone)]
pub struct HttpResponseStreamContext {
    response: Arc<tokio::sync::Mutex<reqwest::Response>>,
}

struct HostState {
    table: ResourceTable,
    wasi: WasiCtx,
    worktree_root: PathBuf,
    work_dir: PathBuf,
    process_capabilities: Vec<ProcessCapability>,
    granted_capabilities: ExtensionCapabilityGrants,
    node_runtime: NodeRuntime,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl context_server::Host for HostState {}
impl common::Host for HostState {}
impl dap::Host for HostState {
    async fn resolve_tcp_template(
        &mut self,
        _template: dap::TcpArgumentsTemplate,
    ) -> wasmtime::Result<Result<dap::TcpArguments, String>> {
        Ok(Err(
            "debug adapters are not supported by AHEAD's language host".into(),
        ))
    }
}

impl github::Host for HostState {
    async fn latest_github_release(
        &mut self,
        repo: String,
        options: github::GithubReleaseOptions,
    ) -> wasmtime::Result<Result<github::GithubRelease, String>> {
        Ok(
            github_release(&repo, None, options.require_assets, options.pre_release)
                .await,
        )
    }

    async fn github_release_by_tag_name(
        &mut self,
        repo: String,
        tag: String,
    ) -> wasmtime::Result<Result<github::GithubRelease, String>> {
        Ok(github_release(&repo, Some(&tag), true, true).await)
    }
}

impl http_client::Host for HostState {
    async fn fetch(
        &mut self,
        request: http_client::HttpRequest,
    ) -> wasmtime::Result<Result<http_client::HttpResponse, String>> {
        let url = request.url.clone();
        let result = async {
            let client = extension_http_client(&request.redirect_policy)?;
            let mut response = client.execute(build_extension_http_request(&client, &request)?)
                .await
                .map_err(|error| error.to_string())?;
            if response.status().is_client_error() || response.status().is_server_error() {
                return Err(format!(
                    "failed to fetch '{url}': status code {}",
                    response.status()
                ));
            }

            let mut headers = Vec::new();
            for (name, value) in response.headers() {
                headers.push((
                    name.to_string(),
                    value.to_str().unwrap_or_default().to_string(),
                ));
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_EXTENSION_HTTP_RESPONSE_BYTES as u64)
            {
                return Err(format!(
                    "HTTP response exceeds the {MAX_EXTENSION_HTTP_RESPONSE_BYTES}-byte limit"
                ));
            }

            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| error.to_string())?
            {
                if body.len().saturating_add(chunk.len()) > MAX_EXTENSION_HTTP_RESPONSE_BYTES {
                    return Err(format!(
                        "HTTP response exceeds the {MAX_EXTENSION_HTTP_RESPONSE_BYTES}-byte limit"
                    ));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(http_client::HttpResponse { headers, body })
        }
        .await;
        Ok(result)
    }

    async fn fetch_stream(
        &mut self,
        request: http_client::HttpRequest,
    ) -> wasmtime::Result<Result<Resource<HttpResponseStreamContext>, String>> {
        let result = async {
            let client = extension_http_client(&request.redirect_policy)?;
            let response = client
                .execute(build_extension_http_request(&client, &request)?)
                .await
                .map_err(|error| error.to_string())?;
            self.table
                .push(HttpResponseStreamContext {
                    response: Arc::new(tokio::sync::Mutex::new(response)),
                })
                .map_err(|error| error.to_string())
        }
        .await;
        Ok(result)
    }
}

impl HostHttpResponseStream for HostState {
    async fn next_chunk(
        &mut self,
        stream: Resource<HttpResponseStreamContext>,
    ) -> wasmtime::Result<Result<Option<Vec<u8>>, String>> {
        let response = self.table.get(&stream)?.response.clone();
        let result = response
            .lock()
            .await
            .chunk()
            .await
            .map(|chunk| chunk.map(|chunk| chunk.to_vec()))
            .map_err(|error| error.to_string());
        Ok(result)
    }

    async fn drop(
        &mut self,
        stream: Resource<HttpResponseStreamContext>,
    ) -> wasmtime::Result<()> {
        self.table.delete(stream)?;
        Ok(())
    }
}

fn extension_http_client(
    redirect_policy: &http_client::RedirectPolicy,
) -> Result<reqwest::Client, String> {
    let redirect_policy = match redirect_policy {
        http_client::RedirectPolicy::NoFollow => reqwest::redirect::Policy::none(),
        http_client::RedirectPolicy::FollowLimit(limit) => {
            reqwest::redirect::Policy::limited(*limit as usize)
        }
        http_client::RedirectPolicy::FollowAll => {
            reqwest::redirect::Policy::custom(|attempt| attempt.follow())
        }
    };
    reqwest::Client::builder()
        .timeout(EXTENSION_DOWNLOAD_TIMEOUT)
        .redirect(redirect_policy)
        .build()
        .map_err(|error| error.to_string())
}

fn build_extension_http_request(
    client: &reqwest::Client,
    request: &http_client::HttpRequest,
) -> Result<reqwest::Request, String> {
    let method = match request.method {
        http_client::HttpMethod::Get => reqwest::Method::GET,
        http_client::HttpMethod::Head => reqwest::Method::HEAD,
        http_client::HttpMethod::Post => reqwest::Method::POST,
        http_client::HttpMethod::Put => reqwest::Method::PUT,
        http_client::HttpMethod::Delete => reqwest::Method::DELETE,
        http_client::HttpMethod::Options => reqwest::Method::OPTIONS,
        http_client::HttpMethod::Patch => reqwest::Method::PATCH,
    };
    let mut builder = client.request(method, &request.url);
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    builder
        .body(request.body.clone().unwrap_or_default())
        .build()
        .map_err(|error| error.to_string())
}

impl platform::Host for HostState {
    async fn current_platform(
        &mut self,
    ) -> wasmtime::Result<(platform::Os, platform::Architecture)> {
        let operating_system = match env::consts::OS {
            "macos" => platform::Os::Mac,
            "linux" => platform::Os::Linux,
            "windows" => platform::Os::Windows,
            other => {
                return Err(wasmtime::Error::msg(format!(
                    "unsupported OS: {other}"
                )));
            }
        };
        let architecture = match env::consts::ARCH {
            "aarch64" => platform::Architecture::Aarch64,
            "x86_64" => platform::Architecture::X8664,
            other => {
                return Err(wasmtime::Error::msg(format!(
                    "unsupported architecture: {other}"
                )));
            }
        };
        Ok((operating_system, architecture))
    }
}

impl process::Host for HostState {
    async fn run_command(
        &mut self,
        command: process::Command,
    ) -> wasmtime::Result<Result<process::Output, String>> {
        if !self.granted_capabilities.allows_process_exec(
            &self.process_capabilities,
            &command.command,
            &command.args,
        ) {
            return Ok(Err(format!(
                "process:exec for {:?} {:?} requires both an extension manifest declaration and a matching user-level granted_extension_capabilities entry",
                command.command, command.args
            )));
        }
        let mut process = ProcessCommand::new(&command.command);
        process.args(&command.args).envs(command.env);
        let output = run_extension_command(process, EXTENSION_COMMAND_TIMEOUT).await;
        Ok(output.map(|output| process::Output {
            status: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        }))
    }
}

impl nodejs::Host for HostState {
    async fn node_binary_path(
        &mut self,
    ) -> wasmtime::Result<Result<String, String>> {
        Ok(self
            .node_runtime
            .node_binary_path()
            .await
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|error| error.to_string()))
    }

    async fn npm_package_latest_version(
        &mut self,
        package_name: String,
    ) -> wasmtime::Result<Result<String, String>> {
        let url = format!("https://registry.npmjs.org/{package_name}");
        Ok(latest_npm_package_version(&url)
            .await
            .map_err(|error| error.to_string()))
    }

    async fn npm_package_installed_version(
        &mut self,
        package_name: String,
    ) -> wasmtime::Result<Result<Option<String>, String>> {
        let result = (|| -> Result<Option<String>> {
            let package_json = checked_package_path(&self.work_dir, &package_name)?
                .join("package.json");
            if !package_json.is_file() {
                return Ok(None);
            }
            let package: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(package_json)?)?;
            Ok(package["version"].as_str().map(str::to_string))
        })();
        Ok(result.map_err(|error| error.to_string()))
    }

    async fn npm_install_package(
        &mut self,
        package_name: String,
        version: String,
    ) -> wasmtime::Result<Result<(), String>> {
        if !self.granted_capabilities.allows_npm_install(&package_name) {
            return Ok(Err(format!(
                "npm:install for {package_name} is not allowed by user-level granted_extension_capabilities"
            )));
        }
        let invocation = match self.node_runtime.npm_invocation().await {
            Ok(invocation) => invocation,
            Err(error) => return Ok(Err(error.to_string())),
        };
        let mut command = ProcessCommand::new(invocation.program);
        command
            .current_dir(&self.work_dir)
            .args(invocation.prefix_arguments)
            .args([
                "install",
                "--prefix",
                self.work_dir.to_string_lossy().as_ref(),
                "--ignore-scripts",
                "--no-package-lock",
                &format!("{package_name}@{version}"),
            ]);
        let result = run_extension_command(command, EXTENSION_COMMAND_TIMEOUT)
            .await
            .and_then(|output| {
                if output.status.success() {
                    Ok(())
                } else {
                    Err(String::from_utf8_lossy(&output.stderr).into_owned())
                }
            });
        Ok(result)
    }
}
impl lsp::Host for HostState {}
impl slash_command::Host for HostState {}

impl HostKeyValueStore for HostState {
    async fn insert(
        &mut self,
        _store: Resource<KeyValueStoreContext>,
        _key: String,
        _value: String,
    ) -> wasmtime::Result<Result<(), String>> {
        Ok(Err(
            "key-value storage is not supported by AHEAD's language host".into(),
        ))
    }

    async fn drop(
        &mut self,
        _store: Resource<KeyValueStoreContext>,
    ) -> wasmtime::Result<()> {
        Ok(())
    }
}

impl HostProject for HostState {
    async fn worktree_ids(
        &mut self,
        _project: Resource<ProjectContext>,
    ) -> wasmtime::Result<Vec<u64>> {
        Ok(Vec::new())
    }

    async fn drop(
        &mut self,
        _project: Resource<ProjectContext>,
    ) -> wasmtime::Result<()> {
        Ok(())
    }
}

impl HostWorktree for HostState {
    async fn id(
        &mut self,
        _worktree: Resource<WorktreeContext>,
    ) -> wasmtime::Result<u64> {
        Ok(1)
    }

    async fn root_path(
        &mut self,
        _worktree: Resource<WorktreeContext>,
    ) -> wasmtime::Result<String> {
        Ok(self.worktree_root.to_string_lossy().into_owned())
    }

    async fn read_text_file(
        &mut self,
        _worktree: Resource<WorktreeContext>,
        path: String,
    ) -> wasmtime::Result<Result<String, String>> {
        let path = checked_worktree_path(&self.worktree_root, &path)
            .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
        Ok(std::fs::read_to_string(path).map_err(|error| error.to_string()))
    }

    async fn which(
        &mut self,
        _worktree: Resource<WorktreeContext>,
        binary_name: String,
    ) -> wasmtime::Result<Option<String>> {
        Ok(which_binary(&binary_name))
    }

    async fn shell_env(
        &mut self,
        _worktree: Resource<WorktreeContext>,
    ) -> wasmtime::Result<Vec<(String, String)>> {
        Ok(env::vars().collect())
    }

    async fn drop(
        &mut self,
        _worktree: Resource<WorktreeContext>,
    ) -> wasmtime::Result<()> {
        Ok(())
    }
}

fn lsp_settings_for_extension(
    worktree_root: &Path,
    user_home: Option<&Path>,
    server_id: Option<&str>,
) -> Result<String> {
    let Some(server_id) = server_id else {
        return Ok("{}".to_string());
    };
    let mut settings = serde_json::Map::new();
    let mut layers = Vec::with_capacity(4);
    if let Some(user_home) = user_home {
        layers.push((user_home, "settings.toml", "~/.ahead/settings.toml"));
    }
    layers.extend([
        (worktree_root, "config.toml", ".ahead/config.toml"),
        (
            worktree_root,
            "config.local.toml",
            ".ahead/config.local.toml",
        ),
        (worktree_root, "settings.toml", ".ahead/settings.toml"),
    ]);
    for (root, filename, source) in layers {
        let Some(content) = ahead_core::config::read_ahead_config(root, filename)
            .with_context(|| format!("reading {source}"))?
        else {
            continue;
        };
        let config: toml::Table =
            content.parse().map_err(|error: toml::de::Error| {
                anyhow!("parsing {source}: {}", error.message())
            })?;
        let Some(servers) = config.get("lsp") else {
            continue;
        };
        let servers = servers
            .as_table()
            .with_context(|| format!("{source} [lsp] must be a table"))?;
        let Some(server) = servers.get(server_id) else {
            continue;
        };
        let server = server.as_table().with_context(|| {
            format!("{source} [lsp.{server_id}] must be a table")
        })?;
        for (key, value) in server {
            anyhow::ensure!(
                matches!(
                    key.as_str(),
                    "binary" | "settings" | "initialization_options"
                ),
                "{source} [lsp.{server_id}] has unsupported key `{key}`"
            );
            anyhow::ensure!(
                filename == "config.local.toml" || key != "binary",
                "{source} cannot set an executable LSP binary; use ignored .ahead/config.local.toml"
            );
            let value = serde_json::to_value(value)?;
            match settings.get_mut(key) {
                Some(existing) => merge_setting(existing, value),
                None => {
                    settings.insert(key.clone(), value);
                }
            }
        }
    }
    Ok(serde_json::to_string(&settings)?)
}

fn language_settings_for_extension(
    worktree_root: &Path,
    user_home: Option<&Path>,
    language_name: Option<&str>,
) -> Result<String> {
    let settings = ahead_core::config::language_editor_settings(
        worktree_root,
        user_home,
        language_name,
    )?;
    Ok(serde_json::json!({
        "tab_size": settings.tab_size,
        "hard_tabs": settings.hard_tabs,
        "preferred_line_length": settings.preferred_line_length,
    })
    .to_string())
}

fn settings_for_extension(
    worktree_root: &Path,
    user_home: Option<&Path>,
    category: &str,
    key: Option<&str>,
) -> Result<String> {
    match category {
        "lsp" => lsp_settings_for_extension(worktree_root, user_home, key),
        "language" => language_settings_for_extension(worktree_root, user_home, key),
        _ => Err(anyhow!(
            "unsupported Zed extension settings category `{category}`; AHEAD supports `language` and `lsp`"
        )),
    }
}

fn merge_setting(
    existing: &mut serde_json::Value,
    override_value: serde_json::Value,
) {
    match (existing, override_value) {
        (
            serde_json::Value::Object(existing),
            serde_json::Value::Object(overrides),
        ) => {
            for (key, value) in overrides {
                match existing.get_mut(&key) {
                    Some(existing) => merge_setting(existing, value),
                    None => {
                        existing.insert(key, value);
                    }
                }
            }
        }
        (existing, override_value) => *existing = override_value,
    }
}

fn apply_lsp_settings_to_command(
    command: &mut LanguageServerCommand,
    settings: serde_json::Value,
    worktree_root: &Path,
) -> Result<()> {
    let settings = settings
        .as_object()
        .context("extension LSP settings must be an object")?;
    if let Some(override_options) = settings.get("initialization_options") {
        match command.initialization_options.as_mut() {
            Some(initialization_options) => {
                merge_setting(initialization_options, override_options.clone());
            }
            None => command.initialization_options = Some(override_options.clone()),
        }
    }

    let Some(binary) = settings.get("binary") else {
        return Ok(());
    };
    let binary = binary
        .as_object()
        .context("extension LSP binary settings must be a table")?;
    for key in binary.keys() {
        anyhow::ensure!(
            matches!(key.as_str(), "path" | "arguments" | "env"),
            "unsupported extension LSP binary setting `{key}`"
        );
    }
    let path = binary
        .get("path")
        .map(|path| {
            let path = path
                .as_str()
                .context("extension LSP binary.path must be a string")?;
            anyhow::ensure!(!path.is_empty(), "extension LSP binary.path is empty");
            Ok::<_, anyhow::Error>(path.to_string())
        })
        .transpose()?;
    let arguments = binary
        .get("arguments")
        .map(|arguments| {
            arguments
                .as_array()
                .context("extension LSP binary.arguments must be an array")?
                .iter()
                .enumerate()
                .map(|(index, argument)| {
                    argument
                        .as_str()
                        .map(str::to_string)
                        .with_context(|| {
                            format!(
                                "extension LSP binary.arguments[{index}] must be a string"
                            )
                        })
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?;

    if let Some(path) = path.as_deref() {
        let path = Path::new(path);
        command.command = if path.is_absolute() {
            path.to_string_lossy().into_owned()
        } else {
            worktree_root.join(path).to_string_lossy().into_owned()
        };
        command.args = arguments.unwrap_or_default();
    } else if let Some(arguments) = arguments {
        command.args = arguments;
    }

    if let Some(environment) = binary.get("env") {
        let environment = environment
            .as_object()
            .context("extension LSP binary.env must be a table")?;
        for (name, value) in environment {
            let value = value.as_str().with_context(|| {
                format!("extension LSP binary.env.{name} must be a string")
            })?;
            let mut found = false;
            for (existing_name, existing_value) in &mut command.env {
                if existing_name == name {
                    *existing_value = value.to_string();
                    found = true;
                }
            }
            if !found {
                command.env.push((name.clone(), value.to_string()));
            }
        }
    }
    Ok(())
}

impl ExtensionImports for HostState {
    async fn get_settings(
        &mut self,
        _location: Option<SettingsLocation>,
        category: String,
        key: Option<String>,
    ) -> wasmtime::Result<Result<String, String>> {
        let settings = settings_for_extension(
            &self.worktree_root,
            ahead_core::directory::Directory::home_dir().as_deref(),
            &category,
            key.as_deref(),
        );
        Ok(settings.map_err(|error| format!("{error:#}")))
    }

    async fn download_file(
        &mut self,
        url: String,
        path: String,
        file_type: DownloadedFileType,
    ) -> wasmtime::Result<Result<(), String>> {
        let result: Result<()> = async {
            let url = url::Url::parse(&url)?;
            anyhow::ensure!(
                self.granted_capabilities.allows_download(&url),
                "download_file for {} is not allowed by user-level granted_extension_capabilities",
                url
            );
            let destination = checked_extension_path(&self.work_dir, &path)?;
            let client = reqwest::Client::builder()
                .timeout(EXTENSION_DOWNLOAD_TIMEOUT)
                .build()?;
            let mut response = client
                .get(url.clone())
                .send()
                .await
                .with_context(|| format!("downloading {url}"))?;
            if !response.status().is_success() {
                return Err(anyhow!(
                    "download failed with status {}",
                    response.status()
                ));
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_EXTENSION_DOWNLOAD_BYTES)
            {
                return Err(anyhow!("extension download exceeds the 512 MiB limit"));
            }
            let mut bytes = Vec::new();
            let maximum_bytes = MAX_EXTENSION_DOWNLOAD_BYTES as usize;
            while let Some(chunk) = response.chunk().await? {
                anyhow::ensure!(
                    bytes.len().saturating_add(chunk.len()) <= maximum_bytes,
                    "extension download exceeds the 512 MiB limit"
                );
                bytes.extend_from_slice(&chunk);
            }
            match file_type {
                DownloadedFileType::Uncompressed => {
                    write_bytes(&destination, &bytes)?
                }
                DownloadedFileType::Gzip => {
                    let mut decoder =
                        flate2::read::GzDecoder::new(Cursor::new(bytes));
                    let mut decoded = Vec::new();
                    decoder.read_to_end(&mut decoded)?;
                    write_bytes(&destination, &decoded)?;
                }
                DownloadedFileType::GzipTar => {
                    let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
                    let mut archive = tar::Archive::new(decoder);
                    for entry in archive.entries()? {
                        let mut entry = entry?;
                        let relative = entry.path()?.into_owned();
                        let relative = Path::new(&path).join(relative);
                        let destination = checked_extension_path(
                            &self.work_dir,
                            relative.to_str().ok_or_else(|| {
                                anyhow!("archive path is not UTF-8")
                            })?,
                        )?;
                        if entry.header().entry_type().is_dir() {
                            std::fs::create_dir_all(destination)?;
                        } else if entry.header().entry_type().is_file() {
                            if let Some(parent) = destination.parent() {
                                std::fs::create_dir_all(parent)?;
                            }
                            entry.unpack(destination)?;
                        }
                    }
                }
                DownloadedFileType::Zip => {
                    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
                    for index in 0..archive.len() {
                        let mut entry = archive.by_index(index)?;
                        let relative = entry
                            .enclosed_name()
                            .ok_or_else(|| {
                                anyhow!("archive path escapes extension root")
                            })?
                            .to_path_buf();
                        let relative = Path::new(&path).join(relative);
                        let destination = checked_extension_path(
                            &self.work_dir,
                            relative.to_str().ok_or_else(|| {
                                anyhow!("archive path is not UTF-8")
                            })?,
                        )?;
                        if entry.is_dir() {
                            std::fs::create_dir_all(destination)?;
                        } else {
                            if let Some(parent) = destination.parent() {
                                std::fs::create_dir_all(parent)?;
                            }
                            let mut output = std::fs::File::create(destination)?;
                            std::io::copy(&mut entry, &mut output)?;
                        }
                    }
                }
            }
            Ok(())
        }
        .await;
        Ok(result.map_err(|error| error.to_string()))
    }

    async fn make_file_executable(
        &mut self,
        path: String,
    ) -> wasmtime::Result<Result<(), String>> {
        let result = (|| -> Result<()> {
            let path = checked_extension_path(&self.work_dir, &path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut permissions = std::fs::metadata(&path)?.permissions();
                permissions.set_mode(permissions.mode() | 0o111);
                std::fs::set_permissions(path, permissions)?;
            }
            Ok(())
        })();
        Ok(result.map_err(|error| error.to_string()))
    }

    async fn set_language_server_installation_status(
        &mut self,
        _server_name: String,
        _status: LanguageServerInstallationStatus,
    ) -> wasmtime::Result<()> {
        Ok(())
    }
}

impl v0_6::zed::extension::dap::Host for HostState {
    async fn resolve_tcp_template(
        &mut self,
        _template: v0_6::zed::extension::dap::TcpArgumentsTemplate,
    ) -> wasmtime::Result<Result<v0_6::zed::extension::dap::TcpArguments, String>>
    {
        Ok(Err(
            "debug adapters are not supported by AHEAD's language host".into(),
        ))
    }
}

fn v0_6_github_release(
    release: github::GithubRelease,
) -> v0_6::zed::extension::github::GithubRelease {
    v0_6::zed::extension::github::GithubRelease {
        version: release.version,
        assets: release
            .assets
            .into_iter()
            .map(|asset| v0_6::zed::extension::github::GithubReleaseAsset {
                name: asset.name,
                download_url: asset.download_url,
            })
            .collect(),
    }
}

impl v0_6::zed::extension::github::Host for HostState {
    async fn latest_github_release(
        &mut self,
        repo: String,
        options: v0_6::zed::extension::github::GithubReleaseOptions,
    ) -> wasmtime::Result<Result<v0_6::zed::extension::github::GithubRelease, String>>
    {
        Ok(
            github_release(&repo, None, options.require_assets, options.pre_release)
                .await
                .map(v0_6_github_release),
        )
    }

    async fn github_release_by_tag_name(
        &mut self,
        repo: String,
        tag: String,
    ) -> wasmtime::Result<Result<v0_6::zed::extension::github::GithubRelease, String>>
    {
        Ok(github_release(&repo, Some(&tag), true, true)
            .await
            .map(v0_6_github_release))
    }
}

impl v0_6::zed::extension::lsp::Host for HostState {}

impl v0_6::zed::extension::platform::Host for HostState {
    async fn current_platform(
        &mut self,
    ) -> wasmtime::Result<(
        v0_6::zed::extension::platform::Os,
        v0_6::zed::extension::platform::Architecture,
    )> {
        let (operating_system, architecture) =
            <HostState as platform::Host>::current_platform(self).await?;
        let operating_system = match operating_system {
            platform::Os::Mac => v0_6::zed::extension::platform::Os::Mac,
            platform::Os::Linux => v0_6::zed::extension::platform::Os::Linux,
            platform::Os::Windows => v0_6::zed::extension::platform::Os::Windows,
        };
        let architecture = match architecture {
            platform::Architecture::Aarch64 => {
                v0_6::zed::extension::platform::Architecture::Aarch64
            }
            platform::Architecture::X8664 => {
                v0_6::zed::extension::platform::Architecture::X8664
            }
        };
        Ok((operating_system, architecture))
    }
}

impl v0_6::HostKeyValueStore for HostState {
    async fn insert(
        &mut self,
        store: Resource<KeyValueStoreContext>,
        key: String,
        value: String,
    ) -> wasmtime::Result<Result<(), String>> {
        <HostState as HostKeyValueStore>::insert(self, store, key, value).await
    }

    async fn drop(
        &mut self,
        store: Resource<KeyValueStoreContext>,
    ) -> wasmtime::Result<()> {
        <HostState as HostKeyValueStore>::drop(self, store).await
    }
}

impl v0_6::HostProject for HostState {
    async fn worktree_ids(
        &mut self,
        project: Resource<ProjectContext>,
    ) -> wasmtime::Result<Vec<u64>> {
        <HostState as HostProject>::worktree_ids(self, project).await
    }

    async fn drop(
        &mut self,
        project: Resource<ProjectContext>,
    ) -> wasmtime::Result<()> {
        <HostState as HostProject>::drop(self, project).await
    }
}

impl v0_6::HostWorktree for HostState {
    async fn id(
        &mut self,
        worktree: Resource<WorktreeContext>,
    ) -> wasmtime::Result<u64> {
        <HostState as HostWorktree>::id(self, worktree).await
    }

    async fn root_path(
        &mut self,
        worktree: Resource<WorktreeContext>,
    ) -> wasmtime::Result<String> {
        <HostState as HostWorktree>::root_path(self, worktree).await
    }

    async fn read_text_file(
        &mut self,
        worktree: Resource<WorktreeContext>,
        path: String,
    ) -> wasmtime::Result<Result<String, String>> {
        <HostState as HostWorktree>::read_text_file(self, worktree, path).await
    }

    async fn which(
        &mut self,
        worktree: Resource<WorktreeContext>,
        binary_name: String,
    ) -> wasmtime::Result<Option<String>> {
        <HostState as HostWorktree>::which(self, worktree, binary_name).await
    }

    async fn shell_env(
        &mut self,
        worktree: Resource<WorktreeContext>,
    ) -> wasmtime::Result<Vec<(String, String)>> {
        <HostState as HostWorktree>::shell_env(self, worktree).await
    }

    async fn drop(
        &mut self,
        worktree: Resource<WorktreeContext>,
    ) -> wasmtime::Result<()> {
        <HostState as HostWorktree>::drop(self, worktree).await
    }
}

impl v0_6::ExtensionImports for HostState {
    async fn get_settings(
        &mut self,
        _location: Option<v0_6::SettingsLocation>,
        category: String,
        key: Option<String>,
    ) -> wasmtime::Result<Result<String, String>> {
        <HostState as ExtensionImports>::get_settings(self, None, category, key)
            .await
    }

    async fn download_file(
        &mut self,
        url: String,
        path: String,
        file_type: v0_6::DownloadedFileType,
    ) -> wasmtime::Result<Result<(), String>> {
        let file_type = match file_type {
            v0_6::DownloadedFileType::Uncompressed => {
                DownloadedFileType::Uncompressed
            }
            v0_6::DownloadedFileType::Gzip => DownloadedFileType::Gzip,
            v0_6::DownloadedFileType::GzipTar => DownloadedFileType::GzipTar,
            v0_6::DownloadedFileType::Zip => DownloadedFileType::Zip,
        };
        <HostState as ExtensionImports>::download_file(self, url, path, file_type)
            .await
    }

    async fn make_file_executable(
        &mut self,
        path: String,
    ) -> wasmtime::Result<Result<(), String>> {
        <HostState as ExtensionImports>::make_file_executable(self, path).await
    }

    async fn set_language_server_installation_status(
        &mut self,
        _server_name: String,
        _status: v0_6::LanguageServerInstallationStatus,
    ) -> wasmtime::Result<()> {
        Ok(())
    }
}

enum GuestExtension {
    V0_6(v0_6::Extension),
    V0_8(Extension),
}

type GuestSession = (Store<HostState>, GuestExtension);
type GuestSlot = Arc<tokio::sync::Mutex<Option<GuestSession>>>;

pub struct ExtensionHost {
    engine: Engine,
    sessions: Mutex<HashMap<(PathBuf, PathBuf), GuestSlot>>,
    granted_capabilities: ExtensionCapabilityGrants,
    node_runtime: NodeRuntime,
    #[cfg(test)]
    instantiations: AtomicUsize,
}

impl ExtensionHost {
    pub fn new() -> Result<Self> {
        Self::new_with_node_runtime(NodeRuntime::default())
    }

    pub fn new_with_node_runtime(node_runtime: NodeRuntime) -> Result<Self> {
        let granted_capabilities = ExtensionCapabilityGrants::load_for_user()?;
        static ENGINE: OnceLock<std::result::Result<Engine, String>> =
            OnceLock::new();
        let engine = ENGINE
            .get_or_init(|| {
                let mut config = Config::new();
                config.wasm_component_model(true);
                // Agent and server launches fork; Wasmtime's Mach ports are not fork-safe.
                #[cfg(target_os = "macos")]
                config.macos_use_mach_ports(false);
                Engine::new(&config).map_err(|error| error.to_string())
            })
            .as_ref()
            .map_err(|error| anyhow!("creating extension host engine: {error}"))?
            .clone();
        Ok(Self {
            engine,
            sessions: Mutex::new(HashMap::new()),
            granted_capabilities,
            node_runtime,
            #[cfg(test)]
            instantiations: AtomicUsize::new(0),
        })
    }

    pub fn validate_component(&self, bytes: &[u8]) -> Result<()> {
        let component = Component::from_binary(&self.engine, bytes)
            .map_err(|error| anyhow!(error))
            .context("validating Zed language extension component")?;
        for export in ["init-extension", "language-server-command"] {
            anyhow::ensure!(
                component
                    .component_type()
                    .get_export(&self.engine, export)
                    .is_some(),
                "language extension component is missing export {export}"
            );
        }
        Ok(())
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn invalidate_sessions(&self) -> Result<()> {
        self.sessions
            .lock()
            .map_err(|_| anyhow!("extension session cache is poisoned"))?
            .clear();
        Ok(())
    }

    pub fn invalidate_extension_sessions(
        &self,
        extension_directory: &Path,
    ) -> Result<()> {
        self.sessions
            .lock()
            .map_err(|_| anyhow!("extension session cache is poisoned"))?
            .retain(|(directory, _), _| directory.as_path() != extension_directory);
        Ok(())
    }

    fn add_v0_6_imports(linker: &mut Linker<HostState>) -> Result<()> {
        v0_6::Extension::add_to_linker::<HostState, HasSelf<HostState>>(
            linker,
            |state: &mut HostState| state,
        )
        .map_err(|error| anyhow!(error))
        .context("registering Zed 0.6 extension imports")
    }

    async fn extension_session(
        &self,
        extension: &LanguageServerExtension,
        worktree_root: PathBuf,
    ) -> Result<tokio::sync::OwnedMutexGuard<Option<GuestSession>>> {
        let slot = {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| anyhow!("extension session cache is poisoned"))?;
            sessions
                .entry((extension.directory.clone(), worktree_root.clone()))
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
                .clone()
        };
        let mut session = slot.lock_owned().await;
        if session.is_none() {
            *session = Some(
                self.instantiate_language_extension(extension, worktree_root)
                    .await?,
            );
        }
        Ok(session)
    }

    async fn resolve_language_server_command(
        store: &mut Store<HostState>,
        extension: &GuestExtension,
        language_server_id: &str,
        worktree: WorktreeContext,
    ) -> Result<LanguageServerCommand> {
        let work_dir = store.data().work_dir.clone();
        let worktree_root_for_command = worktree.root.clone();
        let worktree = store
            .data_mut()
            .table
            .push(worktree)
            .map_err(|error| anyhow!(error))?;
        let (command, args, env) = match extension {
            GuestExtension::V0_6(guest) => {
                let result = guest
                    .call_language_server_command(
                        &mut *store,
                        language_server_id,
                        worktree,
                    )
                    .await
                    .map_err(|error| anyhow!(error))?
                    .map_err(|error| anyhow!(error))?;
                (result.command, result.args, result.env)
            }
            GuestExtension::V0_8(guest) => {
                let result = guest
                    .call_language_server_command(
                        &mut *store,
                        language_server_id,
                        worktree,
                    )
                    .await
                    .map_err(|error| anyhow!(error))?
                    .map_err(|error| anyhow!(error))?;
                (result.command, result.args, result.env)
            }
        };
        let initialization_worktree = store
            .data_mut()
            .table
            .push(WorktreeContext {
                root: worktree_root_for_command.clone(),
            })
            .map_err(|error| anyhow!(error))?;
        let initialization_options = match extension {
            GuestExtension::V0_6(guest) => guest
                .call_language_server_initialization_options(
                    &mut *store,
                    language_server_id,
                    initialization_worktree,
                )
                .await
                .map_err(|error| anyhow!(error))?
                .map_err(|error| anyhow!(error))?,
            GuestExtension::V0_8(guest) => guest
                .call_language_server_initialization_options(
                    &mut *store,
                    language_server_id,
                    initialization_worktree,
                )
                .await
                .map_err(|error| anyhow!(error))?
                .map_err(|error| anyhow!(error))?,
        }
        .map(|options| serde_json::from_str(&options))
        .transpose()
        .context("parsing language server initialization options")?;
        let initialization_options_schema = Self::initialization_options_schema(
            store,
            extension,
            language_server_id,
            worktree_root_for_command.clone(),
        )
        .await?;
        let workspace_configuration = Self::workspace_configuration(
            store,
            extension,
            language_server_id,
            worktree_root_for_command.clone(),
        )
        .await?;
        let workspace_configuration_schema = Self::workspace_configuration_schema(
            store,
            extension,
            language_server_id,
            worktree_root_for_command,
        )
        .await?;
        let command = LanguageServerCommand {
            command: resolve_work_dir_command(&work_dir, command),
            args,
            env,
            initialization_options,
            initialization_options_schema,
            workspace_configuration,
            workspace_configuration_schema,
        };
        Ok(command)
    }

    async fn instantiate_language_extension(
        &self,
        extension: &LanguageServerExtension,
        worktree_root: PathBuf,
    ) -> Result<GuestSession> {
        let bytes = std::fs::read(&extension.wasm_path)
            .with_context(|| format!("reading {}", extension.wasm_path.display()))?;
        let component = Component::from_binary(&self.engine, &bytes)
            .map_err(|error| anyhow!(error))
            .context("loading language extension component")?;
        let mut linker = Linker::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)
            .map_err(|error| anyhow!(error))
            .context("registering WASI imports")?;
        match extension.api_version {
            ExtensionApiVersion::V0_6 => Self::add_v0_6_imports(&mut linker)?,
            ExtensionApiVersion::V0_8 => {
                Extension::add_to_linker::<HostState, HasSelf<HostState>>(
                    &mut linker,
                    |state: &mut HostState| state,
                )
                .map_err(|error| anyhow!(error))
                .context("registering extension host imports")?;
            }
        }
        let work_dir = extension
            .directory
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("work")
            .join(
                extension
                    .directory
                    .file_name()
                    .ok_or_else(|| anyhow!("extension directory has no name"))?,
            );
        std::fs::create_dir_all(&work_dir)?;
        let work_dir_string = work_dir.to_string_lossy().into_owned();
        let mut wasi_builder = WasiCtxBuilder::new();
        // The proxy owns stdin/stdout for framed editor RPC. A guest must not
        // consume requests or print unframed text into that transport.
        wasi_builder
            .inherit_stderr()
            .env("PWD", &work_dir_string)
            .env("RUST_BACKTRACE", "full");
        wasi_builder.preopened_dir(
            &work_dir,
            ".",
            wasmtime_wasi::FsPerms::ReadWrite,
        )?;
        wasi_builder.preopened_dir(
            &work_dir,
            &work_dir_string,
            wasmtime_wasi::FsPerms::ReadWrite,
        )?;
        let mut store = Store::new(
            &self.engine,
            HostState {
                table: ResourceTable::new(),
                wasi: wasi_builder.build(),
                worktree_root,
                work_dir: work_dir.clone(),
                process_capabilities: extension.capabilities.clone(),
                granted_capabilities: self.granted_capabilities.clone(),
                node_runtime: self.node_runtime.clone(),
            },
        );
        let guest = match extension.api_version {
            ExtensionApiVersion::V0_6 => {
                let guest = v0_6::Extension::instantiate_async(
                    &mut store, &component, &linker,
                )
                .await
                .map_err(|error| anyhow!(error))
                .context("instantiating Zed 0.6/0.7 language extension")?;
                guest
                    .call_init_extension(&mut store)
                    .await
                    .map_err(|error| anyhow!(error))?;
                GuestExtension::V0_6(guest)
            }
            ExtensionApiVersion::V0_8 => {
                let guest =
                    Extension::instantiate_async(&mut store, &component, &linker)
                        .await
                        .map_err(|error| anyhow!(error))
                        .context("instantiating Zed 0.8 language extension")?;
                guest
                    .call_init_extension(&mut store)
                    .await
                    .map_err(|error| anyhow!(error))?;
                GuestExtension::V0_8(guest)
            }
        };
        #[cfg(test)]
        self.instantiations.fetch_add(1, Ordering::SeqCst);
        Ok((store, guest))
    }

    async fn workspace_configuration(
        store: &mut Store<HostState>,
        extension: &GuestExtension,
        language_server_id: &str,
        worktree_root: PathBuf,
    ) -> Result<Option<serde_json::Value>> {
        let workspace_worktree = store
            .data_mut()
            .table
            .push(WorktreeContext {
                root: worktree_root,
            })
            .map_err(|error| anyhow!(error))?;
        let workspace_configuration = match extension {
            GuestExtension::V0_6(guest) => guest
                .call_language_server_workspace_configuration(
                    store,
                    language_server_id,
                    workspace_worktree,
                )
                .await
                .map_err(|error| anyhow!(error))?
                .map_err(|error| anyhow!(error))?,
            GuestExtension::V0_8(guest) => guest
                .call_language_server_workspace_configuration(
                    store,
                    language_server_id,
                    workspace_worktree,
                )
                .await
                .map_err(|error| anyhow!(error))?
                .map_err(|error| anyhow!(error))?,
        }
        .map(|options| serde_json::from_str(&options))
        .transpose()
        .context("parsing language server workspace configuration")?;
        Ok(workspace_configuration)
    }

    async fn initialization_options_schema(
        store: &mut Store<HostState>,
        extension: &GuestExtension,
        language_server_id: &str,
        worktree_root: PathBuf,
    ) -> Result<Option<serde_json::Value>> {
        let worktree = store
            .data_mut()
            .table
            .push(WorktreeContext {
                root: worktree_root,
            })
            .map_err(|error| anyhow!(error))?;
        let schema = match extension {
            GuestExtension::V0_6(_) => None,
            GuestExtension::V0_8(guest) => guest
                .call_language_server_initialization_options_schema(
                    store,
                    language_server_id,
                    worktree,
                )
                .await
                .map_err(|error| anyhow!(error))?,
        };
        Ok(parse_extension_json_schema(schema))
    }

    async fn workspace_configuration_schema(
        store: &mut Store<HostState>,
        extension: &GuestExtension,
        language_server_id: &str,
        worktree_root: PathBuf,
    ) -> Result<Option<serde_json::Value>> {
        let worktree = store
            .data_mut()
            .table
            .push(WorktreeContext {
                root: worktree_root,
            })
            .map_err(|error| anyhow!(error))?;
        let schema = match extension {
            GuestExtension::V0_6(_) => None,
            GuestExtension::V0_8(guest) => guest
                .call_language_server_workspace_configuration_schema(
                    store,
                    language_server_id,
                    worktree,
                )
                .await
                .map_err(|error| anyhow!(error))?,
        };
        Ok(parse_extension_json_schema(schema))
    }

    async fn additional_workspace_configuration(
        store: &mut Store<HostState>,
        extension: &GuestExtension,
        language_server_id: &str,
        target_language_server_id: &str,
        worktree_root: PathBuf,
    ) -> Result<Option<serde_json::Value>> {
        let worktree = store
            .data_mut()
            .table
            .push(WorktreeContext {
                root: worktree_root,
            })
            .map_err(|error| anyhow!(error))?;
        let configuration = match extension {
            GuestExtension::V0_6(guest) => guest
                .call_language_server_additional_workspace_configuration(
                    store,
                    language_server_id,
                    target_language_server_id,
                    worktree,
                )
                .await
                .map_err(|error| anyhow!(error))?
                .map_err(|error| anyhow!(error))?,
            GuestExtension::V0_8(guest) => guest
                .call_language_server_additional_workspace_configuration(
                    store,
                    language_server_id,
                    target_language_server_id,
                    worktree,
                )
                .await
                .map_err(|error| anyhow!(error))?
                .map_err(|error| anyhow!(error))?,
        }
        .map(|configuration| serde_json::from_str(&configuration))
        .transpose()
        .context("parsing additional language server workspace configuration")?;
        Ok(configuration)
    }

    async fn additional_initialization_options(
        store: &mut Store<HostState>,
        extension: &GuestExtension,
        language_server_id: &str,
        target_language_server_id: &str,
        worktree_root: PathBuf,
    ) -> Result<Option<serde_json::Value>> {
        let worktree = store
            .data_mut()
            .table
            .push(WorktreeContext {
                root: worktree_root,
            })
            .map_err(|error| anyhow!(error))?;
        let options = match extension {
            GuestExtension::V0_6(guest) => guest
                .call_language_server_additional_initialization_options(
                    store,
                    language_server_id,
                    target_language_server_id,
                    worktree,
                )
                .await
                .map_err(|error| anyhow!(error))?
                .map_err(|error| anyhow!(error))?,
            GuestExtension::V0_8(guest) => guest
                .call_language_server_additional_initialization_options(
                    store,
                    language_server_id,
                    target_language_server_id,
                    worktree,
                )
                .await
                .map_err(|error| anyhow!(error))?
                .map_err(|error| anyhow!(error))?,
        }
        .map(|options| serde_json::from_str(&options))
        .transpose()
        .context("parsing additional language server initialization options")?;
        Ok(options)
    }

    async fn additional_workspace_configuration_for_extension(
        &self,
        extension: &LanguageServerExtension,
        target_language_server_id: &str,
        worktree_root: PathBuf,
    ) -> Result<Option<serde_json::Value>> {
        let mut session = self
            .extension_session(extension, worktree_root.clone())
            .await?;
        let (store, guest) = session
            .as_mut()
            .context("language extension session was not initialized")?;
        Self::additional_workspace_configuration(
            store,
            guest,
            &extension.server_id,
            target_language_server_id,
            worktree_root,
        )
        .await
    }

    async fn additional_initialization_options_for_extension(
        &self,
        extension: &LanguageServerExtension,
        target_language_server_id: &str,
        worktree_root: PathBuf,
    ) -> Result<Option<serde_json::Value>> {
        let mut session = self
            .extension_session(extension, worktree_root.clone())
            .await?;
        let (store, guest) = session
            .as_mut()
            .context("language extension session was not initialized")?;
        Self::additional_initialization_options(
            store,
            guest,
            &extension.server_id,
            target_language_server_id,
            worktree_root,
        )
        .await
    }

    pub async fn language_server_command_with_additional_configuration(
        &self,
        extension: &LanguageServerExtension,
        other_extensions: &[LanguageServerExtension],
        worktree: WorktreeContext,
    ) -> Result<LanguageServerCommand> {
        let mut command = self
            .resolve_language_server_command_for_extension(
                extension,
                worktree.clone(),
            )
            .await?;
        for other_extension in other_extensions
            .iter()
            .filter(|other| other.server_id != extension.server_id)
        {
            match self
                .additional_initialization_options_for_extension(
                    other_extension,
                    &extension.server_id,
                    worktree.root.clone(),
                )
                .await
            {
                Ok(Some(options)) => merge_zed_configuration(
                    &mut command.initialization_options,
                    options,
                ),
                Ok(None) => {}
                Err(error) => tracing::warn!(
                    source_extension = %other_extension.id,
                    source_server = %other_extension.server_id,
                    target_server = %extension.server_id,
                    %error,
                    "loading additional language-server initialization options"
                ),
            }
            match self
                .additional_workspace_configuration_for_extension(
                    other_extension,
                    &extension.server_id,
                    worktree.root.clone(),
                )
                .await
            {
                Ok(Some(configuration)) => merge_zed_configuration(
                    &mut command.workspace_configuration,
                    configuration,
                ),
                Ok(None) => {}
                Err(error) => tracing::warn!(
                    source_extension = %other_extension.id,
                    source_server = %other_extension.server_id,
                    target_server = %extension.server_id,
                    %error,
                    "loading additional language-server workspace configuration"
                ),
            }
        }
        Self::apply_extension_lsp_settings(
            &mut command,
            &extension.server_id,
            &worktree.root,
        )?;
        Ok(command)
    }

    async fn resolve_language_server_command_for_extension(
        &self,
        extension: &LanguageServerExtension,
        worktree: WorktreeContext,
    ) -> Result<LanguageServerCommand> {
        let mut session = self
            .extension_session(extension, worktree.root.clone())
            .await?;
        let (store, guest) = session
            .as_mut()
            .context("language extension session was not initialized")?;
        Self::resolve_language_server_command(
            store,
            guest,
            &extension.server_id,
            worktree,
        )
        .await
    }

    pub async fn language_server_command_for_extension(
        &self,
        extension: &LanguageServerExtension,
        worktree: WorktreeContext,
    ) -> Result<LanguageServerCommand> {
        let mut command = self
            .resolve_language_server_command_for_extension(
                extension,
                worktree.clone(),
            )
            .await?;
        Self::apply_extension_lsp_settings(
            &mut command,
            &extension.server_id,
            &worktree.root,
        )?;
        Ok(command)
    }

    fn apply_extension_lsp_settings(
        command: &mut LanguageServerCommand,
        language_server_id: &str,
        worktree_root: &Path,
    ) -> Result<()> {
        let user_home = ahead_core::directory::Directory::home_dir();
        let settings = lsp_settings_for_extension(
            worktree_root,
            user_home.as_deref(),
            Some(language_server_id),
        )?;
        let settings = serde_json::from_str(&settings)
            .context("parsing extension language server settings")?;
        apply_lsp_settings_to_command(command, settings, worktree_root)
    }

    pub async fn workspace_configuration_for_extension(
        &self,
        extension: &LanguageServerExtension,
        worktree: WorktreeContext,
    ) -> Result<Option<serde_json::Value>> {
        let mut session = self
            .extension_session(extension, worktree.root.clone())
            .await?;
        let (store, guest) = session
            .as_mut()
            .context("language extension session was not initialized")?;
        Self::workspace_configuration(
            store,
            guest,
            &extension.server_id,
            worktree.root,
        )
        .await
    }
}

fn merge_zed_configuration(
    target: &mut Option<serde_json::Value>,
    source: serde_json::Value,
) {
    fn union(source: serde_json::Value, target: &mut serde_json::Value) {
        match (source, target) {
            (
                serde_json::Value::Object(source),
                serde_json::Value::Object(target),
            ) => {
                for (key, value) in source {
                    if let Some(target) = target.get_mut(&key) {
                        union(value, target);
                    } else {
                        target.insert(key, value);
                    }
                }
            }
            (serde_json::Value::Array(source), serde_json::Value::Array(target)) => {
                for value in source {
                    if !target.contains(&value) {
                        target.push(value);
                    }
                }
            }
            (source, target) => *target = source,
        }
    }

    union(source, target.get_or_insert_with(|| serde_json::json!({})));
}

#[cfg(test)]
mod adapter_configuration_tests {
    use super::merge_zed_configuration;

    #[test]
    fn additional_adapter_configuration_uses_zed_union_semantics() {
        let mut target = Some(serde_json::json!({
            "nested": { "kept": true, "shared": "target" },
            "array": [1, 2],
        }));

        merge_zed_configuration(
            &mut target,
            serde_json::json!({
                "nested": { "added": true, "shared": "additional" },
                "array": [2, 3],
                "new": "value",
            }),
        );

        assert_eq!(
            target,
            Some(serde_json::json!({
                "nested": {
                    "kept": true,
                    "added": true,
                    "shared": "additional",
                },
                "array": [1, 2, 3],
                "new": "value",
            }))
        );

        let mut unconfigured = None;
        merge_zed_configuration(
            &mut unconfigured,
            serde_json::json!({ "additional": true }),
        );
        assert_eq!(
            unconfigured,
            Some(serde_json::json!({ "additional": true }))
        );
    }
}

pub fn checked_worktree_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = root.join(relative);
    let canonical_root = root.canonicalize().with_context(|| {
        format!("canonicalizing worktree root {}", root.display())
    })?;
    let canonical_parent = path
        .parent()
        .ok_or_else(|| anyhow!("worktree path has no parent"))?
        .canonicalize()
        .with_context(|| {
            format!("canonicalizing worktree path parent {}", path.display())
        })?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(anyhow!("extension path escapes the worktree"));
    }
    Ok(path)
}

pub(crate) fn checked_extension_path(
    root: &Path,
    relative: &str,
) -> Result<PathBuf> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(anyhow!("extension path escapes the extension directory"));
    }
    std::fs::create_dir_all(root)?;
    let path = root.join(relative_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        let canonical_root = root.canonicalize()?;
        let canonical_parent = parent.canonicalize()?;
        if !canonical_parent.starts_with(canonical_root) {
            return Err(anyhow!("extension path escapes the extension directory"));
        }
    }
    Ok(path)
}

fn checked_package_path(root: &Path, package: &str) -> Result<PathBuf> {
    let package_path = Path::new(package);
    anyhow::ensure!(
        !package_path.is_absolute()
            && !package_path.components().any(|component| {
                matches!(component, std::path::Component::ParentDir)
            }),
        "invalid NPM package name"
    );
    Ok(root.join("node_modules").join(package_path))
}

fn resolve_work_dir_command(work_dir: &Path, command: String) -> String {
    let path = Path::new(&command);
    if path.is_relative()
        && (path.components().count() > 1 || work_dir.join(path).is_file())
    {
        work_dir.join(path).to_string_lossy().into_owned()
    } else {
        command
    }
}

fn which_binary(binary: &str) -> Option<String> {
    let path = Path::new(binary);
    if path.file_name()? != path.as_os_str() {
        return None;
    }
    which::which(binary)
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs4::fs_std::FileExt;
    use std::fs;

    #[test]
    fn extension_language_server_schemas_accept_json_and_ignore_invalid_values() {
        assert_eq!(
            parse_extension_json_schema(Some(
                r#"{"type":"object","properties":{"enabled":{"type":"boolean"}}}"#
                    .into()
            )),
            Some(serde_json::json!({
                "type": "object",
                "properties": { "enabled": { "type": "boolean" } }
            }))
        );
        assert_eq!(parse_extension_json_schema(None), None);
        assert_eq!(parse_extension_json_schema(Some("not json".into())), None);
    }

    fn local_http_response(
        body: impl AsRef<[u8]>,
    ) -> Result<(url::Url, std::thread::JoinHandle<Result<()>>)> {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;
        use std::time::{Duration, Instant};

        let body = body.as_ref().to_vec();
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => return Err(error.into()),
                }
            };
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            let mut request = Vec::with_capacity(1024);
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let bytes_read = stream.read(&mut chunk)?;
                if bytes_read == 0 {
                    return Err(anyhow!(
                        "HTTP client closed before finishing headers"
                    ));
                }
                if request.len() + bytes_read > 16 * 1024 {
                    return Err(anyhow!(
                        "HTTP request headers exceed the test limit"
                    ));
                }
                request.extend_from_slice(&chunk[..bytes_read]);
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            )?;
            stream.write_all(&body)?;
            Ok(())
        });
        Ok((
            url::Url::parse(&format!("http://{address}/package"))?,
            server,
        ))
    }

    #[test]
    fn extension_http_callbacks_support_buffered_and_streamed_responses()
    -> Result<()> {
        let workspace = tempfile::tempdir()?;
        let mut host = HostState {
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().build(),
            worktree_root: workspace.path().to_path_buf(),
            work_dir: workspace.path().to_path_buf(),
            process_capabilities: Vec::new(),
            granted_capabilities: ExtensionCapabilityGrants::zed_defaults(),
            node_runtime: NodeRuntime::default(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        let (url, server) = local_http_response(b"buffered extension response")?;
        let buffered = runtime
            .block_on(<HostState as http_client::Host>::fetch(
                &mut host,
                http_client::HttpRequest {
                    method: http_client::HttpMethod::Get,
                    url: url.to_string(),
                    headers: Vec::new(),
                    body: None,
                    redirect_policy: http_client::RedirectPolicy::NoFollow,
                },
            ))
            .expect("buffered fetch should not trap")
            .map_err(|error| anyhow!(error))?;
        server
            .join()
            .map_err(|_| anyhow!("buffered HTTP fixture panicked"))??;
        assert_eq!(buffered.body, b"buffered extension response");
        assert!(
            buffered
                .headers
                .iter()
                .any(|(name, value)| name.eq_ignore_ascii_case("content-length")
                    && value == "27")
        );

        let (url, server) = local_http_response(b"streamed extension response")?;
        let stream = runtime
            .block_on(<HostState as http_client::Host>::fetch_stream(
                &mut host,
                http_client::HttpRequest {
                    method: http_client::HttpMethod::Get,
                    url: url.to_string(),
                    headers: Vec::new(),
                    body: None,
                    redirect_policy: http_client::RedirectPolicy::NoFollow,
                },
            ))
            .expect("stream fetch should not trap")
            .map_err(|error| anyhow!(error))?;
        let stream_rep = stream.rep();
        let mut streamed = Vec::new();
        loop {
            let chunk = runtime
                .block_on(<HostState as HostHttpResponseStream>::next_chunk(
                    &mut host,
                    Resource::new_borrow(stream_rep),
                ))
                .expect("stream read should not trap")
                .map_err(|error| anyhow!(error))?;
            let Some(chunk) = chunk else {
                break;
            };
            streamed.extend(chunk);
        }
        runtime
            .block_on(<HostState as HostHttpResponseStream>::drop(
                &mut host, stream,
            ))
            .expect("stream resource should be released");
        server
            .join()
            .map_err(|_| anyhow!("streamed HTTP fixture panicked"))??;
        assert_eq!(streamed, b"streamed extension response");
        Ok(())
    }

    #[test]
    fn worktree_binary_lookup_rejects_paths() {
        assert!(which_binary("").is_none());
        assert!(which_binary("../server").is_none());
        assert!(which_binary("/tmp/server").is_none());
        assert!(which_binary("./server").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn extension_command_preserves_output_and_times_out_a_hung_child() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let mut command = ProcessCommand::new("echo");
        command.arg("ready");
        let output = runtime
            .block_on(run_extension_command(
                command,
                std::time::Duration::from_secs(5),
            ))
            .expect("completed extension command");
        assert_eq!(output.stdout, b"ready\n");

        let mut command = ProcessCommand::new("sleep");
        command.arg("5");
        let started = std::time::Instant::now();
        let error = runtime
            .block_on(run_extension_command(
                command,
                std::time::Duration::from_millis(50),
            ))
            .expect_err("hung extension command must time out");
        assert!(error.contains("timed out after 50ms"));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));

        let process_dir = tempfile::tempdir().expect("process fixture");
        let process_id_path = process_dir.path().join("pid");
        let mut command = ProcessCommand::new("sh");
        command
            .arg("-c")
            .arg("echo $$ > \"$1\"; exec sleep 5")
            .arg("sh")
            .arg(&process_id_path);
        let error = runtime
            .block_on(run_extension_command(
                command,
                std::time::Duration::from_millis(200),
            ))
            .expect_err("hung shell must time out");
        assert!(error.contains("timed out after 200ms"));
        let process_id = fs::read_to_string(process_id_path)
            .expect("shell wrote its pid")
            .trim()
            .parse::<libc::pid_t>()
            .expect("valid process id");
        assert_eq!(unsafe { libc::kill(process_id, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_extension_command_kills_and_reaps_child() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let process_dir = tempfile::tempdir().expect("process fixture");
        let process_id_path = process_dir.path().join("pid");

        runtime.block_on(async {
            let mut command = ProcessCommand::new("sh");
            command
                .arg("-c")
                .arg("echo $$ > \"$1\"; exec sleep 5")
                .arg("sh")
                .arg(&process_id_path);
            let task = tokio::spawn(run_extension_command(
                command,
                std::time::Duration::from_secs(30),
            ));
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while !process_id_path.exists() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("child wrote its pid");
            let process_id = fs::read_to_string(&process_id_path)
                .expect("shell wrote its pid")
                .trim()
                .parse::<libc::pid_t>()
                .expect("valid process id");
            task.abort();
            assert!(task.await.expect_err("task was cancelled").is_cancelled());

            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if unsafe { libc::kill(process_id, 0) } == -1
                        && std::io::Error::last_os_error().raw_os_error()
                            == Some(libc::ESRCH)
                    {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("cancelled child was killed and reaped");
        });
    }

    #[test]
    fn extension_archive_digest_must_match_before_install() {
        let archive = b"extension archive";
        let digest = format!("{:x}", Sha256::digest(archive));
        assert!(verify_extension_archive_sha256(archive, &digest).is_ok());
        assert!(
            verify_extension_archive_sha256(archive, &digest.to_uppercase()).is_ok()
        );
        assert!(
            verify_extension_archive_sha256(archive, &"0".repeat(64))
                .expect_err("wrong archive")
                .to_string()
                .contains("mismatch")
        );
        assert!(
            verify_extension_archive_sha256(archive, "not a digest")
                .expect_err("invalid digest")
                .to_string()
                .contains("invalid")
        );
    }

    #[test]
    fn selected_extension_version_must_match_package_before_replacement() {
        let root = tempfile::tempdir().expect("extension root");
        let existing = root.path().join("rust");
        fs::create_dir(&existing).expect("installed extension");
        fs::write(existing.join("sentinel"), b"previous version")
            .expect("installed content");
        let archive = extension_archive("rust");
        let error =
            install_extension_package(&archive, root.path(), "rust", Some("9.9.9"))
                .expect_err(
                    "wrong package version must not replace installed extension",
                );
        assert!(error.to_string().contains("version does not match"));
        assert_eq!(
            fs::read(existing.join("sentinel")).expect("installed content remains"),
            b"previous version"
        );
    }

    #[test]
    fn extension_download_sources_are_limited_to_supported_registries() {
        let allowed_sources = [
            "https://api.zed.dev/extensions/html/0.3.2/download",
            "https://github.com/Kade-Powell/ahead/releases/download/extensions-v1/html-0.3.2.tar.gz",
        ];
        for raw_url in allowed_sources {
            let url = reqwest::Url::parse(raw_url).expect("source URL");
            assert!(
                allowed_extension_download_source_url(&url, "html", "0.3.2"),
                "{raw_url} should be an approved extension source"
            );
        }

        for url in [
            "http://example.com/extension.tar.gz",
            "file:///tmp/extension.tar.gz",
            "https://user:secret@example.com/extension.tar.gz",
            "https://registry.ahead.dev/extension.tar.gz",
            "https://api.zed.dev.evil.example/extensions/html/0.3.2/download",
            "https://api.zed.dev:8443/extensions/html/0.3.2/download",
            "https://api.zed.dev/extensions/html/0.3.3/download",
            "https://api.zed.dev/extensions/html/0.3.2/download/other",
            "https://api.zed.dev/extensions/html/0.3.2/download?source=unexpected",
            "https://github.com/attacker/ahead/releases/download/extensions-v1/html-0.3.2.tar.gz",
            "https://github.com/Kade-Powell/ahead/releases/download/extensions-v1/html-0.3.3.tar.gz",
            "http://user:secret@127.0.0.1:8080/extension.tar.gz",
        ] {
            assert!(
                !allowed_extension_download_source_url(
                    &reqwest::Url::parse(url).expect("test URL"),
                    "html",
                    "0.3.2",
                ),
                "{url} must not be an extension download source"
            );
            assert!(download_bytes(url, "html", "0.3.2").is_err());
        }
        assert!(allowed_extension_download_source_url(
            &reqwest::Url::parse("http://127.0.0.1:8080/extension.tar.gz")
                .expect("test loopback URL"),
            "html",
            "0.3.2",
        ));
    }

    #[test]
    fn extension_download_redirects_are_limited_to_registry_delivery_hosts() {
        for raw_url in [
            "https://api.zed.dev/extensions/html/0.3.2/download",
            "https://zed-extensions.nyc3.digitaloceanspaces.com/extensions/html/0.3.2.tar.gz?X-Amz-Signature=fixture",
            "https://release-assets.githubusercontent.com/github-production-release-asset/fixture?token=fixture",
            "https://objects.githubusercontent.com/github-production-release-asset/fixture?token=fixture",
            "https://github.com/Kade-Powell/ahead/releases/download/extensions-v1/html-0.3.2.tar.gz",
        ] {
            assert!(
                allowed_extension_download_redirect_url(
                    &reqwest::Url::parse(raw_url).expect("redirect URL"),
                    "html",
                    "0.3.2",
                ),
                "{raw_url} should be an approved registry redirect"
            );
        }

        for raw_url in [
            "https://example.com/extension.tar.gz",
            "https://zed-extensions.nyc3.digitaloceanspaces.com.evil.example/archive",
            "https://api.zed.dev.evil.example/extensions/html/0.3.2/download",
            "https://api.zed.dev/extensions/other/0.3.2/download",
            "https://zed-extensions.nyc3.digitaloceanspaces.com:444/archive",
            "https://user:secret@release-assets.githubusercontent.com/archive",
            "http://example.com/archive",
            "http://user:secret@127.0.0.1:8080/archive",
        ] {
            assert!(
                !allowed_extension_download_redirect_url(
                    &reqwest::Url::parse(raw_url).expect("redirect URL"),
                    "html",
                    "0.3.2",
                ),
                "{raw_url} must not be an extension redirect"
            );
        }
        assert!(allowed_extension_download_redirect_url(
            &reqwest::Url::parse("http://127.0.0.1:8080/extension.tar.gz")
                .expect("test loopback URL"),
            "html",
            "0.3.2",
        ));
    }
    #[test]
    fn extension_settings_reject_unsupported_zed_categories() {
        let workspace = tempfile::tempdir().expect("workspace");

        let error = settings_for_extension(
            workspace.path(),
            None,
            "context_servers",
            Some("example"),
        )
        .expect_err("unsupported Zed categories must not look like empty settings");

        assert!(error.to_string().contains("context_servers"));
        assert!(error.to_string().contains("supports `language` and `lsp`"));
    }

    #[test]
    fn lsp_settings_are_scoped_and_executable_overrides_stay_private() {
        let workspace = tempfile::tempdir().expect("workspace");
        let user_home = tempfile::tempdir().expect("user home");
        fs::create_dir(user_home.path().join(".ahead"))
            .expect("user settings directory");
        fs::write(
            user_home.path().join(".ahead/settings.toml"),
            "[lsp.vtsls.settings]\nmaxProblems = 5\nstyle = 'user'\n",
        )
        .expect("user settings");
        let ahead = workspace.path().join(".ahead");
        fs::create_dir(&ahead).expect("config directory");
        fs::write(
            ahead.join("config.toml"),
            "[ai]\nsecret = 'never-forward'\n[lsp.vtsls.settings]\nmaxProblems = 10\n",
        )
        .expect("shared config");
        fs::write(
            ahead.join("config.local.toml"),
            "[lsp.vtsls.binary]\npath = '/private/bin/vtsls'\narguments = ['--stdio']\n",
        )
        .expect("private config");
        fs::write(
            ahead.join("settings.toml"),
            "[lsp.vtsls.settings]\ndiagnostics = false\n",
        )
        .expect("workspace settings");

        let settings = lsp_settings_for_extension(
            workspace.path(),
            Some(user_home.path()),
            Some("vtsls"),
        )
        .expect("LSP settings");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&settings)
                .expect("JSON settings"),
            serde_json::json!({
                "settings": { "maxProblems": 10, "style": "user", "diagnostics": false },
                "binary": {
                    "path": "/private/bin/vtsls",
                    "arguments": ["--stdio"]
                }
            })
        );
        assert!(!settings.contains("never-forward"));
        assert_eq!(
            lsp_settings_for_extension(
                workspace.path(),
                Some(user_home.path()),
                Some("other")
            )
            .expect("unconfigured server"),
            "{}"
        );

        fs::write(
            ahead.join("config.toml"),
            "[lsp.vtsls.binary]\npath = '/tmp/untrusted-server'\n",
        )
        .expect("untrusted override");
        assert!(
            lsp_settings_for_extension(
                workspace.path(),
                Some(user_home.path()),
                Some("vtsls")
            )
            .expect_err("tracked config must not select an executable")
            .to_string()
            .contains("cannot set an executable LSP binary")
        );
        fs::write(ahead.join("config.toml"), "").expect("clear tracked override");
        fs::write(
            user_home.path().join(".ahead/settings.toml"),
            "[lsp.vtsls.binary]\npath = '/tmp/untrusted-server'\n",
        )
        .expect("user binary override");
        assert!(
            lsp_settings_for_extension(
                workspace.path(),
                Some(user_home.path()),
                Some("vtsls")
            )
            .expect_err("user settings must not select an executable")
            .to_string()
            .contains("cannot set an executable LSP binary")
        );
        fs::write(
            user_home.path().join(".ahead/settings.toml"),
            "[ai]\napi_key = NEVER_LOG_ME\n",
        )
        .expect("malformed user settings");
        let error = lsp_settings_for_extension(
            workspace.path(),
            Some(user_home.path()),
            Some("vtsls"),
        )
        .expect_err("malformed user settings must fail")
        .to_string();
        assert!(error.contains("~/.ahead/settings.toml"));
        assert!(!error.contains("NEVER_LOG_ME"));
    }

    #[test]
    fn language_settings_use_zed_defaults_and_merge_ahead_layers() {
        let workspace = tempfile::tempdir().expect("workspace");
        let user_home = tempfile::tempdir().expect("user home");
        fs::create_dir(user_home.path().join(".ahead"))
            .expect("user settings directory");
        fs::write(
            user_home.path().join(".ahead/settings.toml"),
            "[languages.Python]\ntab_size = 2\nhard_tabs = true\n",
        )
        .expect("user settings");
        let ahead = workspace.path().join(".ahead");
        fs::create_dir(&ahead).expect("workspace config directory");
        fs::write(
            ahead.join("config.toml"),
            "[languages.python]\npreferred_line_length = 100\n",
        )
        .expect("shared workspace settings");
        fs::write(
            ahead.join("settings.toml"),
            "[languages.PYTHON]\ntab_size = 8\nhard_tabs = false\n",
        )
        .expect("private workspace settings");

        let settings = language_settings_for_extension(
            workspace.path(),
            Some(user_home.path()),
            Some("Python"),
        )
        .expect("language settings");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&settings)
                .expect("JSON settings"),
            serde_json::json!({
                "tab_size": 8,
                "hard_tabs": false,
                "preferred_line_length": 100
            })
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                &language_settings_for_extension(workspace.path(), None, None,)
                    .expect("base language settings")
            )
            .expect("base JSON settings"),
            serde_json::json!({
                "tab_size": 4,
                "hard_tabs": false,
                "preferred_line_length": 80
            })
        );
    }

    #[test]
    fn language_settings_reject_invalid_zed_values() {
        let workspace = tempfile::tempdir().expect("workspace");
        fs::create_dir(workspace.path().join(".ahead"))
            .expect("workspace config directory");
        fs::write(
            workspace.path().join(".ahead/config.toml"),
            "[languages.Rust]\ntab_size = 0\n",
        )
        .expect("workspace settings");

        let error =
            language_settings_for_extension(workspace.path(), None, Some("Rust"))
                .expect_err("zero-width tab size is invalid")
                .to_string();
        assert!(error.contains("tab_size has an invalid value"));

        fs::write(
            workspace.path().join(".ahead/config.toml"),
            "[languages.Rust]\ntab_size = 2\n[languages.rust]\nhard_tabs = true\n",
        )
        .expect("duplicate language settings");
        let error =
            language_settings_for_extension(workspace.path(), None, Some("Rust"))
                .expect_err("case-insensitive duplicate language tables are invalid")
                .to_string();
        assert!(error.contains("defines language `Rust` more than once"));
    }

    #[test]
    fn lsp_settings_override_extension_command_and_merge_initialization_options() {
        let workspace = tempfile::tempdir().expect("workspace");
        let mut command = LanguageServerCommand {
            command: "extension-server".into(),
            args: vec!["--extension-argument".into()],
            env: vec![("EXTENSION_VALUE".into(), "default".into())],
            initialization_options: Some(serde_json::json!({
                "retained": true,
                "nested": { "extension": true, "shared": "extension" },
                "array": ["extension"]
            })),
            initialization_options_schema: None,
            workspace_configuration: None,
            workspace_configuration_schema: None,
        };

        merge_zed_configuration(
            &mut command.initialization_options,
            serde_json::json!({
                "retained": false,
                "nested": { "shared": "additional", "from_other": true },
                "array": ["additional"],
            }),
        );
        apply_lsp_settings_to_command(
            &mut command,
            serde_json::json!({
                "initialization_options": {
                    "nested": { "shared": "user", "added": true },
                    "array": ["user"]
                },
                "binary": {
                    "path": "tools/custom-lsp",
                    "arguments": ["--stdio"],
                    "env": {
                        "EXTENSION_VALUE": "override",
                        "USER_VALUE": "private"
                    }
                }
            }),
            workspace.path(),
        )
        .expect("apply user LSP settings");

        assert_eq!(
            command.command,
            workspace.path().join("tools/custom-lsp").to_string_lossy()
        );
        assert_eq!(command.args, ["--stdio"]);
        assert_eq!(
            command.initialization_options,
            Some(serde_json::json!({
                "retained": false,
                "nested": {
                    "extension": true,
                    "shared": "user",
                    "added": true,
                    "from_other": true
                },
                "array": ["user"]
            }))
        );
        assert_eq!(
            command.env.iter().cloned().collect::<HashMap<_, _>>(),
            HashMap::from([
                ("EXTENSION_VALUE".to_string(), "override".to_string()),
                ("USER_VALUE".to_string(), "private".to_string()),
            ])
        );

        let error = apply_lsp_settings_to_command(
            &mut command,
            serde_json::json!({ "binary": { "arguments": ["--stdio", 1] } }),
            workspace.path(),
        )
        .expect_err("binary arguments must be strings");
        assert!(error.to_string().contains("arguments[1] must be a string"));
    }

    #[test]
    #[ignore = "set AHEAD_ZED_EXTENSION_SOURCE to a real installed extension directory"]
    fn installed_zed_extension_reads_changed_workspace_settings() -> Result<()> {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let source = PathBuf::from(std::env::var("AHEAD_ZED_EXTENSION_SOURCE")?);
        let extension_id = source
            .file_name()
            .and_then(|name| name.to_str())
            .context("extension source has no directory name")?;
        let server_id = std::env::var("AHEAD_ZED_EXTENSION_SERVER_ID")
            .unwrap_or_else(|_| "buf".to_string());
        let manifest = toml::from_str::<ExtensionManifest>(&fs::read_to_string(
            source.join("extension.toml"),
        )?)?;
        let expected_version = manifest
            .version
            .clone()
            .context("extension source has no package version")?;
        anyhow::ensure!(
            server_id
                .chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || character == '-'),
            "smoke-test server id must be a bare TOML key"
        );
        let project = tempfile::tempdir()?;
        let extension_root = project.path().join("extensions");
        let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        let mut package_files =
            vec!["extension.toml".to_string(), "extension.wasm".to_string()];
        for grammar in manifest.grammars.keys() {
            anyhow::ensure!(
                valid_grammar_name(grammar),
                "invalid extension grammar name {grammar}"
            );
            package_files.push(format!("grammars/{grammar}.wasm"));
        }
        for name in package_files {
            let bytes = fs::read(source.join(&name))?;
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, &name, bytes.as_slice())?;
        }
        let archive = archive.into_inner()?.finish()?;
        let archive_sha256 = format!("{:x}", Sha256::digest(&archive));
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let download_path = format!("/{extension_id}/download");
        let expected_download_path = download_path.clone();
        let server = std::thread::spawn(move || -> Result<()> {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(10);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => return Err(error.into()),
                }
            };
            stream.set_nonblocking(false)?;
            stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer)?;
                anyhow::ensure!(read > 0, "extension request ended before headers");
                request.extend_from_slice(&buffer[..read]);
            }
            let request = String::from_utf8(request)?;
            anyhow::ensure!(
                request.starts_with(&format!("GET {expected_download_path} ")),
                "extension download used the wrong path"
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                archive.len()
            )?;
            stream.write_all(&archive)?;
            Ok(())
        });
        let installed = install_extension_from_url(
            &format!("http://{address}{download_path}"),
            &extension_root,
            extension_id,
            &expected_version,
            Some(&archive_sha256),
        );
        server
            .join()
            .map_err(|_| anyhow!("extension download server panicked"))??;
        installed?;
        let extension = discover_language_server_extensions(&extension_root)?
            .extensions
            .into_iter()
            .find(|extension| extension.server_id == server_id)
            .context("installed extension does not declare the requested server")?;
        let workspace = project.path().join("workspace");
        fs::create_dir_all(workspace.join(".ahead"))?;
        let host = ExtensionHost::new()?;
        if server_id == "buf" {
            let binary = std::env::current_exe()?.to_string_lossy().into_owned();
            fs::write(
                workspace.join(".ahead/config.local.toml"),
                format!(
                    "[lsp.buf.binary]\npath = {}\narguments = ['lsp', 'serve']\n",
                    toml::Value::String(binary.clone())
                ),
            )?;
            fs::write(
                workspace.join(".ahead/config.toml"),
                "[lsp.buf.settings]\naheadSmokeMarker = 'first'\n[lsp.buf.initialization_options]\naheadInitMarker = 'offline'\n",
            )?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let command =
                runtime.block_on(host.language_server_command_for_extension(
                    &extension,
                    WorktreeContext {
                        root: workspace.clone(),
                    },
                ))?;
            assert_eq!(command.command, binary);
            assert_eq!(command.args, ["lsp", "serve"]);
            assert_eq!(
                command.initialization_options,
                Some(serde_json::json!({ "aheadInitMarker": "offline" }))
            );
            assert_eq!(
                command.workspace_configuration,
                Some(serde_json::json!({ "aheadSmokeMarker": "first" }))
            );
            assert_eq!(host.instantiations.load(Ordering::SeqCst), 1);
        }
        for value in ["first", "second"] {
            fs::write(
                workspace.join(".ahead/config.toml"),
                format!(
                    "[lsp.{server_id}.settings]\naheadSmokeMarker = '{value}'\n"
                ),
            )?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let configuration =
                runtime.block_on(host.workspace_configuration_for_extension(
                    &extension,
                    WorktreeContext {
                        root: workspace.clone(),
                    },
                ))?;
            assert_eq!(
                configuration,
                Some(serde_json::json!({ "aheadSmokeMarker": value }))
            );
            assert_eq!(host.instantiations.load(Ordering::SeqCst), 1);
        }
        Ok(())
    }

    #[test]
    fn host_engine_constructs() {
        let first = ExtensionHost::new().expect("first host engine");
        let second = ExtensionHost::new().expect("second host engine");
        assert!(Engine::same(first.engine(), second.engine()));
    }

    #[test]
    fn extension_invalidation_preserves_other_cached_guests() {
        let host = ExtensionHost::new().expect("extension host");
        {
            let mut sessions = host.sessions.lock().expect("session cache");
            for extension_id in ["html", "rust"] {
                sessions.insert(
                    (
                        PathBuf::from(format!("extensions/{extension_id}")),
                        PathBuf::from("workspace"),
                    ),
                    Arc::new(tokio::sync::Mutex::new(None)),
                );
            }
        }

        host.invalidate_extension_sessions(Path::new("extensions/html"))
            .expect("invalidate updated extension");

        let sessions = host.sessions.lock().expect("session cache");
        assert_eq!(sessions.len(), 1);
        assert!(sessions.contains_key(&(
            PathBuf::from("extensions/rust"),
            PathBuf::from("workspace"),
        )));
    }

    #[test]
    fn discovers_language_servers_from_extension_manifests() {
        let root = tempfile::tempdir().expect("extension root");
        let extension = root.path().join("rust");
        fs::create_dir_all(&extension).expect("extension directory");
        fs::write(
            extension.join("extension.toml"),
            "id = 'rust'\n[language_servers.rust-analyzer]\nlanguages = ['Rust']\nopt_in_languages = ['Rust']\n[language_servers.rust-analyzer.language_ids]\nRust = 'rust-lang'\n",
        )
        .expect("manifest");
        fs::write(
            extension.join("extension.wasm"),
            extension_component("0.7.0"),
        )
        .expect("component placeholder");
        let unsupported = root.path().join("unsupported");
        fs::create_dir_all(&unsupported).expect("unsupported extension directory");
        fs::write(
            unsupported.join("extension.toml"),
            "id = 'unsupported'\n[language_servers.unsupported]\nlanguages = ['Rust']\n",
        )
        .expect("unsupported manifest");
        fs::write(
            unsupported.join("extension.wasm"),
            extension_component("0.9.0"),
        )
        .expect("unsupported component placeholder");

        let discovered = discover_language_server_extensions(root.path())
            .expect("discover extensions");
        assert_eq!(discovered.errors.len(), 1);
        assert_eq!(discovered.errors[0].0, unsupported);
        assert!(
            format!("{:#}", discovered.errors[0].1)
                .contains("unsupported Zed extension API")
        );
        assert_eq!(discovered.extensions.len(), 1);
        let extension = &discovered.extensions[0];
        assert!(extension.supports_language("rust"));
        assert_eq!(extension.server_id, "rust-analyzer");
        assert_eq!(extension.api_version, ExtensionApiVersion::V0_6);
        assert_eq!(extension.protocol_language_id("Rust"), "rust-lang");
        assert!(extension.supports_language("rust-lang"));
        assert_eq!(extension.protocol_language_id("rust-lang"), "rust-lang");
        assert_eq!(extension.language_ids(), ["Rust", "rust-lang"]);
        assert!(extension.is_opt_in_for("rust"));
        assert!(extension.is_opt_in_for("rust-lang"));
        assert!(!extension.is_opt_in_for("TypeScript"));
    }

    #[test]
    fn discovery_recovers_install_backup_and_ignores_staging_directory() {
        let root = tempfile::tempdir().expect("extension root");
        for name in [".html.previous", ".tmp-stage"] {
            let directory = root.path().join(name);
            fs::create_dir_all(directory.join("languages/html"))
                .expect("extension language directory");
            fs::write(
                directory.join("extension.toml"),
                "id = 'html'\nlanguages = ['languages/html']\n[language_servers.html]\nlanguages = ['HTML']\n",
            )
            .expect("extension manifest");
            fs::write(
                directory.join("extension.wasm"),
                extension_component("0.7.0"),
            )
            .expect("extension component placeholder");
            fs::write(
                directory.join("languages/html/config.toml"),
                "name = 'HTML'\npath_suffixes = ['html']\n",
            )
            .expect("extension language config");
        }
        let discovered = discover_language_server_extensions(root.path())
            .expect("discover extensions");
        assert!(discovered.errors.is_empty());
        assert_eq!(discovered.extensions.len(), 1);
        assert_eq!(discovered.extensions[0].id, "html");
        assert!(root.path().join("html").is_dir());
        assert!(!root.path().join(".html.previous").exists());
        assert_eq!(
            language_id_for_path(root.path(), Path::new("index.html"), None)
                .expect("language lookup"),
            Some("HTML".to_string())
        );
        let cached_sources =
            language_sources_for_root(root.path()).expect("language sources");
        let reused_sources =
            language_sources_for_root(root.path()).expect("cached language sources");
        assert!(Arc::ptr_eq(&cached_sources, &reused_sources));
        let install_lock = lock_extension_installs(root.path())
            .expect("hold an ongoing install lock");
        let (sender, receiver) = std::sync::mpsc::channel();
        let extension_root = root.path().to_path_buf();
        let lookup = std::thread::spawn(move || {
            sender.send(language_id_for_path(
                &extension_root,
                Path::new("index.html"),
                None,
            ))
        });
        let language = receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("language lookup must not wait for package unpacking")
            .expect("language lookup");
        assert_eq!(language.as_deref(), Some("HTML"));
        drop(install_lock);
        lookup
            .join()
            .expect("language lookup thread")
            .expect("send result");

        let renamed = root.path().join("renamed");
        fs::rename(root.path().join("html"), &renamed)
            .expect("rename extension directory");
        let discovered = discover_language_server_extensions(root.path())
            .expect("discover extensions after rename");
        assert!(discovered.extensions.is_empty());
        assert_eq!(discovered.errors.len(), 1);
        let refreshed_sources = language_sources_for_root(root.path())
            .expect("language sources after extension discovery");
        assert!(!Arc::ptr_eq(&cached_sources, &refreshed_sources));
        assert!(
            language_id_for_path(root.path(), Path::new("index.html"), None)
                .is_err()
        );
    }

    #[test]
    fn discovers_language_ids_from_extension_language_configs() {
        let root = tempfile::tempdir().expect("extension root");
        let extension = root.path().join("gleam");
        fs::create_dir_all(extension.join("languages/gleam"))
            .expect("language directory");
        fs::write(
            extension.join("extension.toml"),
            "id = 'gleam'\nlanguages = ['languages/gleam']\n",
        )
        .expect("manifest");
        fs::write(
            extension.join("languages/gleam/config.toml"),
            "name = 'Gleam'\npath_suffixes = ['gleam']\nline_comments = ['// ', '/// ']\nblock_comment = { start = '/*', prefix = '* ', end = '*/', tab_size = 1 }\n",
        )
        .expect("language config");
        let broken = root.path().join("broken");
        fs::create_dir_all(&broken).expect("broken extension directory");
        fs::write(broken.join("extension.toml"), "id = [").expect("broken manifest");

        assert_eq!(
            language_id_for_path(root.path(), Path::new("src/main.gleam"), None,)
                .expect("language lookup"),
            Some("Gleam".to_string())
        );
        let syntax =
            extension_syntax_for_path(root.path(), Path::new("src/main.gleam"), "")
                .expect("language config lookup")
                .expect("matching extension");
        assert_eq!(syntax.language_name, "Gleam");
        assert_eq!(syntax.captures, Vec::new());
        assert_eq!(
            syntax.comments,
            LanguageComments {
                line_comment: Some("// ".into()),
                block_comment: Some(("/*".into(), "*/".into())),
            }
        );
        assert!(
            language_id_for_path(root.path(), Path::new("src/main.unknown"), None)
                .is_err()
        );
    }

    #[test]
    fn parses_zed_legacy_block_comment_pairs() {
        let config: LanguageConfig =
            toml::from_str("name = 'Legacy'\nblock_comment = ['<!--', '-->']\n")
                .expect("Zed legacy block-comment syntax");
        assert_eq!(
            config
                .block_comment
                .map(BlockCommentDefinition::into_delimiters),
            Some(("<!--".into(), "-->".into()))
        );
    }

    #[test]
    fn installs_grammar_only_extension_and_loads_zed_language_assets() {
        let root = tempfile::tempdir().expect("extension root");
        let archive = grammar_extension_archive("gleam", "Gleam");
        install_extension_package(&archive, root.path(), "gleam", Some("1.0.0"))
            .expect("install grammar-only extension");

        let syntax =
            language_syntax_for_path(root.path(), Path::new("src/main.gleam"), None)
                .expect("load extension syntax")
                .expect("language syntax");
        assert_eq!(syntax.language_name, "Gleam");
        assert_eq!(syntax.grammar_name, "gleam");
        assert_eq!(syntax.wasm, b"tree-sitter grammar wasm");
        assert_eq!(syntax.highlights, "(identifier) @variable\n");
        assert_eq!(syntax.injections, "");
    }

    #[test]
    #[ignore = "requires an installed HTML Zed extension; set AHEAD_HTML_EXTENSION_ROOT"]
    fn installed_html_extension_produces_tree_sitter_capture_ranges() {
        let extensions_root = std::env::var_os("AHEAD_HTML_EXTENSION_ROOT")
            .map(PathBuf::from)
            .expect("AHEAD_HTML_EXTENSION_ROOT must point to the plugins directory");
        let source = "<div title='hello'>Hi</div>";
        let captures = syntax_captures_for_path(
            &extensions_root,
            Path::new("example.html"),
            source,
        )
        .expect("parse installed extension highlights")
        .expect("HTML extension matches the file");

        assert!(captures.iter().any(|capture| {
            capture.scope == "tag"
                && source.get(capture.start..capture.end) == Some("div")
        }));
        assert!(captures.iter().any(|capture| {
            capture.scope == "attribute"
                && source.get(capture.start..capture.end) == Some("title")
        }));
    }

    #[test]
    #[ignore = "requires an installed HTML Zed extension; set AHEAD_HTML_EXTENSION_ROOT"]
    fn installed_html_extension_applies_combined_language_injections() {
        let installed_root = std::env::var_os("AHEAD_HTML_EXTENSION_ROOT")
            .map(PathBuf::from)
            .expect("AHEAD_HTML_EXTENSION_ROOT must point to the plugins directory");
        let installed_html = installed_root.join("html");
        let root = tempfile::tempdir().expect("extension root");
        let html = root.path().join("html");
        for relative_path in [
            "extension.toml",
            "grammars/html.wasm",
            "languages/html/config.toml",
            "languages/html/highlights.scm",
        ] {
            let source = installed_html.join(relative_path);
            let destination = html.join(relative_path);
            fs::create_dir_all(
                destination.parent().expect("extension asset parent"),
            )
            .expect("create extension asset directory");
            fs::copy(source, destination).expect("copy installed HTML asset");
        }
        fs::write(
            html.join("languages/html/injections.scm"),
            "(script_element (raw_text) @injection.content\n  (#set! injection.language \"html\")\n  (#set! injection.combined))\n",
        )
        .expect("write combined injection query");

        let source = "<script><div title='one'>One</div></script><script><span title='two'>Two</span></script>";
        let captures =
            syntax_captures_for_path(root.path(), Path::new("example.html"), source)
                .expect("parse injected HTML")
                .expect("HTML extension matches the file");

        for name in ["div", "span"] {
            assert!(captures.iter().any(|capture| {
                capture.scope == "tag"
                    && source.get(capture.start..capture.end) == Some(name)
            }));
        }
        assert_eq!(
            captures
                .iter()
                .filter(|capture| {
                    capture.scope == "attribute"
                        && source.get(capture.start..capture.end) == Some("title")
                })
                .count(),
            2
        );
    }

    #[cfg(unix)]
    #[test]
    fn extension_syntax_assets_reject_symlinks() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("extension root");
        let extension = root.path().join("gleam");
        fs::create_dir_all(extension.join("grammars")).expect("grammar directory");
        let outside = tempfile::tempdir().expect("outside directory");
        fs::write(outside.path().join("gleam.wasm"), b"outside")
            .expect("outside grammar");
        symlink(
            outside.path().join("gleam.wasm"),
            extension.join("grammars/gleam.wasm"),
        )
        .expect("symlink grammar");

        let error = read_extension_asset(
            &extension,
            Path::new("grammars/gleam.wasm"),
            MAX_EXTENSION_GRAMMAR_BYTES,
        )
        .expect_err("symlinked extension assets must be rejected");
        assert!(error.to_string().contains("symlink"));
    }

    #[cfg(unix)]
    #[test]
    fn extension_manifest_and_language_config_reject_symlinks() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("extension root");
        let extension = root.path().join("gleam");
        let language_directory = extension.join("languages/gleam");
        fs::create_dir_all(&language_directory).expect("language directory");
        let outside = tempfile::tempdir().expect("outside directory");
        let outside_manifest = outside.path().join("extension.toml");
        fs::write(
            &outside_manifest,
            "id = 'gleam'\nlanguages = ['languages/gleam']\n",
        )
        .expect("outside manifest");
        symlink(&outside_manifest, extension.join("extension.toml"))
            .expect("symlink manifest");

        let manifest_error =
            language_id_for_path(root.path(), Path::new("src/main.gleam"), None)
                .expect_err("symlinked extension manifest must be rejected");
        assert!(manifest_error.to_string().contains("symlink"));

        fs::remove_file(extension.join("extension.toml"))
            .expect("remove manifest symlink");
        fs::write(
            extension.join("extension.toml"),
            "id = 'gleam'\nlanguages = ['languages/gleam']\n",
        )
        .expect("extension manifest");
        let outside_config = outside.path().join("config.toml");
        fs::write(
            &outside_config,
            "name = 'Gleam'\npath_suffixes = ['gleam']\n",
        )
        .expect("outside language config");
        symlink(&outside_config, language_directory.join("config.toml"))
            .expect("symlink language config");

        let config_error =
            language_id_for_path(root.path(), Path::new("src/main.gleam"), None)
                .expect_err("symlinked language config must be rejected");
        assert!(config_error.to_string().contains("symlink"));
    }

    #[test]
    fn process_capabilities_match_zed_wildcards() {
        let capability = ProcessCapability {
            command: "cargo".to_string(),
            args: vec!["test".to_string(), "**".to_string()],
        };
        assert!(capability.allows("cargo", &["test".to_string()]));
        assert!(
            capability
                .allows("cargo", &["test".to_string(), "--workspace".to_string()])
        );
        assert!(!capability.allows("cargo", &["build".to_string()]));
    }

    #[test]
    fn extension_capability_defaults_match_zed() {
        let grants = ExtensionCapabilityGrants::from_user_settings(None)
            .expect("Zed default grants");
        let requested = [ProcessCapability {
            command: "git".to_string(),
            args: vec!["status".to_string()],
        }];
        let args = vec!["status".to_string()];
        assert!(grants.allows_process_exec(&requested, "git", &args));
        assert!(grants.allows_download(
            &url::Url::parse("https://example.com/a").expect("URL")
        ));
        assert!(grants.allows_npm_install("@zed/example"));
    }

    #[test]
    fn extension_capability_settings_are_user_grants_not_manifest_grants() {
        let settings = r#"
granted_extension_capabilities = [
    { kind = "process:exec", command = "git", args = ["status", "**"] },
    { kind = "download_file", host = "github.com", path = ["zed-industries", "**"] },
    { kind = "npm:install", package = "@zed-industries/vscode-langservers-extracted" },
]
"#;
        let grants = ExtensionCapabilityGrants::from_user_settings(Some(settings))
            .expect("custom capability grants");
        let requested = [ProcessCapability {
            command: "git".to_string(),
            args: vec!["status".to_string(), "**".to_string()],
        }];
        assert!(grants.allows_process_exec(
            &requested,
            "git",
            &["status".to_string(), "--short".to_string()]
        ));
        assert!(!grants.allows_process_exec(
            &requested,
            "git",
            &["push".to_string()]
        ));
        assert!(
            grants.allows_download(
                &url::Url::parse(
                    "https://github.com/zed-industries/zed/archive/main.zip"
                )
                .expect("URL")
            )
        );
        assert!(
            !grants.allows_download(
                &url::Url::parse("https://example.com/zed/archive/main.zip")
                    .expect("URL")
            )
        );
        assert!(
            grants
                .allows_npm_install("@zed-industries/vscode-langservers-extracted")
        );
        assert!(!grants.allows_npm_install("unapproved-package"));

        let no_grants = ExtensionCapabilityGrants::from_user_settings(Some(
            "granted_extension_capabilities = []",
        ))
        .expect("empty capability grants");
        assert!(!no_grants.allows_process_exec(
            &requested,
            "git",
            &["status".to_string()]
        ));
        assert!(
            !no_grants
                .allows_npm_install("@zed-industries/vscode-langservers-extracted")
        );
        assert!(
            ExtensionCapabilityGrants::from_user_settings(Some(
                "granted_extension_capabilities = [{ kind = 'unknown' }]",
            ))
            .is_err()
        );
    }

    #[test]
    fn downloaded_archives_are_extracted_under_the_requested_path() -> Result<()> {
        use std::io::Write as _;

        let workspace = tempfile::tempdir()?;
        let mut host = HostState {
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().build(),
            worktree_root: workspace.path().to_path_buf(),
            work_dir: workspace.path().to_path_buf(),
            process_capabilities: Vec::new(),
            granted_capabilities: ExtensionCapabilityGrants::zed_defaults(),
            node_runtime: NodeRuntime::default(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(11);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "tombi-cli/bin/tombi", &b"test binary"[..])?;
        let tar_bytes = tar.into_inner()?.finish()?;

        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file(
            "tombi-cli/bin/tombi",
            zip::write::SimpleFileOptions::default().unix_permissions(0o755),
        )?;
        zip.write_all(b"test binary")?;
        let zip_bytes = zip.finish()?.into_inner();

        for (archive, file_type, requested_path) in [
            (tar_bytes, DownloadedFileType::GzipTar, "tar-install"),
            (zip_bytes, DownloadedFileType::Zip, "zip-install"),
        ] {
            let (url, server) = local_http_response(archive)?;
            let host_name = url.host_str().context("mock URL has no host")?;
            host.granted_capabilities =
                ExtensionCapabilityGrants::from_user_settings(Some(&format!(
                    "granted_extension_capabilities = [{{ kind = 'download_file', host = '{host_name}', path = ['package'] }}]"
                )))?;
            runtime
                .block_on(<HostState as ExtensionImports>::download_file(
                    &mut host,
                    url.to_string(),
                    requested_path.into(),
                    file_type,
                ))
                .expect("archive download callback should not trap")
                .map_err(|error| anyhow!(error))?;
            server
                .join()
                .map_err(|_| anyhow!("local archive server panicked"))??;

            assert_eq!(
                fs::read(
                    workspace
                        .path()
                        .join(requested_path)
                        .join("tombi-cli/bin/tombi")
                )?,
                b"test binary"
            );
            assert!(!workspace.path().join("tombi-cli/bin/tombi").exists());
        }

        Ok(())
    }

    #[test]
    fn extension_capability_settings_load_from_the_user_ahead_directory() {
        let home = tempfile::tempdir().expect("AHEAD user home");
        let ahead = home.path().join(".ahead");
        fs::create_dir(&ahead).expect("AHEAD settings directory");
        fs::write(
            ahead.join("settings.toml"),
            "granted_extension_capabilities = []\n",
        )
        .expect("write user settings");

        let grants = ExtensionCapabilityGrants::load_for_home(home.path())
            .expect("load user capability grants");
        assert!(!grants.allows_npm_install("@zed/example"));
    }

    #[test]
    fn latest_npm_package_version_uses_async_http() -> Result<()> {
        let (url, server) =
            local_http_response(br#"{"dist-tags":{"latest":"1.2.3"}}"#)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let version = runtime.block_on(latest_npm_package_version(url.as_str()))?;
        server
            .join()
            .map_err(|_| anyhow!("local npm response server panicked"))??;
        assert_eq!(version, "1.2.3");
        Ok(())
    }

    #[test]
    fn extension_process_callback_requires_manifest_and_user_grants() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        let mut host = HostState {
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().build(),
            worktree_root: workspace.path().to_path_buf(),
            work_dir: workspace.path().to_path_buf(),
            process_capabilities: vec![ProcessCapability {
                command: "echo".into(),
                args: vec!["allowed".into()],
            }],
            granted_capabilities: ExtensionCapabilityGrants::from_user_settings(
                Some("granted_extension_capabilities = []"),
            )?,
            node_runtime: NodeRuntime::default(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        let denied_by_user = runtime
            .block_on(<HostState as process::Host>::run_command(
                &mut host,
                process::Command {
                    command: "echo".into(),
                    args: vec!["allowed".into()],
                    env: Vec::new(),
                },
            ))
            .expect("process host callback should not trap");
        assert!(matches!(
            denied_by_user,
            Err(error) if error.contains("user-level granted_extension_capabilities")
        ));

        host.granted_capabilities = ExtensionCapabilityGrants::from_user_settings(
            Some(
                "granted_extension_capabilities = [{ kind = 'process:exec', command = 'echo', args = ['allowed'] }]",
            ),
        )?;
        host.process_capabilities.clear();
        let denied_by_manifest = runtime
            .block_on(<HostState as process::Host>::run_command(
                &mut host,
                process::Command {
                    command: "echo".into(),
                    args: vec!["allowed".into()],
                    env: Vec::new(),
                },
            ))
            .expect("process host callback should not trap");
        assert!(matches!(
            denied_by_manifest,
            Err(error) if error.contains("extension manifest declaration")
        ));
        Ok(())
    }

    #[test]
    fn extension_guest_capabilities_require_user_grants_and_download_async()
    -> Result<()> {
        let (url, server) = local_http_response(b"extension asset")?;
        let workspace = tempfile::tempdir()?;
        let denied_grants = ExtensionCapabilityGrants::from_user_settings(Some(
            "granted_extension_capabilities = []",
        ))?;
        let mut host = HostState {
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().build(),
            worktree_root: workspace.path().to_path_buf(),
            work_dir: workspace.path().to_path_buf(),
            process_capabilities: Vec::new(),
            granted_capabilities: denied_grants,
            node_runtime: NodeRuntime::default(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        let npm_result = runtime
            .block_on(<HostState as nodejs::Host>::npm_install_package(
                &mut host,
                "@zed/example".into(),
                "1.0.0".into(),
            ))
            .expect("npm host callback should not trap");
        assert!(npm_result.unwrap_err().contains("not allowed"));

        let denied_download = runtime
            .block_on(<HostState as ExtensionImports>::download_file(
                &mut host,
                url.to_string(),
                "downloaded.bin".into(),
                DownloadedFileType::Uncompressed,
            ))
            .expect("download host callback should not trap");
        assert!(denied_download.unwrap_err().contains("not allowed"));

        let host_name = url.host_str().context("mock URL has no host")?;
        let path = url
            .path_segments()
            .and_then(|mut segments| segments.next())
            .context("mock URL has no path")?;
        let settings = format!(
            "granted_extension_capabilities = [{{ kind = 'download_file', host = '{host_name}', path = ['{path}'] }}]"
        );
        host.granted_capabilities =
            ExtensionCapabilityGrants::from_user_settings(Some(&settings))?;
        let download = runtime
            .block_on(<HostState as ExtensionImports>::download_file(
                &mut host,
                url.to_string(),
                "downloaded.bin".into(),
                DownloadedFileType::Uncompressed,
            ))
            .expect("granted download callback should not trap");
        download.map_err(|error| anyhow!(error))?;
        server
            .join()
            .map_err(|_| anyhow!("local download server panicked"))??;
        assert_eq!(
            fs::read(workspace.path().join("downloaded.bin"))?,
            b"extension asset"
        );
        Ok(())
    }

    #[test]
    fn extension_paths_cannot_escape_the_extension_directory() {
        let root = tempfile::tempdir().expect("extension root");
        assert!(checked_extension_path(root.path(), "../outside").is_err());
    }

    #[test]
    fn rejects_an_extension_without_language_exports_before_replacement() {
        let root = tempfile::tempdir().expect("extension root");
        let archive = extension_archive("rust");
        let error = install_extension_package(&archive, root.path(), "rust", None)
            .expect_err("component without extension exports must not install");
        assert!(error.to_string().contains("missing export"));
        assert!(!root.path().join("rust").exists());
    }

    #[test]
    fn extension_install_lock_serializes_independent_handles() {
        let root = tempfile::tempdir().expect("extension root");
        let first =
            lock_extension_installs(root.path()).expect("first install lock");
        let second = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.path().join(".ahead-extension-install.lock"))
            .expect("second install lock handle");
        assert!(!second.try_lock_exclusive().expect("try concurrent install"));
        drop(first);
        assert!(second.try_lock_exclusive().expect("retry install lock"));
        second.unlock().expect("release second install lock");
    }

    #[test]
    fn uninstall_removes_only_the_validated_extension_and_update_backup()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir().expect("extension root");
        let installed = root.path().join("rust");
        let backup = root.path().join(".rust.previous");
        let unrelated = root.path().join("html");
        for directory in [&installed, &backup, &unrelated] {
            fs::create_dir(directory)?;
        }
        for directory in [&installed, &backup, &unrelated] {
            let id = if directory == &unrelated {
                "html"
            } else {
                "rust"
            };
            fs::write(directory.join("extension.toml"), format!("id = '{id}'\n"))?;
            fs::write(directory.join("payload"), b"package data")?;
        }

        uninstall_extension_package(root.path(), "rust")?;

        assert!(!installed.exists());
        assert!(!backup.exists());
        assert!(unrelated.join("payload").exists());
        assert!(
            uninstall_extension_package(root.path(), "rust")
                .expect_err("a second uninstall reports the missing package")
                .to_string()
                .contains("not installed")
        );
        Ok(())
    }

    #[test]
    fn uninstall_rejects_a_directory_with_a_different_manifest_id() {
        let root = tempfile::tempdir().expect("extension root");
        let installed = root.path().join("rust");
        fs::create_dir(&installed).expect("installed directory");
        fs::write(installed.join("extension.toml"), "id = 'other'\n")
            .expect("manifest");

        let error = uninstall_extension_package(root.path(), "rust")
            .expect_err("a mismatched package must be left untouched");

        assert!(error.to_string().contains("ID does not match"));
        assert!(installed.exists());
    }

    #[test]
    fn interrupted_extension_update_restores_backup_before_retry() {
        let root = tempfile::tempdir().expect("extension root");
        let backup = root.path().join(".rust.previous");
        fs::create_dir(&backup).expect("previous extension");
        fs::write(
            backup.join("extension.toml"),
            "id = 'rust'\n[language_servers.rust-analyzer]\nlanguages = ['Rust']\n",
        )
        .expect("previous extension manifest");
        fs::write(backup.join("extension.wasm"), extension_component("0.8.0"))
            .expect("previous extension component");
        fs::write(backup.join("sentinel"), b"previous version")
            .expect("previous extension content");
        install_extension_package(
            &extension_archive("rust"),
            root.path(),
            "rust",
            None,
        )
        .expect_err("invalid replacement must fail");
        assert_eq!(
            fs::read(root.path().join("rust/sentinel"))
                .expect("restored extension content"),
            b"previous version"
        );
        assert!(!backup.exists());
    }

    #[test]
    fn malformed_backup_does_not_hide_healthy_extensions() {
        let root = tempfile::tempdir().expect("extension root");
        let broken = root.path().join(".rust.previous");
        fs::create_dir(&broken).expect("broken backup");
        fs::write(broken.join("extension.toml"), "id = 'other'")
            .expect("broken manifest");
        fs::write(broken.join("extension.wasm"), []).expect("broken component");
        let healthy = root.path().join("html");
        fs::create_dir_all(healthy.join("languages/html"))
            .expect("healthy extension language directory");
        fs::write(
            healthy.join("extension.toml"),
            "id = 'html'\nlanguages = ['languages/html']\n[language_servers.html]\nlanguages = ['HTML']\n",
        )
        .expect("healthy manifest");
        fs::write(healthy.join("extension.wasm"), extension_component("0.7.0"))
            .expect("healthy component");
        fs::write(
            healthy.join("languages/html/config.toml"),
            "name = 'HTML'\npath_suffixes = ['html']\n",
        )
        .expect("healthy language config");

        let discovered = discover_language_server_extensions(root.path())
            .expect("discover extensions");
        assert_eq!(discovered.extensions.len(), 1);
        assert_eq!(discovered.errors.len(), 1);
        assert_eq!(discovered.errors[0].0, broken);
        assert!(!root.path().join("rust").exists());
        assert_eq!(
            language_id_for_path(root.path(), Path::new("index.html"), None)
                .expect("healthy language lookup"),
            Some("HTML".to_string())
        );
    }

    #[test]
    fn rejects_unsupported_extension_api_before_replacement() {
        let root = tempfile::tempdir().expect("extension root");
        let archive = extension_archive_with_api_version("rust", "0.9.0", true);
        let error = install_extension_package(&archive, root.path(), "rust", None)
            .expect_err("unsupported extension API must not install");
        assert!(error.to_string().contains("unsupported Zed extension API"));
        assert!(!root.path().join("rust").exists());
    }

    #[test]
    fn rejects_extension_without_supported_content_before_replacement() {
        let root = tempfile::tempdir().expect("extension root");
        let archive = extension_archive_with_api_version("rust", "0.8.0", false);
        let error = install_extension_package(&archive, root.path(), "rust", None)
            .expect_err("extension without supported content must not install");
        assert!(
            error
                .to_string()
                .contains("no supported language servers or grammar assets")
        );
        assert!(!root.path().join("rust").exists());
    }

    #[test]
    fn extension_archive_limits_apply_before_unpacking() {
        let root = tempfile::tempdir().expect("extension root");
        let archive = extension_archive("rust");
        let size_error = extract_tar_gz_with_limits(&archive, root.path(), 4, 10)
            .expect_err("oversized archive must fail");
        assert!(size_error.to_string().contains("unpacked size limit"));
        let entry_error =
            extract_tar_gz_with_limits(&archive, root.path(), u64::MAX, 1)
                .expect_err("too many entries must fail");
        assert!(entry_error.to_string().contains("entry limit"));
    }

    #[test]
    fn rejects_icon_only_extension_before_replacement() {
        let root = tempfile::tempdir().expect("extension root");
        let encoder = flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        );
        let mut archive = tar::Builder::new(encoder);
        let mut root_header = tar::Header::new_gnu();
        root_header.set_entry_type(tar::EntryType::Directory);
        root_header.set_size(0);
        root_header.set_mode(0o755);
        root_header.set_cksum();
        archive
            .append_data(&mut root_header, "./", &[][..])
            .expect("archive root");
        for (path, content) in [
            (
                "extension.toml",
                "id = 'icons'\nversion = '1.0.0'\nicon_themes = ['icon_themes/icons.json']\n",
            ),
            (
                "icon_themes/icons.json",
                r#"{"themes":[{"name":"Icons","file_icons":{"rust":{"path":"./icons/rust.svg"}},"file_suffixes":{"rs":"rust"}}]}"#,
            ),
            ("icons/rust.svg", "<svg/>"),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, path, content.as_bytes())
                .expect("archive entry");
        }
        let package = archive
            .into_inner()
            .expect("archive")
            .finish()
            .expect("compression");
        let error =
            install_extension_package(&package, root.path(), "icons", Some("1.0.0"))
                .expect_err("icon themes are outside the supported extension scope");
        assert!(
            error
                .to_string()
                .contains("no supported language servers or grammar assets")
        );
        assert!(!root.path().join("icons").exists());
    }

    fn extension_archive(id: &str) -> Vec<u8> {
        extension_archive_with_api_version(id, "0.8.0", true)
    }

    fn extension_component(api_version: &str) -> Vec<u8> {
        let mut components = api_version.split('.');
        let version = [
            components
                .next()
                .and_then(|component| component.parse::<u16>().ok())
                .expect("API major version"),
            components
                .next()
                .and_then(|component| component.parse::<u16>().ok())
                .expect("API minor version"),
            components
                .next()
                .and_then(|component| component.parse::<u16>().ok())
                .expect("API patch version"),
        ];
        assert!(components.next().is_none(), "API version has three parts");

        let section_name = b"zed:api-version";
        let mut section = Vec::with_capacity(section_name.len() + 7);
        append_unsigned_leb128(section_name.len() as u32, &mut section);
        section.extend_from_slice(section_name);
        for component in version {
            section.extend_from_slice(&component.to_be_bytes());
        }

        let mut wasm = b"\0asm\x0d\0\x01\0".to_vec();
        wasm.push(0);
        append_unsigned_leb128(section.len() as u32, &mut wasm);
        wasm.extend_from_slice(&section);
        wasm
    }

    fn append_unsigned_leb128(mut value: u32, output: &mut Vec<u8>) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value > 0 {
                byte |= 0x80;
            }
            output.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    fn extension_archive_with_api_version(
        id: &str,
        version: &str,
        language_server: bool,
    ) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        );
        let mut archive = tar::Builder::new(encoder);
        let manifest = format!(
            "id = '{id}'\nversion = '0.1.0'\n{}",
            if language_server {
                "[language_servers.rust-analyzer]\nlanguages = ['Rust']\n"
            } else {
                ""
            }
        );
        let mut manifest_header = tar::Header::new_gnu();
        manifest_header.set_size(manifest.len() as u64);
        manifest_header.set_mode(0o644);
        manifest_header.set_cksum();
        archive
            .append_data(&mut manifest_header, "extension.toml", manifest.as_bytes())
            .expect("manifest entry");

        let mut wasm_header = tar::Header::new_gnu();
        let wasm = extension_component(version);
        wasm_header.set_size(wasm.len() as u64);
        wasm_header.set_mode(0o644);
        wasm_header.set_cksum();
        archive
            .append_data(&mut wasm_header, "extension.wasm", wasm.as_slice())
            .expect("wasm entry");
        let encoder = archive.into_inner().expect("finish archive");
        encoder.finish().expect("finish compression")
    }

    fn grammar_extension_archive(id: &str, language_name: &str) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        );
        let mut archive = tar::Builder::new(encoder);
        let files = [
            (
                "extension.toml".to_string(),
                format!(
                    "id = '{id}'\nversion = '1.0.0'\nlanguages = ['languages/{id}']\n[grammars.{id}]\nrepository = 'https://example.invalid/tree-sitter-{id}'\ncommit = '0123456789abcdef'\n"
                ),
            ),
            (
                format!("languages/{id}/config.toml"),
                format!(
                    "name = '{}'\ngrammar = '{id}'\npath_suffixes = ['{id}']\n",
                    language_name
                ),
            ),
            (
                format!("languages/{id}/highlights.scm"),
                "(identifier) @variable\n".to_string(),
            ),
            (
                format!("grammars/{id}.wasm"),
                "tree-sitter grammar wasm".to_string(),
            ),
        ];
        for (path, contents) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, path, contents.as_bytes())
                .expect("append grammar extension asset");
        }
        archive
            .into_inner()
            .expect("finish grammar extension archive")
            .finish()
            .expect("finish compressed grammar extension archive")
    }
}
