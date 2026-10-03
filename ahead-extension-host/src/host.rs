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
};
use tokio::process::Command as ProcessCommand;
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
const EXTENSION_DOWNLOAD_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(60);
const EXTENSION_COMMAND_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(120);

async fn run_extension_command(
    mut command: ProcessCommand,
    timeout: std::time::Duration,
) -> Result<std::process::Output, String> {
    command.kill_on_drop(true);
    tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| format!("extension command timed out after {timeout:?}"))?
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
    pub workspace_configuration: Option<serde_json::Value>,
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
    language_ids: BTreeMap<String, String>,
    capabilities: Vec<ProcessCapability>,
    download_capabilities: Vec<DownloadCapability>,
    npm_capabilities: Vec<NpmCapability>,
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
    path_suffixes: Vec<String>,
    #[serde(default)]
    first_line_pattern: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageDefinition {
    pub name: String,
    pub path_suffixes: Vec<String>,
    pub first_line_pattern: Option<String>,
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
    #[serde(rename = "lib")]
    library: Option<ExtensionLibrary>,
    #[serde(default)]
    schema_version: Option<u32>,
    #[serde(default)]
    languages: Vec<PathBuf>,
    #[serde(default)]
    icon_themes: Vec<PathBuf>,
    #[serde(default)]
    language_servers: BTreeMap<String, LanguageServerManifestEntry>,
    #[serde(default)]
    capabilities: Vec<CapabilityManifestEntry>,
}

