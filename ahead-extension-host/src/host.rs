use anyhow::{Context, Result, anyhow};
use regex::Regex;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    env,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
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
const EXTENSION_DOWNLOAD_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(60);

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
    languages: Vec<String>,
    language_ids: BTreeMap<String, String>,
    capabilities: Vec<ProcessCapability>,
    download_capabilities: Vec<DownloadCapability>,
    npm_capabilities: Vec<NpmCapability>,
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
    schema_version: Option<u32>,
    #[serde(default)]
    languages: Vec<PathBuf>,
    #[serde(default)]
    language_servers: BTreeMap<String, LanguageServerManifestEntry>,
    #[serde(default)]
    capabilities: Vec<CapabilityManifestEntry>,
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
) -> Result<Vec<LanguageServerExtension>> {
    let mut extensions = Vec::new();
    if !extensions_root.exists() {
        return Ok(extensions);
    }
    for entry in std::fs::read_dir(extensions_root)? {
        let entry = entry?;
        let directory = entry.path();
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let manifest_path = directory.join("extension.toml");
        let wasm_path = directory.join("extension.wasm");
        if !manifest_path.is_file() || !wasm_path.is_file() {
            continue;
        }
        let manifest: ExtensionManifest = toml::from_str(
            &std::fs::read_to_string(&manifest_path)
                .with_context(|| format!("reading {}", manifest_path.display()))?,
        )
        .with_context(|| format!("parsing {}", manifest_path.display()))?;
        for (server_id, server) in manifest.language_servers {
            let mut languages = server.languages.clone();
            if let Some(language) = server.language {
                languages.push(language);
            }
            let language_ids = server.language_ids;
            languages.extend(language_ids.keys().cloned());
            languages.sort_unstable_by_key(|language| language.to_ascii_lowercase());
            languages.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
            extensions.push(LanguageServerExtension {
                id: manifest.id.clone(),
                server_id,
                directory: directory.clone(),
                wasm_path: wasm_path.clone(),
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
    extensions.sort_by(|left, right| {
        (&left.id, &left.server_id).cmp(&(&right.id, &right.server_id))
    });
    Ok(extensions)
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
    for entry in std::fs::read_dir(extensions_root)? {
        let entry = entry?;
        let directory = entry.path();
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let manifest_path = directory.join("extension.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest: ExtensionManifest = toml::from_str(
            &std::fs::read_to_string(&manifest_path)
                .with_context(|| format!("reading {}", manifest_path.display()))?,
        )
        .with_context(|| format!("parsing {}", manifest_path.display()))?;
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
    }
    Ok(definitions
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
        .map(|definition| definition.name))
}

pub fn install_extension_from_url(
    url: &str,
    extensions_root: &Path,
    extension_id: &str,
) -> Result<PathBuf> {
    let archive = download_bytes(url)?;
    install_extension_package(&archive, extensions_root, extension_id)
}

pub fn install_extension_package(
    archive: &[u8],
    extensions_root: &Path,
    extension_id: &str,
) -> Result<PathBuf> {
    if extension_id.is_empty()
        || extension_id == "."
        || extension_id == ".."
        || extension_id.contains('/')
        || extension_id.contains('\\')
    {
        return Err(anyhow!("invalid extension id"));
    }
    std::fs::create_dir_all(extensions_root)?;
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
    if let Some(schema_version) = manifest.schema_version {
        anyhow::ensure!(
            schema_version <= 1,
            "unsupported language extension manifest schema version {schema_version}"
        );
    }
    anyhow::ensure!(
        package_root.join("extension.wasm").is_file(),
        "extension package is missing extension.wasm"
    );
    let wasm = std::fs::read(package_root.join("extension.wasm"))?;
    ExtensionHost::new()?.validate_component(&wasm)?;

    let target = extensions_root.join(extension_id);
    let backup = extensions_root.join(format!(".{extension_id}.previous"));
    if backup.exists() {
        std::fs::remove_dir_all(&backup)?;
    }
    if target.exists() {
        std::fs::rename(&target, &backup)?;
    }
    if let Err(error) = std::fs::rename(package_root, &target) {
        if backup.exists() {
            std::fs::rename(&backup, &target).ok();
        }
        return Err(error.into());
    }
    if backup.exists() {
        std::fs::remove_dir_all(backup)?;
    }
    Ok(target)
}

fn download_bytes(url: &str) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(EXTENSION_DOWNLOAD_TIMEOUT)
        .build()?;
    let mut response = client
        .get(url)
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
    let decoder = flate2::read::GzDecoder::new(Cursor::new(archive_bytes));
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let relative = entry.path()?.into_owned();
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
        let output = ProcessCommand::new(&command.command)
            .args(&command.args)
            .envs(command.env)
            .output()
            .map_err(|error| error.to_string());
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
        let result = ProcessCommand::new(npm)
            .current_dir(&self.work_dir)
            .args([
                "install",
                "--prefix",
                self.work_dir.to_string_lossy().as_ref(),
                "--ignore-scripts",
                "--no-package-lock",
                &format!("{package_name}@{version}"),
            ])
            .output()
            .map_err(|error| error.to_string())
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
        let path = env::var_os("PATH")
            .into_iter()
            .flat_map(|paths| env::split_paths(&paths).collect::<Vec<_>>())
            .map(|path| path.join(&binary_name))
            .find(|path| path.is_file());
        Ok(path.map(|path| path.to_string_lossy().into_owned()))
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

impl ExtensionImports for HostState {
    async fn get_settings(
        &mut self,
        _location: Option<SettingsLocation>,
        _category: String,
        _key: Option<String>,
    ) -> wasmtime::Result<Result<String, String>> {
        Ok(Ok("{}".to_string()))
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

pub struct ExtensionHost {
    engine: Engine,
}

impl ExtensionHost {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config)
            .map_err(|error| anyhow!("creating extension host engine: {error}"))?;
        Ok(Self { engine })
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

    pub async fn language_server_command(
        &self,
        bytes: &[u8],
        language_server_id: &str,
        worktree: WorktreeContext,
        extension_root: PathBuf,
    ) -> Result<LanguageServerCommand> {
        self.language_server_command_with_capabilities(
            bytes,
            language_server_id,
            worktree,
            extension_root,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn language_server_command_with_capabilities(
        &self,
        bytes: &[u8],
        language_server_id: &str,
        worktree: WorktreeContext,
        extension_root: PathBuf,
        process_capabilities: Vec<ProcessCapability>,
        download_capabilities: Vec<DownloadCapability>,
        npm_capabilities: Vec<NpmCapability>,
    ) -> Result<LanguageServerCommand> {
        let component = Component::from_binary(&self.engine, bytes)
            .map_err(|error| anyhow!(error))
            .context("loading language extension component")?;
        let mut linker = Linker::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)
            .map_err(|error| anyhow!(error))
            .context("registering WASI imports")?;
        Extension::add_to_linker::<HostState, HasSelf<HostState>>(
            &mut linker,
            |state: &mut HostState| state,
        )
        .map_err(|error| anyhow!(error))
        .context("registering extension host imports")?;
        let worktree_root = worktree.root.clone();
        let work_dir = extension_root
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("work")
            .join(
                extension_root
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
                process_capabilities,
                download_capabilities,
                npm_capabilities,
            },
        );
        let extension =
            Extension::instantiate_async(&mut store, &component, &linker)
                .await
                .map_err(|error| anyhow!(error))
                .context("instantiating language extension")?;
        extension
            .call_init_extension(&mut store)
            .await
            .map_err(|error| anyhow!(error))
            .context("initializing language extension")?;
        let worktree_root_for_command = worktree.root.clone();
        let worktree = store
            .data_mut()
            .table
            .push(worktree)
            .map_err(|error| anyhow!(error))?;
        let command = extension
            .call_language_server_command(&mut store, language_server_id, worktree)
            .await
            .map_err(|error| anyhow!(error))?
            .map_err(|error| anyhow!(error))
            .context("resolving language server command")?;
        let initialization_worktree = store
            .data_mut()
            .table
            .push(WorktreeContext {
                root: worktree_root_for_command.clone(),
            })
            .map_err(|error| anyhow!(error))?;
        let initialization_options = extension
            .call_language_server_initialization_options(
                &mut store,
                language_server_id,
                initialization_worktree,
            )
            .await
            .map_err(|error| anyhow!(error))?
            .map_err(|error| anyhow!(error))?
            .map(|options| serde_json::from_str(&options))
            .transpose()
            .context("parsing language server initialization options")?;
        let workspace_worktree = store
            .data_mut()
            .table
            .push(WorktreeContext {
                root: worktree_root_for_command,
            })
            .map_err(|error| anyhow!(error))?;
        let workspace_configuration = extension
            .call_language_server_workspace_configuration(
                &mut store,
                language_server_id,
                workspace_worktree,
            )
            .await
            .map_err(|error| anyhow!(error))?
            .map_err(|error| anyhow!(error))?
            .map(|options| serde_json::from_str(&options))
            .transpose()
            .context("parsing language server workspace configuration")?;
        Ok(LanguageServerCommand {
            command: resolve_work_dir_command(&work_dir, command.command),
            args: command.args,
            env: command.env,
            initialization_options,
            workspace_configuration,
        })
    }

    pub async fn language_server_command_for_extension(
        &self,
        extension: &LanguageServerExtension,
        worktree: WorktreeContext,
    ) -> Result<LanguageServerCommand> {
        let bytes = std::fs::read(&extension.wasm_path)
            .with_context(|| format!("reading {}", extension.wasm_path.display()))?;
        self.language_server_command_with_capabilities(
            &bytes,
            &extension.server_id,
            worktree,
            extension.directory.clone(),
            extension.capabilities.clone(),
            extension.download_capabilities.clone(),
            extension.npm_capabilities.clone(),
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
    env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| env::split_paths(&paths).collect::<Vec<_>>())
        .map(|path| path.join(binary))
        .find(|path| path.is_file())
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
    use std::fs;

    #[test]
    fn host_engine_constructs() {
        let _host = ExtensionHost::new().expect("host engine");
    }

    #[test]
    fn discovers_language_servers_from_extension_manifests() {
        let root = tempfile::tempdir().expect("extension root");
        let extension = root.path().join("rust");
        fs::create_dir_all(&extension).expect("extension directory");
        fs::write(
            extension.join("extension.toml"),
            "id = 'rust'\n[language_servers.rust-analyzer]\nlanguages = ['Rust']\n[language_servers.rust-analyzer.language_ids]\nRust = 'rust-lang'\n",
        )
        .expect("manifest");
        fs::write(extension.join("extension.wasm"), [])
            .expect("component placeholder");

        let discovered = discover_language_server_extensions(root.path())
            .expect("discover extensions");
        assert_eq!(discovered.len(), 1);
        assert!(discovered[0].supports_language("rust"));
        assert_eq!(discovered[0].server_id, "rust-analyzer");
        assert_eq!(discovered[0].protocol_language_id("Rust"), "rust-lang");
        assert!(discovered[0].supports_language("rust-lang"));
        assert_eq!(discovered[0].protocol_language_id("rust-lang"), "rust-lang");
        assert_eq!(discovered[0].language_ids(), ["Rust", "rust-lang"]);
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

        assert_eq!(
            language_id_for_path(root.path(), Path::new("src/main.gleam"), None,)
                .expect("language lookup"),
            Some("Gleam".to_string())
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
        let error = install_extension_package(&archive, root.path(), "rust")
            .expect_err("component without extension exports must not install");
        assert!(error.to_string().contains("missing export"));
        assert!(!root.path().join("rust").exists());
    }

    fn extension_archive(id: &str) -> Vec<u8> {
        extension_archive_with_wasm(id, b"\0asm\x0d\0\x01\0")
    }

    fn extension_archive_with_wasm(id: &str, wasm: &[u8]) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        );
        let mut archive = tar::Builder::new(encoder);
        let manifest = format!(
            "id = '{id}'\n[language_servers.rust-analyzer]\nlanguages = ['Rust']\n"
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
