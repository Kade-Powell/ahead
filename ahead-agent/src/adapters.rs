//! Install and launch AHEAD's curated ACP agent catalog.

use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, OnceLock},
    thread,
    time::{Duration, SystemTime},
};

use ahead_rpc::ahead::{AgentConfigOptionValue, ExternalAcpAdapter};
use anyhow::{Context, Result, bail, ensure};
use fs4::fs_std::FileExt;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::acp_client::HarnessClientConfig;

#[derive(Debug, Clone)]
struct RegistryNpxAdapter {
    package: String,
    version: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct RegistryBinaryAdapter {
    version: String,
    target: RegistryBinaryTarget,
}

#[derive(Debug, Clone)]
enum RegistryAdapter {
    Npx(RegistryNpxAdapter),
    Binary(RegistryBinaryAdapter),
}

#[derive(Debug, Clone)]
struct RegistryAgent {
    id: String,
    version: String,
    adapter: Option<RegistryAdapter>,
}

#[derive(Clone, Copy)]
struct SupportedAgent {
    id: &'static str,
    display_name: &'static str,
    fallback_package: &'static str,
    prerequisite_note: Option<&'static str>,
}

const SUPPORTED_AGENTS: [SupportedAgent; 3] = [
    SupportedAgent {
        id: "pi-acp",
        display_name: "Pi",
        fallback_package: "pi-acp@0.0.34",
        prerequisite_note: Some(
            "Requires the Pi CLI v0.81+ on PATH; AHEAD installs only the ACP bridge.",
        ),
    },
    SupportedAgent {
        id: "codex-acp",
        display_name: "Codex",
        fallback_package: "@agentclientprotocol/codex-acp@2.2.2",
        prerequisite_note: None,
    },
    SupportedAgent {
        id: "claude-acp",
        display_name: "Claude Code",
        fallback_package: "@agentclientprotocol/claude-agent-acp@0.94.0",
        prerequisite_note: None,
    },
];

#[derive(Debug, Clone, Deserialize)]
struct RegistryIndex {
    agents: Vec<RegistryAgentEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct RegistryAgentEntry {
    id: String,
    version: String,
    distribution: RegistryDistribution,
}

#[derive(Debug, Clone, Deserialize)]
struct RegistryDistribution {
    #[serde(default)]
    binary: Option<BTreeMap<String, RegistryBinaryTarget>>,
    #[serde(default)]
    npx: Option<RegistryNpxDistribution>,
}

#[derive(Debug, Clone, Deserialize)]
struct RegistryBinaryTarget {
    archive: String,
    cmd: String,
    #[serde(default)]
    args: Vec<String>,
    sha256: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RegistryNpxDistribution {
    package: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct InstallProvenance {
    generation: uuid::Uuid,
    package_spec: String,
    package_json_sha256: String,
    package_lock_sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct BinaryInstallProvenance {
    generation: uuid::Uuid,
    version: String,
    archive: String,
    archive_sha256: String,
    command: String,
    executable_sha256: String,
}

struct InstalledNpx {
    script: PathBuf,
    generation: PathBuf,
}

struct ResolvedNpx {
    script: PathBuf,
    lease: Option<Arc<File>>,
}

#[derive(Debug, Clone)]
struct ConfiguredAdapter {
    adapter: ExternalAcpAdapter,
    registry: Option<RegistryAdapter>,
}

fn supported_agent(id: &str) -> Option<SupportedAgent> {
    SUPPORTED_AGENTS
        .iter()
        .copied()
        .find(|agent| agent.id == id)
}

fn current_platform_key() -> Option<&'static str> {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        return None;
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        return None;
    };
    match (os, arch) {
        ("darwin", "aarch64") => Some("darwin-aarch64"),
        ("darwin", "x86_64") => Some("darwin-x86_64"),
        ("linux", "aarch64") => Some("linux-aarch64"),
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("windows", "aarch64") => Some("windows-aarch64"),
        ("windows", "x86_64") => Some("windows-x86_64"),
        _ => None,
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn package_version(package_spec: &str) -> Option<&str> {
    package_spec
        .rsplit_once('@')
        .filter(|(name, _)| !name.is_empty())
        .map(|(_, version)| version)
        .filter(|version| !version.is_empty())
}

fn usable_binary_target(target: &RegistryBinaryTarget) -> bool {
    if !target.sha256.as_deref().is_some_and(valid_sha256) {
        return false;
    }
    let (Ok(kind), Ok(command)) = (
        binary_archive_kind(&target.archive),
        relative_binary_command(&target.cmd),
    ) else {
        return false;
    };
    kind != BinaryArchiveKind::Raw || command.is_some()
}

const REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
const REGISTRY_CACHE_MAX_AGE: Duration = Duration::from_secs(60 * 60);
const MAX_REGISTRY_BYTES: u64 = 4 * 1024 * 1024;
#[cfg(unix)]
const CROSS_PROCESS_GENERATION_LEASE_MARKER: &str = "ahead-generation-lease-v1:";
const PROCESS_LOCAL_GENERATION_LEASE_MARKER: &str =
    "ahead-generation-lease-local-v1:";
static CACHE_INSTANCE_ID: OnceLock<uuid::Uuid> = OnceLock::new();

fn cache_instance_id() -> uuid::Uuid {
    *CACHE_INSTANCE_ID.get_or_init(uuid::Uuid::new_v4)
}

fn generation_lease_marker() -> String {
    #[cfg(unix)]
    let prefix = CROSS_PROCESS_GENERATION_LEASE_MARKER;
    #[cfg(not(unix))]
    let prefix = PROCESS_LOCAL_GENERATION_LEASE_MARKER;
    format!("{prefix}{}", cache_instance_id())
}

fn generation_marker_can_be_pruned(marker: &str) -> bool {
    #[cfg(unix)]
    if marker
        .strip_prefix(CROSS_PROCESS_GENERATION_LEASE_MARKER)
        .is_some_and(|instance| uuid::Uuid::parse_str(instance).is_ok())
    {
        return true;
    }

    marker
        .strip_prefix(PROCESS_LOCAL_GENERATION_LEASE_MARKER)
        .is_some_and(|instance| instance == cache_instance_id().to_string())
}

fn configured_adapters(
    storage: &Path,
    user_agents_dir: &Path,
) -> Result<Vec<ConfiguredAdapter>> {
    let registry = load_registry(storage).unwrap_or_else(|error| {
        tracing::warn!(%error, "ACP registry unavailable; using pinned supported adapters");
        Vec::new()
    });

    curated_adapters(&registry, user_agents_dir, storage)
}

fn curated_adapters(
    registry: &[RegistryAgent],
    user_agents_dir: &Path,
    storage: &Path,
) -> Result<Vec<ConfiguredAdapter>> {
    SUPPORTED_AGENTS
        .into_iter()
        .map(|supported| {
            let registry_entry =
                registry.iter().find(|entry| entry.id == supported.id);
            let fallback = || RegistryNpxAdapter {
                package: supported.fallback_package.to_string(),
                version: package_version(supported.fallback_package)
                    .unwrap_or("unknown")
                    .to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
            };
            let (registry, latest_version) = match registry_entry {
                Some(entry) => (entry.adapter.clone(), Some(entry.version.clone())),
                None => {
                    let fallback = fallback();
                    let version = fallback.version.clone();
                    (Some(RegistryAdapter::Npx(fallback)), Some(version))
                }
            };
            let installed =
                installation_marker(user_agents_dir, supported.id)?.is_file();
            let installed_version = installed
                .then(|| {
                    installed_adapter_version(
                        storage,
                        supported.id,
                        registry.as_ref(),
                    )
                })
                .flatten();
            let prerequisite_note = if registry.is_none() {
                Some(
                    "No compatible ACP distribution is available on this platform."
                        .to_string(),
                )
            } else {
                supported.prerequisite_note.map(str::to_string)
            };
            Ok(ConfiguredAdapter {
                adapter: ExternalAcpAdapter {
                    id: supported.id.to_string(),
                    display_name: supported.display_name.to_string(),
                    installed,
                    available: registry.is_some(),
                    latest_version: latest_version.clone(),
                    update_available: installed_version
                        .as_ref()
                        .zip(latest_version.as_ref())
                        .is_some_and(|(installed, latest)| installed != latest),
                    installed_version,
                    prerequisite_note,
                },
                registry,
            })
        })
        .collect()
}

fn installed_adapter_version(
    storage: &Path,
    adapter_id: &str,
    registry: Option<&RegistryAdapter>,
) -> Option<String> {
    let (preferred, alternate) = match registry {
        Some(RegistryAdapter::Npx(_)) => ("npx", "binary"),
        Some(RegistryAdapter::Binary(_)) => ("binary", "npx"),
        None => ("binary", "npx"),
    };
    [preferred, alternate].into_iter().find_map(|cache_kind| {
        let manifest = storage
            .join("registry")
            .join(cache_kind)
            .join(sanitize_path_component(adapter_id))
            .join("installed.json");
        let contents = fs::read(manifest).ok()?;
        if cache_kind == "npx" {
            let provenance: InstallProvenance =
                serde_json::from_slice(&contents).ok()?;
            package_version(&provenance.package_spec).map(str::to_string)
        } else {
            let provenance: BinaryInstallProvenance =
                serde_json::from_slice(&contents).ok()?;
            Some(provenance.version)
        }
    })
}

fn load_registry(storage: &Path) -> Result<Vec<RegistryAgent>> {
    let registry_path = storage.join("registry/registry.json");
    let cached_file = open_registry_cache(&registry_path);
    let cache_is_fresh = cached_file
        .as_ref()
        .ok()
        .and_then(|file| file.metadata().ok())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < REGISTRY_CACHE_MAX_AGE);

    if !cache_is_fresh {
        refresh_registry_in_background(registry_path);
    }
    let body = match cached_file {
        Ok(file) => read_registry_bytes(file)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error.into()),
    };

    let index: RegistryIndex =
        serde_json::from_slice(&body).context("parsing ACP registry")?;
    Ok(index
        .agents
        .into_iter()
        .map(|entry| {
            let platform_binary = current_platform_key()
                .and_then(|platform| {
                    entry
                        .distribution
                        .binary
                        .as_ref()
                        .and_then(|targets| targets.get(platform))
                })
                .filter(|target| usable_binary_target(target))
                .cloned();
            let npx = entry.distribution.npx.map(|npx| RegistryNpxAdapter {
                package: npx.package,
                version: entry.version.clone(),
                args: npx.args,
                env: npx.env,
            });
            let adapter = match (platform_binary, npx) {
                (Some(target), _) => {
                    Some(RegistryAdapter::Binary(RegistryBinaryAdapter {
                        version: entry.version.clone(),
                        target,
                    }))
                }
                (None, Some(npx)) => Some(RegistryAdapter::Npx(npx)),
                (None, None) => None,
            };
            RegistryAgent {
                id: entry.id,
                version: entry.version,
                adapter,
            }
        })
        .collect())
}

fn open_registry_cache(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "missing registry cache directory",
            )
        })?;
        let directory = File::open(parent)?;
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "missing registry cache filename",
            )
        })?;
        ahead_core::secure_fs::open_relative_regular_file(
            &directory,
            Path::new(name),
        )
    }
    #[cfg(not(unix))]
    {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "registry cache is not a regular file",
            ));
        }
        File::open(path)
    }
}

fn read_registry_bytes(reader: impl Read) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    reader.take(MAX_REGISTRY_BYTES + 1).read_to_end(&mut body)?;
    ensure!(
        body.len() as u64 <= MAX_REGISTRY_BYTES,
        "ACP registry exceeds the 4 MiB limit"
    );
    Ok(body)
}

fn publish_registry_cache(path: &Path, body: &[u8]) -> Result<()> {
    ensure!(
        body.len() as u64 <= MAX_REGISTRY_BYTES,
        "ACP registry exceeds the 4 MiB limit"
    );
    serde_json::from_slice::<RegistryIndex>(body)
        .context("validating ACP registry before cache publication")?;
    let parent = path.parent().context("missing registry cache directory")?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(body)?;
    temporary.persist(path)?;
    Ok(())
}