#[derive(Debug, Deserialize)]
struct ExtensionLibrary {
    version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionApiVersion {
    V0_6,
    V0_8,
}

impl ExtensionManifest {
    fn api_version(&self) -> Result<ExtensionApiVersion> {
        match self
            .library
            .as_ref()
            .and_then(|library| library.version.as_deref())
        {
            Some("0.6.0" | "0.7.0") => Ok(ExtensionApiVersion::V0_6),
            Some("0.8.0") => Ok(ExtensionApiVersion::V0_8),
            Some(version) => {
                Err(anyhow!("unsupported Zed extension API version {version}"))
            }
            None => Err(anyhow!("Zed extension manifest is missing [lib].version")),
        }
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

#[derive(Debug, Deserialize, Default)]
struct LanguageServerManifestEntry {
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    language_ids: BTreeMap<String, String>,
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
        let api_version = manifest.api_version().with_context(|| {
            format!(
                "checking extension API version in {}",
                manifest_path.display()
            )
        });
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
                download_capabilities: manifest
                    .capabilities
                    .iter()
                    .filter(|capability| capability.kind == "download_file")
                    .filter_map(|capability| {
                        Some(DownloadCapability {
                            host: capability.host.clone()?,
                            path: capability.path.clone(),
                        })
                    })
                    .collect(),
                npm_capabilities: manifest
                    .capabilities
                    .iter()
                    .filter(|capability| capability.kind == "npm:install")
                    .filter_map(|capability| {
                        Some(NpmCapability {
                            package: capability.package.clone()?,
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
    let mut definitions = Vec::new();
    if !extensions_root.exists() {
        return Ok(None);
    }
    let mut first_error = None;
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
        match language_definitions_for_extension(&directory, &manifest_path) {
            Ok(extension_definitions) => definitions.extend(extension_definitions),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    let language = definitions
        .into_iter()
        .filter(|definition| definition.matches(path, content))
        .max_by_key(|definition| {
            definition
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
        .map(|definition| definition.name);
    match (language, first_error) {
        (Some(language), _) => Ok(Some(language)),
        (None, Some(error)) => Err(error),
        (None, None) => Ok(None),
    }
}

fn language_definitions_for_extension(
    directory: &Path,
    manifest_path: &Path,
) -> Result<Vec<LanguageDefinition>> {
    let manifest: ExtensionManifest = toml::from_str(
        &std::fs::read_to_string(manifest_path)
            .with_context(|| format!("reading {}", manifest_path.display()))?,
    )
    .with_context(|| format!("parsing {}", manifest_path.display()))?;
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
        let config_path = directory.join(&language_path).join("config.toml");
        if !config_path.is_file() {
            continue;
        }
        let config: LanguageConfig = toml::from_str(
            &std::fs::read_to_string(&config_path)
                .with_context(|| format!("reading {}", config_path.display()))?,
        )
        .with_context(|| format!("parsing {}", config_path.display()))?;
        definitions.push(LanguageDefinition {
            name: config.name,
            path_suffixes: config.path_suffixes,
            first_line_pattern: config.first_line_pattern,
        });
    }
    Ok(definitions)
}

pub fn install_extension_from_url(
    url: &str,
    extensions_root: &Path,
    extension_id: &str,
    expected_version: &str,
    expected_sha256: Option<&str>,
) -> Result<PathBuf> {
    let archive = download_bytes(url)?;
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
        !manifest.language_servers.is_empty() || !manifest.icon_themes.is_empty(),
        "extension package declares no supported language servers or icon themes"
    );
    if !manifest.language_servers.is_empty() {
        manifest.api_version()?;
        anyhow::ensure!(
            package_root.join("extension.wasm").is_file(),
            "extension package is missing extension.wasm"
        );
        let wasm = std::fs::read(package_root.join("extension.wasm"))?;
        ExtensionHost::new()?.validate_component(&wasm)?;
    }
    for theme_file in &manifest.icon_themes {
        crate::icon_theme::load_icon_theme(&package_root, theme_file)?;
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
    Ok(target)
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
        !manifest.language_servers.is_empty() || !manifest.icon_themes.is_empty(),
        "extension backup has no supported content"
    );
    if !manifest.language_servers.is_empty() {
        manifest.api_version()?;
        anyhow::ensure!(
            std::fs::symlink_metadata(backup.join("extension.wasm"))?.is_file(),
            "extension backup has no language server component"
        );
    }
    for theme_file in &manifest.icon_themes {
        crate::icon_theme::load_icon_theme(&backup, theme_file)?;
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

fn download_bytes(url: &str) -> Result<Vec<u8>> {
    let url = reqwest::Url::parse(url).context("invalid extension download URL")?;
    anyhow::ensure!(
        allowed_extension_download_url(&url),
        "extension downloads require HTTPS without URL credentials"
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(EXTENSION_DOWNLOAD_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() < 10
                && allowed_extension_download_url(attempt.url())
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

fn allowed_extension_download_url(url: &reqwest::Url) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    match url.scheme() {
        "https" => true,
        "http" if cfg!(test) => match url.host() {
            Some(url::Host::Ipv4(address)) => address.is_loopback(),
            Some(url::Host::Ipv6(address)) => address.is_loopback(),
            Some(url::Host::Domain("localhost")) => true,
            _ => false,
        },
        _ => false,
    }
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
pub struct HttpResponseStreamContext;

struct HostState {
    table: ResourceTable,
    wasi: WasiCtx,
    worktree_root: PathBuf,
    work_dir: PathBuf,
    process_capabilities: Vec<ProcessCapability>,
    download_capabilities: Vec<DownloadCapability>,
    npm_capabilities: Vec<NpmCapability>,
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
        _request: http_client::HttpRequest,
    ) -> wasmtime::Result<Result<http_client::HttpResponse, String>> {
        Ok(Err("HTTP is not supported by AHEAD's language host".into()))
    }

    async fn fetch_stream(
        &mut self,
        _request: http_client::HttpRequest,
    ) -> wasmtime::Result<Result<Resource<HttpResponseStreamContext>, String>> {
        Ok(Err("HTTP is not supported by AHEAD's language host".into()))
    }
}

impl HostHttpResponseStream for HostState {
    async fn next_chunk(
        &mut self,
        _stream: Resource<HttpResponseStreamContext>,
    ) -> wasmtime::Result<Result<Option<Vec<u8>>, String>> {
        Ok(Err("HTTP is not supported by AHEAD's language host".into()))
    }

    async fn drop(
        &mut self,
        _stream: Resource<HttpResponseStreamContext>,
    ) -> wasmtime::Result<()> {
        Ok(())
    }
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
        if !self
            .process_capabilities
            .iter()
            .any(|capability| capability.allows(&command.command, &command.args))
        {
            return Ok(Err(format!(
                "process:exec is not allowed for {:?} {:?}",
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
        Ok(which_binary("node")
            .ok_or_else(|| "Node.js is not installed or not on PATH".to_string()))
    }

    async fn npm_package_latest_version(
        &mut self,
        package_name: String,
    ) -> wasmtime::Result<Result<String, String>> {
        let result = (|| -> Result<String> {
            let response = reqwest::blocking::Client::builder()
                .timeout(EXTENSION_DOWNLOAD_TIMEOUT)
                .build()?
                .get(format!("https://registry.npmjs.org/{package_name}"))
                .send()?
                .error_for_status()?;
            let package: serde_json::Value = response.json()?;
            package["dist-tags"]["latest"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow!("NPM response has no latest version"))
        })();
        Ok(result.map_err(|error| error.to_string()))
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
        if !self
            .npm_capabilities
            .iter()
            .any(|capability| capability.allows(&package_name))
        {
            return Ok(Err(format!(
                "npm:install is not allowed for {package_name}"
            )));
        }
        let Some(npm) = which_binary("npm") else {
            return Ok(Err("npm is not installed or not on PATH".into()));
        };
        let mut command = ProcessCommand::new(npm);
        command.current_dir(&self.work_dir).args([
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

impl ExtensionImports for HostState {
    async fn get_settings(
        &mut self,
        _location: Option<SettingsLocation>,
        category: String,
        key: Option<String>,
    ) -> wasmtime::Result<Result<String, String>> {
        let settings = match category.as_str() {
            "lsp" => lsp_settings_for_extension(
                &self.worktree_root,
                ahead_core::directory::Directory::home_dir().as_deref(),
                key.as_deref(),
            ),
            _ => Ok("{}".to_string()),
        };
        Ok(settings.map_err(|error| format!("{error:#}")))
    }

    async fn download_file(
        &mut self,
        url: String,
        path: String,
        file_type: DownloadedFileType,
    ) -> wasmtime::Result<Result<(), String>> {
        let result = (|| -> Result<()> {
            let url = url::Url::parse(&url)?;
            anyhow::ensure!(
                self.download_capabilities
                    .iter()
                    .any(|capability| capability.allows(&url)),
                "download_file is not allowed for {}",
                url
            );
            let destination = checked_extension_path(&self.work_dir, &path)?;
            let client = reqwest::blocking::Client::builder()
                .timeout(EXTENSION_DOWNLOAD_TIMEOUT)
                .build()?;
            let mut response = client
                .get(url.clone())
                .send()
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
            response
                .by_ref()
                .take(MAX_EXTENSION_DOWNLOAD_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_EXTENSION_DOWNLOAD_BYTES {
                return Err(anyhow!("extension download exceeds the 512 MiB limit"));
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
        })();
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
    // ponytail: sessions live until proxy restart; invalidate per extension when hot upgrades exist.
    sessions: Mutex<HashMap<(PathBuf, PathBuf), GuestSlot>>,
    #[cfg(test)]
    instantiations: AtomicUsize,
}

impl ExtensionHost {
    pub fn new() -> Result<Self> {
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
        let workspace_configuration = Self::workspace_configuration(
            store,
            extension,
            language_server_id,
            worktree_root_for_command,
        )
        .await?;
        Ok(LanguageServerCommand {
            command: resolve_work_dir_command(&work_dir, command),
            args,
            env,
            initialization_options,
            workspace_configuration,
        })
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
                download_capabilities: extension.download_capabilities.clone(),
                npm_capabilities: extension.npm_capabilities.clone(),
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

    pub async fn language_server_command_for_extension(
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

fn checked_extension_path(root: &Path, relative: &str) -> Result<PathBuf> {
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
    fn extension_download_rejects_insecure_and_credentialed_urls() {
        for url in [
            "http://example.com/extension.tar.gz",
            "file:///tmp/extension.tar.gz",
            "https://user:secret@example.com/extension.tar.gz",
        ] {
            assert!(
                !allowed_extension_download_url(
                    &reqwest::Url::parse(url).expect("test URL")
                ),
                "{url} must not be an extension download source"
            );
            assert!(download_bytes(url).is_err());
        }
        assert!(allowed_extension_download_url(
            &reqwest::Url::parse("https://registry.ahead.dev/extension.tar.gz")
                .expect("HTTPS URL")
        ));
        assert!(allowed_extension_download_url(
            &reqwest::Url::parse("http://127.0.0.1:8080/extension.tar.gz")
                .expect("test loopback URL")
        ));
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
        let expected_version = toml::from_str::<ExtensionManifest>(
            &fs::read_to_string(source.join("extension.toml"))?,
        )?
        .version
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
        for name in ["extension.toml", "extension.wasm"] {
            let bytes = fs::read(source.join(name))?;
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, name, bytes.as_slice())?;
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
    fn discovers_language_servers_from_extension_manifests() {
        let root = tempfile::tempdir().expect("extension root");
        let extension = root.path().join("rust");
        fs::create_dir_all(&extension).expect("extension directory");
        fs::write(
            extension.join("extension.toml"),
            "id = 'rust'\n[lib]\nversion = '0.7.0'\n[language_servers.rust-analyzer]\nlanguages = ['Rust']\n[language_servers.rust-analyzer.language_ids]\nRust = 'rust-lang'\n",
        )
        .expect("manifest");
        fs::write(extension.join("extension.wasm"), [])
            .expect("component placeholder");
        let unsupported = root.path().join("unsupported");
        fs::create_dir_all(&unsupported).expect("unsupported extension directory");
        fs::write(
            unsupported.join("extension.toml"),
            "id = 'unsupported'\n[lib]\nversion = '0.9.0'\n[language_servers.unsupported]\nlanguages = ['Rust']\n",
        )
        .expect("unsupported manifest");
        fs::write(unsupported.join("extension.wasm"), [])
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
                "id = 'html'\nlanguages = ['languages/html']\n[lib]\nversion = '0.7.0'\n[language_servers.html]\nlanguages = ['HTML']\n",
            )
            .expect("extension manifest");
            fs::write(directory.join("extension.wasm"), [])
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
            "name = 'Gleam'\npath_suffixes = ['gleam']\n",
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
        assert!(
            language_id_for_path(root.path(), Path::new("src/main.unknown"), None)
                .is_err()
        );
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
    fn interrupted_extension_update_restores_backup_before_retry() {
        let root = tempfile::tempdir().expect("extension root");
        let backup = root.path().join(".rust.previous");
        fs::create_dir(&backup).expect("previous extension");
        fs::write(
            backup.join("extension.toml"),
            "id = 'rust'\n[lib]\nversion = '0.8.0'\n[language_servers.rust-analyzer]\nlanguages = ['Rust']\n",
        )
        .expect("previous extension manifest");
        fs::write(backup.join("extension.wasm"), [])
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
            "id = 'html'\nlanguages = ['languages/html']\n[lib]\nversion = '0.7.0'\n[language_servers.html]\nlanguages = ['HTML']\n",
        )
        .expect("healthy manifest");
        fs::write(healthy.join("extension.wasm"), []).expect("healthy component");
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
        let archive =
            extension_archive_with_wasm("rust", "0.9.0", b"\0asm\x0d\0\x01\0", true);
        let error = install_extension_package(&archive, root.path(), "rust", None)
            .expect_err("unsupported extension API must not install");
        assert!(error.to_string().contains("unsupported Zed extension API"));
        assert!(!root.path().join("rust").exists());
    }

    #[test]
    fn rejects_extension_without_supported_content_before_replacement() {
        let root = tempfile::tempdir().expect("extension root");
        let archive = extension_archive_with_wasm(
            "rust",
            "0.8.0",
            b"\0asm\x0d\0\x01\0",
            false,
        );
        let error = install_extension_package(&archive, root.path(), "rust", None)
            .expect_err("extension without supported content must not install");
        assert!(
            error
                .to_string()
                .contains("no supported language servers or icon themes")
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
    fn installs_icon_only_extension_without_wasm() {
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
        let installed =
            install_extension_package(&package, root.path(), "icons", Some("1.0.0"))
                .expect("install icon theme");
        assert!(!installed.join("extension.wasm").exists());
        assert!(installed.join("icons/rust.svg").is_file());
        assert_eq!(
            crate::icon_theme::load_icon_theme(
                &installed,
                Path::new("icon_themes/icons.json")
            )
            .expect("load installed theme")
            .icon_for_file(Path::new("main.rs"))
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str()),
            Some("rust.svg")
        );
    }

    #[test]
    #[ignore = "set AHEAD_MATERIAL_ICON_THEME_ARCHIVE to the published Zed package"]
    fn published_material_icon_theme_package_loads() {
        let archive = std::fs::read(
            std::env::var("AHEAD_MATERIAL_ICON_THEME_ARCHIVE")
                .expect("published archive path"),
        )
        .expect("published archive");
        let root = tempfile::tempdir().expect("extension root");
        let installed = install_extension_package(
            &archive,
            root.path(),
            "material-icon-theme",
            Some("1.3.1"),
        )
        .expect("install published package");
        let theme = crate::icon_theme::load_icon_theme(
            &installed,
            Path::new("icon_themes/material-icon-theme.json"),
        )
        .expect("load published theme");
        assert_eq!(
            theme
                .icon_for_file(Path::new("main.rs"))
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str()),
            Some("rust.svg")
        );
        assert_eq!(
            theme
                .icon_for_file(Path::new("main.py"))
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str()),
            Some("python.svg")
        );
    }

    fn extension_archive(id: &str) -> Vec<u8> {
        extension_archive_with_wasm(id, "0.8.0", b"\0asm\x0d\0\x01\0", true)
    }

    fn extension_archive_with_wasm(
        id: &str,
        version: &str,
        wasm: &[u8],
        language_server: bool,
    ) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        );
        let mut archive = tar::Builder::new(encoder);
        let manifest = format!(
            "id = '{id}'\nversion = '0.1.0'\n[lib]\nversion = '{version}'\n{}",
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
        wasm_header.set_size(wasm.len() as u64);
        wasm_header.set_mode(0o644);
        wasm_header.set_cksum();
        archive
            .append_data(&mut wasm_header, "extension.wasm", wasm)
            .expect("wasm entry");
        let encoder = archive.into_inner().expect("finish archive");
        encoder.finish().expect("finish compression")
    }
}
