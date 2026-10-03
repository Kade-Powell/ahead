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
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::acp_client::HarnessClientConfig;

#[derive(Debug, Clone)]
struct RegistryNpxAdapter {
    package: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct RegistryAgent {
    id: String,
    npx: Option<RegistryNpxAdapter>,
}

#[derive(Clone, Copy)]
struct SupportedAgent {
    id: &'static str,
    display_name: &'static str,
    fallback_package: &'static str,
}

const SUPPORTED_AGENTS: [SupportedAgent; 3] = [
    SupportedAgent {
        id: "pi-acp",
        display_name: "Pi",
        fallback_package: "pi-acp@0.0.34",
    },
    SupportedAgent {
        id: "codex-acp",
        display_name: "Codex",
        fallback_package: "@agentclientprotocol/codex-acp@1.13.1",
    },
    SupportedAgent {
        id: "claude-acp",
        display_name: "Claude Code",
        fallback_package: "@agentclientprotocol/claude-agent-acp@0.81.2",
    },
];

#[derive(Debug, Clone, Deserialize)]
struct RegistryIndex {
    agents: Vec<RegistryAgentEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct RegistryAgentEntry {
    id: String,
    distribution: RegistryDistribution,
}

#[derive(Debug, Clone, Deserialize)]
struct RegistryDistribution {
    npx: Option<RegistryNpxDistribution>,
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
    registry: RegistryNpxAdapter,
}

fn supported_agent(id: &str) -> Option<SupportedAgent> {
    SUPPORTED_AGENTS
        .iter()
        .copied()
        .find(|agent| agent.id == id)
}

const REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
const REGISTRY_CACHE_MAX_AGE: Duration = Duration::from_secs(60 * 60);
const MAX_REGISTRY_BYTES: u64 = 4 * 1024 * 1024;
static CACHE_INSTANCE_ID: OnceLock<uuid::Uuid> = OnceLock::new();

fn cache_instance_id() -> uuid::Uuid {
    *CACHE_INSTANCE_ID.get_or_init(uuid::Uuid::new_v4)
}

fn configured_adapters(
    storage: &Path,
    user_agents_dir: &Path,
) -> Result<Vec<ConfiguredAdapter>> {
    let registry = load_registry(storage).unwrap_or_else(|error| {
        tracing::warn!(%error, "ACP registry unavailable; using pinned supported adapters");
        Vec::new()
    });

    curated_adapters(&registry, user_agents_dir)
}

fn curated_adapters(
    registry: &[RegistryAgent],
    user_agents_dir: &Path,
) -> Result<Vec<ConfiguredAdapter>> {
    SUPPORTED_AGENTS
        .into_iter()
        .map(|supported| {
            let registry = registry
                .iter()
                .find(|entry| entry.id == supported.id)
                .and_then(|entry| entry.npx.clone())
                .unwrap_or_else(|| RegistryNpxAdapter {
                    package: supported.fallback_package.to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                });
            Ok(ConfiguredAdapter {
                adapter: ExternalAcpAdapter {
                    id: supported.id.to_string(),
                    display_name: supported.display_name.to_string(),
                    installed: installation_marker(user_agents_dir, supported.id)?
                        .is_file(),
                },
                registry,
            })
        })
        .collect()
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
        refresh_registry_in_background(registry_path.clone());
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
        .map(|entry| RegistryAgent {
            id: entry.id,
            npx: entry.distribution.npx.map(|npx| RegistryNpxAdapter {
                package: npx.package,
                args: npx.args,
                env: npx.env,
            }),
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

    let defaults_path = default_config_options_path(user_agents_dir, adapter_id)?;
    let mut config = install_npx_adapter(adapter_id, &entry.registry, cwd, storage)?;
    config.default_config_options = load_default_config_options(&defaults_path)?;
    config.default_config_options_path = Some(defaults_path);
    Ok(config)
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
    let temporary_path = path.with_extension("tmp");
    fs::write(
        &temporary_path,
        serde_json::to_vec_pretty(&defaults)
            .context("serializing ACP config defaults")?,
    )
    .with_context(|| {
        format!("writing ACP config defaults {}", temporary_path.display())
    })?;
    fs::rename(&temporary_path, path).with_context(|| {
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
    let bytes = fs::read(path).with_context(|| {
        format!("reading package provenance file {}", path.display())
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
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
    let instance_id = cache_instance_id().to_string();
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
        let owner = match fs::read_to_string(&lease_path) {
            Ok(owner) => owner,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading {}", lease_path.display()));
            }
        };
        if owner != instance_id {
            // A prior app process may have left a live ACP child behind.
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
    if let Some(installed) = installed_npx_executable(install_dir, package_spec)? {
        return resolved_npx(install_dir, installed);
    }

    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(install_dir)
        .context("creating ACP adapter staging directory")?;
    install(staging.path())?;
    npx_package_executable(staging.path(), package_spec)?;
    fs::write(
        staging.path().join(".ahead-lease"),
        cache_instance_id().to_string(),
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
        let output = Command::new("npm")
            .args(["install", "--prefix"])
            .arg(staging)
            .args([
                "--save-exact",
                "--package-lock=true",
                "--no-audit",
                "--no-fund",
                "--",
                &registry.package,
            ])
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
    fn catalog_is_curated_and_install_state_is_user_local() {
        let storage = tempfile::tempdir().expect("temporary ACP catalog");
        let registry = [RegistryAgent {
            id: "unapproved-acp-agent".to_string(),
            npx: None,
        }];
        let adapters =
            curated_adapters(&registry, storage.path()).expect("curated catalog");
        assert_eq!(
            adapters
                .iter()
                .map(|entry| entry.adapter.id.as_str())
                .collect::<Vec<_>>(),
            vec!["pi-acp", "codex-acp", "claude-acp"]
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
        let adapters =
            curated_adapters(&[], storage.path()).expect("updated catalog");
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
            curated_adapters(&[], storage.path())
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

    #[test]
    fn parses_acp_registry_npx_distribution() {
        let index: RegistryIndex = serde_json::from_str(
            r#"
            {
              "version": "1.0.0",
              "agents": [{
                "id": "example-acp",
                "name": "Example",
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
        assert_eq!(npx.package, "example-acp@1.2.3");
        assert_eq!(npx.args, ["--acp"]);
        assert_eq!(
            npx.env.get("EXAMPLE_NO_UPDATE").map(String::as_str),
            Some("1")
        );
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
            let mut command = Command::new("npm");
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
            scripts.push((script, version));
        }
        for (script, version) in scripts {
            let output = Command::new("node").arg(script.script).output()?;
            assert!(output.status.success());
            assert_eq!(String::from_utf8(output.stdout)?, version);
        }
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
        let pi_home = temporary
            .path()
            .join("pi-home")
            .to_string_lossy()
            .into_owned();
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
    #[ignore = "requires npm, an installed Pi CLI, and configured Pi model auth"]
    fn pi_registry_adapter_completes_an_acp_turn() {
        let workspace = tempfile::tempdir().expect("temporary Pi workspace");
        let cwd = workspace.path().to_path_buf();
        let storage = cwd.join("external-agents");
        let user_agents_dir = cwd.join("user/.ahead/agents/external-acp");
        set_external_acp_adapter_installed_at("pi-acp", true, &user_agents_dir)
            .expect("install Pi adapter");
        let config = external_agent_config_at(
            Some("pi-acp"),
            cwd.clone(),
            &storage,
            &user_agents_dir,
        )
        .expect("install and resolve Pi ACP");
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
        let model = std::env::var("AHEAD_PI_TEST_MODEL").ok();
        let session_id =
            match client.new_session(&cwd, "agent", model.as_deref(), None) {
                Ok(session_id) => session_id,
                Err(error) => {
                    client.shutdown();
                    panic!("create Pi ACP session: {error}");
                }
            };
        if let Some(model) = model.as_deref() {
            let selected = events.lock().iter().any(|event| {
                matches!(
                    event,
                    HarnessEvent::ConfigOptions { options, .. }
                        if options.iter().any(|option| {
                            option.category.as_deref() == Some("model")
                                && matches!(
                                    &option.current_value,
                                    ahead_rpc::ahead::AgentConfigOptionValue::Select(value)
                                        if value == model
                                )
                        })
                )
            });
            if !selected {
                client.shutdown();
                panic!("Pi ACP did not confirm selected model `{model}`");
            }
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
