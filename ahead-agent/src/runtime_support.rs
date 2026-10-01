use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::{ErrorKind, Read, Write},
    path::{Component, Path, PathBuf},
};

use ahead_rpc::ahead::McpServerDeclaration;
use anyhow::{Context, Result};
use codex_config::{AppToolApproval, McpServerConfig, McpServerTransportConfig};
use codex_protocol::{
    models::PermissionProfile,
    permissions::{
        FileSystemAccessMode, FileSystemPath, FileSystemSandboxEntry,
        FileSystemSandboxPolicy, FileSystemSpecialPath, NetworkSandboxPolicy,
    },
};
use codex_utils_absolute_path::AbsolutePathBuf;
use fs4::fs_std::FileExt;
use sha2::{Digest, Sha256};

const MAX_MCP_CONFIG_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct AgentScope {
    pub(crate) workspace: PathBuf,
    pub(crate) allowed_paths: Vec<String>,
    pub(crate) read_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpServerPolicy {
    Disabled,
    PromptEveryCall,
}

#[derive(Debug, Clone)]
struct RuntimeProvider {
    id: String,
    name: String,
    base_url: String,
    model: String,
    api_key: Option<String>,
}

pub(crate) fn prepare_runtime_home(workspace: &Path) -> Result<PathBuf> {
    let explicit = std::env::var_os("AHEAD_HOME");
    let home = explicit
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join(".ahead/runtime"));
    std::fs::create_dir_all(&home).with_context(|| {
        format!("failed to create AHEAD runtime home `{}`", home.display())
    })?;
    let user_home = crate::instructions::user_home();
    if let Some(config) =
        runtime_config_from_sources(workspace, user_home.as_deref())
    {
        let path = home.join("config.toml");
        if explicit.is_none() || !path.exists() {
            write_runtime_config(&path, &config)?;
        }
    }
    Ok(home)
}

fn write_runtime_config(path: &Path, contents: &str) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(
            metadata.is_file(),
            "AHEAD runtime config `{}` must be a regular file",
            path.display()
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to inspect AHEAD runtime config `{}`",
                    path.display()
                )
            });
        }
    }

    let parent = path
        .parent()
        .context("AHEAD runtime config has no parent")?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).with_context(|| {
            format!(
                "failed to create AHEAD runtime config in `{}`",
                parent.display()
            )
        })?;
    temporary.write_all(contents.as_bytes()).with_context(|| {
        format!("failed to write AHEAD runtime config `{}`", path.display())
    })?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| {
            format!(
                "failed to persist AHEAD runtime config `{}`",
                path.display()
            )
        })?;
    Ok(())
}

pub(crate) fn mcp_servers_for_workspace(
    workspace: &Path,
    policy: McpServerPolicy,
) -> Result<HashMap<String, McpServerConfig>> {
    if policy == McpServerPolicy::Disabled {
        return Ok(HashMap::new());
    }

    let settings = read_project_toml_file(workspace, ".ahead/settings.toml")?;
    let Some(settings) = settings else {
        return Ok(HashMap::new());
    };
    let Some(enabled_ids) =
        enabled_mcp_server_ids(&settings, ".ahead/settings.toml")?
    else {
        return Ok(HashMap::new());
    };
    if enabled_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let config = read_project_toml_file(workspace, ".ahead/config.toml")?.context(
        "AHEAD MCP servers are enabled but `.ahead/config.toml` is missing",
    )?;
    let mcp = config
        .get("mcp")
        .context("AHEAD MCP servers are enabled but `.ahead/config.toml` has no `[mcp]` table")?;
    let declarations = mcp.get("servers").and_then(toml::Value::as_table).context(
        "AHEAD MCP servers are enabled but `[mcp.servers]` is missing or invalid",
    )?;
    let tool_permissions = settings
        .get("mcp")
        .and_then(|mcp| mcp.get("tool_permissions"))
        .map(|permissions| {
            permissions.as_table().context(
                "`.ahead/settings.toml` field `mcp.tool_permissions` must be a table",
            )
        })
        .transpose()?;
    let approved_declarations = settings
        .get("mcp")
        .and_then(|mcp| mcp.get("approved_declarations"))
        .map(|approved| {
            approved.as_table().context(
                "`.ahead/settings.toml` field `mcp.approved_declarations` must be a table",
            )
        })
        .transpose()?;

    let mut servers = HashMap::with_capacity(enabled_ids.len());
    for id in enabled_ids {
        anyhow::ensure!(
            id != "codex_apps",
            "`codex_apps` is reserved and cannot name an AHEAD MCP server"
        );
        let value = declarations.get(&id).with_context(|| {
            format!("AHEAD MCP server `{id}` is enabled but not declared in `.ahead/config.toml`")
        })?;
        let mut server: McpServerConfig =
            value.clone().try_into().with_context(|| {
                format!("invalid AHEAD MCP server declaration `{id}`")
            })?;
        anyhow::ensure!(
            server.is_local_environment(),
            "AHEAD MCP server `{id}` uses an unsupported non-local environment"
        );
        match &server.transport {
            McpServerTransportConfig::Stdio { env, env_vars, .. } => {
                anyhow::ensure!(
                    env.is_none(),
                    "AHEAD MCP server `{id}` must reference host environment variables with `env_vars`, not store values in tracked config"
                );
                anyhow::ensure!(
                    env_vars.iter().all(|variable| {
                        variable.source().map_or(true, |source| source == "local")
                    }),
                    "AHEAD MCP server `{id}` references an unsupported non-local environment variable source"
                );
            }
            McpServerTransportConfig::StreamableHttp { .. } => {
                anyhow::bail!(
                    "AHEAD currently supports stdio MCP servers only; HTTP authentication and secret storage are not wired"
                );
            }
        }
        let fingerprint = mcp_declaration_fingerprint(value)?;
        anyhow::ensure!(
            approved_declarations
                .and_then(|approved| approved.get(&id))
                .and_then(toml::Value::as_str)
                == Some(fingerprint.as_str()),
            "AHEAD MCP server `{id}` declaration is not approved; inspect `.ahead/config.toml`, then set `[mcp.approved_declarations]` `{id} = \"{fingerprint}\"` in ignored `.ahead/settings.toml`"
        );
        server.enabled = true;
        server.default_tools_approval_mode = Some(AppToolApproval::Prompt);
        for tool in server.tools.values_mut() {
            tool.approval_mode = Some(AppToolApproval::Prompt);
        }
        if let Some(permissions) =
            tool_permissions.and_then(|permissions| permissions.get(&id))
        {
            let permissions = permissions.as_table().with_context(|| {
                format!("`.ahead/settings.toml` MCP tool permissions for `{id}` must be a table")
            })?;
            for (tool_name, choice) in permissions {
                anyhow::ensure!(
                    !tool_name.trim().is_empty() && tool_name == tool_name.trim(),
                    "`.ahead/settings.toml` MCP tool name for `{id}` must not be empty or padded"
                );
                match choice.as_str() {
                    Some("allow") => {
                        server
                            .tools
                            .entry(tool_name.clone())
                            .or_default()
                            .approval_mode = Some(AppToolApproval::Approve);
                    }
                    Some("deny") => {
                        let disabled =
                            server.disabled_tools.get_or_insert_with(Vec::new);
                        if !disabled.contains(tool_name) {
                            disabled.push(tool_name.clone());
                        }
                    }
                    Some("confirm") => {}
                    _ => anyhow::bail!(
                        "`.ahead/settings.toml` MCP tool `{id}.{tool_name}` must be `allow`, `deny`, or `confirm`"
                    ),
                }
            }
        }
        servers.insert(id, server);
    }
    Ok(servers)
}