/// Zed refreshes its registry asynchronously from `AgentRegistryStore`. AHEAD's
/// RPC list call is synchronous, so return the cached index immediately and do
/// the same refresh off-thread rather than blocking the editor on HTTP.
fn refresh_registry_in_background(registry_path: PathBuf) {
    let Some(parent) = registry_path.parent().map(Path::to_path_buf) else {
        return;
    };
    if fs::create_dir_all(&parent).is_err() {
        return;
    }
    let Ok(Some(lock)) =
        InstallLock::try_acquire(registry_path.with_extension("lock"))
    else {
        return;
    };
    let refresh = thread::Builder::new()
        .name("ahead-acp-registry-refresh".to_string())
        .spawn(move || {
            let result = (|| -> Result<()> {
                let response = reqwest::blocking::Client::builder()
                    .timeout(Duration::from_secs(30))
                    .build()
                    .context("creating ACP registry HTTP client")?
                    .get(REGISTRY_URL)
                    .send()
                    .context("fetching ACP registry")?
                    .error_for_status()
                    .context("ACP registry returned an error status")?;
                let body =
                    read_registry_bytes(response).context("reading ACP registry")?;
                publish_registry_cache(&registry_path, &body).with_context(
                    || {
                        format!(
                            "publishing ACP registry cache {}",
                            registry_path.display()
                        )
                    },
                )?;
                Ok(())
            })();
            if let Err(error) = result {
                tracing::debug!(%error, "ACP registry refresh failed");
            }
            drop(lock);
        });
    match refresh {
        Ok(handle) => drop(handle),
        Err(error) => {
            tracing::debug!(%error, "failed to start ACP registry refresh");
        }
    }
}

/// Lists AHEAD's small, supported ACP catalog and each agent's install state.
pub fn external_acp_adapters() -> Result<Vec<ExternalAcpAdapter>> {
    let storage = external_agents_dir()?;
    let user_agents_dir = external_agents_user_dir()?;
    Ok(configured_adapters(&storage, &user_agents_dir)?
        .into_iter()
        .map(|entry| entry.adapter)
        .collect())
}

/// Adds or removes an ACP agent from AHEAD's user-local agent catalog.
pub fn set_external_acp_adapter_installed(
    adapter_id: &str,
    installed: bool,
) -> Result<()> {
    let user_agents_dir = external_agents_user_dir()?;
    set_external_acp_adapter_installed_at(adapter_id, installed, &user_agents_dir)
}

/// Checks an installed marker without loading or refreshing the ACP registry.
pub fn external_acp_adapter_is_installed(adapter_id: &str) -> Result<bool> {
    let user_agents_dir = external_agents_user_dir()?;
    external_acp_adapter_is_installed_at(adapter_id, &user_agents_dir)
}

fn external_acp_adapter_is_installed_at(
    adapter_id: &str,
    user_agents_dir: &Path,
) -> Result<bool> {
    Ok(installation_marker(user_agents_dir, adapter_id)?.is_file())
}

fn set_external_acp_adapter_installed_at(
    adapter_id: &str,
    installed: bool,
    user_agents_dir: &Path,
) -> Result<()> {
    let marker = installation_marker(user_agents_dir, adapter_id)?;
    if installed {
        fs::create_dir_all(user_agents_dir)?;
        File::create(&marker).with_context(|| {
            format!("marking ACP agent '{adapter_id}' installed")
        })?;
    } else if marker.exists() {
        fs::remove_file(&marker).with_context(|| {
            format!("removing ACP agent '{adapter_id}' from AHEAD")
        })?;
    }
    Ok(())
}

fn installation_marker(user_agents_dir: &Path, adapter_id: &str) -> Result<PathBuf> {
    supported_agent(adapter_id)
        .map(|agent| user_agents_dir.join(format!("{}.installed", agent.id)))
        .with_context(|| {
            format!("ACP agent '{adapter_id}' is not supported by AHEAD")
        })
}

fn external_agents_user_dir() -> Result<PathBuf> {
    crate::instructions::user_home()
        .map(|home| home.join(".ahead/agents/external-acp"))
        .context("AHEAD user home is unavailable for ACP agent settings")
}

fn external_agents_dir() -> Result<PathBuf> {
    ahead_core::directory::Directory::data_local_directory()
        .map(|directory| directory.join("external_agents"))
        .context("AHEAD user data directory is unavailable for ACP adapters")
}

/// Resolves an installed supported agent into its cached or installable command.
pub fn external_agent_config(
    adapter_id: Option<&str>,
    cwd: PathBuf,
) -> Result<HarnessClientConfig> {
    external_agent_config_at(
        adapter_id,
        cwd,
        &external_agents_dir()?,
        &external_agents_user_dir()?,
    )
}

fn external_agent_config_at(
    adapter_id: Option<&str>,
    cwd: PathBuf,
    storage: &Path,
    user_agents_dir: &Path,
) -> Result<HarnessClientConfig> {
    let Some(adapter_id) = adapter_id else {
        bail!("No external ACP adapter was selected")
    };
    let entry = configured_adapters(storage, user_agents_dir)?
        .into_iter()
        .find(|entry| entry.adapter.id == adapter_id)
        .with_context(|| format!("ACP adapter '{adapter_id}' is not available"))?;
    anyhow::ensure!(
        entry.adapter.installed,
        "ACP agent '{adapter_id}' is not installed in AHEAD"
    );
    if adapter_id == "pi-acp" {
        ensure_pi_cli_available()?;
    }

    let defaults_path = default_config_options_path(user_agents_dir, adapter_id)?;
    let registry = entry
        .registry
        .context("ACP agent has no distribution for this platform")?;
    let mut config = match &registry {
        RegistryAdapter::Npx(registry) => {
            install_npx_adapter(adapter_id, registry, cwd, storage)?
        }
        RegistryAdapter::Binary(registry) => {
            install_binary_adapter(adapter_id, registry, cwd, storage)?
        }
    };
    config.default_config_options = load_default_config_options(&defaults_path)?;
    config.default_config_options_path = Some(defaults_path);
    Ok(config)
}

fn ensure_pi_cli_available() -> Result<()> {
    ensure_pi_cli_command(pi_cli_version_command())
}

fn pi_cli_version_command() -> Command {
    #[cfg(windows)]
    {
        let shell = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
        let mut command = Command::new(shell);
        command.args(["/d", "/s", "/c", "pi --version"]);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new("pi");
        command.arg("--version");
        command
    }
}

fn ensure_pi_cli_command(mut command: Command) -> Result<()> {
    const INSTALL: &str = "npm install -g @earendil-works/pi-coding-agent";

    let output = command.output().with_context(|| {
        format!(
            "Pi ACP requires the Pi CLI v0.81.0 or newer on PATH. Install it with `{INSTALL}`"
        )
    })?;
    ensure!(
        output.status.success(),
        "Pi CLI `pi --version` failed: {}. Install or update it with `{INSTALL}`",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let reported_version = String::from_utf8_lossy(&output.stdout);
    ensure_pi_cli_version(&reported_version)
}

fn ensure_pi_cli_version(reported_version: &str) -> Result<()> {
    const INSTALL: &str = "npm install -g @earendil-works/pi-coding-agent";

    let version_text = reported_version.trim().trim_start_matches('v');
    let version = Version::parse(version_text).with_context(|| {
        format!("Pi CLI returned an unrecognized version: {version_text:?}")
    })?;
    ensure!(
        version >= Version::new(0, 81, 0),
        "Pi CLI v{version} is too old; AHEAD requires v0.81.0 or newer. Update it with `{INSTALL}`"
    );
    Ok(())
}

fn default_config_options_path(
    user_agents_dir: &Path,
    adapter_id: &str,
) -> Result<PathBuf> {
    supported_agent(adapter_id)
        .map(|agent| {
            user_agents_dir.join(format!("{}.config-options.json", agent.id))
        })
        .with_context(|| {
            format!("ACP agent '{adapter_id}' is not supported by AHEAD")
        })
}

fn load_default_config_options(
    path: &Path,
) -> Result<HashMap<String, AgentConfigOptionValue>> {
    let contents = match fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(HashMap::new());
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("reading ACP config defaults {}", path.display())
            });
        }
    };
    serde_json::from_slice(&contents)
        .with_context(|| format!("parsing ACP config defaults {}", path.display()))
}

pub(crate) fn persist_default_config_option(
    path: &Path,
    config_id: &str,
    value: &AgentConfigOptionValue,
) -> Result<HashMap<String, AgentConfigOptionValue>> {
    let parent = path
        .parent()
        .context("ACP config defaults path has no parent")?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "creating ACP config defaults directory {}",
            parent.display()
        )
    })?;
    let _lock = InstallLock::acquire(path.with_extension("lock"))?;
    let mut defaults = load_default_config_options(path)?;
    defaults.insert(config_id.to_string(), value.clone());
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .context("creating temporary ACP config defaults")?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), &defaults)
        .context("serializing ACP config defaults")?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| {
            format!("publishing ACP config defaults {}", path.display())
        })?;
    Ok(defaults)
}

fn sanitize_path_component(input: &str) -> String {
    let sanitized = input
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => character,
            _ => '-',
        })
        .collect::<String>();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        "unknown".to_string()
    } else {
        sanitized
    }
}

/// Mirrors Zed's `LocalRegistryNpxAgent`: install one pinned package in an
/// isolated directory, resolve its package `bin`, then launch it with Node.
/// The lock/provenance file is the small bit Zed can delegate to its serialized
/// project store; AHEAD has multiple agent threads sharing one process.
struct InstallLock {
    _file: File,
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        // A forked child may still hold this open-file description.
        if let Err(error) = FileExt::unlock(&self._file) {
            tracing::error!(%error, "failed to unlock ACP cache");
        }
    }
}

impl InstallLock {
    fn acquire(path: PathBuf) -> Result<Self> {
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| {
                format!("opening ACP adapter lock {}", path.display())
            })?;
        file.lock_exclusive().with_context(|| {
            format!("locking ACP adapter cache {}", path.display())
        })?;
        Ok(Self { _file: file })
    }

    fn try_acquire(path: PathBuf) -> Result<Option<Self>> {
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| {
                format!("opening ACP registry lock {}", path.display())
            })?;
        match file.try_lock_exclusive() {
            Ok(true) => Ok(Some(Self { _file: file })),
            Ok(false) => Ok(None),
            Err(error) => Err(error).with_context(|| {
                format!("locking ACP registry cache {}", path.display())
            }),
        }
    }
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| {
        format!("reading package provenance file {}", path.display())
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        digest.update(&buffer[..bytes_read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn package_name(package_spec: &str) -> &str {
    package_spec
        .rsplit_once('@')
        .map(|(name, _)| if name.is_empty() { package_spec } else { name })
        .unwrap_or(package_spec)
}

fn package_bin<'a>(
    package: &'a serde_json::Value,
    package_name: &str,
) -> Result<&'a str> {
    match package.get("bin") {
        Some(serde_json::Value::String(path)) => Ok(path),
        Some(serde_json::Value::Object(entries)) => {
            let executable = package_name
                .rsplit('/')
                .next()
                .and_then(|name| entries.get(name))
                .and_then(serde_json::Value::as_str)
                .or_else(|| entries.values().find_map(serde_json::Value::as_str));
            executable.context("ACP adapter package has no executable")
        }
        _ => bail!("ACP adapter package has no executable"),
    }
}

fn npx_package_executable(
    installation: &Path,
    package_spec: &str,
) -> Result<PathBuf> {
    let installation = fs::canonicalize(installation)?;
    let package_name = package_name(package_spec);
    let package_root =
        fs::canonicalize(installation.join("node_modules").join(package_name))?;
    anyhow::ensure!(
        package_root.starts_with(&installation) && package_root != installation,
        "ACP adapter package escapes its installation directory"
    );
    let package_json = package_root.join("package.json");
    let package: serde_json::Value = serde_json::from_slice(
        &fs::read(&package_json)
            .with_context(|| format!("reading {}", package_json.display()))?,
    )
    .with_context(|| format!("parsing {}", package_json.display()))?;
    let script =
        fs::canonicalize(package_root.join(package_bin(&package, package_name)?))
            .context("resolving ACP adapter executable")?;
    anyhow::ensure!(
        script.starts_with(&package_root) && script.is_file(),
        "ACP adapter executable must be a file inside its package directory"
    );
    Ok(script)
}

fn installed_npx_executable(
    install_dir: &Path,
    package_spec: &str,
) -> Result<Option<InstalledNpx>> {
    let marker = install_dir.join("installed.json");
    let contents = match fs::read(&marker) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading {}", marker.display()));
        }
    };
    let provenance: InstallProvenance = serde_json::from_slice(&contents)
        .with_context(|| format!("parsing {}", marker.display()))?;
    if provenance.package_spec != package_spec {
        return Ok(None);
    }

    let generation =
        fs::canonicalize(install_dir.join(provenance.generation.to_string()))?;
    let cache_root = fs::canonicalize(install_dir)?;
    anyhow::ensure!(
        generation.parent() == Some(cache_root.as_path()),
        "ACP adapter generation escapes its cache directory"
    );
    let script = npx_package_executable(&generation, package_spec)?;
    let package_json = generation
        .join("node_modules")
        .join(package_name(package_spec))
        .join("package.json");
    if sha256_file(&package_json)? != provenance.package_json_sha256
        || sha256_file(&generation.join("package-lock.json"))?
            != provenance.package_lock_sha256
    {
        return Ok(None);
    }
    Ok(Some(InstalledNpx { script, generation }))
}

fn recover_abandoned_staging(install_dir: &Path) -> Result<()> {
    for entry in fs::read_dir(install_dir)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(".install-")
            && entry.file_type()?.is_dir()
        {
            fs::remove_dir_all(entry.path()).with_context(|| {
                format!("removing abandoned ACP install {}", entry.path().display())
            })?;
        }
    }
    Ok(())
}

fn lease_generation(generation: &Path) -> Result<Option<Arc<File>>> {
    let path = generation.join(".ahead-lease");
    let file = match fs::OpenOptions::new().read(true).write(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Older installations have no lease contract, so never prune them.
            return Ok(None);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("opening {}", path.display()));
        }
    };
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "ACP generation lease is not a file"
    );
    FileExt::lock_shared(&file).with_context(|| {
        format!("leasing ACP generation {}", generation.display())
    })?;
    Ok(Some(Arc::new(file)))
}

fn prune_unleased_generations(install_dir: &Path, current: &Path) -> Result<()> {
    for entry in fs::read_dir(install_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.file_name() == current.file_name()
            || !entry.file_type()?.is_dir()
            || uuid::Uuid::parse_str(&entry.file_name().to_string_lossy()).is_err()
        {
            continue;
        }
        let lease_path = path.join(".ahead-lease");
        let marker = match fs::read_to_string(&lease_path) {
            Ok(marker) => marker,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading {}", lease_path.display()));
            }
        };
        if !generation_marker_can_be_pruned(&marker) {
            // Older markers do not prove that the ACP child inherited its
            // lease, so retain those generations rather than risk deleting
            // files still used by a process from an earlier app version.
            continue;
        }
        let file = match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lease_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("opening {}", lease_path.display()));
            }
        };
        if !file.metadata()?.is_file() || !FileExt::try_lock_exclusive(&file)? {
            continue;
        }
        FileExt::unlock(&file)?;
        drop(file);
        fs::remove_dir_all(&path).with_context(|| {
            format!("removing unused ACP generation {}", path.display())
        })?;
    }
    Ok(())
}

fn resolved_npx(install_dir: &Path, installed: InstalledNpx) -> Result<ResolvedNpx> {
    let lease = lease_generation(&installed.generation)?;
    if let Err(error) =
        prune_unleased_generations(install_dir, &installed.generation)
    {
        tracing::warn!(%error, "could not prune unused ACP adapter generations");
    }
    Ok(ResolvedNpx {
        script: installed.script,
        lease,
    })
}

fn ensure_npx_package(
    install_dir: &Path,
    package_spec: &str,
    install: impl FnOnce(&Path) -> Result<()>,
) -> Result<ResolvedNpx> {
    fs::create_dir_all(install_dir).with_context(|| {
        format!("creating ACP adapter directory {}", install_dir.display())
    })?;
    let _lock = InstallLock::acquire(install_dir.join(".install.lock"))?;
    if let Err(error) = recover_abandoned_staging(install_dir) {
        tracing::warn!(%error, "could not clear abandoned ACP adapter staging");
    }
    match installed_npx_executable(install_dir, package_spec) {
        Ok(Some(installed)) => return resolved_npx(install_dir, installed),
        Ok(None) => {}
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
        {
            tracing::warn!(%error, "ACP adapter cache is incomplete; reinstalling");
        }
        Err(error) => return Err(error),
    }

    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(install_dir)
        .context("creating ACP adapter staging directory")?;
    install(staging.path())?;
    npx_package_executable(staging.path(), package_spec)?;
    fs::write(
        staging.path().join(".ahead-lease"),
        generation_lease_marker(),
    )?;
    let provenance = InstallProvenance {
        generation: uuid::Uuid::new_v4(),
        package_spec: package_spec.to_string(),
        package_json_sha256: sha256_file(
            &staging
                .path()
                .join("node_modules")
                .join(package_name(package_spec))
                .join("package.json"),
        )?,
        package_lock_sha256: sha256_file(&staging.path().join("package-lock.json"))?,
    };
    let generation = install_dir.join(provenance.generation.to_string());
    fs::rename(staging.path(), &generation)
        .context("publishing ACP adapter package directory")?;
    // Resolve again after the move; Node must never receive the temporary path.
    let script = npx_package_executable(&generation, package_spec)?;
    let mut marker = tempfile::NamedTempFile::new_in(install_dir)?;
    serde_json::to_writer(marker.as_file_mut(), &provenance)?;
    marker.as_file().sync_all()?;
    marker
        .persist(install_dir.join("installed.json"))
        .map_err(|error| error.error)
        .context("publishing ACP adapter manifest")?;
    resolved_npx(install_dir, InstalledNpx { script, generation })
}

fn install_npx_adapter(
    adapter_id: &str,
    registry: &RegistryNpxAdapter,
    cwd: PathBuf,
    storage: &Path,
) -> Result<HarnessClientConfig> {
    let install_dir = storage
        .join("registry/npx")
        .join(sanitize_path_component(adapter_id));
    let resolved = ensure_npx_package(&install_dir, &registry.package, |staging| {
        let output = npx_install_command(staging, storage, &registry.package)
            .output()
            .with_context(|| {
                format!("installing ACP adapter '{adapter_id}' with npm")
            })?;
        anyhow::ensure!(
            output.status.success(),
            "npm failed to install ACP adapter '{adapter_id}': {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(())
    })?;
    let mut config = HarnessClientConfig::new(
        std::env::var("AHEAD_NODE_PATH").unwrap_or_else(|_| "node".to_string()),
        cwd,
    );
    config.env = registry
        .env
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    config
        .args
        .push(resolved.script.to_string_lossy().into_owned());
    config.generation_lease = resolved.lease;
    config.args.extend(registry.args.iter().cloned());
    Ok(config)
}

fn npx_install_command(staging: &Path, storage: &Path, package: &str) -> Command {
    let npm_cache = storage.join("registry/npm-cache");
    let mut command = npm_command();
    command
        .args(["install", "--prefix"])
        .arg(staging)
        .args([
            "--save-exact",
            "--package-lock=true",
            "--no-audit",
            "--no-fund",
            "--cache",
        ])
        .arg(npm_cache)
        .arg("--")
        .arg(package);
    command
}

const MAX_BINARY_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_BINARY_EXTRACTED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_BINARY_ARCHIVE_ENTRIES: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinaryArchiveKind {
    Zip,
    TarGzip,
    TarBzip2,
    Raw,
}

struct InstalledBinary {
    command: Option<PathBuf>,
    generation: PathBuf,
}

struct ResolvedBinary {
    command: Option<PathBuf>,
    lease: Option<Arc<File>>,
}

fn binary_archive_kind(archive: &str) -> Result<BinaryArchiveKind> {
    let url = url::Url::parse(archive).context("parsing ACP binary archive URL")?;
    ensure!(
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none(),
        "ACP binary archive must use HTTPS without embedded credentials"
    );
    let path = url.path().to_ascii_lowercase();
    if path.ends_with(".zip") {
        Ok(BinaryArchiveKind::Zip)
    } else if path.ends_with(".tar.gz") || path.ends_with(".tgz") {
        Ok(BinaryArchiveKind::TarGzip)
    } else if path.ends_with(".tar.bz2") || path.ends_with(".tbz2") {
        Ok(BinaryArchiveKind::TarBzip2)
    } else if [
        ".dmg",
        ".pkg",
        ".deb",
        ".rpm",
        ".msi",
        ".appimage",
        ".tar.xz",
        ".txz",
        ".tar",
        ".gz",
        ".bz2",
        ".xz",
        ".7z",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
    {
        bail!("unsupported ACP binary archive type: {archive}")
    } else {
        Ok(BinaryArchiveKind::Raw)
    }
}

fn verify_archive_sha256(actual: &str, expected: &str) -> Result<()> {
    ensure!(valid_sha256(expected), "invalid ACP archive SHA-256");
    ensure!(
        actual.eq_ignore_ascii_case(expected),
        "ACP binary archive SHA-256 mismatch"
    );
    Ok(())
}

fn relative_binary_command(command: &str) -> Result<Option<PathBuf>> {
    if command == "node" {
        return Ok(None);
    }
    let normalized = command.replace('\\', "/");
    let relative = normalized.strip_prefix("./").unwrap_or(&normalized);
    ensure!(
        !relative.is_empty()
            && relative.split('/').all(|component| {
                !component.is_empty()
                    && component != "."
                    && component != ".."
                    && !component.contains(':')
                    && !component.contains('\0')
            }),
        "ACP binary command must stay inside its installation directory"
    );
    Ok(Some(PathBuf::from(relative)))
}

fn validated_archive_path(path: &Path) -> Result<PathBuf> {
    ensure!(
        !path.as_os_str().is_empty()
            && path.components().all(|component| matches!(
                component,
                std::path::Component::Normal(_)
            )),
        "ACP binary archive contains an unsafe path"
    );
    Ok(path.to_path_buf())
}

fn download_and_extract_binary(
    target: &RegistryBinaryTarget,
    staging: &Path,
) -> Result<()> {
    let expected_sha256 = target
        .sha256
        .as_deref()
        .filter(|digest| valid_sha256(digest))
        .context("ACP binary distribution requires a valid SHA-256 checksum")?;
    let kind = binary_archive_kind(&target.archive)?;
    let command_path = relative_binary_command(&target.cmd)?;
    ensure!(
        kind != BinaryArchiveKind::Raw || command_path.is_some(),
        "raw ACP binary must provide a relative executable command"
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5 * 60))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.url().scheme() == "https" && attempt.previous().len() < 10 {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .context("creating ACP binary download client")?;
    let mut response = client
        .get(target.archive.as_str())
        .send()
        .context("downloading ACP binary distribution")?
        .error_for_status()
        .context("ACP binary distribution returned an error status")?;
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= MAX_BINARY_ARCHIVE_BYTES),
        "ACP binary archive exceeds the 256 MiB limit"
    );

    let mut downloaded = tempfile::NamedTempFile::new_in(staging)
        .context("creating temporary ACP binary archive")?;
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let bytes_read = response.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        total = total
            .checked_add(bytes_read as u64)
            .context("ACP archive size overflow")?;
        ensure!(
            total <= MAX_BINARY_ARCHIVE_BYTES,
            "ACP binary archive exceeds the 256 MiB limit"
        );
        digest.update(&buffer[..bytes_read]);
        downloaded.write_all(&buffer[..bytes_read])?;
    }
    downloaded.as_file().sync_all()?;
    let actual_sha256 = format!("{:x}", digest.finalize());
    verify_archive_sha256(&actual_sha256, expected_sha256)?;

    match kind {
        BinaryArchiveKind::Zip => extract_zip(downloaded.reopen()?, staging)?,
        BinaryArchiveKind::TarGzip => {
            extract_tar(flate2::read::GzDecoder::new(downloaded.reopen()?), staging)?
        }
        BinaryArchiveKind::TarBzip2 => {
            extract_tar(bzip2::read::BzDecoder::new(downloaded.reopen()?), staging)?
        }
        BinaryArchiveKind::Raw => {
            let command_path = command_path.as_ref().context(
                "raw ACP binary must provide a relative executable command",
            )?;
            let destination = staging.join(command_path);
            let parent = destination
                .parent()
                .context("ACP binary command has no parent directory")?;
            fs::create_dir_all(parent)?;
            fs::copy(downloaded.path(), &destination)?;
        }
    }
    if let Some(command_path) = command_path {
        let command = staging.join(command_path);
        let metadata = fs::symlink_metadata(&command)?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "ACP binary command is not a regular file"
        );
        set_executable(&command, true)?;
    }
    Ok(())
}

fn extract_zip(archive_file: File, destination: &Path) -> Result<()> {
    let mut archive =
        zip::ZipArchive::new(archive_file).context("reading ACP ZIP archive")?;
    ensure!(
        archive.len() <= MAX_BINARY_ARCHIVE_ENTRIES,
        "ACP binary archive has too many entries"
    );
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let path = entry
            .enclosed_name()
            .context("ACP ZIP archive contains an unsafe path")?;
        let path = validated_archive_path(&path)?;
        let output = destination.join(path);
        if entry.is_dir() {
            fs::create_dir_all(output)?;
            continue;
        }
        if let Some(mode) = entry.unix_mode() {
            ensure!(
                mode & 0o170000 != 0o120000,
                "ACP ZIP archive contains a symbolic link"
            );
        }
        copy_archive_file(&mut entry, &output, &mut total)?;
        let executable = entry.unix_mode().is_some_and(|mode| mode & 0o111 != 0);
        set_executable(&output, executable)?;
    }
    Ok(())
}