pub(crate) fn mcp_declaration_fingerprint(value: &toml::Value) -> Result<String> {
    // Hash the parsed declaration so comments and formatting do not revoke approval.
    let canonical = toml::to_string(value)
        .context("failed to serialize AHEAD MCP declaration")?;
    Ok(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
}

pub fn mcp_server_declarations(
    workspace: &Path,
) -> Result<Vec<McpServerDeclaration>> {
    let config = read_project_toml_file(workspace, ".ahead/config.toml")?;
    let settings = read_project_toml_file(workspace, ".ahead/settings.toml")?;
    let enabled = settings
        .as_ref()
        .map(|settings| enabled_mcp_server_ids(settings, ".ahead/settings.toml"))
        .transpose()?
        .flatten()
        .unwrap_or_default();
    let approved = settings
        .as_ref()
        .and_then(|settings| settings.get("mcp"))
        .and_then(|mcp| mcp.get("approved_declarations"))
        .map(|approved| {
            approved.as_table().context(
                "`.ahead/settings.toml` field `mcp.approved_declarations` must be a table",
            )
        })
        .transpose()?;
    let mut entries = Vec::new();
    if let Some(declarations) = config
        .as_ref()
        .and_then(|config| config.get("mcp"))
        .and_then(|mcp| mcp.get("servers"))
    {
        let declarations = declarations
            .as_table()
            .context("`.ahead/config.toml` field `mcp.servers` must be a table")?;
        for (id, declaration) in declarations {
            let fingerprint = mcp_declaration_fingerprint(declaration)?;
            entries.push(McpServerDeclaration {
                id: id.clone(),
                declared: true,
                declaration_toml: toml::to_string(declaration)?,
                enabled: enabled.contains(id),
                approved: approved
                    .and_then(|approved| approved.get(id))
                    .and_then(toml::Value::as_str)
                    == Some(fingerprint.as_str()),
                fingerprint,
            });
        }
    }
    for id in enabled {
        if !entries.iter().any(|entry| entry.id == id) {
            entries.push(McpServerDeclaration {
                id,
                declared: false,
                declaration_toml: String::new(),
                fingerprint: String::new(),
                enabled: true,
                approved: false,
            });
        }
    }
    entries.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(entries)
}

struct McpSettingsLock(std::fs::File);

impl McpSettingsLock {
    fn acquire(file: std::fs::File) -> Result<Self> {
        file.lock_exclusive()
            .context("failed to lock AHEAD MCP settings")?;
        Ok(Self(file))
    }
}

impl Drop for McpSettingsLock {
    fn drop(&mut self) {
        // Releasing only this descriptor can leave a fork-inherited lock held.
        if let Err(error) = FileExt::unlock(&self.0) {
            tracing::error!(%error, "failed to unlock AHEAD MCP settings");
        }
    }
}

pub fn set_mcp_server_approval(
    workspace: &Path,
    server_id: &str,
    expected_fingerprint: &str,
    enabled: bool,
) -> Result<()> {
    anyhow::ensure!(
        !server_id.trim().is_empty()
            && server_id == server_id.trim()
            && server_id != "codex_apps",
        "invalid AHEAD MCP server ID"
    );
    let workspace = workspace
        .canonicalize()
        .context("AHEAD workspace is unavailable")?;
    #[cfg(not(unix))]
    let ahead = workspace.join(".ahead");
    #[cfg(unix)]
    let ahead_directory = open_mcp_settings_directory(&workspace)?;
    #[cfg(not(unix))]
    anyhow::ensure!(
        std::fs::symlink_metadata(&ahead)?.is_dir(),
        "AHEAD MCP settings directory must be a regular directory"
    );
    #[cfg(not(unix))]
    let lock_path = ahead.join("settings.toml.lock");
    #[cfg(not(unix))]
    match std::fs::symlink_metadata(&lock_path) {
        Ok(metadata) => anyhow::ensure!(
            metadata.is_file(),
            "AHEAD MCP settings lock must be a regular file"
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).context("failed to inspect AHEAD MCP settings lock");
        }
    }
    #[cfg(unix)]
    let lock = open_mcp_settings_lock(&ahead_directory)?;
    #[cfg(not(unix))]
    let lock = open_mcp_settings_lock(&lock_path)?;
    let _lock = McpSettingsLock::acquire(lock)?;
    if enabled {
        #[cfg(unix)]
        let config = read_mcp_toml_at(&ahead_directory, "config.toml")?
            .context("AHEAD MCP declaration file is missing")?;
        #[cfg(not(unix))]
        let config = read_project_toml_file(&workspace, ".ahead/config.toml")?
            .context("AHEAD MCP declaration file is missing")?;
        let declaration = config
            .get("mcp")
            .and_then(|mcp| mcp.get("servers"))
            .and_then(|servers| servers.get(server_id))
            .with_context(|| {
                format!("AHEAD MCP server `{server_id}` is not declared")
            })?;
        let fingerprint = mcp_declaration_fingerprint(declaration)?;
        anyhow::ensure!(
            expected_fingerprint == fingerprint,
            "AHEAD MCP server `{server_id}` changed; review the current declaration before approving"
        );
        let server: McpServerConfig = declaration.clone().try_into()?;
        anyhow::ensure!(
            server.is_local_environment(),
            "MCP server must use the local environment"
        );
        match &server.transport {
            McpServerTransportConfig::Stdio { env, env_vars, .. } => {
                anyhow::ensure!(
                    env.is_none(),
                    "MCP server must use env_vars instead of literal env values"
                );
                anyhow::ensure!(
                    env_vars.iter().all(|variable| variable
                        .source()
                        .map_or(true, |source| source == "local")),
                    "MCP server has a non-local environment variable source"
                );
            }
            McpServerTransportConfig::StreamableHttp { .. } => {
                anyhow::bail!("AHEAD currently supports stdio MCP servers only")
            }
        }
    }

    #[cfg(not(unix))]
    let path = ahead.join("settings.toml");
    #[cfg(unix)]
    let mut settings = read_mcp_toml_at(&ahead_directory, "settings.toml")?
        .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
    #[cfg(not(unix))]
    let mut settings = read_project_toml_file(&workspace, ".ahead/settings.toml")?
        .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
    let mut ids = enabled_mcp_server_ids(&settings, ".ahead/settings.toml")?
        .unwrap_or_default();
    let mcp = settings
        .as_table_mut()
        .context("AHEAD MCP settings must be a TOML table")?
        .entry("mcp")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .context("`.ahead/settings.toml` field `mcp` must be a table")?;
    let previously_approved = mcp
        .get("approved_declarations")
        .map(|approved| {
            approved
                .as_table()
                .context("`.ahead/settings.toml` field `mcp.approved_declarations` must be a table")
        })
        .transpose()?
        .and_then(|approved| approved.get(server_id))
        .and_then(toml::Value::as_str);
    if !enabled || previously_approved != Some(expected_fingerprint) {
        if let Some(permissions) = mcp.get_mut("tool_permissions") {
            permissions
                .as_table_mut()
                .context("`.ahead/settings.toml` field `mcp.tool_permissions` must be a table")?
                .remove(server_id);
        }
    }
    let approved = mcp
        .entry("approved_declarations")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .context("`.ahead/settings.toml` field `mcp.approved_declarations` must be a table")?;
    if enabled {
        approved.insert(
            server_id.to_string(),
            toml::Value::String(expected_fingerprint.to_string()),
        );
        if !ids.iter().any(|id| id == server_id) {
            ids.push(server_id.to_string());
        }
    } else {
        approved.remove(server_id);
        ids.retain(|id| id != server_id);
    }
    mcp.insert(
        "enabled_servers".to_string(),
        toml::Value::Array(ids.into_iter().map(toml::Value::String).collect()),
    );
    let content = toml::to_string(&settings)?;
    anyhow::ensure!(
        content.len() <= MAX_MCP_CONFIG_BYTES,
        "AHEAD MCP settings exceed the 1 MiB limit"
    );
    #[cfg(unix)]
    return write_mcp_settings_at(&ahead_directory, &content);
    #[cfg(not(unix))]
    write_runtime_config(&path, &content)
}

#[cfg(unix)]
fn open_mcp_settings_directory(workspace: &Path) -> Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags, openat};

    let workspace_directory =
        ahead_core::secure_fs::open_canonical_directory(workspace)
            .context("failed to open AHEAD workspace directory")?;
    let directory: std::fs::File = openat(
        &workspace_directory,
        ".ahead",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .context("AHEAD MCP settings directory must be a regular directory")?
    .into();
    Ok(directory)
}

#[cfg(unix)]
fn open_mcp_settings_lock(directory: &std::fs::File) -> Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags, openat};

    let file: std::fs::File = openat(
        directory,
        "settings.toml.lock",
        OFlags::RDWR
            | OFlags::CREATE
            | OFlags::NOFOLLOW
            | OFlags::NONBLOCK
            | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .context("failed to open AHEAD MCP settings lock")?
    .into();
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "AHEAD MCP settings lock must be a regular file"
    );
    Ok(file)
}

#[cfg(not(unix))]
fn open_mcp_settings_lock(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    let file = options
        .open(path)
        .context("failed to open AHEAD MCP settings lock")?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "AHEAD MCP settings lock must be a regular file"
    );
    Ok(file)
}

#[cfg(unix)]
fn read_mcp_toml_at(
    directory: &std::fs::File,
    name: &str,
) -> Result<Option<toml::Value>> {
    use rustix::fs::{Mode, OFlags, openat};

    let file: std::fs::File = match openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(file) => file.into(),
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to read AHEAD config `.ahead/{name}`")
            });
        }
    };
    read_mcp_toml(file, &format!(".ahead/{name}")).map(Some)
}

#[cfg(unix)]
fn write_mcp_settings_at(directory: &std::fs::File, content: &str) -> Result<()> {
    use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, unlinkat};

    let temporary_name = format!(".settings.toml-{}", uuid::Uuid::new_v4());
    let result = (|| -> Result<()> {
        let mut temporary: std::fs::File = openat(
            directory,
            temporary_name.as_str(),
            OFlags::WRONLY
                | OFlags::CREATE
                | OFlags::EXCL
                | OFlags::NOFOLLOW
                | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?
        .into();
        temporary.write_all(content.as_bytes())?;
        temporary.sync_all()?;
        renameat(
            directory,
            temporary_name.as_str(),
            directory,
            "settings.toml",
        )
        .context("failed to persist AHEAD MCP settings")?;
        Ok(())
    })();
    if result.is_err() {
        match unlinkat(directory, temporary_name.as_str(), AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => {}
            Err(error) => {
                tracing::warn!(%error, "failed to remove temporary AHEAD MCP settings")
            }
        }
    }
    result
}

fn read_project_toml_file(
    workspace: &Path,
    relative_path: &str,
) -> Result<Option<toml::Value>> {
    let workspace = workspace
        .canonicalize()
        .context("AHEAD MCP configuration requires an existing workspace")?;
    let path = workspace.join(relative_path);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to inspect AHEAD config `{relative_path}`")
            });
        }
    };
    anyhow::ensure!(
        metadata.file_type().is_file(),
        "AHEAD config `{relative_path}` must be a regular file"
    );
    let resolved_path = path.canonicalize().with_context(|| {
        format!("failed to resolve AHEAD config `{relative_path}`")
    })?;
    anyhow::ensure!(
        resolved_path.starts_with(&workspace),
        "AHEAD config `{relative_path}` resolves outside the workspace"
    );
    #[cfg(unix)]
    let file = ahead_core::secure_fs::open_canonical_regular_file(&resolved_path)
        .with_context(|| format!("failed to read AHEAD config `{relative_path}`"))?;
    #[cfg(not(unix))]
    let file = std::fs::File::open(&resolved_path)
        .with_context(|| format!("failed to read AHEAD config `{relative_path}`"))?;
    read_mcp_toml(file, relative_path).map(Some)
}