fn extract_tar(reader: impl Read, destination: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive.entries().context("reading ACP tar archive")?;
    let mut total = 0_u64;
    let mut count = 0_usize;
    for entry in entries {
        let mut entry = entry?;
        count += 1;
        ensure!(
            count <= MAX_BINARY_ARCHIVE_ENTRIES,
            "ACP binary archive has too many entries"
        );
        let path = validated_archive_path(&entry.path()?.into_owned())?;
        let output = destination.join(path);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            fs::create_dir_all(output)?;
            continue;
        }
        ensure!(
            kind.is_file(),
            "ACP tar archive contains a link or special file"
        );
        let executable = entry.header().mode().is_ok_and(|mode| mode & 0o111 != 0);
        copy_archive_file(&mut entry, &output, &mut total)?;
        set_executable(&output, executable)?;
    }
    Ok(())
}

fn copy_archive_file(
    reader: &mut impl Read,
    destination: &Path,
    total: &mut u64,
) -> Result<()> {
    let parent = destination
        .parent()
        .context("ACP archive entry has no parent directory")?;
    fs::create_dir_all(parent)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| {
            format!("creating ACP archive entry {}", destination.display())
        })?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        *total = total
            .checked_add(bytes_read as u64)
            .context("ACP extracted size overflow")?;
        ensure!(
            *total <= MAX_BINARY_EXTRACTED_BYTES,
            "ACP binary archive expands beyond the 1 GiB limit"
        );
        file.write_all(&buffer[..bytes_read])?;
    }
    file.sync_all()?;
    Ok(())
}

fn set_executable(path: &Path, executable: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
        )?;
    }
    #[cfg(not(unix))]
    let _ = (path, executable);
    Ok(())
}

fn installed_binary_executable(
    install_dir: &Path,
    adapter: &RegistryBinaryAdapter,
) -> Result<Option<InstalledBinary>> {
    let manifest_path = install_dir.join("installed.json");
    let contents = match fs::read(&manifest_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let provenance: BinaryInstallProvenance = serde_json::from_slice(&contents)
        .with_context(|| format!("parsing {}", manifest_path.display()))?;
    let checksum = adapter
        .target
        .sha256
        .as_deref()
        .filter(|digest| valid_sha256(digest))
        .context("ACP binary distribution requires a valid SHA-256 checksum")?;
    if provenance.version != adapter.version
        || provenance.archive != adapter.target.archive
        || !provenance.archive_sha256.eq_ignore_ascii_case(checksum)
        || provenance.command != adapter.target.cmd
    {
        return Ok(None);
    }

    let cache_root = fs::canonicalize(install_dir)?;
    let generation =
        fs::canonicalize(install_dir.join(provenance.generation.to_string()))?;
    ensure!(
        generation.parent() == Some(cache_root.as_path()),
        "ACP binary generation escapes its cache directory"
    );
    let command = relative_binary_command(&adapter.target.cmd)?
        .map(|relative| generation.join(relative));
    if let Some(command) = &command {
        let metadata = fs::symlink_metadata(command)?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "ACP binary command is not a regular file"
        );
        let command = fs::canonicalize(command)?;
        ensure!(
            command.starts_with(&generation)
                && sha256_file(&command)? == provenance.executable_sha256,
            "ACP binary command failed cache integrity verification"
        );
        return Ok(Some(InstalledBinary {
            command: Some(command),
            generation,
        }));
    }
    Ok(Some(InstalledBinary {
        command: None,
        generation,
    }))
}

fn resolved_binary(
    install_dir: &Path,
    installed: InstalledBinary,
) -> Result<ResolvedBinary> {
    let lease = lease_generation(&installed.generation)?;
    if let Err(error) =
        prune_unleased_generations(install_dir, &installed.generation)
    {
        tracing::warn!(%error, "could not prune unused ACP binary generations");
    }
    Ok(ResolvedBinary {
        command: installed.command,
        lease,
    })
}

fn ensure_binary_agent(
    install_dir: &Path,
    adapter: &RegistryBinaryAdapter,
    install: impl FnOnce(&Path) -> Result<()>,
) -> Result<ResolvedBinary> {
    ensure!(
        adapter.target.sha256.as_deref().is_some_and(valid_sha256),
        "ACP binary distribution requires a valid SHA-256 checksum"
    );
    binary_archive_kind(&adapter.target.archive)?;
    relative_binary_command(&adapter.target.cmd)?;
    fs::create_dir_all(install_dir).with_context(|| {
        format!("creating ACP binary directory {}", install_dir.display())
    })?;
    let _lock = InstallLock::acquire(install_dir.join(".install.lock"))?;
    if let Err(error) = recover_abandoned_staging(install_dir) {
        tracing::warn!(%error, "could not clear abandoned ACP binary staging");
    }
    match installed_binary_executable(install_dir, adapter) {
        Ok(Some(installed)) => return resolved_binary(install_dir, installed),
        Ok(None) => {}
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
        {
            tracing::warn!(%error, "ACP binary cache is incomplete; reinstalling");
        }
        Err(error) => return Err(error),
    }

    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(install_dir)
        .context("creating ACP binary staging directory")?;
    install(staging.path())?;
    let command_path = relative_binary_command(&adapter.target.cmd)?;
    let executable = command_path
        .as_ref()
        .map(|relative| staging.path().join(relative));
    let executable_sha256 = if let Some(executable) = &executable {
        let metadata = fs::symlink_metadata(executable)?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "ACP binary command is not a regular file"
        );
        sha256_file(executable)?
    } else {
        String::new()
    };
    fs::write(
        staging.path().join(".ahead-lease"),
        generation_lease_marker(),
    )?;
    let provenance = BinaryInstallProvenance {
        generation: uuid::Uuid::new_v4(),
        version: adapter.version.clone(),
        archive: adapter.target.archive.clone(),
        archive_sha256: adapter
            .target
            .sha256
            .as_deref()
            .context("ACP binary distribution requires a SHA-256 checksum")?
            .to_ascii_lowercase(),
        command: adapter.target.cmd.clone(),
        executable_sha256,
    };
    let generation = install_dir.join(provenance.generation.to_string());
    fs::rename(staging.path(), &generation)
        .context("publishing ACP binary generation")?;
    let mut marker = tempfile::NamedTempFile::new_in(install_dir)?;
    serde_json::to_writer(marker.as_file_mut(), &provenance)?;
    marker.as_file().sync_all()?;
    marker
        .persist(install_dir.join("installed.json"))
        .map_err(|error| error.error)
        .context("publishing ACP binary manifest")?;
    let installed = installed_binary_executable(install_dir, adapter)?
        .context("resolving newly installed ACP binary")?;
    resolved_binary(install_dir, installed)
}

fn install_binary_adapter(
    adapter_id: &str,
    adapter: &RegistryBinaryAdapter,
    cwd: PathBuf,
    storage: &Path,
) -> Result<HarnessClientConfig> {
    let install_dir = storage
        .join("registry/binary")
        .join(sanitize_path_component(adapter_id));
    let resolved = ensure_binary_agent(&install_dir, adapter, |staging| {
        download_and_extract_binary(&adapter.target, staging)
    })?;
    let command = resolved
        .command
        .as_ref()
        .map(|command| command.to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            std::env::var("AHEAD_NODE_PATH").unwrap_or_else(|_| "node".to_string())
        });
    let mut config = HarnessClientConfig::new(command, cwd);
    config.args = adapter.target.args.clone();
    config.env = adapter.target.env.clone().into_iter().collect();
    config.generation_lease = resolved.lease;
    Ok(config)
}