fn read_mcp_toml(file: std::fs::File, relative_path: &str) -> Result<toml::Value> {
    let metadata = file.metadata().with_context(|| {
        format!("failed to inspect AHEAD config `{relative_path}`")
    })?;
    anyhow::ensure!(
        metadata.file_type().is_file(),
        "AHEAD config `{relative_path}` must be a regular file"
    );
    anyhow::ensure!(
        metadata.len() <= MAX_MCP_CONFIG_BYTES as u64,
        "AHEAD config `{relative_path}` exceeds AHEAD's 1 MiB file limit"
    );
    let mut contents = String::new();
    file.take(MAX_MCP_CONFIG_BYTES as u64 + 1)
        .read_to_string(&mut contents)
        .with_context(|| format!("failed to read AHEAD config `{relative_path}`"))?;
    anyhow::ensure!(
        contents.len() <= MAX_MCP_CONFIG_BYTES,
        "AHEAD config `{relative_path}` exceeds AHEAD's 1 MiB file limit"
    );
    let table = contents.parse::<toml::Table>().with_context(|| {
        format!("invalid TOML in AHEAD config `{relative_path}`")
    })?;
    Ok(toml::Value::Table(table))
}

fn enabled_mcp_server_ids(
    settings: &toml::Value,
    source: &str,
) -> Result<Option<Vec<String>>> {
    let Some(mcp) = settings.get("mcp") else {
        return Ok(None);
    };
    let mcp = mcp
        .as_table()
        .with_context(|| format!("`{source}` field `mcp` must be a table"))?;
    let Some(enabled) = mcp.get("enabled_servers") else {
        return Ok(None);
    };
    let enabled = enabled.as_array().with_context(|| {
        format!("`{source}` field `mcp.enabled_servers` must be an array")
    })?;
    let mut ids = Vec::with_capacity(enabled.len());
    let mut seen = BTreeSet::new();
    for value in enabled {
        let id = value.as_str().with_context(|| {
            format!("`{source}` enabled MCP server IDs must be strings")
        })?;
        anyhow::ensure!(
            !id.trim().is_empty() && id == id.trim(),
            "`{source}` contains an empty or whitespace-padded MCP server ID"
        );
        anyhow::ensure!(
            seen.insert(id),
            "`{source}` contains duplicate MCP server ID `{id}`"
        );
        ids.push(id.to_string());
    }
    Ok(Some(ids))
}

pub(crate) fn generate_thread_title(request: &str) -> String {
    let title = request
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|character: char| character.is_ascii_punctuation())
        .chars()
        .take(72)
        .collect::<String>();
    if title.is_empty() {
        "New agent thread".to_string()
    } else {
        title
    }
}

pub(crate) fn file_change_allowed(
    mode: &str,
    paths: &[String],
    scope: Option<&AgentScope>,
) -> bool {
    if mode == "read-only"
        || scope.is_some_and(|scope| scope.read_only)
        || paths.is_empty()
    {
        return false;
    }
    let Some(scope) = scope else {
        return false;
    };
    paths
        .iter()
        .all(|path| path_is_allowed(path, &scope.allowed_paths, &scope.workspace))
}

pub(crate) fn permission_profile(
    mode: &str,
    cwd: &Path,
    scope: Option<&AgentScope>,
) -> Result<PermissionProfile> {
    let read_only =
        mode == "read-only" || scope.is_some_and(|scope| scope.read_only);
    let workspace = scope.map_or(cwd, |scope| scope.workspace.as_path());
    let workspace =
        AbsolutePathBuf::from_absolute_path(workspace).with_context(|| {
            format!(
                "AHEAD workspace must be absolute: `{}`",
                workspace.display()
            )
        })?;
    let mut private_roots = vec![workspace.as_path().join(".ahead")];
    if let Some(home) = crate::instructions::user_home() {
        private_roots.push(home.join(".ahead"));
    }
    if let Some(runtime_home) = std::env::var_os("AHEAD_HOME") {
        private_roots.push(PathBuf::from(runtime_home));
    }
    let write_roots = match scope {
        _ if read_only => Vec::new(),
        Some(scope) if !scope.allowed_paths.is_empty() => scope
            .allowed_paths
            .iter()
            .map(|path| scoped_absolute_path(path, &scope.workspace))
            .collect::<Result<Vec<_>>>()?,
        _ => vec![workspace],
    };

    let mut entries = vec![FileSystemSandboxEntry::new(
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Root,
        },
        FileSystemAccessMode::Read,
    )];
    entries.extend(write_roots.into_iter().map(|path| {
        FileSystemSandboxEntry::new(path.into(), FileSystemAccessMode::Write)
    }));
    for root in private_roots {
        let root = AbsolutePathBuf::from_absolute_path(&root)
            .context("AHEAD private agent path must be absolute")?;
        entries.push(FileSystemSandboxEntry::new(
            root.into(),
            FileSystemAccessMode::Deny,
        ));
    }
    if !read_only {
        entries.push(FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Tmpdir,
            },
            FileSystemAccessMode::Write,
        ));
        #[cfg(unix)]
        entries.push(FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::SlashTmp,
            },
            FileSystemAccessMode::Write,
        ));
    }

    Ok(PermissionProfile::from_runtime_permissions(
        &FileSystemSandboxPolicy::restricted(entries),
        NetworkSandboxPolicy::Restricted,
    ))
}

fn scoped_absolute_path(path: &str, workspace: &Path) -> Result<AbsolutePathBuf> {
    let normalized = normalized_relative_path(path, workspace)
        .with_context(|| format!("invalid AHEAD scope path `{path}`"))?;
    anyhow::ensure!(
        !normalized.starts_with(".ahead"),
        "AHEAD private workspace state cannot be an agent write scope"
    );
    let mut existing_component = workspace.to_path_buf();
    for component in normalized.components() {
        existing_component.push(component.as_os_str());
        anyhow::ensure!(
            !existing_component.is_symlink(),
            "AHEAD agent write scopes cannot cross symlinks"
        );
    }
    AbsolutePathBuf::from_absolute_path(workspace.join(normalized)).with_context(
        || {
            format!(
                "AHEAD scope path is not absolute under `{}`",
                workspace.display()
            )
        },
    )
}

fn normalized_relative_path(path: &str, workspace: &Path) -> Option<PathBuf> {
    let path = Path::new(path);
    let relative = if path.is_absolute() {
        path.strip_prefix(workspace).ok()?
    } else {
        path
    };
    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => normalized.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return None;
            }
        }
    }
    (!normalized.as_os_str().is_empty()).then_some(normalized)
}

/// Checks a repository path against AHEAD's normalized workspace scope.
pub fn path_is_allowed(path: &str, allowed: &[String], workspace: &Path) -> bool {
    let Some(path) = normalized_relative_path(path, workspace) else {
        return false;
    };
    if path.starts_with(".ahead") {
        return false;
    }
    if allowed.is_empty() {
        return true;
    }
    allowed.iter().any(|root| {
        normalized_relative_path(root, workspace)
            .is_some_and(|root| path == root || path.starts_with(root))
    })
}

fn runtime_config_from_sources(
    workspace: &Path,
    user_home: Option<&Path>,
) -> Option<String> {
    let mut paths = Vec::with_capacity(4);
    if let Some(user_home) = user_home {
        paths.push(user_home.join(".ahead/settings.toml"));
    }
    paths.extend([
        workspace.join(".ahead/settings.toml"),
        workspace.join(".ahead/config.toml"),
        workspace.join(".ahead/config.local.toml"),
    ]);
    let mut providers = BTreeMap::<String, RuntimeProvider>::new();
    let mut preferred_provider = None;
    for path in paths {
        let Ok(contents) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(table) = contents.parse::<toml::Table>() else {
            continue;
        };
        let value = toml::Value::Table(table);
        let Some(ai) = value.get("ai") else { continue };
        preferred_provider = ai
            .get("active_connection")
            .and_then(toml::Value::as_str)
            .map(str::to_string)
            .or(preferred_provider);
        let fallback = ai
            .get("provider")
            .and_then(toml::Value::as_str)
            .unwrap_or("openai-compatible");
        if let Some(connections) =
            ai.get("connections").and_then(toml::Value::as_array)
        {
            for connection in connections {
                if let Some(provider) = runtime_provider(connection, fallback) {
                    providers.insert(provider.id.clone(), provider);
                }
            }
        } else if let Some(provider) = runtime_provider(ai, fallback) {
            providers.insert(provider.id.clone(), provider);
        }
    }
    render_runtime_config(&providers, preferred_provider.as_deref())
}

fn runtime_provider(
    value: &toml::Value,
    fallback_id: &str,
) -> Option<RuntimeProvider> {
    let table = value.as_table()?;
    let name = table
        .get("name")
        .and_then(toml::Value::as_str)
        .unwrap_or("OpenAI-compatible server")
        .trim();
    let base_url = table.get("base_url").and_then(toml::Value::as_str)?.trim();
    let model = table
        .get("model")
        .and_then(toml::Value::as_str)
        .or_else(|| {
            table
                .get("models")
                .and_then(toml::Value::as_array)
                .and_then(|models| models.first())
                .and_then(toml::Value::as_str)
        })?
        .trim();
    if name.is_empty() || base_url.is_empty() || model.is_empty() {
        return None;
    }
    let id = table
        .get("provider_id")
        .or_else(|| table.get("id"))
        .and_then(toml::Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| provider_id_for(name, fallback_id));
    Some(RuntimeProvider {
        id,
        name: name.to_string(),
        base_url: base_url.to_string(),
        model: model.to_string(),
        api_key: table
            .get("api_key")
            .and_then(toml::Value::as_str)
            .filter(|key| !key.trim().is_empty())
            .map(str::to_string),
    })
}

fn provider_id_for(name: &str, fallback: &str) -> String {
    let id = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let id = id.trim_matches('-');
    format!("ahead-{}", if id.is_empty() { fallback } else { id })
}