fn npm_command() -> Command {
    Command::new(if cfg!(windows) { "npm.cmd" } else { "npm" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp_client::{HarnessClient, HarnessEvent};
    use parking_lot::Mutex;

    #[test]
    fn registry_cache_bounds_and_publication_preserve_last_good_index() {
        let storage = tempfile::tempdir().expect("temporary ACP registry");
        let cache = storage.path().join("registry/registry.json");
        let original = br#"{"agents":[]}"#;
        publish_registry_cache(&cache, original).expect("publish initial registry");
        assert_eq!(
            load_registry(storage.path()).expect("load registry").len(),
            0
        );
        assert!(
            read_registry_bytes(std::io::Cursor::new(vec![0; 4 * 1024 * 1024 + 1]))
                .is_err()
        );
        assert!(publish_registry_cache(&cache, b"not json").is_err());
        assert!(publish_registry_cache(&cache, br#"{}"#).is_err());
        assert_eq!(fs::read(&cache).expect("retain valid registry"), original);

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = storage.path().join("outside");
            fs::write(&outside, b"untouched").expect("outside sentinel");
            symlink(&outside, cache.with_extension("tmp"))
                .expect("old predictable staging pathname");
            publish_registry_cache(&cache, br#"{"agents":[]}"#)
                .expect("replace registry without following staging symlink");
            assert_eq!(fs::read(outside).expect("outside sentinel"), b"untouched");
            let linked = storage.path().join("registry/linked.json");
            symlink(&cache, &linked).expect("linked cache");
            assert!(open_registry_cache(&linked).is_err());
        }
    }

    #[test]
    fn catalog_is_curated_with_pi_cli_prerequisite() {
        let storage = tempfile::tempdir().expect("temporary ACP catalog");
        let registry = [RegistryAgent {
            id: "unapproved-acp-agent".to_string(),
            version: "1.0.0".to_string(),
            adapter: None,
        }];
        let adapters = curated_adapters(&registry, storage.path(), storage.path())
            .expect("curated catalog");
        assert_eq!(
            adapters
                .iter()
                .map(|entry| entry.adapter.id.as_str())
                .collect::<Vec<_>>(),
            vec!["pi-acp", "codex-acp", "claude-acp"]
        );
        assert_eq!(
            adapters
                .iter()
                .map(|entry| {
                    match entry.registry.as_ref().expect("fallback package") {
                        RegistryAdapter::Npx(package) => package.package.as_str(),
                        RegistryAdapter::Binary(_) => panic!("fallback must be npx"),
                    }
                })
                .collect::<Vec<_>>(),
            vec![
                "pi-acp@0.0.34",
                "@agentclientprotocol/codex-acp@2.2.2",
                "@agentclientprotocol/claude-agent-acp@0.94.0",
            ]
        );
        assert_eq!(
            adapters
                .iter()
                .find(|entry| entry.adapter.id == "pi-acp")
                .and_then(|entry| entry.adapter.prerequisite_note.as_deref()),
            Some(
                "Requires the Pi CLI v0.81+ on PATH; AHEAD installs only the ACP bridge."
            )
        );
        assert!(
            adapters
                .iter()
                .filter(|entry| entry.adapter.id != "pi-acp")
                .all(|entry| entry.adapter.prerequisite_note.is_none())
        );
        assert!(adapters.iter().all(|entry| !entry.adapter.installed));
        assert!(
            !external_acp_adapter_is_installed_at("claude-acp", storage.path())
                .expect("check local install marker")
        );
        assert!(
            external_acp_adapter_is_installed_at("custom", storage.path()).is_err()
        );

        set_external_acp_adapter_installed_at("claude-acp", true, storage.path())
            .expect("install supported agent");
        assert!(
            external_acp_adapter_is_installed_at("claude-acp", storage.path())
                .expect("read local install marker")
        );
        let adapters = curated_adapters(&[], storage.path(), storage.path())
            .expect("updated catalog");
        assert!(
            adapters
                .iter()
                .find(|entry| entry.adapter.id == "claude-acp")
                .expect("Claude Code catalog entry")
                .adapter
                .installed
        );

        set_external_acp_adapter_installed_at("claude-acp", false, storage.path())
            .expect("remove supported agent");
        assert!(
            !external_acp_adapter_is_installed_at("claude-acp", storage.path())
                .expect("read removed install marker")
        );
        assert!(
            curated_adapters(&[], storage.path(), storage.path())
                .expect("catalog after removal")
                .iter()
                .all(|entry| !entry.adapter.installed)
        );
        assert!(
            set_external_acp_adapter_installed_at("custom", true, storage.path())
                .is_err()
        );
    }

    #[test]
    fn supported_agent_cannot_launch_before_install() {
        let temporary = tempfile::tempdir().expect("temporary ACP directories");
        let package_cache = temporary.path().join("cache");
        let registry_cache = package_cache.join("registry");
        fs::create_dir_all(&registry_cache).expect("create registry cache");
        fs::write(registry_cache.join("registry.json"), r#"{"agents":[]}"#)
            .expect("write fresh registry cache");

        let error = external_agent_config_at(
            Some("pi-acp"),
            temporary.path().to_path_buf(),
            &package_cache,
            &temporary.path().join("user/.ahead/agents/external-acp"),
        )
        .expect_err("Pi requires explicit installation");
        assert!(error.to_string().contains("not installed"));
    }

    #[test]
    fn pi_cli_prerequisite_requires_version_081_or_newer() {
        let temporary = tempfile::tempdir().expect("temporary Pi CLI path");
        let missing =
            ensure_pi_cli_command(Command::new(temporary.path().join("pi")))
                .expect_err("missing Pi CLI should fail with install guidance");
        assert!(missing.to_string().contains("npm install -g"));
    }

    #[test]
    fn pi_cli_version_check_enforces_minimum_and_rejects_invalid_output() {
        ensure_pi_cli_version("0.81.0")
            .expect("minimum supported version should pass");
        ensure_pi_cli_version("v0.81.1").expect("v-prefixed version should pass");
        assert!(
            ensure_pi_cli_version("0.80.9")
                .expect_err("older Pi CLI should be rejected")
                .to_string()
                .contains("v0.80.9 is too old")
        );
        assert!(
            ensure_pi_cli_version("Pi CLI")
                .expect_err("unparseable version should be rejected")
                .to_string()
                .contains("unrecognized version")
        );
    }

    #[test]
    fn acp_install_uses_an_ahead_owned_npm_cache() {
        let temporary = tempfile::tempdir().expect("temporary ACP storage");
        let staging = temporary.path().join("staging");
        let npm_cache = temporary.path().join("registry/npm-cache");
        let command =
            npx_install_command(&staging, temporary.path(), "pi-acp@0.0.34");
        assert_eq!(
            command.get_program(),
            if cfg!(windows) { "npm.cmd" } else { "npm" }
        );
        let arguments = command.get_args().collect::<Vec<_>>();
        let cache_flag = arguments
            .iter()
            .position(|argument| *argument == std::ffi::OsStr::new("--cache"))
            .expect("npm cache option");

        assert_eq!(arguments.get(cache_flag + 1), Some(&npm_cache.as_os_str()));
        assert_eq!(
            arguments.last(),
            Some(&std::ffi::OsStr::new("pi-acp@0.0.34"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn npx_command_resolves_the_windows_npm_shim() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let directory = temporary.path().join("npm shim & tools");
        fs::create_dir(&directory)?;
        fs::write(
            directory.join("npm.cmd"),
            "@echo off\r\necho AHEAD_NPM_SHIM_OK\r\n",
        )?;
        let output = npm_command()
            .env("PATH", &directory)
            .arg("--version")
            .output()?;
        ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout)?.trim(),
            "AHEAD_NPM_SHIM_OK"
        );
        Ok(())
    }

    #[test]
    fn config_defaults_persist_per_supported_agent() {
        let temporary = tempfile::tempdir().expect("temporary ACP settings");
        let user_agents_dir = temporary.path().join(".ahead/agents/external-acp");
        let pi_path = default_config_options_path(&user_agents_dir, "pi-acp")
            .expect("Pi settings path");
        let codex_path = default_config_options_path(&user_agents_dir, "codex-acp")
            .expect("Codex settings path");

        persist_default_config_option(
            &pi_path,
            "model",
            &AgentConfigOptionValue::Select("provider/model-x".into()),
        )
        .expect("save Pi model default");
        let defaults = persist_default_config_option(
            &pi_path,
            "verbose",
            &AgentConfigOptionValue::Boolean(true),
        )
        .expect("save Pi boolean default");

        assert_eq!(
            load_default_config_options(&pi_path).expect("reload Pi settings"),
            defaults
        );
        assert_eq!(
            defaults.get("model"),
            Some(&AgentConfigOptionValue::Select("provider/model-x".into()))
        );
        assert_eq!(
            defaults.get("verbose"),
            Some(&AgentConfigOptionValue::Boolean(true))
        );
        assert!(
            load_default_config_options(&codex_path)
                .expect("Codex defaults")
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn config_defaults_publication_does_not_follow_predictable_temp_symlinks() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().expect("temporary ACP settings");
        let defaults_path = temporary.path().join("pi-acp.config-options.json");
        let outside = temporary.path().join("outside");
        let predictable_temp = defaults_path.with_extension("tmp");
        fs::write(&outside, b"untouched").expect("outside sentinel");
        symlink(&outside, &predictable_temp).expect("predictable temp symlink");

        persist_default_config_option(
            &defaults_path,
            "verbose",
            &AgentConfigOptionValue::Boolean(true),
        )
        .expect("publish config defaults safely");

        assert_eq!(fs::read(&outside).expect("outside sentinel"), b"untouched");
        assert_eq!(
            load_default_config_options(&defaults_path)
                .expect("published defaults")
                .get("verbose"),
            Some(&AgentConfigOptionValue::Boolean(true))
        );
    }

    #[test]
    fn parses_acp_registry_npx_distribution() {
        let index: RegistryIndex = serde_json::from_str(
            r#"
            {
              "version": "1.0.0",
              "agents": [{
                "id": "example-acp",
                "name": "Example",
                "version": "1.2.3",
                "distribution": {
                  "npx": {
                    "package": "example-acp@1.2.3",
                    "args": ["--acp"],
                    "env": {"EXAMPLE_NO_UPDATE": "1"}
                  }
                }
              }]
            }
            "#,
        )
        .unwrap();
        let entry = index.agents.into_iter().next().unwrap();
        let npx = entry.distribution.npx.unwrap();
        assert_eq!(entry.id, "example-acp");
        assert_eq!(entry.version, "1.2.3");
        assert_eq!(npx.package, "example-acp@1.2.3");
        assert_eq!(npx.args, ["--acp"]);
        assert_eq!(
            npx.env.get("EXAMPLE_NO_UPDATE").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn registry_prefers_a_checksummed_current_platform_binary() -> Result<()> {
        let storage = tempfile::tempdir()?;
        let platform = current_platform_key().context("supported test platform")?;
        let mut binary_targets = serde_json::Map::new();
        binary_targets.insert(
            platform.to_string(),
            serde_json::json!({
                "archive": "https://example.test/codex.zip",
                "cmd": "codex.exe",
                "sha256": "ab".repeat(32),
                "args": ["--acp"]
            }),
        );
        let body = serde_json::to_vec(&serde_json::json!({
            "version": "1.0.0",
            "agents": [{
                "id": "codex-acp",
                "name": "Codex",
                "version": "3.0.0",
                "distribution": {
                    "binary": binary_targets,
                    "npx": { "package": "codex-acp@3.0.0" }
                }
            }]
        }))?;
        publish_registry_cache(
            &storage.path().join("registry/registry.json"),
            &body,
        )?;

        let agents = load_registry(storage.path())?;
        let agent = agents.first().context("parsed Codex registry entry")?;
        assert_eq!(agent.version, "3.0.0");
        assert!(matches!(
            agent.adapter.as_ref(),
            Some(RegistryAdapter::Binary(RegistryBinaryAdapter {
                target,
                ..
            })) if target.cmd == "codex.exe"
        ));
        Ok(())
    }

    #[test]
    fn binary_archives_require_https_supported_formats_and_safe_commands() {
        assert_eq!(
            binary_archive_kind("https://example.test/agent.tgz?token=ignored")
                .expect("tgz kind"),
            BinaryArchiveKind::TarGzip
        );
        assert_eq!(
            binary_archive_kind("https://example.test/agent.tar.bz2")
                .expect("bzip2 tar kind"),
            BinaryArchiveKind::TarBzip2
        );
        assert_eq!(
            binary_archive_kind("https://example.test/agent")
                .expect("raw binary kind"),
            BinaryArchiveKind::Raw
        );
        assert!(binary_archive_kind("http://example.test/agent.zip").is_err());
        assert!(binary_archive_kind("https://example.test/agent.tar.xz").is_err());
        assert!(relative_binary_command("./bin/agent").is_ok());
        assert!(relative_binary_command("amp-acp.exe").is_ok());
        assert_eq!(
            relative_binary_command("bin\\agent.exe")
                .expect("safe Windows relative path"),
            Some(PathBuf::from("bin/agent.exe"))
        );
        assert!(relative_binary_command("node").is_ok());
        for command in [
            "../agent",
            "/tmp/agent",
            "./../agent",
            "./C:/agent",
            "C:\\agent.exe",
            "./bin//agent",
        ] {
            assert!(relative_binary_command(command).is_err(), "{command}");
        }
        assert!(validated_archive_path(Path::new("bin/agent")).is_ok());
        for path in ["../agent", "/tmp/agent", "a/../../agent"] {
            assert!(validated_archive_path(Path::new(path)).is_err(), "{path}");
        }
        assert!(valid_sha256(&"aB".repeat(32)));
        assert!(!valid_sha256("not-a-checksum"));
        let checksum = "ab".repeat(32);
        verify_archive_sha256(&checksum.to_ascii_uppercase(), &checksum)
            .expect("checksum comparison ignores hex case");
        assert!(verify_archive_sha256(&"cd".repeat(32), &checksum).is_err());
        assert!(verify_archive_sha256("not-a-checksum", &checksum).is_err());
    }

    #[test]
    fn tar_extraction_rejects_symlinks() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let safe_destination = temporary.path().join("safe");
        fs::create_dir(&safe_destination)?;
        let mut safe_archive = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut safe_archive);
            let contents = b"agent";
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, "bin/agent", &contents[..])?;
            builder.finish()?;
        }
        extract_tar(io::Cursor::new(safe_archive), &safe_destination)?;
        assert_eq!(
            fs::read(safe_destination.join("bin/agent"))?.as_slice(),
            b"agent"
        );

        let mut contents = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut contents);
            let mut header = tar::Header::new_gnu();
            header.set_path("link")?;
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            header.set_link_name("../../outside")?;
            header.set_cksum();
            builder.append(&header, io::empty())?;
            builder.finish()?;
        }
        assert!(extract_tar(io::Cursor::new(contents), temporary.path()).is_err());
        assert!(!temporary.path().join("outside").exists());
        Ok(())
    }

    #[test]
    fn curated_catalog_reports_a_newer_registry_version() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let user_agents = temporary.path().join("user-agents");
        let cache = temporary.path().join("registry/npx/codex-acp");
        fs::create_dir_all(&cache)?;
        set_external_acp_adapter_installed_at("codex-acp", true, &user_agents)?;
        let installed = InstallProvenance {
            generation: uuid::Uuid::new_v4(),
            package_spec: "@agentclientprotocol/codex-acp@1.0.0".to_string(),
            package_json_sha256: "0".repeat(64),
            package_lock_sha256: "0".repeat(64),
        };
        fs::write(
            cache.join("installed.json"),
            serde_json::to_vec(&installed)?,
        )?;
        let registry = [RegistryAgent {
            id: "codex-acp".to_string(),
            version: "2.0.0".to_string(),
            adapter: Some(RegistryAdapter::Npx(RegistryNpxAdapter {
                package: "@agentclientprotocol/codex-acp@2.0.0".to_string(),
                version: "2.0.0".to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
            })),
        }];

        let catalog = curated_adapters(&registry, &user_agents, temporary.path())?;
        let codex = catalog
            .iter()
            .find(|entry| entry.adapter.id == "codex-acp")
            .context("curated Codex adapter")?;
        assert!(codex.adapter.installed);
        assert_eq!(codex.adapter.latest_version.as_deref(), Some("2.0.0"));
        assert_eq!(codex.adapter.installed_version.as_deref(), Some("1.0.0"));
        assert!(codex.adapter.update_available);
        Ok(())
    }

    #[test]
    fn binary_update_keeps_live_generation_until_its_lease_is_dropped() -> Result<()>
    {
        let temporary = tempfile::tempdir()?;
        let cache = temporary.path().join("binary-agent");
        let make_adapter = |version: &str| RegistryBinaryAdapter {
            version: version.to_string(),
            target: RegistryBinaryTarget {
                archive: format!("https://example.test/agent-{version}.zip"),
                cmd: "./bin/agent".to_string(),
                args: vec!["--acp".to_string()],
                sha256: Some("ab".repeat(32)),
                env: BTreeMap::new(),
            },
        };
        let previous =
            ensure_binary_agent(&cache, &make_adapter("1.0.0"), |staging| {
                let executable = staging.join("bin/agent");
                fs::create_dir_all(executable.parent().context("agent parent")?)?;
                fs::write(&executable, "version one")?;
                Ok(())
            })?;
        let previous_command =
            previous.command.clone().context("installed binary")?;

        let current =
            ensure_binary_agent(&cache, &make_adapter("2.0.0"), |staging| {
                let executable = staging.join("bin/agent");
                fs::create_dir_all(executable.parent().context("agent parent")?)?;
                fs::write(&executable, "version two")?;
                Ok(())
            })?;
        assert!(
            previous_command.is_file(),
            "old leased process remains usable"
        );
        assert_eq!(
            fs::read_to_string(
                current.command.as_ref().context("current command")?
            )?,
            "version two"
        );
        drop(previous);

        let cached = ensure_binary_agent(&cache, &make_adapter("2.0.0"), |_| {
            bail!("matching binary version must not reinstall")
        })?;
        assert!(
            !previous_command.exists(),
            "unleased old generation is pruned"
        );
        assert_eq!(cached.command, current.command);
        Ok(())
    }

    #[test]
    fn resolves_the_package_bin_without_accepting_a_path_escape() {
        let package: serde_json::Value = serde_json::json!({
            "bin": {"example-acp": "bin/server.js"}
        });
        assert_eq!(
            package_bin(&package, "example-acp").unwrap(),
            "bin/server.js"
        );
    }

    #[test]
    fn registry_ids_cannot_escape_the_adapter_directory() {
        assert_eq!(sanitize_path_component("pi-acp"), "pi-acp");
        assert_eq!(sanitize_path_component("../outside"), "..-outside");
        assert_eq!(sanitize_path_component(".."), "unknown");
    }

    #[test]
    fn adapter_install_lock_is_released_without_deleting_its_file() {
        let temporary = tempfile::tempdir().expect("temporary adapter cache");
        let path = temporary.path().join("install.lock");
        let held = InstallLock::acquire(path.clone()).expect("first lock");
        let inherited = held._file.try_clone().expect("inherited lock handle");
        assert!(
            InstallLock::try_acquire(path.clone())
                .expect("second lock probe")
                .is_none()
        );
        drop(held);
        assert!(path.is_file());
        assert!(
            InstallLock::try_acquire(path)
                .expect("lock after release")
                .is_some()
        );
        drop(inherited);
    }

    #[test]
    fn npx_updates_preserve_previous_launches_and_failed_installs() -> Result<()> {
        use std::sync::mpsc;

        let temporary = tempfile::tempdir()?;
        let storage = temporary.path().join("cache");
        let cache = storage.join("registry/npx/pi-acp");
        let old_script = ensure_npx_package(&cache, "pi-acp@1.0.0", |staging| {
            write_npx_fixture(staging, "pi-acp@1.0.0", "old")
        })?;
        let old_manifest = fs::read(cache.join("installed.json"))?;

        thread::scope(|scope| -> Result<()> {
            let cache = &cache;
            let (started, updating) = mpsc::channel();
            let (release, resume) = mpsc::channel();
            let (resolved, launch) = mpsc::channel();
            let updater = scope.spawn(move || {
                ensure_npx_package(cache, "pi-acp@2.0.0", |staging| {
                    fs::write(staging.join("partial-package"), "unfinished")?;
                    started.send(())?;
                    resume.recv_timeout(Duration::from_secs(10))?;
                    bail!("injected npm failure")
                })
            });
            updating.recv_timeout(Duration::from_secs(5))?;
            let reader = scope.spawn(move || {
                resolved.send(ensure_npx_package(cache, "pi-acp@1.0.0", |_| {
                    bail!("cached launch must not run npm")
                }))
            });
            assert!(matches!(
                launch.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            release.send(())?;
            let update = updater.join().expect("installer thread");
            let launched = launch.recv_timeout(Duration::from_secs(5));
            reader.join().expect("launch thread")?;
            assert_eq!(launched??.script, old_script.script);
            assert!(update.is_err_and(|error| {
                error.to_string().contains("injected npm failure")
            }));
            Ok(())
        })?;

        assert_eq!(fs::read(cache.join("installed.json"))?, old_manifest);
        assert_eq!(fs::read_to_string(&old_script.script)?, "old");
        assert!(fs::read_dir(&cache)?.all(|entry| {
            entry.is_ok_and(|entry| {
                !entry.file_name().to_string_lossy().starts_with(".install-")
            })
        }));

        let new_script = ensure_npx_package(&cache, "pi-acp@2.0.0", |staging| {
            write_npx_fixture(staging, "pi-acp@2.0.0", "new")
        })?;
        assert_ne!(new_script.script, old_script.script);
        assert_eq!(fs::read_to_string(&new_script.script)?, "new");
        assert_eq!(fs::read_to_string(&old_script.script)?, "old");
        let registry = RegistryNpxAdapter {
            package: "pi-acp@2.0.0".to_string(),
            version: "2.0.0".to_string(),
            args: vec!["--acp".to_string()],
            env: BTreeMap::from([("EXAMPLE_NO_UPDATE".into(), "1".into())]),
        };
        // Exercise the real resolver without invoking npm or changing PATH.
        let config = install_npx_adapter(
            "pi-acp",
            &registry,
            temporary.path().to_path_buf(),
            &storage,
        )?;
        assert_eq!(config.cwd, temporary.path());
        assert_eq!(config.args.get(1), Some(&"--acp".to_string()));
        assert_eq!(config.env.get("EXAMPLE_NO_UPDATE"), Some(&"1".to_string()));
        assert_eq!(
            fs::read_to_string(config.args.first().context("adapter script")?)?,
            "new"
        );
        Ok(())
    }

    #[test]
    fn missing_npx_cache_is_repaired_without_losing_previous_launches() -> Result<()>
    {
        let temporary = tempfile::tempdir()?;
        for missing in ["generation", "package", "script", "lockfile"] {
            let cache = temporary.path().join(missing);
            let previous = ensure_npx_package(&cache, "pi-acp@1.0.0", |staging| {
                write_npx_fixture(staging, "pi-acp@1.0.0", "previous")
            })?;
            let current = ensure_npx_package(&cache, "pi-acp@2.0.0", |staging| {
                write_npx_fixture(staging, "pi-acp@2.0.0", "current")
            })?;
            let generation = installed_npx_executable(&cache, "pi-acp@2.0.0")?
                .context("current adapter generation")?
                .generation;
            let old_script = current.script.clone();
            drop(current);
            match missing {
                "generation" => fs::remove_dir_all(&generation)?,
                "package" => fs::remove_file(
                    generation.join("node_modules/pi-acp/package.json"),
                )?,
                "script" => fs::remove_file(&old_script)?,
                "lockfile" => fs::remove_file(generation.join("package-lock.json"))?,
                _ => unreachable!("cache fixture"),
            }
            let manifest = fs::read(cache.join("installed.json"))?;
            let error = ensure_npx_package(&cache, "pi-acp@2.0.0", |_| {
                bail!("injected repair failure")
            })
            .err()
            .context("repair must fail")?;
            assert!(
                error.to_string().contains("injected repair failure"),
                "{missing}: {error:#}"
            );
            assert_eq!(fs::read(cache.join("installed.json"))?, manifest);
            assert_eq!(fs::read_to_string(&previous.script)?, "previous");

            let repaired = ensure_npx_package(&cache, "pi-acp@2.0.0", |staging| {
                write_npx_fixture(staging, "pi-acp@2.0.0", "repaired")
            })?;
            assert_ne!(repaired.script, old_script);
            assert_eq!(fs::read_to_string(&repaired.script)?, "repaired");
            assert_eq!(fs::read_to_string(&previous.script)?, "previous");
            let cached = ensure_npx_package(&cache, "pi-acp@2.0.0", |_| {
                bail!("repaired adapter must not reinstall")
            })?;
            assert_eq!(cached.script, repaired.script);
        }
        Ok(())
    }

    #[test]
    fn invalid_npx_cache_does_not_trigger_a_repair_install() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        for invalid in ["manifest", "package", "escape"] {
            let cache = temporary.path().join(invalid);
            let installed = ensure_npx_package(&cache, "pi-acp@1.0.0", |staging| {
                write_npx_fixture(staging, "pi-acp@1.0.0", "retained")
            })?;
            let package = installed.script.parent().context("adapter package")?;
            match invalid {
                "manifest" => {
                    fs::write(cache.join("installed.json"), "invalid JSON")?
                }
                "package" => {
                    fs::remove_file(package.join("package.json"))?;
                    fs::create_dir(package.join("package.json"))?;
                }
                "escape" => {
                    fs::write(
                        package.parent().context("node_modules")?.join("outside.js"),
                        "outside",
                    )?;
                    fs::write(
                        package.join("package.json"),
                        r#"{"bin":"../outside.js"}"#,
                    )?;
                }
                _ => unreachable!("cache fixture"),
            }
            let manifest = fs::read(cache.join("installed.json"))?;
            let attempted_repair = std::cell::Cell::new(false);
            let result = ensure_npx_package(&cache, "pi-acp@1.0.0", |_| {
                attempted_repair.set(true);
                bail!("invalid cache must not run npm")
            });
            assert!(result.is_err(), "{invalid} must fail closed");
            assert!(!attempted_repair.get(), "{invalid} must not reinstall");
            assert_eq!(fs::read(cache.join("installed.json"))?, manifest);
            assert_eq!(fs::read_to_string(&installed.script)?, "retained");
        }
        Ok(())
    }

    #[test]
    fn concurrent_npx_installs_publish_one_generation() -> Result<()> {
        use std::sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
        };

        let temporary = tempfile::tempdir()?;
        let cache = temporary.path().join("codex-acp");
        let start = Barrier::new(2);
        let installs = AtomicUsize::new(0);
        thread::scope(|scope| -> Result<()> {
            let install = || {
                start.wait();
                ensure_npx_package(
                    &cache,
                    "@agentclientprotocol/codex-acp@1.0.0",
                    |staging| {
                        installs.fetch_add(1, Ordering::SeqCst);
                        write_npx_fixture(
                            staging,
                            "@agentclientprotocol/codex-acp@1.0.0",
                            "codex",
                        )
                    },
                )
            };
            let first = scope.spawn(install);
            let second = scope.spawn(install);
            assert_eq!(
                first.join().expect("first install")?.script,
                second.join().expect("second install")?.script
            );
            Ok(())
        })?;
        assert_eq!(installs.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn cache_cleanup_waits_for_resolved_commands_and_live_children() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let cache = temporary.path().join("pi-acp");
        let old = ensure_npx_package(&cache, "pi-acp@1.0.0", |staging| {
            write_npx_fixture(staging, "pi-acp@1.0.0", "old")
        })?;
        let old_script = old.script.clone();
        let abandoned = cache.join(".install-abandoned");
        fs::create_dir(&abandoned)?;
        fs::write(abandoned.join("partial"), "unfinished")?;

        let mut config =
            HarnessClientConfig::new("sh", temporary.path().to_path_buf());
        config.args = vec!["-c".into(), "exec sleep 10".into()];
        config.generation_lease = old.lease.clone();
        let child =
            HarnessClient::spawn_without_editor_mcp(&config, Arc::new(|_| {}))?;
        drop(config);
        drop(old);

        let new = ensure_npx_package(&cache, "pi-acp@2.0.0", |staging| {
            write_npx_fixture(staging, "pi-acp@2.0.0", "new")
        })?;
        assert!(!abandoned.exists());
        assert!(
            old_script.is_file(),
            "live child must retain old generation"
        );

        child.shutdown();
        drop(child);
        let cached = ensure_npx_package(&cache, "pi-acp@2.0.0", |_| {
            bail!("cached adapter must not reinstall")
        })?;
        assert_eq!(cached.script, new.script);
        assert!(!old_script.exists(), "unused generation should be pruned");

        let prior_process_script = cached.script.clone();
        let canonical_cache = fs::canonicalize(&cache)?;
        let prior_process_generation = prior_process_script
            .ancestors()
            .find(|path| path.parent() == Some(canonical_cache.as_path()))
            .context("find ACP generation")?;
        fs::write(
            prior_process_generation.join(".ahead-lease"),
            uuid::Uuid::new_v4().to_string(),
        )?;
        drop(cached);
        drop(new);
        ensure_npx_package(&cache, "pi-acp@3.0.0", |staging| {
            write_npx_fixture(staging, "pi-acp@3.0.0", "newer")
        })?;
        assert!(
            prior_process_script.is_file(),
            "generations from a prior app process must be retained"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn inherited_agent_lease_survives_parent_handle_release() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let generation = temporary.path().join("generation");
        fs::create_dir(&generation)?;
        let lease_path = generation.join(".ahead-lease");
        fs::write(&lease_path, generation_lease_marker())?;
        let lease = lease_generation(&generation)?.context("generation lease")?;

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec sleep 10"]);
        crate::acp_client::retain_generation_lease_in_child(&mut command, &lease);
        let mut child = command.spawn()?;
        drop(lease);

        let probe = File::open(lease_path)?;
        assert!(
            !FileExt::try_lock_exclusive(&probe)?,
            "the agent child must retain the generation lease"
        );
        child.kill()?;
        child.wait()?;
        assert!(
            FileExt::try_lock_exclusive(&probe)?,
            "the generation becomes reclaimable after the child exits"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_reclaims_unleased_generation_from_another_app_instance() -> Result<()>
    {
        let temporary = tempfile::tempdir()?;
        let cache = temporary.path().join("pi-acp");
        let old = ensure_npx_package(&cache, "pi-acp@1.0.0", |staging| {
            write_npx_fixture(staging, "pi-acp@1.0.0", "old")
        })?;
        let old_script = old.script.clone();
        let canonical_cache = fs::canonicalize(&cache)?;
        let old_generation = old_script
            .ancestors()
            .find(|path| path.parent() == Some(canonical_cache.as_path()))
            .context("find old ACP generation")?;
        fs::write(
            old_generation.join(".ahead-lease"),
            format!(
                "{CROSS_PROCESS_GENERATION_LEASE_MARKER}{}",
                uuid::Uuid::new_v4()
            ),
        )?;
        drop(old);

        ensure_npx_package(&cache, "pi-acp@2.0.0", |staging| {
            write_npx_fixture(staging, "pi-acp@2.0.0", "new")
        })?;
        assert!(
            !old_script.exists(),
            "unleased generations from a prior app instance should be reclaimed"
        );
        Ok(())
    }

    #[test]
    fn invalid_npx_executable_cannot_replace_a_valid_install() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let cache = temporary.path().join("pi-acp");
        let old_script = ensure_npx_package(&cache, "pi-acp@1.0.0", |staging| {
            write_npx_fixture(staging, "pi-acp@1.0.0", "old")
        })?;
        let old_manifest = fs::read(cache.join("installed.json"))?;
        for invalid_bin in ["../outside.js", ".", "missing.js"] {
            let result = ensure_npx_package(&cache, "pi-acp@2.0.0", |staging| {
                write_npx_fixture(staging, "pi-acp@2.0.0", "new")?;
                fs::write(staging.join("node_modules/outside.js"), "outside")?;
                fs::write(
                    staging.join("node_modules/pi-acp/package.json"),
                    serde_json::to_vec(&serde_json::json!({"bin": invalid_bin}))?,
                )?;
                Ok(())
            });
            assert!(result.is_err(), "invalid executable: {invalid_bin}");
            assert_eq!(fs::read(cache.join("installed.json"))?, old_manifest);
            assert_eq!(fs::read_to_string(&old_script.script)?, "old");
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires local Node/npm; uses only a disposable local package offline"]
    fn npx_generations_launch_locally_packed_packages_offline() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let package = temporary.path().join("fixture");
        fs::create_dir(&package)?;
        fs::write(
            package.join("agent.js"),
            "process.stdout.write(require('./message.json').text);",
        )?;
        let user_config = temporary.path().join("user.npmrc");
        let global_config = temporary.path().join("global.npmrc");
        fs::write(&user_config, "")?;
        fs::write(&global_config, "")?;
        let npm = || {
            let mut command = npm_command();
            command
                .current_dir(temporary.path())
                .args(["--offline", "--ignore-scripts", "--no-audit", "--no-fund"])
                .arg("--cache")
                .arg(temporary.path().join("npm-cache"))
                .arg("--userconfig")
                .arg(&user_config)
                .arg("--globalconfig")
                .arg(&global_config);
            command
        };
        let cache = temporary.path().join("adapter-cache");
        let mut scripts = Vec::new();
        for version in ["1.0.0", "2.0.0"] {
            fs::write(
                package.join("package.json"),
                serde_json::to_vec(&serde_json::json!({
                    "name": "ahead-acp-offline-test",
                    "version": version,
                    "bin": "agent.js",
                }))?,
            )?;
            fs::write(
                package.join("message.json"),
                serde_json::to_vec(&serde_json::json!({"text": version}))?,
            )?;
            let packed = npm()
                .args(["pack", "--json", "--pack-destination"])
                .arg(temporary.path())
                .arg(&package)
                .output()?;
            anyhow::ensure!(
                packed.status.success(),
                "offline npm pack: {}",
                String::from_utf8_lossy(&packed.stderr)
            );
            let packed: Vec<serde_json::Value> =
                serde_json::from_slice(&packed.stdout)?;
            let filename = packed
                .first()
                .and_then(|package| package.get("filename"))
                .and_then(serde_json::Value::as_str)
                .context("npm pack returned no archive filename")?;
            let script = ensure_npx_package(
                &cache,
                &format!("ahead-acp-offline-test@{version}"),
                |staging| {
                    let installed = npm()
                        .args(["install", "--prefix"])
                        .arg(staging)
                        .args(["--save-exact", "--package-lock=true", "--"])
                        .arg(temporary.path().join(filename))
                        .output()?;
                    anyhow::ensure!(
                        installed.status.success(),
                        "offline npm install: {}",
                        String::from_utf8_lossy(&installed.stderr)
                    );
                    Ok(())
                },
            )?;
            scripts.push((script, version, temporary.path().join(filename)));
        }
        for (script, version, _) in &scripts {
            let output = Command::new("node").arg(&script.script).output()?;
            assert!(output.status.success());
            assert_eq!(String::from_utf8(output.stdout)?, *version);
        }
        let (current, _, archive) = scripts.last().context("current npm fixture")?;
        fs::remove_file(&current.script)?;
        let repaired =
            ensure_npx_package(&cache, "ahead-acp-offline-test@2.0.0", |staging| {
                let installed = npm()
                    .args(["install", "--prefix"])
                    .arg(staging)
                    .args(["--save-exact", "--package-lock=true", "--"])
                    .arg(archive)
                    .output()?;
                ensure!(
                    installed.status.success(),
                    "offline npm repair: {}",
                    String::from_utf8_lossy(&installed.stderr)
                );
                Ok(())
            })?;
        assert_ne!(repaired.script, current.script);
        let output = Command::new("node").arg(&repaired.script).output()?;
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout)?, "2.0.0");
        let previous = &scripts.first().context("previous npm fixture")?.0;
        let output = Command::new("node").arg(&previous.script).output()?;
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout)?, "1.0.0");
        Ok(())
    }

    fn write_npx_fixture(
        installation: &Path,
        package_spec: &str,
        script: &str,
    ) -> Result<()> {
        let package_dir = installation
            .join("node_modules")
            .join(package_name(package_spec));
        fs::create_dir_all(&package_dir)?;
        fs::write(
            package_dir.join("package.json"),
            serde_json::to_vec(&serde_json::json!({"bin": "agent.js"}))?,
        )?;
        fs::write(package_dir.join("agent.js"), script)?;
        fs::write(installation.join("package-lock.json"), package_spec)?;
        Ok(())
    }

    #[test]
    #[ignore = "requires public npm and an installed Pi CLI; no provider access"]
    fn pi_registry_adapter_round_trips_offline_session() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let cwd = temporary.path().join("project");
        fs::create_dir(&cwd)?;
        let storage = temporary.path().join("external-agents");
        let user_agents_dir =
            temporary.path().join("user/.ahead/agents/external-acp");
        set_external_acp_adapter_installed_at("pi-acp", true, &user_agents_dir)?;
        let mut config = external_agent_config_at(
            Some("pi-acp"),
            cwd.clone(),
            &storage,
            &user_agents_dir,
        )?;
        let pi_home = temporary.path().join("pi-home");
        fs::create_dir_all(&pi_home)?;
        fs::write(
            pi_home.join("models.json"),
            serde_json::to_vec(&serde_json::json!({
                "providers": {
                    "ahead-offline": {
                        "baseUrl": "http://127.0.0.1:1/v1",
                        "api": "openai-completions",
                        "apiKey": "not-a-real-key",
                        "models": [
                            { "id": "ahead-offline-one", "name": "AHEAD Offline One" },
                            { "id": "ahead-offline-two", "name": "AHEAD Offline Two" }
                        ]
                    }
                }
            }))?,
        )?;
        let pi_home = pi_home.to_string_lossy().into_owned();
        config
            .env
            .insert("PI_CODING_AGENT_DIR".to_string(), pi_home.clone());
        config.env.insert("PI_OFFLINE".to_string(), "1".to_string());

        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        let client = HarnessClient::spawn_without_editor_mcp(
            &config,
            Arc::new(move |event| captured.lock().push(event)),
        )?;
        let result: Result<(String, String)> = (|| {
            let initialized = client.initialize()?;
            ensure!(
                initialized.is_object(),
                "Pi ACP initialize returned no object"
            );
            let session_id = client.new_session(&cwd, "agent", None, None)?;
            ensure!(
                !session_id.is_empty(),
                "Pi ACP returned an empty session ID"
            );
            let model = events
                .lock()
                .iter()
                .rev()
                .find_map(|event| match event {
                    HarnessEvent::ConfigOptions {
                        acp_session_id,
                        options,
                    } if acp_session_id == &session_id => options
                        .iter()
                        .find(|option| option.category.as_deref() == Some("model"))
                        .cloned(),
                    _ => None,
                })
                .context("Pi ACP did not publish a model config option")?;
            let ahead_rpc::ahead::AgentConfigOptionValue::Select(current) =
                &model.current_value
            else {
                bail!("Pi ACP model option is not a selection");
            };
            let next = model
                .choices
                .iter()
                .find(|choice| choice.value != *current)
                .context("Pi ACP advertised no alternate model")?
                .value
                .clone();
            client.set_config_option(
                &session_id,
                &model.id,
                &ahead_rpc::ahead::AgentConfigOptionValue::Select(next.clone()),
            )?;
            ensure!(
                events.lock().iter().rev().any(|event| matches!(
                    event,
                    HarnessEvent::ConfigOptions { acp_session_id, options }
                        if acp_session_id == &session_id
                            && options.iter().any(|option| option.id == model.id
                                && option.current_value == ahead_rpc::ahead::AgentConfigOptionValue::Select(next.clone()))
                )),
                "Pi ACP did not publish the selected model"
            );
            Ok((session_id, next))
        })();
        client.shutdown();
        let (session_id, selected_model) = result?;
        let mut restored_config = external_agent_config_at(
            Some("pi-acp"),
            cwd.clone(),
            &storage,
            &user_agents_dir,
        )?;
        restored_config
            .env
            .insert("PI_CODING_AGENT_DIR".to_string(), pi_home);
        restored_config
            .env
            .insert("PI_OFFLINE".to_string(), "1".to_string());
        let restored_events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&restored_events);
        let restored = HarnessClient::spawn_without_editor_mcp(
            &restored_config,
            Arc::new(move |event| captured.lock().push(event)),
        )?;
        let result = (|| {
            restored.initialize()?;
            restored.load_session(&session_id, &cwd, "agent", None, None)?;
            ensure!(
                restored_events.lock().iter().rev().any(|event| matches!(
                    event,
                    HarnessEvent::ConfigOptions { acp_session_id, options }
                        if acp_session_id == &session_id
                            && options.iter().any(|option|
                                option.category.as_deref() == Some("model")
                                    && option.current_value == ahead_rpc::ahead::AgentConfigOptionValue::Select(selected_model.clone()))
                )),
                "Pi ACP did not restore the saved model selection"
            );
            Ok(())
        })();
        restored.shutdown();
        result
    }

    #[test]
    #[ignore = "requires the public ACP registry, npm and the Pi CLI; uses synthetic keys and no model turns"]
    fn curated_acp_session_model_options_offline() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let registry_storage = temporary.path().join("live-registry");
        let registry_path = registry_storage.join("registry/registry.json");
        refresh_registry_in_background(registry_path.clone());
        let deadline = std::time::Instant::now() + Duration::from_secs(35);
        let registry_body = loop {
            match open_registry_cache(&registry_path) {
                Ok(file) => break read_registry_bytes(file)?,
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound
                        && std::time::Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(error) => {
                    return Err(error)
                        .context("waiting for the live ACP registry refresh");
                }
            }
        };
        let registry: RegistryIndex = serde_json::from_slice(&registry_body)
            .context("parsing ACP registry")?;
        for supported in SUPPORTED_AGENTS {
            let package = registry
                .agents
                .iter()
                .find(|entry| entry.id == supported.id)
                .and_then(|entry| entry.distribution.npx.as_ref())
                .map(|npx| &npx.package)
                .with_context(|| {
                    format!("{} has no live registry npm distribution", supported.id)
                })?;
            eprintln!(
                "{}: installing live registry package {package}",
                supported.display_name
            );
            let root = temporary.path().join(supported.id);
            let cwd = root.join("project");
            let storage = root.join("external-agents");
            let user_agents_dir = root.join("user/.ahead/agents/external-acp");
            let home = root.join("home");
            fs::create_dir_all(&cwd)?;
            fs::create_dir_all(&home)?;
            publish_registry_cache(
                &storage.join("registry/registry.json"),
                &registry_body,
            )?;
            set_external_acp_adapter_installed_at(
                supported.id,
                true,
                &user_agents_dir,
            )?;
            let mut config = external_agent_config_at(
                Some(supported.id),
                cwd.clone(),
                &storage,
                &user_agents_dir,
            )?;
            let provenance: InstallProvenance = serde_json::from_slice(&fs::read(
                storage
                    .join("registry/npx")
                    .join(sanitize_path_component(supported.id))
                    .join("installed.json"),
            )?)?;
            ensure!(
                provenance.package_spec == *package,
                "{} did not install its live registry package",
                supported.display_name
            );
            let home = home.to_string_lossy().into_owned();
            config.env.insert("HOME".to_string(), home.clone());
            match supported.id {
                "pi-acp" => {
                    let pi_home = PathBuf::from(&home).join(".pi/agent");
                    fs::create_dir_all(&pi_home)?;
                    fs::write(
                        pi_home.join("models.json"),
                        serde_json::to_vec(&serde_json::json!({
                            "providers": {
                                "ahead-offline": {
                                    "baseUrl": "http://127.0.0.1:1/v1",
                                    "api": "openai-completions",
                                    "apiKey": "not-a-real-key",
                                    "models": [
                                        { "id": "ahead-offline-one", "name": "AHEAD Offline One" },
                                        { "id": "ahead-offline-two", "name": "AHEAD Offline Two" }
                                    ]
                                }
                            }
                        }))?,
                    )?;
                    config.env.insert(
                        "PI_CODING_AGENT_DIR".to_string(),
                        pi_home.to_string_lossy().into_owned(),
                    );
                    config.env.insert("PI_OFFLINE".to_string(), "1".to_string());
                }
                "codex-acp" => {
                    let codex_home = PathBuf::from(&home).join(".codex");
                    fs::create_dir_all(&codex_home)?;
                    config.env.insert(
                        "CODEX_HOME".to_string(),
                        codex_home.to_string_lossy().into_owned(),
                    );
                }
                "claude-acp" => {
                    let claude_home = PathBuf::from(&home).join(".claude");
                    fs::create_dir_all(&claude_home)?;
                    config.env.insert(
                        "CLAUDE_CONFIG_DIR".to_string(),
                        claude_home.to_string_lossy().into_owned(),
                    );
                }
                _ => bail!("unsupported ACP test agent {}", supported.id),
            }

            for name in [
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "OPENAI_API_KEY",
                "OPENROUTER_API_KEY",
                "CODEX_API_KEY",
                "AWS_ACCESS_KEY_ID",
                "AWS_SECRET_ACCESS_KEY",
                "AWS_SESSION_TOKEN",
            ] {
                config.env.insert(name.to_string(), String::new());
            }
            if supported.id == "codex-acp" {
                config.env.insert(
                    "CODEX_API_KEY".to_string(),
                    "ahead-offline-invalid-key".to_string(),
                );
            }
            for name in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
                config
                    .env
                    .insert(name.to_string(), "http://127.0.0.1:1".to_string());
                config.env.insert(
                    name.to_ascii_lowercase(),
                    "http://127.0.0.1:1".to_string(),
                );
            }
            config
                .env
                .insert("NO_PROXY".to_string(), "127.0.0.1,localhost".to_string());
            config.env.insert("NO_BROWSER".to_string(), "1".to_string());
            config
                .env
                .insert("NODE_USE_ENV_PROXY".to_string(), "1".to_string());

            let events = Arc::new(Mutex::new(Vec::new()));
            let captured = Arc::clone(&events);
            let client = HarnessClient::spawn_without_editor_mcp(
                &config,
                Arc::new(move |event| captured.lock().push(event)),
            )?;
            let result = (|| {
                let initialized = client.initialize()?;
                ensure!(
                    initialized.is_object(),
                    "{} ACP initialize returned no object",
                    supported.display_name
                );
                eprintln!("{}: ACP initialize completed", supported.display_name);
                if supported.id == "codex-acp" {
                    let method = client
                        .auth_methods()
                        .into_iter()
                        .find(|method| {
                            method.method_type
                                == ahead_rpc::ahead::AgentAuthMethodType::Agent
                                && format!("{} {}", method.id, method.name)
                                    .to_ascii_lowercase()
                                    .contains("api")
                        })
                        .context(
                            "Codex ACP did not advertise API-key authentication",
                        )?;
                    client.authenticate(&method.id).context(
                        "Codex ACP synthetic API-key authentication failed; model options remain unverified",
                    )?;
                }
                let session_id =
                    client.new_session(&cwd, "agent", None, None).with_context(
                        || format!("{} ACP session/new", supported.display_name),
                    )?;
                let options = events
                    .lock()
                    .iter()
                    .rev()
                    .find_map(|event| match event {
                        HarnessEvent::ConfigOptions {
                            acp_session_id,
                            options,
                        } if acp_session_id == &session_id => Some(options.clone()),
                        _ => None,
                    })
                    .with_context(|| {
                        format!(
                            "{} ACP session/new returned no config options",
                            supported.display_name
                        )
                    })?;
                let model = options
                    .iter()
                    .find(|option| {
                        option.category.as_deref() == Some("model")
                            || option.id == "model"
                    })
                    .with_context(|| {
                        format!(
                            "{} ACP session/new returned no model option",
                            supported.display_name
                        )
                    })?;
                let current = match &model.current_value {
                    AgentConfigOptionValue::Select(value) => value,
                    AgentConfigOptionValue::Boolean(_) => {
                        bail!(
                            "{} ACP model option is not a selection",
                            supported.display_name
                        )
                    }
                };
                let next = model
                    .choices
                    .iter()
                    .find(|choice| choice.value != *current)
                    .with_context(|| {
                        format!(
                            "{} ACP advertised no alternate model",
                            supported.display_name
                        )
                    })?
                    .value
                    .clone();
                client.set_config_option(
                    &session_id,
                    &model.id,
                    &AgentConfigOptionValue::Select(next.clone()),
                )?;
                ensure!(
                    events.lock().iter().rev().any(|event| matches!(
                        event,
                        HarnessEvent::ConfigOptions { acp_session_id, options }
                            if acp_session_id == &session_id
                                && options.iter().any(|option| {
                                    option.id == model.id
                                        && option.current_value
                                            == AgentConfigOptionValue::Select(next.clone())
                                })
                    )),
                    "{} ACP did not publish the selected model",
                    supported.display_name
                );
                let defaults = load_default_config_options(
                    config
                        .default_config_options_path
                        .as_deref()
                        .context("missing per-agent config defaults path")?,
                )?;
                ensure!(
                    defaults.get(&model.id)
                        == Some(&AgentConfigOptionValue::Select(next)),
                    "{} did not persist its selected model",
                    supported.display_name
                );
                eprintln!(
                    "{}: session/new exposed {} config options and {} model choices; model selection changed and saved",
                    supported.display_name,
                    options.len(),
                    model.choices.len(),
                );
                Ok(())
            })();
            client.shutdown();
            result?;
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires public npm, an installed Pi CLI, and OPENROUTER_API_KEY"]
    fn pi_registry_adapter_completes_an_acp_turn() {
        let workspace = tempfile::tempdir().expect("temporary Pi workspace");
        let cwd = workspace.path().to_path_buf();
        let storage = cwd.join("external-agents");
        let user_agents_dir = cwd.join("user/.ahead/agents/external-acp");
        let pi_home = cwd.join("pi-home");
        let pi_agent_dir = pi_home.join(".pi/agent");
        fs::create_dir_all(&pi_agent_dir)
            .expect("create isolated Pi agent directory");
        let model = std::env::var("AHEAD_PI_TEST_MODEL").unwrap_or_else(|_| {
            "openrouter/nvidia/nemotron-3-ultra-550b-a55b:free".to_string()
        });
        let model_id = model.strip_prefix("openrouter/").unwrap_or(&model);
        let api_key = std::env::var("OPENROUTER_API_KEY")
            .expect("set OPENROUTER_API_KEY for the Pi ACP provider smoke");
        fs::write(
            pi_agent_dir.join("models.json"),
            serde_json::to_vec(&serde_json::json!({
                "providers": {
                    "openrouter": {
                        "baseUrl": "https://openrouter.ai/api/v1",
                        "api": "openai-completions",
                        "models": [{ "id": model_id, "name": "AHEAD ACP smoke model" }]
                    }
                }
            }))
            .expect("serialize isolated Pi model config"),
        )
        .expect("write isolated Pi model config");
        set_external_acp_adapter_installed_at("pi-acp", true, &user_agents_dir)
            .expect("install Pi adapter");
        let mut config = external_agent_config_at(
            Some("pi-acp"),
            cwd.clone(),
            &storage,
            &user_agents_dir,
        )
        .expect("install and resolve Pi ACP");
        config
            .env
            .insert("HOME".to_string(), pi_home.to_string_lossy().into_owned());
        config.env.insert(
            "PI_CODING_AGENT_DIR".to_string(),
            pi_agent_dir.to_string_lossy().into_owned(),
        );
        config.env.insert("OPENROUTER_API_KEY".to_string(), api_key);
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let client = HarnessClient::spawn(
            &config,
            Arc::new(move |event| captured.lock().push(event)),
        )
        .expect("spawn Pi ACP");
        if let Err(error) = client.initialize() {
            client.shutdown();
            panic!("initialize Pi ACP: {error}");
        }
        let session_id = match client.new_session(&cwd, "agent", Some(&model), None)
        {
            Ok(session_id) => session_id,
            Err(error) => {
                client.shutdown();
                panic!("create Pi ACP session: {error}");
            }
        };
        let selected = events.lock().iter().any(|event| {
            matches!(
                event,
                HarnessEvent::ConfigOptions { options, .. }
                    if options.iter().any(|option| {
                        option.category.as_deref() == Some("model")
                            && matches!(
                                &option.current_value,
                                ahead_rpc::ahead::AgentConfigOptionValue::Select(value)
                                    if value == &model
                            )
                    })
            )
        });
        if !selected {
            client.shutdown();
            panic!("Pi ACP did not confirm selected model `{model}`");
        }
        let prompt_result =
            client.prompt(&session_id, "Reply exactly PONG without using tools.");
        let events = events.lock().clone();
        let received_pong = events.iter().any(|event| {
            matches!(event, HarnessEvent::AgentDelta { text, .. } if text.contains("PONG"))
        });
        let event_summary = events
            .iter()
            .filter_map(|event| match event {
                HarnessEvent::AgentDelta { text, .. } => Some(format!(
                    "AgentDelta({:?})",
                    text.chars().take(160).collect::<String>()
                )),
                HarnessEvent::AgentThought { text, .. } => Some(format!(
                    "AgentThought({:?})",
                    text.chars().take(160).collect::<String>()
                )),
                HarnessEvent::Warning { message, .. } => Some(format!(
                    "Warning({:?})",
                    message.chars().take(160).collect::<String>()
                )),
                HarnessEvent::ToolCall { title, status, .. } => {
                    Some(format!("ToolCall({title:?}, {status})"))
                }
                HarnessEvent::ConfigOptions { options, .. } => {
                    let selected = options
                        .iter()
                        .filter(|option| option.category.as_deref() == Some("model"))
                        .map(|option| match &option.current_value {
                            ahead_rpc::ahead::AgentConfigOptionValue::Select(
                                value,
                            ) => {
                                format!("{}={value}", option.id)
                            }
                            ahead_rpc::ahead::AgentConfigOptionValue::Boolean(
                                value,
                            ) => {
                                format!("{}={value}", option.id)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    Some(format!("ConfigOptions({selected})"))
                }
                HarnessEvent::AvailableCommands { commands, .. } => {
                    Some(format!("AvailableCommands({} commands)", commands.len()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        client.shutdown();
        let stop_reason = prompt_result.expect("complete Pi ACP prompt");
        assert!(
            received_pong,
            "Pi ACP stopped with {stop_reason:?}; relevant events: {event_summary:#?}"
        );
    }
}