fn render_runtime_config(
    providers: &BTreeMap<String, RuntimeProvider>,
    preferred_provider: Option<&str>,
) -> Option<String> {
    let default = preferred_provider
        .and_then(|preferred| {
            providers.values().find(|provider| {
                provider.id == preferred || provider.name == preferred
            })
        })
        .or_else(|| providers.values().next())?;
    let mut root = toml::map::Map::new();
    root.insert("model".into(), toml::Value::String(default.model.clone()));
    root.insert(
        "model_provider".into(),
        toml::Value::String(default.id.clone()),
    );
    let model_providers = providers
        .values()
        .map(|provider| {
            let mut table = toml::map::Map::new();
            table.insert("name".into(), toml::Value::String(provider.name.clone()));
            table.insert(
                "base_url".into(),
                toml::Value::String(provider.base_url.clone()),
            );
            table.insert("wire_api".into(), toml::Value::String("responses".into()));
            if let Some(api_key) = &provider.api_key {
                table.insert(
                    "experimental_bearer_token".into(),
                    toml::Value::String(api_key.clone()),
                );
            }
            (provider.id.clone(), toml::Value::Table(table))
        })
        .collect();
    root.insert(
        "model_providers".into(),
        toml::Value::Table(model_providers),
    );
    toml::to_string(&toml::Value::Table(root)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_settings_lock_releases_inherited_handles_on_success_and_error()
    -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("settings.toml.lock");
        for fail in [false, true] {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)?;
            let inherited = file.try_clone()?;
            let contender = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)?;
            let result = (|| -> Result<()> {
                let _lock = McpSettingsLock::acquire(file)?;
                anyhow::ensure!(
                    !contender.try_lock_exclusive()?,
                    "critical section must exclude another writer"
                );
                anyhow::ensure!(!fail, "injected approval failure");
                Ok(())
            })();
            assert_eq!(result.is_err(), fail);
            assert!(
                contender.try_lock_exclusive()?,
                "release even while the inherited handle remains open"
            );
            FileExt::unlock(&contender)?;
            drop(inherited);
        }
        assert!(path.is_file(), "keep the same inode for future waiters");
        Ok(())
    }

    fn approve_docs(ahead: &Path, extra: &str) {
        let config = std::fs::read_to_string(ahead.join("config.toml"))
            .expect("read MCP declaration")
            .parse::<toml::Table>()
            .expect("parse MCP declaration");
        let fingerprint =
            mcp_declaration_fingerprint(&config["mcp"]["servers"]["docs"])
                .expect("fingerprint MCP declaration");
        std::fs::write(
            ahead.join("settings.toml"),
            format!(
                "[mcp]\nenabled_servers = [\"docs\"]\n[mcp.approved_declarations]\ndocs = \"{fingerprint}\"\n{extra}"
            ),
        )
        .expect("write local MCP approval");
    }

    #[test]
    fn runtime_config_replaces_regular_file_without_partial_contents() {
        let directory = tempfile::tempdir().expect("runtime home");
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "old config").expect("initial config");

        write_runtime_config(&path, "new config").expect("replace config");

        assert_eq!(
            std::fs::read_to_string(path).expect("read config"),
            "new config"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                std::fs::metadata(directory.path().join("config.toml"))
                    .expect("config metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn mcp_declaration_approval_is_pinned_and_preserves_private_settings() {
        let directory = tempfile::tempdir().expect("workspace");
        let ahead = directory.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("AHEAD settings directory");
        std::fs::write(
            ahead.join("config.toml"),
            "[mcp.servers.docs]\ncommand = 'echo'\nargs = ['old']\n",
        )
        .expect("tracked MCP declaration");
        std::fs::write(
            ahead.join("settings.toml"),
            "[ai]\napi_key = 'private'\n[mcp]\nenabled_servers = ['docs']\n[mcp.approved_declarations]\ndocs = 'sha256:old'\n[mcp.tool_permissions.docs]\nread = 'allow'\n",
        )
        .expect("private settings");

        let listed =
            mcp_server_declarations(directory.path()).expect("list declarations");
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].approved);
        let first = listed[0].fingerprint.clone();
        set_mcp_server_approval(directory.path(), "docs", &first, true)
            .expect("approve current declaration");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(ahead.join("settings.toml.lock"))
                .expect("settings lock metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "settings lock must be owner-only");
        }
        let settings =
            read_project_toml_file(directory.path(), ".ahead/settings.toml")
                .expect("read settings")
                .expect("settings exist");
        assert_eq!(settings["ai"]["api_key"].as_str(), Some("private"));
        assert_eq!(
            settings["mcp"]["approved_declarations"]["docs"].as_str(),
            Some(first.as_str())
        );
        assert!(settings["mcp"]["tool_permissions"].get("docs").is_none());

        std::fs::write(
            ahead.join("config.toml"),
            "[mcp.servers.docs]\ncommand = 'echo'\nargs = ['changed']\n",
        )
        .expect("change tracked declaration");
        let before = std::fs::read_to_string(ahead.join("settings.toml"))
            .expect("settings before stale approval");
        assert!(
            set_mcp_server_approval(directory.path(), "docs", &first, true).is_err()
        );
        assert_eq!(
            std::fs::read_to_string(ahead.join("settings.toml"))
                .expect("settings after stale approval"),
            before
        );
        let current =
            mcp_server_declarations(directory.path()).expect("current declaration");
        assert!(!current[0].approved);
        set_mcp_server_approval(
            directory.path(),
            "docs",
            &current[0].fingerprint,
            true,
        )
        .expect("approve changed declaration");
        std::fs::remove_file(ahead.join("config.toml"))
            .expect("remove tracked declaration");
        let orphan =
            mcp_server_declarations(directory.path()).expect("list orphaned opt-in");
        assert_eq!(orphan.len(), 1);
        assert!(!orphan[0].declared);
        assert!(orphan[0].enabled);
        set_mcp_server_approval(directory.path(), "docs", "", false)
            .expect("revoke declaration");
        let final_state =
            mcp_server_declarations(directory.path()).expect("revoked declaration");
        assert!(final_state.is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let target = directory.path().join("outside.lock");
            std::fs::write(&target, "untouched").expect("outside lock target");
            std::fs::remove_file(ahead.join("settings.toml.lock"))
                .expect("remove unlocked settings lock");
            symlink(&target, ahead.join("settings.toml.lock"))
                .expect("symlink settings lock");
            let ahead_directory = open_mcp_settings_directory(
                &directory
                    .path()
                    .canonicalize()
                    .expect("canonical workspace"),
            )
            .expect("open AHEAD settings directory");
            assert!(
                open_mcp_settings_lock(&ahead_directory).is_err(),
                "the lock opener must reject a symlink even after preflight"
            );
            assert!(
                set_mcp_server_approval(directory.path(), "docs", "", false)
                    .is_err()
            );
            assert_eq!(
                std::fs::read_to_string(target).expect("read outside lock target"),
                "untouched"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn mcp_settings_write_stays_with_open_directory_after_parent_replacement() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().expect("disposable workspace");
        let ahead = workspace.path().join(".ahead");
        let moved = workspace.path().join("ahead-moved");
        let outside = tempfile::tempdir().expect("outside directory");
        std::fs::create_dir(&ahead).expect("AHEAD settings directory");
        std::fs::write(ahead.join("config.toml"), "source = 'inside'\n")
            .expect("inside declaration");
        std::fs::write(ahead.join("settings.toml"), "source = 'inside'\n")
            .expect("inside settings");
        std::fs::write(outside.path().join("config.toml"), "source = 'outside'\n")
            .expect("outside declaration");
        std::fs::write(outside.path().join("settings.toml"), "untouched = true\n")
            .expect("outside settings");
        let directory = open_mcp_settings_directory(
            &workspace
                .path()
                .canonicalize()
                .expect("canonical workspace"),
        )
        .expect("open AHEAD settings directory");
        std::fs::rename(&ahead, &moved).expect("move AHEAD directory");
        symlink(outside.path(), &ahead).expect("replace AHEAD path");

        let lock = open_mcp_settings_lock(&directory)
            .expect("lock stays inside opened directory");
        lock.lock_exclusive().expect("lock settings");
        assert_eq!(
            read_mcp_toml_at(&directory, "config.toml")
                .expect("read declaration")
                .expect("declaration exists")["source"]
                .as_str(),
            Some("inside")
        );
        assert_eq!(
            read_mcp_toml_at(&directory, "settings.toml")
                .expect("read settings")
                .expect("settings exist")["source"]
                .as_str(),
            Some("inside")
        );
        write_mcp_settings_at(&directory, "approved = true\n")
            .expect("write through opened directory");

        assert_eq!(
            std::fs::read_to_string(moved.join("settings.toml"))
                .expect("moved settings"),
            "approved = true\n"
        );
        assert_eq!(
            std::fs::read_to_string(outside.path().join("settings.toml"))
                .expect("outside settings"),
            "untouched = true\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn runtime_config_refuses_symlinks_without_touching_the_target() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("runtime home");
        let target = directory.path().join("outside.toml");
        let path = directory.path().join("config.toml");
        std::fs::write(&target, "outside config").expect("target config");
        symlink(&target, &path).expect("config symlink");

        let error = write_runtime_config(&path, "secret provider config")
            .expect_err("runtime config symlink must be rejected");

        assert!(format!("{error:#}").contains("must be a regular file"));
        assert_eq!(
            std::fs::read_to_string(target).expect("read target"),
            "outside config"
        );
        assert!(
            std::fs::symlink_metadata(path)
                .expect("inspect symlink")
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn scope_rejects_parent_traversal_and_accepts_descendants() {
        let workspace = Path::new("/workspace");
        assert!(path_is_allowed(
            "src/lib.rs",
            &["src".to_string()],
            workspace
        ));
        assert!(!path_is_allowed(
            "../secret",
            &["src".to_string()],
            workspace
        ));
        assert!(!path_is_allowed(
            "src-private/lib.rs",
            &["src".to_string()],
            workspace
        ));
    }

    #[test]
    fn empty_scope_allows_only_paths_inside_the_workspace() {
        let workspace = tempfile::tempdir().expect("workspace");
        let workspace_path = workspace.path();
        let outside_path = workspace_path
            .parent()
            .expect("temporary directory parent")
            .join("outside.txt");

        assert!(path_is_allowed("src/lib.rs", &[], workspace_path));
        assert!(path_is_allowed(
            &workspace_path.join("src/lib.rs").to_string_lossy(),
            &[],
            workspace_path
        ));
        assert!(!path_is_allowed("../outside.txt", &[], workspace_path));
        assert!(!path_is_allowed(
            &outside_path.to_string_lossy(),
            &[],
            workspace_path
        ));

        let scope = AgentScope {
            workspace: workspace_path.to_path_buf(),
            allowed_paths: Vec::new(),
            read_only: false,
        };
        assert!(file_change_allowed(
            "assist",
            &["src/lib.rs".to_string()],
            Some(&scope)
        ));
        assert!(!file_change_allowed(
            "assist",
            &[outside_path.to_string_lossy().into_owned()],
            Some(&scope)
        ));
    }

    #[test]
    fn file_changes_require_an_edit_scope() {
        let scope = AgentScope {
            workspace: PathBuf::from("/workspace"),
            allowed_paths: vec!["src".to_string()],
            read_only: false,
        };
        assert!(file_change_allowed(
            "assist",
            &["src/lib.rs".to_string()],
            Some(&scope)
        ));
        assert!(!file_change_allowed(
            "assist",
            &["README.md".to_string()],
            Some(&scope)
        ));
        assert!(!file_change_allowed(
            "read-only",
            &["src/lib.rs".to_string()],
            Some(&scope)
        ));
        assert!(!file_change_allowed(
            "assist",
            &["src/lib.rs".to_string()],
            None
        ));
    }

    #[test]
    fn assistance_profile_keeps_sandbox_network_restricted() {
        let scope = AgentScope {
            workspace: PathBuf::from("/workspace"),
            allowed_paths: vec!["src".to_string()],
            read_only: false,
        };

        let profile =
            permission_profile("agent", Path::new("/workspace"), Some(&scope))
                .expect("assistance permission profile");

        assert_eq!(
            profile.network_sandbox_policy(),
            NetworkSandboxPolicy::Restricted,
        );
    }

    #[test]
    fn managed_agent_cannot_read_or_write_private_ahead_state() {
        let workspace = tempfile::tempdir().expect("workspace");
        let root = workspace.path();
        let scope = AgentScope {
            workspace: root.to_path_buf(),
            allowed_paths: Vec::new(),
            read_only: false,
        };
        let profile = permission_profile("agent", root, Some(&scope))
            .expect("managed permission profile");
        let files = profile.file_system_sandbox_policy();
        assert!(files.can_write_path_with_cwd(&root.join("src/main.rs"), root));
        assert!(
            !files.can_read_path_with_cwd(&root.join(".ahead/settings.toml"), root)
        );
        assert!(
            !files.can_write_path_with_cwd(&root.join(".ahead/config.toml"), root)
        );
        if let Some(home) = crate::instructions::user_home() {
            assert!(
                !files.can_read_path_with_cwd(
                    &home.join(".ahead/settings.toml"),
                    root
                )
            );
        }
        assert!(!path_is_allowed(".ahead/settings.toml", &[], root));
        assert!(!file_change_allowed(
            "agent",
            &[".ahead/config.toml".to_string()],
            Some(&scope),
        ));
        let private_scope = AgentScope {
            allowed_paths: vec![".ahead/settings.toml".to_string()],
            ..scope.clone()
        };
        assert!(permission_profile("agent", root, Some(&private_scope)).is_err());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join(".ahead"), root.join("private"))
                .expect("create private alias");
            let alias_scope = AgentScope {
                allowed_paths: vec!["private/settings.toml".to_string()],
                ..scope
            };
            assert!(permission_profile("agent", root, Some(&alias_scope)).is_err());
        }
    }

    #[test]
    fn title_is_short_and_deterministic() {
        assert_eq!(
            generate_thread_title("Make the built in agent production ready now"),
            "Make the built in agent production"
        );
    }

    #[test]
    fn per_turn_read_only_scope_denies_an_otherwise_allowed_edit() {
        let scope = AgentScope {
            workspace: PathBuf::from("/workspace"),
            allowed_paths: vec!["src".to_string()],
            read_only: true,
        };
        assert!(!file_change_allowed(
            "agent",
            &["src/lib.rs".to_string()],
            Some(&scope)
        ));
        let profile =
            permission_profile("agent", Path::new("/workspace"), Some(&scope))
                .expect("read-only permission profile");
        let files = profile.file_system_sandbox_policy();
        assert!(!files.can_write_path_with_cwd(
            Path::new("/workspace/src/lib.rs"),
            Path::new("/workspace")
        ));
        assert!(!files.can_read_path_with_cwd(
            Path::new("/workspace/.ahead/settings.toml"),
            Path::new("/workspace")
        ));
    }

    #[test]
    fn workspace_provider_settings_override_user_defaults() {
        let temp = tempfile::tempdir().expect("temp dir");
        let user_home = temp.path().join("user");
        let workspace = temp.path().join("workspace");
        let user_settings = user_home.join(".ahead/settings.toml");
        let workspace_settings = workspace.join(".ahead/settings.toml");
        std::fs::create_dir_all(
            user_settings.parent().expect("user settings parent"),
        )
        .expect("create user settings directory");
        std::fs::create_dir_all(
            workspace_settings
                .parent()
                .expect("workspace settings parent"),
        )
        .expect("create workspace settings directory");
        std::fs::write(
            user_settings,
            "[ai]\nactive_connection = \"User\"\n[[ai.connections]]\nname = \"Shared\"\nprovider_id = \"shared\"\nbase_url = \"https://user.example/v1\"\nmodel = \"user-model\"\n[[ai.connections]]\nname = \"User only\"\nprovider_id = \"user-only\"\nbase_url = \"https://user-only.example/v1\"\nmodel = \"user-only-model\"\n",
        )
        .expect("write user settings");
        std::fs::write(
            workspace_settings,
            "[ai]\nactive_connection = \"Workspace\"\n[[ai.connections]]\nname = \"Workspace\"\nprovider_id = \"shared\"\nbase_url = \"https://workspace.example/v1\"\nmodel = \"workspace-model\"\n",
        )
        .expect("write workspace settings");

        let rendered = runtime_config_from_sources(&workspace, Some(&user_home))
            .expect("provider config");
        let config = rendered.parse::<toml::Table>().expect("valid config");
        assert_eq!(config["model"].as_str(), Some("workspace-model"));
        let providers = config["model_providers"].as_table().expect("providers");
        assert_eq!(
            providers["shared"]["base_url"].as_str(),
            Some("https://workspace.example/v1")
        );
        assert_eq!(providers["user-only"]["name"].as_str(), Some("User only"));
    }

    #[test]
    fn workspace_mcp_servers_require_opt_in_and_always_prompt() {
        let workspace = tempfile::tempdir().expect("workspace");
        let ahead = workspace.path().join(".ahead");
        std::fs::create_dir_all(&ahead).expect("create AHEAD config directory");
        std::fs::write(
            ahead.join("config.toml"),
            "[mcp.servers.docs]\ncommand = \"echo\"\nargs = [\"docs\"]\nenv_vars = [\"DOCS_TOKEN\"]\nenabled = true\ndefault_tools_approval_mode = \"approve\"\n[mcp.servers.docs.tools.search]\napproval_mode = \"approve\"\n",
        )
        .expect("write tracked server declaration");

        let without_opt_in = mcp_servers_for_workspace(
            workspace.path(),
            McpServerPolicy::PromptEveryCall,
        )
        .expect("unselected servers stay disabled");
        assert!(without_opt_in.is_empty());

        approve_docs(&ahead, "");
        let servers = mcp_servers_for_workspace(
            workspace.path(),
            McpServerPolicy::PromptEveryCall,
        )
        .expect("load explicitly selected server");
        let server = servers.get("docs").expect("selected server");
        assert!(server.enabled);
        assert_eq!(
            server.default_tools_approval_mode,
            Some(AppToolApproval::Prompt)
        );
        assert_eq!(
            server.tools["search"].approval_mode,
            Some(AppToolApproval::Prompt)
        );

        let declaration_path = ahead.join("config.toml");
        let original = std::fs::read_to_string(&declaration_path)
            .expect("read approved declaration");
        std::fs::write(&declaration_path, format!("# comment\n{original}"))
            .expect("rewrite formatting only");
        assert!(
            mcp_servers_for_workspace(
                workspace.path(),
                McpServerPolicy::PromptEveryCall,
            )
            .is_ok()
        );
        std::fs::write(
            &declaration_path,
            original.replace("[\"docs\"]", "[\"changed\"]"),
        )
        .expect("change executable arguments");
        let error = mcp_servers_for_workspace(
            workspace.path(),
            McpServerPolicy::PromptEveryCall,
        )
        .expect_err("changed command must require reapproval");
        assert!(format!("{error:#}").contains("declaration is not approved"));

        std::fs::write(&declaration_path, original).expect("restore declaration");
        std::fs::write(
            ahead.join("settings.toml"),
            "[mcp]\nenabled_servers = [\"docs\"]\n",
        )
        .expect("write legacy ID-only opt-in");
        assert!(
            mcp_servers_for_workspace(
                workspace.path(),
                McpServerPolicy::PromptEveryCall,
            )
            .is_err()
        );
    }

    #[test]
    fn local_mcp_tool_permissions_override_tracked_approval_modes() {
        let workspace = tempfile::tempdir().expect("workspace");
        let ahead = workspace.path().join(".ahead");
        std::fs::create_dir_all(&ahead).expect("create AHEAD config directory");
        std::fs::write(
            ahead.join("config.toml"),
            "[mcp.servers.docs]\ncommand = \"echo\"\ndefault_tools_approval_mode = \"approve\"\n[mcp.servers.docs.tools.read]\napproval_mode = \"approve\"\n",
        )
        .expect("write tracked declaration");
        approve_docs(
            &ahead,
            "[mcp.tool_permissions.docs]\nread = \"allow\"\nwrite = \"deny\"\nother = \"confirm\"\n",
        );

        let servers = mcp_servers_for_workspace(
            workspace.path(),
            McpServerPolicy::PromptEveryCall,
        )
        .expect("load local choices");
        let server = &servers["docs"];
        assert_eq!(
            server.default_tools_approval_mode,
            Some(AppToolApproval::Prompt)
        );
        assert_eq!(
            server.tools["read"].approval_mode,
            Some(AppToolApproval::Approve)
        );
        assert_eq!(
            server.disabled_tools.as_deref(),
            Some(["write".to_string()].as_slice())
        );
        assert_eq!(server.tools.get("other"), None);

        approve_docs(
            &ahead,
            "[mcp.tool_permissions.docs]\nread = \"automatic\"\n",
        );
        assert!(
            mcp_servers_for_workspace(
                workspace.path(),
                McpServerPolicy::PromptEveryCall,
            )
            .is_err()
        );
    }

    #[test]
    fn workspace_mcp_config_files_are_bounded() {
        let workspace = tempfile::tempdir().expect("workspace");
        let ahead = workspace.path().join(".ahead");
        std::fs::create_dir_all(&ahead).expect("create AHEAD config directory");
        std::fs::write(
            ahead.join("settings.toml"),
            vec![b' '; MAX_MCP_CONFIG_BYTES + 1],
        )
        .expect("write oversized MCP settings");

        let error = mcp_servers_for_workspace(
            workspace.path(),
            McpServerPolicy::PromptEveryCall,
        )
        .expect_err("oversized MCP config must be rejected");
        assert!(format!("{error:#}").contains("exceeds AHEAD's 1 MiB file limit"));
    }

    #[test]
    fn workspace_mcp_opt_in_must_match_a_declared_server() {
        let workspace = tempfile::tempdir().expect("workspace");
        let ahead = workspace.path().join(".ahead");
        std::fs::create_dir_all(&ahead).expect("create AHEAD config directory");
        std::fs::write(
            ahead.join("config.toml"),
            "[mcp.servers.docs]\ncommand = \"echo\"\n",
        )
        .expect("write server declaration");
        std::fs::write(
            ahead.join("settings.toml"),
            "[mcp]\nenabled_servers = [\"missing\"]\n",
        )
        .expect("write invalid opt-in");

        let error = mcp_servers_for_workspace(
            workspace.path(),
            McpServerPolicy::PromptEveryCall,
        )
        .expect_err("unknown MCP server IDs must fail closed");
        assert!(format!("{error:#}").contains("missing"));
    }

    #[test]
    fn workspace_mcp_servers_reject_inline_secrets_and_unwired_http_auth() {
        for declaration in [
            "[mcp.servers.docs]\ncommand = \"docs-mcp\"\nenv = { TOKEN = \"secret-placeholder\" }\n",
            "[mcp.servers.docs]\nurl = \"https://example.test/mcp\"\n",
        ] {
            let workspace = tempfile::tempdir().expect("workspace");
            let ahead = workspace.path().join(".ahead");
            std::fs::create_dir_all(&ahead).expect("create AHEAD config directory");
            std::fs::write(ahead.join("config.toml"), declaration)
                .expect("write server declaration");
            std::fs::write(
                ahead.join("settings.toml"),
                "[mcp]\nenabled_servers = [\"docs\"]\n",
            )
            .expect("write local server opt-in");

            assert!(
                mcp_servers_for_workspace(
                    workspace.path(),
                    McpServerPolicy::PromptEveryCall,
                )
                .is_err()
            );
        }
    }
}
