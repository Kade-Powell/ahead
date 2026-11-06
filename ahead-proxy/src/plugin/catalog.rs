use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    fs::OpenOptions,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use ahead_extension_host::{
    ExtensionHost, LanguageServerCommand, LanguageServerExtension, NodeRuntime,
    NpmInvocation, WorktreeContext, discover_language_server_extensions,
    uninstall_extension_package,
};
use ahead_rpc::plugin::ServerId;
use ahead_rpc::{
    RpcError,
    dap_types::{self, DapId, DapServer, RunDebugConfig, SetBreakpointsResponse},
    delta::AheadDelta,
    plugin::PluginId,
    proxy::{ProxyNotification, ProxyRpcHandler},
    style::LineStyle,
};
use anyhow::Context;
use fs4::fs_std::FileExt;
use jsonrpc_lite::Id;
use lsp_types::{
    Diagnostic, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentDiagnosticParams, DocumentDiagnosticReport,
    DocumentDiagnosticReportResult, PublishDiagnosticsParams, SemanticTokens,
    TextDocumentIdentifier, TextDocumentItem, VersionedTextDocumentIdentifier,
    notification::{DidCloseTextDocument, DidOpenTextDocument, Notification},
    request::{DocumentDiagnosticRequest, Request},
};
use parking_lot::Mutex;
use ropey::Rope;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::lsp::LspClient;
use super::{
    DiagnosticSource, PluginCatalogNotification, PluginCatalogRpcHandler,
    dap::{DapClient, DapRpcHandler},
    psp::{ClonableCallback, PluginServerRpc, PluginServerRpcHandler, RpcCallback},
    routing::LanguageServerRouting,
    watched_files::WatchedFiles,
};

pub struct PluginCatalog {
    routing: Result<LanguageServerRouting, String>,
    watched_files: WatchedFiles,
    workspace: Option<PathBuf>,
    plugin_rpc: PluginCatalogRpcHandler,
    proxy_rpc: Option<ProxyRpcHandler>,
    installing_servers: Arc<Mutex<HashSet<&'static str>>>,
    checked_builtin_npm_servers: HashSet<&'static str>,
    node_runtime: NodeRuntime,
    cancel_installs: Arc<AtomicBool>,
    resolving_servers: Arc<AtomicUsize>,
    pending_extension_resolutions: HashMap<String, u64>,
    pending_builtin_npm_version_checks:
        HashMap<PluginId, (BuiltinLanguageServer, PathBuf)>,
    plugins: HashMap<PluginId, PluginServerRpcHandler>,
    daps: HashMap<DapId, DapRpcHandler>,
    open_files: HashMap<PathBuf, OpenDocument>,
    diagnostics:
        HashMap<url::Url, HashMap<(PluginId, DiagnosticSource), Vec<Diagnostic>>>,
    extension_host: Option<Arc<ExtensionHost>>,
    language_extensions: Vec<LanguageServerExtension>,
    extension_servers: HashMap<PluginId, LanguageServerExtension>,
    extension_commands: HashMap<PluginId, LanguageServerCommand>,
    configuration_generation: u64,
    pending_builtin_resolutions: HashMap<String, u64>,
    /// Running servers by language id, including complementary analyzers/formatters.
    lsp_servers: HashMap<String, Vec<PluginId>>,
}

#[derive(Clone, Copy)]
struct BuiltinLanguageServer {
    command: &'static str,
    languages: &'static [&'static str],
    args: &'static [&'static str],
}

impl BuiltinLanguageServer {
    fn npm_package(&self) -> Option<(&'static str, &'static str, &'static str)> {
        match self.command {
            "basedpyright-langserver" => {
                Some(("basedpyright", "basedpyright", "langserver.index.js"))
            }
            "vtsls" => Some(("vtsls", "@vtsls/language-server", "bin/vtsls.js")),
            _ => None,
        }
    }

    fn cached_npm_command(
        &self,
        cache: &Path,
        node: &Path,
    ) -> Option<LanguageServerCommand> {
        let (directory, package_name, entrypoint) = self.npm_package()?;
        let package = cache
            .join("language-servers")
            .join(directory)
            .join("node_modules")
            .join(package_name);
        let script = valid_npm_entrypoint(&package, package_name, entrypoint)?;
        Some(LanguageServerCommand {
            command: node.to_str()?.into(),
            args: vec![script.to_str()?.into(), "--stdio".into()],
            env: Vec::new(),
            initialization_options: None,
            initialization_options_schema: None,
            workspace_configuration: None,
            workspace_configuration_schema: None,
        })
    }
}

fn valid_npm_entrypoint(
    package: &Path,
    expected_name: &str,
    entrypoint: &str,
) -> Option<PathBuf> {
    let script = package.join(entrypoint);
    let manifest = package.join("package.json");
    if !std::fs::symlink_metadata(&script).ok()?.is_file()
        || !std::fs::symlink_metadata(&manifest).ok()?.is_file()
    {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(manifest)
        .ok()?
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 64 * 1024 {
        return None;
    }
    let manifest: Value = serde_json::from_slice(&bytes).ok()?;
    let version = manifest.get("version")?.as_str()?;
    (manifest.get("name")?.as_str()? == expected_name
        && semver::Version::parse(version).is_ok())
    .then_some(script)
}

fn cached_npm_package_version(
    cache: &Path,
    builtin: BuiltinLanguageServer,
) -> Option<semver::Version> {
    let (directory, package_name, entrypoint) = builtin.npm_package()?;
    let package = cache
        .join("language-servers")
        .join(directory)
        .join("node_modules")
        .join(package_name);
    valid_npm_entrypoint(&package, package_name, entrypoint)?;
    let mut bytes = Vec::new();
    std::fs::File::open(package.join("package.json"))
        .ok()?
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 64 * 1024 {
        return None;
    }
    let manifest: Value = serde_json::from_slice(&bytes).ok()?;
    semver::Version::parse(manifest.get("version")?.as_str()?).ok()
}

fn parse_npm_package_version(output: &[u8]) -> anyhow::Result<semver::Version> {
    let output = std::str::from_utf8(output)?.trim();
    let version =
        serde_json::from_str::<String>(output).unwrap_or_else(|_| output.to_owned());
    semver::Version::parse(version.trim())
        .context("npm returned an invalid package version")
}

fn npm_update_available(
    installed: &semver::Version,
    latest: &semver::Version,
) -> bool {
    latest > installed
}

fn acquire_language_server_cache_lock(
    root: &Path,
    name: &str,
    cancelled: &AtomicBool,
) -> anyhow::Result<std::fs::File> {
    std::fs::create_dir_all(root)?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let lock_path = root.join(format!(".{name}.lock"));
    let lock = options.open(&lock_path)?;
    anyhow::ensure!(
        lock.metadata()?.is_file(),
        "invalid {name} language-server cache lock"
    );
    loop {
        anyhow::ensure!(
            !cancelled.load(Ordering::Relaxed),
            "installing {name} was cancelled"
        );
        anyhow::ensure!(
            Instant::now() < deadline,
            "timed out waiting for {name} language-server cache lock"
        );
        if FileExt::try_lock_exclusive(&lock)? {
            return Ok(lock);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

const MAX_NPM_STDERR_BYTES: usize = 16 * 1024;
const MAX_NPM_VERSION_OUTPUT_BYTES: usize = 64 * 1024;
const NPM_VERSION_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

fn read_npm_stderr_tail(mut stderr: impl Read) -> std::io::Result<String> {
    let mut tail = VecDeque::with_capacity(MAX_NPM_STDERR_BYTES);
    let mut chunk = [0; 4096];
    loop {
        let bytes_read = stderr.read(&mut chunk)?;
        if bytes_read == 0 {
            break;
        }
        for byte in &chunk[..bytes_read] {
            if tail.len() == MAX_NPM_STDERR_BYTES {
                tail.pop_front();
            }
            tail.push_back(*byte);
        }
    }
    let tail = tail.into_iter().collect::<Vec<_>>();
    Ok(String::from_utf8_lossy(&tail).into_owned())
}

fn read_bounded_npm_version_output(
    mut stdout: impl Read,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::new();
    let mut oversized = false;
    let mut chunk = [0; 4096];
    loop {
        let bytes_read = stdout.read(&mut chunk)?;
        if bytes_read == 0 {
            break;
        }
        let remaining = MAX_NPM_VERSION_OUTPUT_BYTES.saturating_sub(output.len());
        let retained = bytes_read.min(remaining);
        output.extend_from_slice(&chunk[..retained]);
        oversized |= retained < bytes_read;
    }
    Ok((output, oversized))
}

fn spawn_npm_version_output_reader(
    stdout: std::process::ChildStdout,
) -> thread::JoinHandle<std::io::Result<(Vec<u8>, bool)>> {
    thread::spawn(move || read_bounded_npm_version_output(stdout))
}

fn join_npm_version_output_reader(
    reader: thread::JoinHandle<std::io::Result<(Vec<u8>, bool)>>,
) -> anyhow::Result<(Vec<u8>, bool)> {
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("npm version output reader panicked"))?
        .context("reading npm version output")
}

fn latest_npm_package_version(
    npm: &NpmInvocation,
    package: &str,
    cancelled: &AtomicBool,
) -> anyhow::Result<semver::Version> {
    let mut child = Command::new(&npm.program)
        .args(&npm.prefix_arguments)
        .args([
            "view",
            package,
            "version",
            "--json",
            "--fetch-timeout=5000",
            "--fetch-retries=0",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting npm version lookup for {package}"))?;
    let Some(stdout) = child.stdout.take() else {
        if let Err(error) = child.kill() {
            tracing::warn!(?error, package, "stopping npm without version output");
        }
        if let Err(error) = child.wait() {
            tracing::warn!(?error, package, "reaping npm without version output");
        }
        anyhow::bail!("npm version output was unavailable for {package}");
    };
    let reader = spawn_npm_version_output_reader(stdout);
    let deadline = Instant::now() + NPM_VERSION_LOOKUP_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if cancelled.load(Ordering::Relaxed) => {
                break Err(anyhow::anyhow!("npm version lookup was cancelled"));
            }
            Ok(None) if Instant::now() >= deadline => {
                break Err(anyhow::anyhow!("npm version lookup timed out"));
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => break Err(error.into()),
        }
    };
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            if let Err(kill_error) = child.kill() {
                tracing::warn!(?kill_error, package, "stopping npm version lookup");
            }
            if let Err(wait_error) = child.wait() {
                tracing::warn!(?wait_error, package, "reaping npm version lookup");
            }
            if let Err(reader_error) = join_npm_version_output_reader(reader) {
                tracing::debug!(
                    ?reader_error,
                    package,
                    "discarding npm version output"
                );
            }
            return Err(error)
                .with_context(|| format!("checking npm package {package}"));
        }
    };
    let (output, oversized) = join_npm_version_output_reader(reader)?;
    anyhow::ensure!(
        status.success(),
        "npm could not query the latest {package} version"
    );
    anyhow::ensure!(
        !oversized,
        "npm package version output exceeded the size limit"
    );
    parse_npm_package_version(&output)
}

fn npm_error_code(stderr: &str) -> Option<&str> {
    stderr.lines().rev().find_map(|line| {
        let line = line.trim();
        let code = line
            .strip_prefix("npm error code ")
            .or_else(|| line.strip_prefix("npm ERR! code "))?
            .trim();
        (!code.is_empty()
            && code.len() <= 32
            && code.bytes().all(|byte| {
                byte.is_ascii_uppercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'_' | b'-')
            }))
        .then_some(code)
    })
}

fn npm_diagnostic_suffix(stderr: &str) -> String {
    npm_error_code(stderr)
        .map(|code| format!("; npm reported {code}"))
        .unwrap_or_default()
}

fn spawn_npm_stderr_reader(
    stderr: std::process::ChildStderr,
) -> thread::JoinHandle<String> {
    thread::spawn(move || match read_npm_stderr_tail(stderr) {
        Ok(stderr) => stderr,
        Err(error) => {
            tracing::warn!(?error, "reading npm install diagnostics");
            String::new()
        }
    })
}

fn join_npm_stderr_reader(reader: thread::JoinHandle<String>) -> String {
    match reader.join() {
        Ok(stderr) => stderr,
        Err(_) => {
            tracing::warn!("npm install diagnostic reader panicked");
            String::new()
        }
    }
}

#[cfg(test)]
fn install_builtin_npm_package(
    cache: &Path,
    npm: &Path,
    builtin: BuiltinLanguageServer,
    cancelled: &AtomicBool,
) -> anyhow::Result<()> {
    install_builtin_npm_package_with_invocation(
        cache,
        &NpmInvocation {
            program: npm.to_path_buf(),
            prefix_arguments: Vec::new(),
        },
        builtin,
        cancelled,
    )
}

fn install_builtin_npm_package_with_invocation(
    cache: &Path,
    npm: &NpmInvocation,
    builtin: BuiltinLanguageServer,
    cancelled: &AtomicBool,
) -> anyhow::Result<()> {
    let (directory, package, entrypoint) = builtin
        .npm_package()
        .ok_or_else(|| anyhow::anyhow!("{} has no npm package", builtin.command))?;
    let root = cache.join("language-servers");
    let _lock = acquire_language_server_cache_lock(&root, directory, cancelled)?;
    let staging = tempfile::Builder::new()
        .prefix(&format!(".{directory}-install-"))
        .tempdir_in(&root)?;
    anyhow::ensure!(
        !cancelled.load(Ordering::Relaxed),
        "npm install of {package} was cancelled"
    );
    let mut child = Command::new(&npm.program)
        .args(&npm.prefix_arguments)
        .args(["install", "--prefix"])
        .arg(staging.path())
        .arg(format!("{package}@latest"))
        .args(["--cache"])
        .arg(root.join("npm-cache"))
        .args([
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--no-package-lock",
            "--save-exact",
            "--fetch-retries=2",
            "--fetch-timeout=10000",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let Some(stderr) = child.stderr.take() else {
        if let Err(error) = child.kill() {
            tracing::warn!(
                ?error,
                package,
                "stopping npm without a diagnostic pipe"
            );
        }
        if let Err(error) = child.wait() {
            tracing::warn!(?error, package, "reaping npm without a diagnostic pipe");
        }
        anyhow::bail!(
            "npm diagnostic stream was unavailable while installing {package}"
        );
    };
    let stderr_reader = spawn_npm_stderr_reader(stderr);
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if cancelled.load(Ordering::Relaxed) => {
                break Err(anyhow::anyhow!(
                    "npm install of {package} was cancelled"
                ));
            }
            Ok(None) if Instant::now() >= deadline => {
                break Err(anyhow::anyhow!(
                    "npm install of {package} timed out after 120 seconds"
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => break Err(error.into()),
        }
    };
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            if let Err(kill_error) = child.kill() {
                tracing::warn!(?kill_error, package, "stopping npm install");
            }
            if let Err(wait_error) = child.wait() {
                tracing::warn!(?wait_error, package, "reaping npm install");
            }
            let stderr = join_npm_stderr_reader(stderr_reader);
            return Err(anyhow::anyhow!(
                "{error}{}",
                npm_diagnostic_suffix(&stderr)
            ));
        }
    };
    let stderr = join_npm_stderr_reader(stderr_reader);
    anyhow::ensure!(
        status.success(),
        "npm could not install {package} ({status}{})",
        npm_diagnostic_suffix(&stderr)
    );
    anyhow::ensure!(
        !cancelled.load(Ordering::Relaxed),
        "npm install of {package} was cancelled"
    );
    let installed = staging.path().join("node_modules").join(package);
    anyhow::ensure!(
        valid_npm_entrypoint(&installed, package, entrypoint).is_some(),
        "npm finished without a valid {package} language-server package"
    );
    let destination = root.join(directory);
    let backup = root.join(format!(".{directory}-backup-{}", uuid::Uuid::new_v4()));
    let replaced = destination.exists();
    if replaced {
        std::fs::rename(&destination, &backup)?;
    }
    if let Err(error) = std::fs::rename(staging.path(), &destination) {
        if replaced {
            std::fs::rename(&backup, &destination)?;
        }
        return Err(error.into());
    }
    if replaced {
        if let Err(error) = std::fs::remove_dir_all(&backup) {
            tracing::warn!(?error, path = %backup.display(), "removing replaced language-server cache");
        }
    }
    Ok(())
}

fn remove_cached_builtin_npm_package(
    cache: &Path,
    builtin: BuiltinLanguageServer,
    cancelled: &AtomicBool,
) -> anyhow::Result<()> {
    let (directory, _, _) = builtin.npm_package().ok_or_else(|| {
        anyhow::anyhow!("{} has no managed npm package", builtin.command)
    })?;
    let root = cache.join("language-servers");
    ensure_real_language_server_cache_root(&root)?;
    let _lock = acquire_language_server_cache_lock(&root, directory, cancelled)?;
    ensure_real_language_server_cache_root(&root)?;
    anyhow::ensure!(
        !cancelled.load(Ordering::Relaxed),
        "repairing {} was cancelled",
        builtin.command
    );

    let package_directory = root.join(directory);
    match std::fs::symlink_metadata(&package_directory) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            std::fs::remove_dir_all(package_directory).with_context(|| {
                format!("removing cached {} package", builtin.command)
            })?;
        }
        Ok(_) => anyhow::bail!(
            "refusing to remove non-directory {} package cache",
            builtin.command
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!("checking cached {} package", builtin.command)
            });
        }
    }
    Ok(())
}

fn ensure_real_language_server_cache_root(root: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(root) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "refusing to use language-server cache root {} because it is not a directory",
                root.display()
            );
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!("checking language-server cache {}", root.display())
        }),
    }
}

const MAX_NATIVE_SERVER_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_NATIVE_SERVER_BINARY_BYTES: u64 = 256 * 1024 * 1024;
const NATIVE_SERVER_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(serde::Deserialize)]
struct NativeServerRelease {
    tag_name: String,
    assets: Vec<NativeServerReleaseAsset>,
}

#[derive(serde::Deserialize)]
struct NativeServerReleaseAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

fn rust_analyzer_release_asset_name() -> anyhow::Result<(String, bool)> {
    #[cfg(target_os = "macos")]
    let (platform, is_zip) = ("apple-darwin", false);
    #[cfg(target_os = "linux")]
    let (platform, is_zip) = (
        if cfg!(target_env = "musl") {
            "unknown-linux-musl"
        } else {
            "unknown-linux-gnu"
        },
        false,
    );
    #[cfg(target_os = "windows")]
    let (platform, is_zip) = ("pc-windows-msvc", true);
    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    )))]
    anyhow::bail!(
        "managed Rust Analyzer downloads are unsupported on this platform"
    );

    Ok((
        format!(
            "rust-analyzer-{}-{platform}.{}",
            std::env::consts::ARCH,
            if is_zip { "zip" } else { "gz" }
        ),
        is_zip,
    ))
}

fn native_server_binary_path(cache: &Path, server_name: &str) -> PathBuf {
    cache
        .join("language-servers")
        .join(server_name)
        .join(if cfg!(windows) {
            format!("{server_name}.exe")
        } else {
            server_name.to_string()
        })
}

fn cached_native_server_binary(cache: &Path, server_name: &str) -> Option<PathBuf> {
    let directory = cache.join("language-servers").join(server_name);
    let binary = native_server_binary_path(cache, server_name);
    let binary_metadata = std::fs::symlink_metadata(&binary).ok()?;
    if !binary_metadata.file_type().is_file() {
        return None;
    }
    let metadata_path = directory.join("metadata.json");
    let mut metadata = Vec::new();
    std::fs::File::open(metadata_path)
        .ok()?
        .take(64 * 1024 + 1)
        .read_to_end(&mut metadata)
        .ok()?;
    if metadata.len() > 64 * 1024 {
        return None;
    }
    let metadata: Value = serde_json::from_slice(&metadata).ok()?;
    (!metadata
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .is_empty())
    .then_some(binary)
}

fn stop_probe_process(child: &mut std::process::Child) {
    if let Err(error) = child.kill() {
        tracing::debug!(?error, "stopping language-server probe");
    }
    if let Err(error) = child.wait() {
        tracing::debug!(?error, "reaping language-server probe");
    }
}

fn native_server_accepts(
    path: &Path,
    argument: &str,
    cancelled: &AtomicBool,
) -> anyhow::Result<bool> {
    anyhow::ensure!(
        !cancelled.load(Ordering::Relaxed),
        "Language server resolution was cancelled"
    );
    let mut child = match Command::new(path)
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            tracing::debug!(path = %path.display(), ?error, "language server probe could not start");
            return Ok(false);
        }
    };
    let deadline = Instant::now() + NATIVE_SERVER_PROBE_TIMEOUT;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            stop_probe_process(&mut child);
            anyhow::bail!("Language server resolution was cancelled");
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) if Instant::now() >= deadline => {
                stop_probe_process(&mut child);
                return Ok(false);
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                stop_probe_process(&mut child);
                return Err(error).context("checking language server process");
            }
        }
    }
}

fn rust_analyzer_command(path: &Path) -> LanguageServerCommand {
    LanguageServerCommand {
        command: path.to_string_lossy().into_owned(),
        args: Vec::new(),
        env: Vec::new(),
        initialization_options: None,
        initialization_options_schema: None,
        workspace_configuration: None,
        workspace_configuration_schema: None,
    }
}

fn workspace_has_rust_toolchain_override(workspace: &Path) -> bool {
    ["rust-toolchain.toml", "rust-toolchain"]
        .iter()
        .any(|file_name| workspace.join(file_name).is_file())
}

fn rustup_rust_analyzer_for_workspace(
    workspace: &Path,
    rustup: &Path,
    cancelled: &AtomicBool,
) -> anyhow::Result<Option<PathBuf>> {
    anyhow::ensure!(
        !cancelled.load(Ordering::Relaxed),
        "Rust Analyzer resolution was cancelled"
    );
    let mut child = match Command::new(rustup)
        .args(["which", "rust-analyzer"])
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            tracing::debug!(?rustup, ?error, "running rustup which rust-analyzer");
            return Ok(None);
        }
    };
    let deadline = Instant::now() + NATIVE_SERVER_PROBE_TIMEOUT;
    let status = loop {
        if cancelled.load(Ordering::Relaxed) {
            stop_probe_process(&mut child);
            anyhow::bail!("Rust Analyzer resolution was cancelled");
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                stop_probe_process(&mut child);
                tracing::debug!(?rustup, "rustup which rust-analyzer timed out");
                return Ok(None);
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                stop_probe_process(&mut child);
                return Err(error).context("checking rustup process");
            }
        }
    };
    if !status.success() {
        tracing::debug!(
            ?rustup,
            ?status,
            "rustup could not find the worktree Rust Analyzer"
        );
        return Ok(None);
    }
    let output = child
        .wait_with_output()
        .context("reading rustup which rust-analyzer output")?;
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    Ok((!path.as_os_str().is_empty()).then_some(path))
}

fn resolve_rust_analyzer_command(
    cache: &Path,
    workspace: &Path,
    cancelled: &AtomicBool,
) -> anyhow::Result<LanguageServerCommand> {
    if workspace_has_rust_toolchain_override(workspace)
        && let Ok(rustup) = which::which("rustup")
        && let Some(path) =
            rustup_rust_analyzer_for_workspace(workspace, &rustup, cancelled)?
    {
        if native_server_accepts(&path, "--help", cancelled)? {
            return Ok(rust_analyzer_command(&path));
        }
        tracing::info!(
            path = %path.display(),
            "ignoring non-runnable Rust Analyzer selected by rustup"
        );
    }

    if let Ok(path) = which::which("rust-analyzer") {
        if native_server_accepts(&path, "--help", cancelled)? {
            return Ok(rust_analyzer_command(&path));
        }
        tracing::info!(
            path = %path.display(),
            "ignoring non-runnable Rust Analyzer found on PATH"
        );
    }

    if let Some(path) = cached_native_server_binary(cache, "rust-analyzer")
        && native_server_accepts(&path, "--version", cancelled)?
    {
        return Ok(rust_analyzer_command(&path));
    }

    install_builtin_native_server(cache, "rust-analyzer", cancelled)?;
    let path = cached_native_server_binary(cache, "rust-analyzer")
        .context("Rust Analyzer installation completed without a cached binary")?;
    anyhow::ensure!(
        native_server_accepts(&path, "--version", cancelled)?,
        "installed Rust Analyzer failed its version check"
    );
    Ok(rust_analyzer_command(&path))
}

fn ruff_command(path: &Path) -> LanguageServerCommand {
    LanguageServerCommand {
        command: path.to_string_lossy().into_owned(),
        args: vec!["server".into()],
        env: Vec::new(),
        initialization_options: None,
        initialization_options_schema: None,
        workspace_configuration: None,
        workspace_configuration_schema: None,
    }
}

fn resolve_ruff_command(
    cache: &Path,
    workspace: &Path,
    cancelled: &AtomicBool,
) -> anyhow::Result<LanguageServerCommand> {
    let virtualenv_binary = workspace.join(if cfg!(windows) {
        ".venv/Scripts/ruff.exe"
    } else {
        ".venv/bin/ruff"
    });
    for path in std::iter::once(virtualenv_binary).chain(which::which("ruff").ok()) {
        if path.is_file() && native_server_accepts(&path, "--version", cancelled)? {
            return Ok(ruff_command(&path));
        }
    }
    if let Some(path) = cached_native_server_binary(cache, "ruff")
        && native_server_accepts(&path, "--version", cancelled)?
    {
        return Ok(ruff_command(&path));
    }
    install_builtin_native_server(cache, "ruff", cancelled)?;
    let path = cached_native_server_binary(cache, "ruff")
        .context("Ruff installation completed without a cached binary")?;
    Ok(ruff_command(&path))
}

fn ruff_release_asset_name() -> anyhow::Result<(String, bool)> {
    #[cfg(target_os = "macos")]
    let platform = "apple-darwin";
    #[cfg(target_os = "linux")]
    let platform = if cfg!(target_env = "musl") {
        "unknown-linux-musl"
    } else {
        "unknown-linux-gnu"
    };
    #[cfg(target_os = "windows")]
    let platform = "pc-windows-msvc";
    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    )))]
    anyhow::bail!("managed Ruff downloads are unsupported on this platform");
    let architecture = match std::env::consts::ARCH {
        "x86" => "i686",
        architecture => architecture,
    };
    Ok((
        format!(
            "ruff-{architecture}-{platform}.{}",
            if cfg!(windows) { "zip" } else { "tar.gz" }
        ),
        cfg!(windows),
    ))
}

fn read_bounded_response(
    response: reqwest::blocking::Response,
    maximum_bytes: u64,
) -> anyhow::Result<Vec<u8>> {
    let response = response.error_for_status()?;
    if let Some(length) = response.content_length() {
        anyhow::ensure!(
            length <= maximum_bytes,
            "language-server download exceeds the size limit"
        );
    }
    let mut reader = response.take(maximum_bytes + 1);
    let mut body = Vec::new();
    reader.read_to_end(&mut body)?;
    anyhow::ensure!(
        body.len() as u64 <= maximum_bytes,
        "language-server download exceeds the size limit"
    );
    Ok(body)
}

fn write_bounded_binary(reader: impl Read, path: &Path) -> anyhow::Result<()> {
    let mut reader = reader.take(MAX_NATIVE_SERVER_BINARY_BYTES + 1);
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let written = std::io::copy(&mut reader, &mut file)?;
    anyhow::ensure!(
        written <= MAX_NATIVE_SERVER_BINARY_BYTES,
        "Language server binary exceeds the size limit"
    );
    file.sync_all()?;
    Ok(())
}

fn install_builtin_native_server(
    cache: &Path,
    server_name: &str,
    cancelled: &AtomicBool,
) -> anyhow::Result<()> {
    let root = cache.join("language-servers");
    let _lock = acquire_language_server_cache_lock(&root, server_name, cancelled)?;
    if let Some(path) = cached_native_server_binary(cache, server_name)
        && native_server_accepts(&path, "--version", cancelled)?
    {
        return Ok(());
    }
    let (repository, (asset_name, is_zip)) = match server_name {
        "rust-analyzer" => (
            "rust-lang/rust-analyzer",
            rust_analyzer_release_asset_name()?,
        ),
        "ruff" => ("astral-sh/ruff", ruff_release_asset_name()?),
        _ => anyhow::bail!("unsupported native language server: {server_name}"),
    };
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("AHEAD/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(90))
        .build()?;
    let release_response = client
        .get(format!(
            "https://api.github.com/repos/{repository}/releases/latest"
        ))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()?;
    let release_bytes = read_bounded_response(release_response, 8 * 1024 * 1024)?;
    let release: NativeServerRelease = serde_json::from_slice(&release_bytes)?;
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == asset_name)
        .with_context(|| {
            format!("{server_name} release has no `{asset_name}` asset")
        })?;
    let asset_url = url::Url::parse(&asset.browser_download_url)?;
    anyhow::ensure!(
        asset_url.scheme() == "https"
            && asset_url.host_str() == Some("github.com")
            && asset_url.username().is_empty()
            && asset_url.password().is_none()
            && asset_url
                .path()
                .starts_with(&format!("/{repository}/releases/download/")),
        "Language server release asset URL is not an official HTTPS GitHub URL"
    );
    anyhow::ensure!(
        !cancelled.load(Ordering::Relaxed),
        "Language server installation was cancelled"
    );
    let archive_response = client.get(asset_url).send()?;
    let archive =
        read_bounded_response(archive_response, MAX_NATIVE_SERVER_ARCHIVE_BYTES)?;
    let archive_sha256 = format!("{:x}", Sha256::digest(&archive));
    if let Some(digest) = asset.digest.as_deref() {
        let expected = digest
            .strip_prefix("sha256:")
            .context("GitHub supplied an unsupported language server digest")?;
        anyhow::ensure!(
            archive_sha256.eq_ignore_ascii_case(expected),
            "Language server release checksum does not match GitHub metadata"
        );
    }

    let staging = tempfile::Builder::new()
        .prefix(&format!(".{server_name}-install-"))
        .tempdir_in(&root)?;
    let binary_name = if cfg!(windows) {
        format!("{server_name}.exe")
    } else {
        server_name.to_string()
    };
    let binary = staging.path().join(&binary_name);
    if is_zip {
        let mut zip = zip::ZipArchive::new(Cursor::new(archive))?;
        let mut entry = zip.by_name(&binary_name)?;
        write_bounded_binary(&mut entry, &binary)?;
    } else if server_name == "ruff" {
        extract_ruff_binary(&archive, &asset_name, &binary)?;
    } else {
        let decoder = flate2::read::GzDecoder::new(Cursor::new(archive));
        write_bounded_binary(decoder, &binary)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))?;
    }
    anyhow::ensure!(
        native_server_accepts(&binary, "--version", cancelled)?,
        "downloaded language server failed its version check"
    );
    std::fs::write(
        staging.path().join("metadata.json"),
        serde_json::to_vec(&serde_json::json!({
            "version": release.tag_name,
            "asset": asset_name,
            "digest": asset.digest,
            "archive_sha256": archive_sha256,
        }))?,
    )?;
    anyhow::ensure!(
        !cancelled.load(Ordering::Relaxed),
        "Language server installation was cancelled"
    );

    let destination = root.join(server_name);
    let replaced = match std::fs::symlink_metadata(&destination) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.file_type().is_dir(),
                "existing language server cache is not a directory"
            );
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    let backup =
        root.join(format!(".{server_name}-backup-{}", uuid::Uuid::new_v4()));
    if replaced {
        std::fs::rename(&destination, &backup)?;
    }
    if let Err(error) = std::fs::rename(staging.path(), &destination) {
        if replaced {
            std::fs::rename(&backup, &destination)?;
        }
        return Err(error.into());
    }
    if replaced && let Err(error) = std::fs::remove_dir_all(&backup) {
        tracing::warn!(?error, path = %backup.display(), "removing replaced native language server cache");
    }
    Ok(())
}

fn extract_ruff_binary(
    archive: &[u8],
    asset_name: &str,
    binary: &Path,
) -> anyhow::Result<()> {
    let stem = asset_name
        .strip_suffix(".tar.gz")
        .context("invalid Ruff archive name")?;
    let expected = Path::new(stem).join("ruff");
    let decoder = flate2::read::GzDecoder::new(Cursor::new(archive));
    let mut archive =
        tar::Archive::new(decoder.take(MAX_NATIVE_SERVER_BINARY_BYTES + 1));
    for (index, entry) in archive.entries()?.enumerate() {
        anyhow::ensure!(index < 256, "Ruff archive exceeds the entry limit");
        let mut entry = entry?;
        if entry.path()? == expected {
            anyhow::ensure!(
                entry.header().entry_type().is_file(),
                "Ruff binary is not a regular file"
            );
            return write_bounded_binary(&mut entry, binary);
        }
    }
    anyhow::bail!("Ruff archive has no {}", expected.display())
}

fn builtin_language_server(language_id: &str) -> Option<BuiltinLanguageServer> {
    let (command, languages, args): (_, &[_], &[_]) =
        match builtin_protocol_language_id(language_id)? {
            "rust" => ("rust-analyzer", &["rust", "Rust"], &[]),
            "python" => (
                "basedpyright-langserver",
                &["python", "Python"],
                &["--stdio"],
            ),
            "javascript" | "javascriptreact" | "typescript" | "typescriptreact" => (
                "vtsls",
                &[
                    "javascript",
                    "javascriptreact",
                    "typescript",
                    "typescriptreact",
                    "JavaScript",
                    "JSX",
                    "TypeScript",
                    "TSX",
                ],
                &["--stdio"],
            ),
            _ => return None,
        };
    Some(BuiltinLanguageServer {
        command,
        languages,
        args,
    })
}

fn builtin_language_servers(language_id: &str) -> Vec<BuiltinLanguageServer> {
    let mut servers = builtin_language_server(language_id)
        .into_iter()
        .collect::<Vec<_>>();
    if builtin_protocol_language_id(language_id) == Some("python") {
        servers.push(BuiltinLanguageServer {
            command: "ruff",
            languages: &["python", "Python"],
            args: &["server"],
        });
    }
    servers
}

fn zed_default_language_servers(
    language: &str,
    available: &[String],
    defaults: Vec<String>,
    opt_in: &[String],
) -> Vec<String> {
    // Mirror Zed's server order and exclusions from ../zed/assets/settings/default.json at 37faef8.
    // The final "..." in those settings means every other installed, non-opt-in server.
    let language = language.to_ascii_lowercase();
    let language = match language.as_str() {
        "jsx" | "javascriptreact" => "javascript",
        "tsx" | "typescriptreact" => "typescript",
        language => language,
    };
    let (preferred, excluded): (&[&[&str]], &[&str]) = match language {
        "astro" => (&[&["astro-language-server"]], &[]),
        "csharp" => (&[&["roslyn"]], &["csharp-ls", "omnisharp"]),
        "eex" | "heex" => (
            &[&["elixir-ls"]],
            &["expert", "dexter", "next-ls", "lexical"],
        ),
        "elixir" => (
            &[&["elixir-ls"]],
            &[
                "expert",
                "dexter",
                "next-ls",
                "lexical",
                "emmet-language-server",
            ],
        ),
        "erlang" => (&[&["erlang-ls"]], &["elp"]),
        "html+erb" => (&[&["herb"]], &["ruby-lsp"]),
        "javascript" | "typescript" => {
            (&[&["vtsls"]], &["typescript-language-server"])
        }
        "js+erb" | "yaml+erb" => (&[], &["ruby-lsp"]),
        "kotlin" => (&[&["kotlin-lsp"]], &["kotlin-language-server"]),
        "latex" => (&[&["texlab"]], &[]),
        "php" => (&[&["phpactor"]], &["intelephense", "phptools", "phpantom"]),
        "proto" => (&[&["buf"]], &["protols", "protobuf-language-server"]),
        "python" => (
            &[&["basedpyright", "basedpyright-langserver"], &["ruff"]],
            &["ty", "pyrefly", "pyright", "pylsp"],
        ),
        "ruby" => (
            &[&["solargraph"]],
            &[
                "ruby-lsp",
                "rubocop",
                "sorbet",
                "steep",
                "kanayago",
                "fuzzy-ruby-server",
            ],
        ),
        "starlark" => (&[&["starpls"]], &["buck2-lsp", "tilt"]),
        "systemverilog" => (&[&["slang"]], &["verible", "veridian", "svls"]),
        "vue.js" => (&[&["vue-language-server"], &["vtsls"]], &[]),
        "zig" => (&[&["zls"]], &[]),
        _ => return defaults,
    };

    let mut selected = Vec::with_capacity(available.len());
    for group in preferred {
        for server in *group {
            if available
                .iter()
                .any(|available| available.as_str() == *server)
                && !opt_in.iter().any(|opt_in| opt_in.as_str() == *server)
            {
                selected.push((*server).to_owned());
                break;
            }
        }
    }
    for server in available {
        let is_preferred = preferred.iter().any(|group| {
            group.iter().any(|candidate| *candidate == server.as_str())
        });
        if !opt_in.contains(server)
            && !excluded.contains(&server.as_str())
            && !is_preferred
            && !selected.contains(server)
        {
            selected.push(server.clone());
        }
    }
    selected
}

fn builtin_npm_language_server(server_id: &str) -> Option<BuiltinLanguageServer> {
    let language_id = match server_id {
        "basedpyright-langserver" => "python",
        "vtsls" => "typescript",
        _ => return None,
    };
    builtin_language_servers(language_id)
        .into_iter()
        .find(|server| server.command == server_id && server.npm_package().is_some())
}

pub(super) fn builtin_protocol_language_id(language: &str) -> Option<&'static str> {
    match language.to_ascii_lowercase().as_str() {
        "rust" => Some("rust"),
        "python" => Some("python"),
        "typescript" => Some("typescript"),
        "javascript" => Some("javascript"),
        "tsx" | "typescriptreact" => Some("typescriptreact"),
        "jsx" | "javascriptreact" => Some("javascriptreact"),
        _ => None,
    }
}

struct OpenDocument {
    uri: url::Url,
    language_id: String,
    version: i32,
    text: Rope,
    generation: Arc<AtomicU64>,
}

impl OpenDocument {
    fn latest_item(&self) -> TextDocumentItem {
        TextDocumentItem::new(
            self.uri.clone(),
            self.language_id.clone(),
            self.version,
            self.text.to_string(),
        )
    }
}

fn report_extension_issues(
    plugin_rpc: &PluginCatalogRpcHandler,
    errors: Vec<(PathBuf, anyhow::Error)>,
) {
    let issues = errors
        .into_iter()
        .map(|(directory, error)| {
            tracing::error!(path = %directory.display(), ?error, "language extension issue");
            ahead_rpc::core::LanguageExtensionIssue {
                name: directory
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("unknown")
                    .to_owned(),
                message: format!("{error:#}"),
            }
        })
        .collect();
    plugin_rpc.core_rpc.language_extension_issues(issues);
}

fn language_server_command_requires_restart(
    previous: &LanguageServerCommand,
    next: &LanguageServerCommand,
) -> bool {
    previous.command != next.command
        || previous.args != next.args
        || previous.env != next.env
        || previous.initialization_options != next.initialization_options
}

impl PluginCatalog {
    pub fn new(
        workspace: Option<PathBuf>,
        plugin_rpc: PluginCatalogRpcHandler,
    ) -> Self {
        let cancel_installs = Arc::new(AtomicBool::new(false));
        let node_runtime = NodeRuntime::new(
            ahead_core::directory::Directory::cache_directory(),
            cancel_installs.clone(),
        );
        let mut extension_issues = Vec::new();
        let language_extensions =
            ahead_core::directory::Directory::plugins_directory()
                .and_then(|root| match discover_language_server_extensions(&root) {
                    Ok(discovery) => {
                        extension_issues.extend(discovery.errors);
                        Some(discovery.extensions)
                    }
                    Err(error) => {
                        extension_issues.push((root, error));
                        None
                    }
                })
                .unwrap_or_default();
        let extension_host =
            match ExtensionHost::new_with_node_runtime(node_runtime.clone()) {
                Ok(host) => Some(Arc::new(host)),
                Err(error) => {
                    extension_issues
                        .push((PathBuf::from("Language extension host"), error));
                    None
                }
            };
        report_extension_issues(&plugin_rpc, extension_issues);
        let routing = Self::load_routing(workspace.as_deref());
        Self {
            routing,
            watched_files: WatchedFiles::default(),
            workspace,
            plugin_rpc,
            proxy_rpc: None,
            installing_servers: Arc::new(Mutex::new(HashSet::new())),
            checked_builtin_npm_servers: HashSet::new(),
            node_runtime,
            cancel_installs,
            resolving_servers: Arc::new(AtomicUsize::new(0)),
            pending_extension_resolutions: HashMap::new(),
            pending_builtin_npm_version_checks: HashMap::new(),
            plugins: HashMap::new(),
            daps: HashMap::new(),
            open_files: HashMap::new(),
            diagnostics: HashMap::new(),
            extension_host,
            language_extensions,
            extension_servers: HashMap::new(),
            extension_commands: HashMap::new(),
            configuration_generation: 0,
            pending_builtin_resolutions: HashMap::new(),
            lsp_servers: HashMap::new(),
        }
    }

    pub fn with_proxy_rpc(mut self, proxy_rpc: ProxyRpcHandler) -> Self {
        self.proxy_rpc = Some(proxy_rpc);
        self
    }

    pub fn with_file_watcher(
        mut self,
        watcher: crate::watcher::FileWatcherHandle,
    ) -> Self {
        self.watched_files.set_watcher(watcher);
        self
    }

    fn load_routing(
        workspace: Option<&Path>,
    ) -> Result<LanguageServerRouting, String> {
        LanguageServerRouting::load(
            workspace,
            ahead_core::directory::Directory::home_dir().as_deref(),
        )
        .map_err(|error| format!("{error:#}"))
    }

    fn language_server_names(
        &self,
        language: &str,
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        let mut available = self
            .language_extensions
            .iter()
            .filter(|extension| extension.supports_language(language))
            .map(|extension| extension.server_id.clone())
            .collect::<Vec<_>>();
        available.sort();
        available.dedup();
        let mut defaults = available
            .iter()
            .filter(|server_id| {
                self.language_extensions.iter().any(|extension| {
                    extension.server_id.as_str() == server_id.as_str()
                        && extension.supports_language(language)
                        && !extension.is_opt_in_for(language)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let opt_in = available
            .iter()
            .filter(|server_id| {
                self.language_extensions.iter().any(|extension| {
                    extension.server_id.as_str() == server_id.as_str()
                        && extension.supports_language(language)
                        && extension.is_opt_in_for(language)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        for builtin in builtin_language_servers(language) {
            if !available.iter().any(|name| name == builtin.command) {
                available.push(builtin.command.into());
            }
            if (defaults.is_empty() || builtin.command == "ruff")
                && !defaults.iter().any(|name| name == builtin.command)
            {
                defaults.push(builtin.command.into());
            }
        }
        defaults =
            zed_default_language_servers(language, &available, defaults, &opt_in);
        (available, defaults, opt_in)
    }

    fn selected_servers(&self, language: &str) -> Result<Vec<String>, RpcError> {
        let (available, defaults, opt_in) = self.language_server_names(language);
        self.routing
            .as_ref()
            .map_err(|message| RpcError {
                code: -32602,
                message: message.clone(),
            })?
            .servers(language, &available, &defaults, &opt_in)
            .map_err(|error| RpcError {
                code: -32602,
                message: error.to_string(),
            })
    }

    fn request_servers(
        &self,
        language: Option<&str>,
        method: &str,
    ) -> Result<VecDeque<PluginServerRpcHandler>, RpcError> {
        let mut servers = self.plugins.values().cloned().collect::<Vec<_>>();
        servers.sort_by(|left, right| {
            (&left.server_id.name, left.plugin_id.0)
                .cmp(&(&right.server_id.name, right.plugin_id.0))
        });
        if let Some(language) = language {
            let mut selected = self.selected_servers(language)?;
            if let Some(name) = self
                .routing
                .as_ref()
                .ok()
                .and_then(|routing| routing.request_server(language, method))
            {
                if !selected.iter().any(|selected| selected == name) {
                    return Err(RpcError {
                        code: -32602,
                        message: format!(
                            "Language server {name} is not enabled for {language} ({method})"
                        ),
                    });
                }
                selected = vec![name.into()];
            }
            servers.retain(|plugin| {
                selected.iter().any(|name| {
                    plugin
                        .server_id
                        .name
                        .strip_prefix("lsp-")
                        .unwrap_or(&plugin.server_id.name)
                        == name
                })
            });
            servers.sort_by_key(|plugin| {
                selected.iter().position(|name| {
                    plugin
                        .server_id
                        .name
                        .strip_prefix("lsp-")
                        .unwrap_or(&plugin.server_id.name)
                        == name
                })
            });
        }
        Ok(servers.into())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn handle_server_request(
        &mut self,
        plugin_id: Option<PluginId>,
        request_sent: Option<Arc<AtomicUsize>>,
        cancellation_id: Option<Id>,
        method: Cow<'static, str>,
        params: Value,
        language_id: Option<String>,
        path: Option<PathBuf>,
        check: bool,
        f: Box<dyn ClonableCallback<Value, RpcError>>,
    ) {
        if let Some(plugin_id) = plugin_id {
            if let Some(plugin) = self.plugins.get(&plugin_id) {
                let callback = move |result| f(plugin_id, result);
                if let Some(cancellation_id) = cancellation_id {
                    plugin.server_request_async_with_id(
                        cancellation_id,
                        method,
                        params,
                        language_id,
                        path,
                        check,
                        callback,
                    );
                } else {
                    plugin.server_request_async(
                        method,
                        params,
                        language_id,
                        path,
                        check,
                        callback,
                    );
                }
            } else {
                f(
                    plugin_id,
                    Err(RpcError {
                        code: 0,
                        message: "plugin doesn't exist".to_string(),
                    }),
                );
            }
            return;
        }

        let language_id = path
            .as_ref()
            .and_then(|path| self.open_files.get(path))
            .map(|document| document.language_id.clone())
            .or(language_id);
        let servers = match self.request_servers(language_id.as_deref(), &method) {
            Ok(servers) => servers,
            Err(error) => {
                if let Some(count) = request_sent {
                    count.fetch_add(1, Ordering::Release);
                }
                f(PluginId(0), Err(error));
                return;
            }
        };
        if request_sent.is_none() && check {
            PluginServerRpcHandler::request_first_capable(
                servers,
                method,
                params,
                language_id,
                path,
                cancellation_id,
                f,
            );
            return;
        }
        if let Some(count) = &request_sent {
            count.fetch_add(servers.len().max(1), Ordering::Release);
        }
        if servers.is_empty() {
            f(
                PluginId(0),
                Err(RpcError {
                    code: -32601,
                    message: format!(
                        "No selected language server is running for {method}"
                    ),
                }),
            );
            return;
        }
        for plugin in servers {
            let f = dyn_clone::clone_box(&*f);
            let plugin_id = plugin.plugin_id;
            plugin.server_request_async(
                method.clone(),
                params.clone(),
                language_id.clone(),
                path.clone(),
                check,
                move |result| {
                    f(plugin_id, result);
                },
            );
        }
    }

    pub fn cancel_completion(&self, request_id: Id) {
        for plugin in self.plugins.values() {
            plugin.cancel_request(request_id.clone());
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn handle_server_notification(
        &mut self,
        plugin_id: Option<PluginId>,
        method: impl Into<Cow<'static, str>>,
        params: Value,
        language_id: Option<String>,
        path: Option<PathBuf>,
        check: bool,
    ) {
        if let Some(plugin_id) = plugin_id {
            if let Some(plugin) = self.plugins.get(&plugin_id) {
                plugin.server_notification(method, params, language_id, path, check);
            }

            return;
        }

        // Otherwise send it to all plugins
        let method = method.into();
        for plugin in self.plugins.values() {
            plugin.server_notification(
                method.clone(),
                params.clone(),
                language_id.clone(),
                path.clone(),
                check,
            );
        }
    }

    pub fn handle_did_open_text_document(&mut self, document: TextDocumentItem) {
        match document.uri.to_file_path() {
            Ok(path) => {
                if let Some(previous) = self.open_files.insert(
                    path,
                    OpenDocument {
                        uri: document.uri.clone(),
                        language_id: document.language_id.clone(),
                        version: document.version,
                        text: Rope::from_str(&document.text),
                        generation: Arc::new(AtomicU64::new(0)),
                    },
                ) {
                    previous.generation.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(err) => {
                tracing::error!("{:?}", err);
            }
        }
        self.ensure_lsp_server(&document.language_id.clone());

        let path = document.uri.to_file_path().ok();
        let lsp_plugin_ids = self.lsp_servers.get(&document.language_id);
        for (plugin_id, plugin) in self.plugins.iter() {
            let text_document =
                if lsp_plugin_ids.is_some_and(|ids| ids.contains(plugin_id)) {
                    let mut document = document.clone();
                    if let Some(extension) = self.extension_servers.get(plugin_id) {
                        document.language_id =
                            extension.protocol_language_id(&document.language_id);
                    } else if let Some(language_id) =
                        builtin_protocol_language_id(&document.language_id)
                    {
                        document.language_id = language_id.into();
                    }
                    document
                } else {
                    document.clone()
                };
            plugin.server_notification(
                DidOpenTextDocument::METHOD,
                DidOpenTextDocumentParams { text_document },
                Some(document.language_id.clone()),
                path.clone(),
                true,
            );
        }
        for plugin_id in lsp_plugin_ids.into_iter().flatten() {
            self.refresh_diagnostics(*plugin_id);
        }
    }

    pub fn handle_did_close_text_document(&mut self, path: PathBuf) {
        let Some(document) = self.open_files.remove(&path) else {
            return;
        };
        document.generation.fetch_add(1, Ordering::Relaxed);
        let Ok(uri) = url::Url::from_file_path(&path) else {
            tracing::error!(?path, "invalid closed document path");
            return;
        };
        for plugin in self.plugins.values() {
            plugin.server_notification(
                DidCloseTextDocument::METHOD,
                DidCloseTextDocumentParams {
                    text_document: TextDocumentIdentifier::new(uri.clone()),
                },
                Some(document.language_id.clone()),
                Some(path.clone()),
                true,
            );
        }
        self.diagnostics.remove(&uri);
        self.plugin_rpc
            .core_rpc
            .publish_diagnostics(PublishDiagnosticsParams::new(
                uri,
                Vec::new(),
                None,
            ));
    }

    pub fn publish_diagnostics(
        &mut self,
        plugin_id: PluginId,
        source: DiagnosticSource,
        mut diagnostics: PublishDiagnosticsParams,
    ) {
        if !self.plugins.contains_key(&plugin_id) {
            return;
        }
        // Rust Analyzer pushes rustc errors separately from its pulled native
        // diagnostics. Like Zed, replace only this server/source's contribution.
        let sources = self.diagnostics.entry(diagnostics.uri.clone()).or_default();
        if diagnostics.diagnostics.is_empty() {
            sources.remove(&(plugin_id, source));
        } else {
            sources.insert((plugin_id, source), diagnostics.diagnostics);
        }
        diagnostics.diagnostics = sources.values().flatten().cloned().collect();
        diagnostics.diagnostics.sort_by(|left, right| {
            (left.range.start, left.range.end, &left.message).cmp(&(
                right.range.start,
                right.range.end,
                &right.message,
            ))
        });
        // The merged sources need not describe the same document version.
        diagnostics.version = None;
        if sources.is_empty() {
            self.diagnostics.remove(&diagnostics.uri);
        }
        self.plugin_rpc.core_rpc.publish_diagnostics(diagnostics);
    }

    pub fn refresh_diagnostics(&self, plugin_id: PluginId) {
        let Some(plugin) = self.plugins.get(&plugin_id).cloned() else {
            return;
        };
        for (path, document) in &self.open_files {
            let Ok(uri) = url::Url::from_file_path(path) else {
                continue;
            };
            request_document_diagnostics(
                plugin.clone(),
                plugin_id,
                path.clone(),
                uri,
                document.language_id.clone(),
                self.plugin_rpc.clone(),
                document.generation.load(Ordering::Relaxed),
                document.generation.clone(),
                0,
            );
        }
    }

    fn has_lsp_server(&self, language_id: &str, server_id: &str) -> bool {
        self.lsp_servers.get(language_id).is_some_and(|ids| {
            ids.iter().any(|id| {
                self.plugins.get(id).is_some_and(|plugin| {
                    plugin.server_id.name.strip_prefix("lsp-") == Some(server_id)
                })
            })
        })
    }

    fn ensure_lsp_server(&mut self, language_id: &str) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        match ahead_core::workspace_trust::is_trusted(&workspace) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                self.report_language_server_error(
                    language_id,
                    &anyhow::Error::from(error),
                );
                return;
            }
        }
        // Startup discovery and explicit install/update restarts refresh this
        // list; keep package scans and the install lock off the document-open path.
        let selected = match self.selected_servers(language_id) {
            Ok(selected) => selected,
            Err(error) => {
                self.report_routing_error(error.message);
                return;
            }
        };
        let extensions = self
            .language_extensions
            .iter()
            .filter(|extension| {
                extension.supports_language(language_id)
                    && selected.contains(&extension.server_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        for extension in &extensions {
            if !self.has_lsp_server(language_id, &extension.server_id) {
                self.start_extension_resolution(
                    language_id,
                    extension.clone(),
                    workspace.clone(),
                );
            }
        }
        for builtin in builtin_language_servers(language_id) {
            if !selected.iter().any(|name| name == builtin.command)
                || extensions
                    .iter()
                    .any(|extension| extension.server_id == builtin.command)
                || self.has_lsp_server(language_id, builtin.command)
                || self.installing_servers.lock().contains(builtin.command)
            {
                continue;
            }
            if matches!(builtin.command, "rust-analyzer" | "ruff") {
                self.start_native_server_resolution(language_id, builtin);
                continue;
            }
            let has_user_binary = which::which(builtin.command).is_ok();
            let cached_npm = if !has_user_binary {
                ahead_core::directory::Directory::cache_directory().and_then(
                    |cache| {
                        self.node_runtime.cached_node_binary().and_then(|node| {
                            builtin
                                .cached_npm_command(&cache, &node)
                                .map(|command| (command, cache))
                        })
                    },
                )
            } else {
                None
            };
            let cached_command =
                cached_npm.as_ref().map(|(command, _)| command.clone());
            if !has_user_binary
                && cached_command.is_none()
                && builtin.npm_package().is_some()
            {
                self.start_builtin_install(builtin, false);
                continue;
            }
            let plugin_id = self.start_lsp_server(
                &workspace,
                language_id,
                cached_command.unwrap_or_else(|| LanguageServerCommand {
                    command: builtin.command.into(),
                    args: builtin.args.iter().map(|arg| (*arg).into()).collect(),
                    env: Vec::new(),
                    initialization_options: None,
                    initialization_options_schema: None,
                    workspace_configuration: None,
                    workspace_configuration_schema: None,
                }),
                builtin.command.to_string(),
                builtin
                    .languages
                    .iter()
                    .map(|language| (*language).into())
                    .collect(),
                None,
            );
            if let (Some(plugin_id), Some((_, cache))) = (plugin_id, cached_npm) {
                self.pending_builtin_npm_version_checks
                    .insert(plugin_id, (builtin, cache));
            }
        }
    }

    fn start_lsp_server(
        &mut self,
        workspace: &Path,
        language_id: &str,
        mut server: LanguageServerCommand,
        server_id: String,
        mut languages: Vec<String>,
        extension: Option<LanguageServerExtension>,
    ) -> Option<PluginId> {
        if server_id == "basedpyright-langserver" && extension.is_none() {
            server.workspace_configuration = Some(serde_json::json!({
                "basedpyright": {
                    "disableOrganizeImports": true
                },
                "basedpyright.analysis": { "typeCheckingMode": "standard" }
            }));
        }
        let extension_command = extension.as_ref().map(|_| server.clone());
        if let Some(command) = extension_command.as_ref() {
            self.plugin_rpc.core_rpc.language_server_schemas(
                server_id.clone(),
                ahead_rpc::core::LanguageServerSchemas {
                    initialization_options: command
                        .initialization_options_schema
                        .clone(),
                    settings: command.workspace_configuration_schema.clone(),
                },
            );
        }
        languages.retain(|language| {
            self.selected_servers(language)
                .is_ok_and(|names| names.contains(&server_id))
        });
        if !languages.iter().any(|language| language == language_id) {
            languages.push(language_id.to_string());
        }
        let selector = languages
            .iter()
            .map(|language| lsp_types::DocumentFilter {
                language: Some(language.clone()),
                scheme: None,
                pattern: None,
            })
            .collect();
        let uri = if server.command.contains(std::path::MAIN_SEPARATOR) {
            url::Url::from_file_path(&server.command).map_err(|_| {
                anyhow::anyhow!("Invalid language server path: {}", server.command)
            })
        } else {
            url::Url::parse(&format!("urn:{}", server.command))
                .map_err(anyhow::Error::from)
        };
        let result = uri.and_then(|uri| {
            self.plugin_rpc.core_rpc.server_status(
                ahead_rpc::core::ServerStatusParams::starting(server_id.clone()),
            );
            LspClient::start(
                self.plugin_rpc.clone(),
                selector,
                Some(workspace.to_path_buf()),
                ServerId {
                    author: "ahead".to_string(),
                    name: format!("lsp-{server_id}"),
                },
                server_id.clone(),
                Some(PluginId::next()),
                Some(workspace.to_path_buf()),
                uri,
                server.args,
                server.env,
                server.initialization_options,
                server.workspace_configuration,
            )
        });
        match result {
            Ok((plugin_id, handler)) => {
                self.plugins.insert(plugin_id, handler);
                if let Some(extension) = extension {
                    self.extension_servers.insert(plugin_id, extension);
                    if let Some(command) = extension_command {
                        self.extension_commands.insert(plugin_id, command);
                    }
                }
                for language in languages {
                    self.lsp_servers
                        .entry(language)
                        .or_default()
                        .push(plugin_id);
                }
                Some(plugin_id)
            }
            Err(error) => {
                self.report_language_server_error(&server_id, &error);
                None
            }
        }
    }

    fn restart_extension_language_server(&mut self, plugin_id: PluginId) {
        let Some(plugin) = self.plugins.get(&plugin_id).cloned() else {
            return;
        };
        let server_name = plugin
            .server_id
            .name
            .strip_prefix("lsp-")
            .unwrap_or(&plugin.server_id.name)
            .to_owned();
        let languages = self
            .lsp_servers
            .iter()
            .filter_map(|(language, plugin_ids)| {
                plugin_ids.contains(&plugin_id).then(|| language.clone())
            })
            .collect::<Vec<_>>();

        plugin.shutdown();
        if !plugin.wait_for_shutdown() {
            self.report_language_server_error(
                &server_name,
                &anyhow::anyhow!(
                    "Language server did not stop before its shutdown deadline."
                ),
            );
            return;
        }
        self.remove_language_server(plugin_id);
        for language in languages {
            self.ensure_lsp_server(&language);
        }
    }

    fn start_extension_resolution(
        &mut self,
        language_id: &str,
        extension: LanguageServerExtension,
        workspace: PathBuf,
    ) {
        let generation = self.configuration_generation;
        if self.pending_extension_resolutions.get(&extension.server_id)
            == Some(&generation)
        {
            return;
        }
        let Some(host) = self.extension_host.clone() else {
            self.report_language_server_error(
                language_id,
                &anyhow::anyhow!("Language extension host is unavailable"),
            );
            return;
        };
        self.pending_extension_resolutions
            .insert(extension.server_id.clone(), generation);
        let language_id = language_id.to_string();
        let rpc = self.plugin_rpc.clone();
        let cancelled = self.cancel_installs.clone();
        let resolving = self.resolving_servers.clone();
        let other_extensions = self.language_extensions.clone();
        resolving.fetch_add(1, Ordering::Relaxed);
        self.plugin_rpc.core_rpc.server_status(
            ahead_rpc::core::ServerStatusParams::starting(
                extension.server_id.clone(),
            ),
        );
        thread::spawn(move || {
            let result = (|| -> anyhow::Result<LanguageServerCommand> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                runtime.block_on(async {
                    tokio::select! {
                        result = host.language_server_command_with_additional_configuration(
                            &extension,
                            &other_extensions,
                            WorktreeContext { root: workspace },
                        ) => result,
                        _ = async {
                            while !cancelled.load(Ordering::Relaxed) {
                                tokio::time::sleep(Duration::from_millis(50)).await;
                            }
                        } => anyhow::bail!("language extension resolution cancelled"),
                    }
                })
            })();
            resolving.fetch_sub(1, Ordering::Relaxed);
            if !cancelled.load(Ordering::Relaxed)
                && let Err(error) = rpc.catalog_notification(
                    PluginCatalogNotification::LanguageServerResolved {
                        language_id,
                        extension,
                        generation,
                        result: result.map_err(|error| format!("{error:#}")),
                    },
                )
            {
                tracing::warn!(%error, "reporting language extension resolution");
            }
        });
    }

    fn start_native_server_resolution(
        &mut self,
        language_id: &str,
        builtin: BuiltinLanguageServer,
    ) {
        let generation = self.configuration_generation;
        if self.pending_builtin_resolutions.get(builtin.command) == Some(&generation)
        {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let Some(cache) = ahead_core::directory::Directory::cache_directory() else {
            self.report_language_server_error(
                builtin.command,
                &anyhow::anyhow!("AHEAD cache directory is unavailable"),
            );
            return;
        };
        self.pending_builtin_resolutions
            .insert(builtin.command.to_string(), generation);
        self.plugin_rpc.core_rpc.server_status(
            ahead_rpc::core::ServerStatusParams::starting(builtin.command.into()),
        );
        let rpc = self.plugin_rpc.clone();
        let cancelled = self.cancel_installs.clone();
        let resolving = self.resolving_servers.clone();
        let language_id = language_id.to_string();
        resolving.fetch_add(1, Ordering::Relaxed);
        thread::spawn(move || {
            let result = if builtin.command == "rust-analyzer" {
                resolve_rust_analyzer_command(&cache, &workspace, &cancelled)
            } else {
                resolve_ruff_command(&cache, &workspace, &cancelled)
            }
            .map_err(|error| format!("{error:#}"));
            resolving.fetch_sub(1, Ordering::Relaxed);
            if !cancelled.load(Ordering::Relaxed)
                && let Err(error) = rpc.catalog_notification(
                    PluginCatalogNotification::BuiltinServerResolved {
                        language_id,
                        server_name: builtin.command,
                        generation,
                        result,
                    },
                )
            {
                tracing::warn!(%error, "reporting built-in language server resolution");
            }
        });
    }

    fn handle_builtin_server_resolved(
        &mut self,
        language_id: String,
        server_name: &'static str,
        generation: u64,
        result: Result<LanguageServerCommand, String>,
    ) {
        if self.pending_builtin_resolutions.get(server_name) != Some(&generation) {
            return;
        }
        self.pending_builtin_resolutions.remove(server_name);
        if generation != self.configuration_generation {
            if self
                .open_files
                .values()
                .any(|document| document.language_id == language_id)
            {
                self.ensure_lsp_server(&language_id);
            }
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        if !matches!(
            ahead_core::workspace_trust::is_trusted(&workspace),
            Ok(true)
        ) || self.has_lsp_server(&language_id, server_name)
        {
            return;
        }
        let documents = self
            .open_files
            .values()
            .filter(|document| {
                builtin_language_servers(&document.language_id)
                    .iter()
                    .any(|builtin| builtin.command == server_name)
            })
            .map(OpenDocument::latest_item)
            .collect::<Vec<_>>();
        if documents.is_empty() {
            return;
        }
        let Some(builtin) = builtin_language_servers(&language_id)
            .into_iter()
            .find(|builtin| builtin.command == server_name)
        else {
            return;
        };
        match result {
            Ok(server) => {
                let languages = builtin
                    .languages
                    .iter()
                    .map(|language| (*language).into())
                    .collect();
                let plugin_id = self.start_lsp_server(
                    &workspace,
                    &language_id,
                    server,
                    builtin.command.to_string(),
                    languages,
                    None,
                );
                if let Some(plugin_id) = plugin_id
                    && let Some(plugin) = self.plugins.get(&plugin_id)
                {
                    for mut document in documents {
                        let language_id = document.language_id.clone();
                        let path = document.uri.to_file_path().ok();
                        document.language_id =
                            builtin_protocol_language_id(&language_id)
                                .unwrap_or(&language_id)
                                .to_string();
                        plugin.server_notification(
                            DidOpenTextDocument::METHOD,
                            DidOpenTextDocumentParams {
                                text_document: document,
                            },
                            Some(language_id),
                            path,
                            true,
                        );
                    }
                    self.refresh_diagnostics(plugin_id);
                }
            }
            Err(message) => self.report_language_server_error(
                builtin.command,
                &anyhow::anyhow!(message),
            ),
        }
    }

    fn start_builtin_install(
        &self,
        builtin: BuiltinLanguageServer,
        is_update: bool,
    ) {
        let prerequisites = (|| -> anyhow::Result<_> {
            let proxy_rpc = self.proxy_rpc.clone().ok_or_else(|| {
                anyhow::anyhow!("managed server installer is unavailable")
            })?;
            let cache = ahead_core::directory::Directory::cache_directory()
                .ok_or_else(|| {
                    anyhow::anyhow!("AHEAD cache directory is unavailable")
                })?;
            Ok((proxy_rpc, cache))
        })();
        let (proxy_rpc, cache) = match prerequisites {
            Ok(prerequisites) => prerequisites,
            Err(error) => {
                if is_update {
                    tracing::warn!(
                        server = builtin.command,
                        ?error,
                        "could not prepare cached language-server update"
                    );
                } else {
                    self.report_language_server_error(builtin.command, &error);
                }
                return;
            }
        };
        if !self.installing_servers.lock().insert(builtin.command) {
            return;
        }
        if !is_update {
            let mut status = ahead_rpc::core::ServerStatusParams::starting(
                builtin.command.into(),
            );
            status.message = Some("Installing from npm into AHEAD's cache…".into());
            self.plugin_rpc.core_rpc.server_status(status);
        }
        let core_rpc = self.plugin_rpc.core_rpc.clone();
        let installing = self.installing_servers.clone();
        let cancelled = self.cancel_installs.clone();
        let node_runtime = self.node_runtime.clone();
        thread::spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let npm = runtime.block_on(node_runtime.npm_invocation())?;
                install_builtin_npm_package_with_invocation(
                    &cache, &npm, builtin, &cancelled,
                )
            })();
            installing.lock().remove(builtin.command);
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            match result {
                Ok(()) => proxy_rpc.notification(
                    ProxyNotification::RestartLanguageServers { extension_id: None },
                ),
                Err(error) => {
                    if is_update {
                        tracing::warn!(
                            server = builtin.command,
                            ?error,
                            "could not update cached language server; keeping the current version"
                        );
                    } else {
                        tracing::error!(
                            server = builtin.command,
                            ?error,
                            "installing language server"
                        );
                        core_rpc.server_status(
                            ahead_rpc::core::ServerStatusParams::failed(
                                builtin.command.into(),
                                format!("{error:#}"),
                            ),
                        );
                    }
                }
            }
        });
    }

    fn start_builtin_npm_version_check(
        &mut self,
        builtin: BuiltinLanguageServer,
        cache: PathBuf,
    ) {
        if self.checked_builtin_npm_servers.contains(builtin.command) {
            return;
        }
        let Some((_, package, _)) = builtin.npm_package() else {
            return;
        };
        let Some(installed_version) = cached_npm_package_version(&cache, builtin)
        else {
            return;
        };
        self.checked_builtin_npm_servers.insert(builtin.command);
        let Some(npm) = self.node_runtime.cached_npm_invocation() else {
            return;
        };

        let plugin_rpc = self.plugin_rpc.clone();
        let cancelled = self.cancel_installs.clone();
        let resolving = self.resolving_servers.clone();
        resolving.fetch_add(1, Ordering::Relaxed);
        thread::spawn(move || {
            let latest = latest_npm_package_version(&npm, package, &cancelled);
            if !cancelled.load(Ordering::Relaxed) {
                match latest {
                    Ok(latest_version)
                        if npm_update_available(
                            &installed_version,
                            &latest_version,
                        ) =>
                    {
                        if let Err(error) = plugin_rpc.catalog_notification(
                            PluginCatalogNotification::BuiltinNpmServerUpdateAvailable {
                                server_id: builtin.command.into(),
                                latest_version: latest_version.to_string(),
                            },
                        ) {
                            tracing::error!(
                                server = builtin.command,
                                ?error,
                                "reporting a managed language-server update"
                            );
                        }
                    }
                    Ok(_) => {}
                    Err(error) => tracing::debug!(
                        server = builtin.command,
                        ?error,
                        "keeping cached language server after version check"
                    ),
                }
            }
            resolving.fetch_sub(1, Ordering::Relaxed);
        });
    }

    fn update_builtin_npm_server(
        &mut self,
        server_id: String,
        latest_version: String,
    ) {
        let Some(builtin) = builtin_npm_language_server(&server_id) else {
            return;
        };
        let Ok(latest_version) = semver::Version::parse(&latest_version) else {
            tracing::warn!(server = %server_id, "ignoring invalid latest language-server version");
            return;
        };
        if which::which(builtin.command).is_ok()
            || self.installing_servers.lock().contains(builtin.command)
        {
            return;
        }
        if !builtin
            .languages
            .iter()
            .any(|language| self.has_lsp_server(language, builtin.command))
        {
            return;
        }
        let Some(cache) = ahead_core::directory::Directory::cache_directory() else {
            return;
        };
        let Some(installed_version) = cached_npm_package_version(&cache, builtin)
        else {
            return;
        };
        if !npm_update_available(&installed_version, &latest_version) {
            return;
        }
        let Some(workspace) = self.workspace.as_deref() else {
            return;
        };
        if !matches!(ahead_core::workspace_trust::is_trusted(workspace), Ok(true)) {
            return;
        }

        tracing::info!(
            server = builtin.command,
            %installed_version,
            %latest_version,
            "updating cached language server"
        );
        self.start_builtin_install(builtin, true);
    }

    fn repair_builtin_npm_server(&mut self, server_id: String) {
        let Some(builtin) = builtin_npm_language_server(&server_id) else {
            self.report_language_server_error(
                &server_id,
                &anyhow::anyhow!(
                    "this language server has no AHEAD-managed npm package"
                ),
            );
            return;
        };
        let Some(workspace) = self.workspace.as_deref() else {
            self.report_language_server_error(
                &server_id,
                &anyhow::anyhow!(
                    "open a workspace before repairing language servers"
                ),
            );
            return;
        };
        if !matches!(ahead_core::workspace_trust::is_trusted(workspace), Ok(true)) {
            self.report_language_server_error(
                &server_id,
                &anyhow::anyhow!(
                    "trust this workspace before repairing language servers"
                ),
            );
            return;
        }
        if which::which(builtin.command).is_ok() {
            self.report_language_server_error(
                &server_id,
                &anyhow::anyhow!(
                    "A system-installed server is selected from PATH; update it with its package manager, then restart language servers"
                ),
            );
            return;
        }
        if builtin
            .languages
            .iter()
            .any(|language| self.has_lsp_server(language, builtin.command))
        {
            self.handle_notification(
                PluginCatalogNotification::RestartLanguageServers {
                    documents: Vec::new(),
                    extension_id: None,
                },
            );
            if builtin
                .languages
                .iter()
                .any(|language| self.has_lsp_server(language, builtin.command))
            {
                self.report_language_server_error(
                    &server_id,
                    &anyhow::anyhow!(
                        "the language server could not be stopped for repair"
                    ),
                );
                return;
            }
        }
        let cache = match ahead_core::directory::Directory::cache_directory() {
            Some(cache) => cache,
            None => {
                self.report_language_server_error(
                    &server_id,
                    &anyhow::anyhow!("AHEAD cache directory is unavailable"),
                );
                return;
            }
        };
        if !self.installing_servers.lock().insert(builtin.command) {
            let mut status =
                ahead_rpc::core::ServerStatusParams::starting(server_id.clone());
            status.message = Some("Installing from npm into AHEAD's cache…".into());
            self.plugin_rpc.core_rpc.server_status(status);
            return;
        }

        let mut status =
            ahead_rpc::core::ServerStatusParams::starting(server_id.clone());
        status.message =
            Some("Repairing AHEAD's cached language-server package…".into());
        self.plugin_rpc.core_rpc.server_status(status);
        let plugin_rpc = self.plugin_rpc.clone();
        let cancelled = self.cancel_installs.clone();
        thread::spawn(move || {
            let result =
                remove_cached_builtin_npm_package(&cache, builtin, &cancelled)
                    .map_err(|error| format!("{error:#}"));
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            if let Err(error) = plugin_rpc.catalog_notification(
                PluginCatalogNotification::RepairBuiltinNpmServerFinished {
                    server_id,
                    result,
                },
            ) {
                tracing::error!(?error, "reporting language-server cache repair");
            }
        });
    }

    fn reload_language_extensions(&mut self) -> anyhow::Result<()> {
        let Some(root) = ahead_core::directory::Directory::plugins_directory()
        else {
            self.plugin_rpc
                .core_rpc
                .language_extension_issues(Vec::new());
            self.language_extensions.clear();
            return Ok(());
        };
        let discovery = match discover_language_server_extensions(&root) {
            Ok(discovery) => discovery,
            Err(error) => {
                report_extension_issues(
                    &self.plugin_rpc,
                    vec![(root, anyhow::anyhow!("{error:#}"))],
                );
                return Err(error);
            }
        };
        report_extension_issues(&self.plugin_rpc, discovery.errors);
        self.language_extensions = discovery.extensions;
        Ok(())
    }

    pub(super) fn uninstall_language_extension(
        &mut self,
        extension_id: String,
        callback: Box<dyn RpcCallback<(), RpcError>>,
    ) {
        let result = self.uninstall_language_extension_package(&extension_id);
        callback.call(result.map_err(|error| RpcError {
            code: 0,
            message: format!("{error:#}"),
        }));
    }

    fn uninstall_language_extension_package(
        &mut self,
        extension_id: &str,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.language_extensions
                .iter()
                .any(|extension| extension.id == extension_id),
            "language extension {extension_id} is not installed"
        );
        let extensions_root = ahead_core::directory::Directory::plugins_directory()
            .ok_or_else(|| {
                anyhow::anyhow!("AHEAD plugin directory is unavailable")
            })?;
        let documents = self
            .open_files
            .values()
            .map(OpenDocument::latest_item)
            .collect::<Vec<_>>();

        for plugin in self.plugins.values() {
            plugin.shutdown();
        }
        for plugin in self.plugins.values() {
            if !plugin.wait_for_shutdown() {
                let error = anyhow::anyhow!(
                    "language servers did not stop; extension was not uninstalled"
                );
                self.report_language_server_error("Language extensions", &error);
                return Err(error);
            }
        }
        for plugin_id in self.plugins.keys().copied().collect::<Vec<_>>() {
            self.remove_language_server(plugin_id);
        }

        let uninstall_result =
            uninstall_extension_package(&extensions_root, extension_id);
        self.handle_notification(
            PluginCatalogNotification::RestartLanguageServers {
                documents,
                extension_id: None,
            },
        );
        uninstall_result
    }

    fn report_language_server_error(
        &self,
        server_name: &str,
        error: &anyhow::Error,
    ) {
        tracing::error!(server_name, ?error, "starting language server");
        self.plugin_rpc.core_rpc.server_status(
            ahead_rpc::core::ServerStatusParams::failed(
                server_name.into(),
                format!("{error:#}"),
            ),
        );
    }

    fn report_routing_error(&self, message: String) {
        tracing::error!(%message, "reading language-server routing");
        self.plugin_rpc.core_rpc.show_message(
            "Language server settings".into(),
            lsp_types::ShowMessageParams {
                typ: lsp_types::MessageType::ERROR,
                message,
            },
        );
    }

    fn refresh_workspace_configurations(&mut self) {
        match Self::load_routing(self.workspace.as_deref()) {
            Ok(routing) if self.routing.as_ref() != Ok(&routing) => {
                self.routing = Ok(routing);
                let documents = self
                    .open_files
                    .values()
                    .map(OpenDocument::latest_item)
                    .collect();
                self.handle_notification(
                    PluginCatalogNotification::RestartLanguageServers {
                        documents,
                        extension_id: None,
                    },
                );
                return;
            }
            Err(message) => {
                self.report_routing_error(message);
                return;
            }
            _ => {}
        }
        let (Some(workspace), Some(host)) =
            (self.workspace.as_ref(), self.extension_host.as_ref())
        else {
            return;
        };
        let other_extensions = self.language_extensions.clone();
        self.configuration_generation =
            self.configuration_generation.wrapping_add(1);
        let generation = self.configuration_generation;
        for (plugin_id, extension) in &self.extension_servers {
            if !self.plugins.contains_key(plugin_id) {
                continue;
            }
            let host = host.clone();
            let workspace = workspace.clone();
            let extension = extension.clone();
            let other_extensions = other_extensions.clone();
            let plugin_id = *plugin_id;
            let catalog_rpc = self.plugin_rpc.clone();
            thread::spawn(move || {
                let result = (|| -> anyhow::Result<LanguageServerCommand> {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    runtime.block_on(
                        host.language_server_command_with_additional_configuration(
                            &extension,
                            &other_extensions,
                            WorktreeContext { root: workspace },
                        ),
                    )
                })()
                .map_err(|error| format!("{error:#}"));
                if let Err(error) = catalog_rpc.catalog_notification(
                    PluginCatalogNotification::ExtensionConfigurationResolved {
                        plugin_id,
                        generation,
                        result,
                    },
                ) {
                    tracing::error!(?error, "reporting language-server settings");
                }
            });
        }
    }

    pub fn handle_did_save_text_document(
        &mut self,
        language_id: String,
        path: PathBuf,
        text_document: TextDocumentIdentifier,
        text: Rope,
    ) {
        for plugin in self.plugins.values() {
            plugin.handle_rpc(PluginServerRpc::DidSaveTextDocument {
                language_id: language_id.clone(),
                path: path.clone(),
                text_document: text_document.clone(),
                text: text.clone(),
            });
        }
    }

    pub fn handle_did_change_text_document(
        &mut self,
        language_id: String,
        document: VersionedTextDocumentIdentifier,
        delta: AheadDelta,
        text: Rope,
        new_text: Rope,
    ) {
        if let Ok(path) = document.uri.to_file_path()
            && let Some(open_document) = self.open_files.get_mut(&path)
        {
            open_document.generation.fetch_add(1, Ordering::Relaxed);
            open_document.version = document.version;
            open_document.text = new_text.clone();
        }
        let change = Arc::new(Mutex::new((None, None)));
        for plugin in self.plugins.values() {
            plugin.handle_rpc(PluginServerRpc::DidChangeTextDocument {
                language_id: language_id.clone(),
                document: document.clone(),
                delta: delta.clone(),
                text: text.clone(),
                new_text: new_text.clone(),
                change: change.clone(),
            });
        }
        // ponytail: pull per edit; debounce by document if LSP traffic becomes costly.
        for plugin_id in self.plugins.keys().copied() {
            self.refresh_diagnostics(plugin_id);
        }
    }

    pub fn format_semantic_tokens(
        &self,
        plugin_id: PluginId,
        tokens: SemanticTokens,
        text: Rope,
        f: Box<dyn RpcCallback<Vec<LineStyle>, RpcError>>,
    ) {
        if let Some(plugin) = self.plugins.get(&plugin_id) {
            plugin.handle_rpc(PluginServerRpc::FormatSemanticTokens {
                tokens,
                text,
                f,
            });
        } else {
            f.call(Err(RpcError {
                code: 0,
                message: "plugin doesn't exist".to_string(),
            }));
        }
    }

    pub fn dap_variable(
        &self,
        dap_id: DapId,
        reference: usize,
        f: Box<dyn RpcCallback<Vec<dap_types::Variable>, RpcError>>,
    ) {
        if let Some(dap) = self.daps.get(&dap_id) {
            dap.variables_async(
                reference,
                |result: Result<dap_types::VariablesResponse, RpcError>| {
                    f.call(result.map(|resp| resp.variables))
                },
            );
        } else {
            f.call(Err(RpcError {
                code: 0,
                message: "plugin doesn't exist".to_string(),
            }));
        }
    }

    pub fn dap_get_scopes(
        &self,
        dap_id: DapId,
        frame_id: usize,
        f: Box<
            dyn RpcCallback<
                    Vec<(dap_types::Scope, Vec<dap_types::Variable>)>,
                    RpcError,
                >,
        >,
    ) {
        if let Some(dap) = self.daps.get(&dap_id) {
            let local_dap = dap.clone();
            dap.scopes_async(
                frame_id,
                move |result: Result<dap_types::ScopesResponse, RpcError>| {
                    match result {
                        Ok(resp) => {
                            let scopes = resp.scopes.clone();
                            if let Some(scope) = resp.scopes.first() {
                                let scope = scope.to_owned();
                                thread::spawn(move || {
                                    local_dap.variables_async(
                                        scope.variables_reference,
                                        move |result: Result<
                                            dap_types::VariablesResponse,
                                            RpcError,
                                        >| {
                                            let resp: Vec<(
                                                dap_types::Scope,
                                                Vec<dap_types::Variable>,
                                            )> = scopes
                                                .iter()
                                                .enumerate()
                                                .map(|(index, s)| {
                                                    (
                                                        s.clone(),
                                                        if index == 0 {
                                                            result
                                                                .as_ref()
                                                                .map(|resp| {
                                                                    resp.variables
                                                                        .clone()
                                                                })
                                                                .unwrap_or_default()
                                                        } else {
                                                            Vec::new()
                                                        },
                                                    )
                                                })
                                                .collect();
                                            f.call(Ok(resp));
                                        },
                                    );
                                });
                            } else {
                                f.call(Ok(Vec::new()));
                            }
                        }
                        Err(e) => {
                            f.call(Err(e));
                        }
                    }
                },
            );
        } else {
            f.call(Err(RpcError {
                code: 0,
                message: "plugin doesn't exist".to_string(),
            }));
        }
    }

    pub fn handle_notification(&mut self, notification: PluginCatalogNotification) {
        use PluginCatalogNotification::*;
        match notification {
            RegisterWatchedFiles {
                plugin_id,
                id,
                registration,
            } => {
                if self.plugins.contains_key(&plugin_id) {
                    self.watched_files.register(plugin_id, id, registration);
                    self.plugin_rpc.core_rpc.log(
                        ahead_rpc::core::LogLevel::Trace,
                        format!(
                            "language server registered watched-file capability; roots: {:?}",
                            self.watched_files.root_paths()
                        ),
                        Some("ahead_proxy::plugin::catalog::watched_files".into()),
                    );
                }
            }
            UnregisterWatchedFiles { plugin_id, id } => {
                self.watched_files.unregister(plugin_id, &id);
            }
            WatchedFilesChanged(event) => {
                let changes = self.watched_files.changes(&event);
                self.plugin_rpc.core_rpc.log(
                    ahead_rpc::core::LogLevel::Trace,
                    format!(
                        "LSP file watcher matched {} server(s) for {:?} {:?}",
                        changes.len(),
                        event.kind,
                        event.paths
                    ),
                    Some("ahead_proxy::plugin::catalog::watched_files".into()),
                );
                for (plugin_id, changes) in changes {
                    if let Some(plugin) = self.plugins.get(&plugin_id) {
                        plugin.server_notification(
                            lsp_types::notification::DidChangeWatchedFiles::METHOD,
                            lsp_types::DidChangeWatchedFilesParams { changes },
                            None,
                            None,
                            false,
                        );
                    }
                }
            }
            RefreshWorkspaceConfigurations => {
                self.refresh_workspace_configurations();
            }
            RepairBuiltinNpmServer { server_id } => {
                self.repair_builtin_npm_server(server_id);
            }
            RepairBuiltinNpmServerFinished { server_id, result } => {
                let Some(builtin) = builtin_npm_language_server(&server_id) else {
                    tracing::error!(%server_id, "ignoring completion for unknown language-server repair");
                    return;
                };
                self.installing_servers.lock().remove(builtin.command);
                if let Err(message) = result {
                    self.report_language_server_error(
                        builtin.command,
                        &anyhow::anyhow!(message),
                    );
                    return;
                }
                let documents = self
                    .open_files
                    .values()
                    .map(OpenDocument::latest_item)
                    .collect();
                self.handle_notification(RestartLanguageServers {
                    documents,
                    extension_id: None,
                });
            }
            BuiltinNpmServerUpdateAvailable {
                server_id,
                latest_version,
            } => self.update_builtin_npm_server(server_id, latest_version),
            BuiltinServerResolved {
                language_id,
                server_name,
                generation,
                result,
            } => self.handle_builtin_server_resolved(
                language_id,
                server_name,
                generation,
                result,
            ),
            LanguageServerResolved {
                language_id,
                extension,
                generation,
                result,
            } => {
                if self.pending_extension_resolutions.get(&extension.server_id)
                    != Some(&generation)
                {
                    return;
                }
                self.pending_extension_resolutions
                    .remove(&extension.server_id);
                if generation != self.configuration_generation
                    || !self.language_extensions.contains(&extension)
                {
                    if self
                        .open_files
                        .values()
                        .any(|document| document.language_id == language_id)
                    {
                        self.ensure_lsp_server(&language_id);
                    }
                    return;
                }
                let Some(workspace) = self.workspace.clone() else {
                    return;
                };
                if !matches!(
                    ahead_core::workspace_trust::is_trusted(&workspace),
                    Ok(true)
                ) || self.has_lsp_server(&language_id, &extension.server_id)
                {
                    return;
                }
                let documents = self
                    .open_files
                    .values()
                    .filter(|document| {
                        extension.supports_language(&document.language_id)
                    })
                    .map(OpenDocument::latest_item)
                    .collect::<Vec<_>>();
                if documents.is_empty() {
                    return;
                }
                match result {
                    Ok(server) => {
                        let plugin_id = self.start_lsp_server(
                            &workspace,
                            &language_id,
                            server,
                            extension.server_id.clone(),
                            extension.language_ids(),
                            Some(extension.clone()),
                        );
                        if let Some(plugin_id) = plugin_id
                            && let Some(plugin) = self.plugins.get(&plugin_id)
                        {
                            for mut document in documents {
                                let language_id = document.language_id.clone();
                                let path = document.uri.to_file_path().ok();
                                document.language_id =
                                    extension.protocol_language_id(&language_id);
                                plugin.server_notification(
                                    DidOpenTextDocument::METHOD,
                                    DidOpenTextDocumentParams {
                                        text_document: document,
                                    },
                                    Some(language_id),
                                    path,
                                    true,
                                );
                            }
                            self.refresh_diagnostics(plugin_id);
                        }
                    }
                    Err(message) => self.report_language_server_error(
                        &extension.server_id,
                        &anyhow::anyhow!(message),
                    ),
                }
            }
            ExtensionConfigurationResolved {
                plugin_id,
                generation,
                result,
            } => {
                if generation != self.configuration_generation {
                    return;
                }
                let Some(extension) =
                    self.extension_servers.get(&plugin_id).cloned()
                else {
                    return;
                };
                match result {
                    Ok(command) => {
                        let command_changed = self
                            .extension_commands
                            .get(&plugin_id)
                            .is_some_and(|previous| {
                                language_server_command_requires_restart(
                                    previous, &command,
                                )
                            });
                        if command_changed {
                            self.restart_extension_language_server(plugin_id);
                        } else {
                            if let Some(server) = self.plugins.get(&plugin_id) {
                                server.update_workspace_configuration(
                                    command.workspace_configuration.clone(),
                                );
                            }
                            self.extension_commands.insert(plugin_id, command);
                        }
                    }
                    Err(message) => {
                        tracing::error!(server = %extension.server_id, %message, "refreshing language-server configuration");
                        self.plugin_rpc.core_rpc.show_message(
                            "Language server settings".into(),
                            lsp_types::ShowMessageParams {
                                typ: lsp_types::MessageType::ERROR,
                                message: format!(
                                    "{}: {message}",
                                    extension.server_id
                                ),
                            },
                        );
                    }
                }
            }
            RestartLanguageServers {
                documents,
                extension_id,
            } => {
                match Self::load_routing(self.workspace.as_deref()) {
                    Ok(routing) => self.routing = Ok(routing),
                    Err(message) => {
                        self.report_routing_error(message);
                        return;
                    }
                }
                self.configuration_generation =
                    self.configuration_generation.wrapping_add(1);
                self.pending_extension_resolutions.clear();
                self.pending_builtin_resolutions.clear();
                self.plugin_rpc.core_rpc.clear_language_server_statuses();
                let installing = self
                    .installing_servers
                    .lock()
                    .iter()
                    .copied()
                    .collect::<Vec<_>>();
                for name in installing {
                    let mut status =
                        ahead_rpc::core::ServerStatusParams::starting(name.into());
                    status.message =
                        Some("Installing from npm into AHEAD's cache…".into());
                    self.plugin_rpc.core_rpc.server_status(status);
                }
                for plugin in self.plugins.values() {
                    plugin.shutdown();
                }
                for plugin in self.plugins.values() {
                    if !plugin.wait_for_shutdown() {
                        self.report_language_server_error(plugin.server_id.name.strip_prefix("lsp-").unwrap_or(&plugin.server_id.name), &anyhow::anyhow!("Language server did not stop before its shutdown deadline. Try restarting again."));
                        return;
                    }
                }
                for plugin_id in self.plugins.keys().copied().collect::<Vec<_>>() {
                    self.remove_language_server(plugin_id);
                }
                let invalidation =
                    self.extension_host.as_ref().map_or(Ok(()), |host| {
                        if let Some(extension_id) = extension_id.as_deref() {
                            self.language_extensions
                                .iter()
                                .find(|extension| extension.id == extension_id)
                                .map_or(Ok(()), |extension| {
                                    host.invalidate_extension_sessions(
                                        &extension.directory,
                                    )
                                })
                        } else {
                            host.invalidate_sessions()
                        }
                    });
                if let Err(error) = invalidation {
                    tracing::error!(
                        ?error,
                        "invalidating language extension sessions"
                    );
                    self.plugin_rpc.core_rpc.show_message(
                        "Language extensions".into(),
                        lsp_types::ShowMessageParams {
                            typ: lsp_types::MessageType::ERROR,
                            message: format!(
                                "Could not reload language extension runtime: {error:#}"
                            ),
                        },
                    );
                    return;
                }
                if let Err(error) = self.reload_language_extensions() {
                    tracing::error!(
                        ?error,
                        "reloading installed language extensions"
                    );
                    self.plugin_rpc.core_rpc.show_message(
                        "Language extensions".into(),
                        lsp_types::ShowMessageParams {
                            typ: lsp_types::MessageType::ERROR,
                            message: format!(
                                "Could not reload language extensions: {error:#}"
                            ),
                        },
                    );
                }
                for document in documents {
                    self.handle_did_open_text_document(document);
                }
            }
            LanguageServerStopped { plugin_id, message } => {
                if let Some(plugin) = self.plugins.get(&plugin_id) {
                    let name = plugin
                        .server_id
                        .name
                        .strip_prefix("lsp-")
                        .unwrap_or(&plugin.server_id.name)
                        .to_owned();
                    self.remove_language_server(plugin_id);
                    self.plugin_rpc.core_rpc.server_status(
                        ahead_rpc::core::ServerStatusParams::failed(name, message),
                    );
                }
            }
            LanguageServerStatus { plugin_id, params } => {
                if self.plugins.contains_key(&plugin_id) {
                    if params.is_ok()
                        && let Some((builtin, cache)) = self
                            .pending_builtin_npm_version_checks
                            .remove(&plugin_id)
                    {
                        self.start_builtin_npm_version_check(builtin, cache);
                    }
                    self.plugin_rpc.core_rpc.server_status(params);
                }
            }
            DapStart {
                config,
                breakpoints,
            } => {
                // Keep disconnecting adapters owned until cleanup finishes, so
                // catalog shutdown can still wait for their processes.
                self.daps.retain(|_, dap| {
                    !dap.wait_for_shutdown_until(std::time::Instant::now())
                });
                let server = resolve_dap_server(&config, self.workspace.clone());
                let dap_id = config.dap_id;
                match DapClient::start(
                    server,
                    config,
                    breakpoints,
                    self.plugin_rpc.clone(),
                ) {
                    Ok(dap_rpc) => {
                        if let Some(previous) =
                            self.daps.insert(dap_rpc.dap_id, dap_rpc)
                        {
                            previous.shutdown();
                        }
                    }
                    Err(error) => self.plugin_rpc.core_rpc.dap_session_state(
                        dap_id,
                        ahead_rpc::dap_types::DapSessionState::Failed(format!(
                            "Could not start debugger: {error}"
                        )),
                    ),
                }
            }
            DapTerminalResponse { response } => {
                if let Some(dap) = self.daps.get(&response.dap_id)
                    && let Err(error) = dap.answer_terminal_request(response)
                {
                    tracing::error!(?error, "answering debug terminal request");
                }
            }
            DapContinue { dap_id, thread_id } => {
                if let Some(dap) = self.daps.get(&dap_id) {
                    dap.continue_thread(thread_id);
                }
            }
            DapPause { dap_id, thread_id } => {
                if let Some(dap) = self.daps.get(&dap_id) {
                    dap.pause_thread(thread_id);
                }
            }
            DapStepOver { dap_id, thread_id } => {
                if let Some(dap) = self.daps.get(&dap_id).cloned() {
                    dap.next(thread_id);
                }
            }
            DapStepInto { dap_id, thread_id } => {
                if let Some(dap) = self.daps.get(&dap_id).cloned() {
                    dap.step_in(thread_id);
                }
            }
            DapStepOut { dap_id, thread_id } => {
                if let Some(dap) = self.daps.get(&dap_id).cloned() {
                    dap.step_out(thread_id);
                }
            }
            DapStop { dap_id } => {
                if let Some(dap) = self.daps.get(&dap_id) {
                    dap.stop();
                }
            }
            DapDisconnect { dap_id } => {
                if let Some(dap) = self.daps.get(&dap_id).cloned() {
                    thread::spawn(move || {
                        if let Err(err) = dap.disconnect() {
                            tracing::error!("{:?}", err);
                        }
                        dap.shutdown();
                    });
                }
            }
            DapRestart {
                dap_id,
                breakpoints,
            } => {
                if let Some(dap) = self.daps.get(&dap_id) {
                    dap.restart(breakpoints);
                }
            }
            DapSetBreakpoints {
                dap_id,
                path,
                breakpoints,
            } => {
                if let Some(dap) = self.daps.get(&dap_id) {
                    let core_rpc = self.plugin_rpc.core_rpc.clone();
                    dap.set_breakpoints_async(
                        path.clone(),
                        breakpoints,
                        move |result: Result<SetBreakpointsResponse, RpcError>| {
                            match result {
                                Ok(resp) => {
                                    core_rpc.dap_breakpoints_resp(
                                        dap_id,
                                        path,
                                        resp.breakpoints.unwrap_or_default(),
                                    );
                                }
                                Err(err) => {
                                    tracing::error!("{:?}", err);
                                }
                            }
                        },
                    );
                }
            }
            Shutdown => {
                self.watched_files.clear();
                self.cancel_installs.store(true, Ordering::Relaxed);
                let install_deadline = Instant::now() + Duration::from_secs(1);
                while (!self.installing_servers.lock().is_empty()
                    || self.resolving_servers.load(Ordering::Relaxed) > 0)
                    && Instant::now() < install_deadline
                {
                    thread::sleep(Duration::from_millis(10));
                }
                if !self.installing_servers.lock().is_empty()
                    || self.resolving_servers.load(Ordering::Relaxed) > 0
                {
                    tracing::warn!(
                        "language extension acquisition did not stop before shutdown deadline"
                    );
                }
                for plugin in self.plugins.values() {
                    plugin.shutdown();
                }
                for dap in self.daps.values() {
                    dap.shutdown();
                }
                let deadline = std::time::Instant::now()
                    + super::psp::SERVER_SHUTDOWN_TIMEOUT
                    + std::time::Duration::from_secs(1);
                for plugin in self.plugins.values() {
                    if !plugin.wait_for_shutdown_until(deadline) {
                        tracing::error!(
                            ?plugin.plugin_id,
                            "language server did not stop before the catalog shutdown deadline"
                        );
                    }
                }
                for dap in self.daps.values() {
                    if !dap.wait_for_shutdown_until(deadline) {
                        tracing::error!(?dap.dap_id, "debug adapter did not stop before the catalog shutdown deadline");
                    }
                }
            }
        }
    }

    fn remove_language_server(&mut self, plugin_id: PluginId) {
        self.watched_files.remove_server(plugin_id);
        self.pending_builtin_npm_version_checks.remove(&plugin_id);
        self.plugins.remove(&plugin_id);
        self.extension_servers.remove(&plugin_id);
        self.extension_commands.remove(&plugin_id);
        self.lsp_servers.retain(|_, ids| {
            ids.retain(|id| *id != plugin_id);
            !ids.is_empty()
        });
        self.diagnostics.retain(|uri, sources| {
            let before = sources.len();
            sources.retain(|(id, _), _| *id != plugin_id);
            if before != sources.len() {
                self.plugin_rpc.core_rpc.publish_diagnostics(
                    PublishDiagnosticsParams::new(
                        uri.clone(),
                        sources.values().flatten().cloned().collect(),
                        None,
                    ),
                );
            }
            !sources.is_empty()
        });
    }
}

fn full_document_diagnostics(
    report: DocumentDiagnosticReportResult,
) -> Option<Vec<Diagnostic>> {
    match report {
        DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(
            report,
        )) => Some(report.full_document_diagnostic_report.items),
        _ => None,
    }
}

const MAX_DIAGNOSTIC_RETRIES: u8 = 3;

fn request_document_diagnostics(
    plugin: PluginServerRpcHandler,
    plugin_id: PluginId,
    path: PathBuf,
    uri: url::Url,
    language_id: String,
    catalog_rpc: PluginCatalogRpcHandler,
    generation: u64,
    current_generation: Arc<AtomicU64>,
    retries: u8,
) {
    if current_generation.load(Ordering::Relaxed) != generation {
        return;
    }
    let retry_plugin = plugin.clone();
    plugin.server_request_async(
        DocumentDiagnosticRequest::METHOD,
        DocumentDiagnosticParams {
            text_document: TextDocumentIdentifier::new(uri.clone()),
            identifier: None,
            previous_result_id: None,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        },
        Some(language_id.clone()),
        Some(path.clone()),
        true,
        move |result: Result<Value, RpcError>| match result {
            Ok(value) => {
                match serde_json::from_value::<DocumentDiagnosticReportResult>(value)
                {
                    Ok(report) => {
                        if current_generation.load(Ordering::Relaxed) == generation
                            && let Some(diagnostics) =
                                full_document_diagnostics(report)
                        {
                            catalog_rpc.publish_diagnostics(
                                plugin_id,
                                DiagnosticSource::Pulled,
                                PublishDiagnosticsParams {
                                    uri,
                                    diagnostics,
                                    version: None,
                                },
                            );
                        }
                    }
                    Err(error) => {
                        tracing::warn!(?error, "decoding document diagnostics")
                    }
                }
            }
            Err(error)
                if error.code == lsp_types::error_codes::SERVER_CANCELLED
                    && retries < MAX_DIAGNOSTIC_RETRIES
                    && current_generation.load(Ordering::Relaxed) == generation =>
            {
                request_document_diagnostics(
                    retry_plugin,
                    plugin_id,
                    path,
                    uri,
                    language_id,
                    catalog_rpc,
                    generation,
                    current_generation,
                    retries + 1,
                );
            }
            Err(error) => tracing::debug!(?error, "pulling document diagnostics"),
        },
    );
}

#[cfg(test)]
mod document_diagnostic_tests {
    use super::*;
    use crate::plugin::PluginCatalogRpc;

    fn test_extension_component() -> Vec<u8> {
        let section_name = b"zed:api-version";
        let mut section = Vec::with_capacity(section_name.len() + 7);
        append_unsigned_leb128(section_name.len() as u32, &mut section);
        section.extend_from_slice(section_name);
        section.extend_from_slice(&[0, 0, 0, 7, 0, 0]);

        let mut component = b"\0asm\x0d\0\x01\0".to_vec();
        component.push(0);
        append_unsigned_leb128(section.len() as u32, &mut component);
        component.extend_from_slice(&section);
        component
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

    #[test]
    fn zed_language_server_defaults_prefer_one_supported_adapter() {
        let available = [
            "basedpyright-langserver",
            "pyright",
            "pyrefly",
            "pylsp",
            "ruff",
            "ty",
            "custom-python-server",
        ]
        .map(str::to_owned);
        assert_eq!(
            zed_default_language_servers("Python", &available, Vec::new(), &[]),
            ["basedpyright-langserver", "ruff", "custom-python-server"]
                .map(str::to_owned)
        );

        let available = [
            "typescript-language-server",
            "vtsls",
            "custom-typescript-server",
        ]
        .map(str::to_owned);
        assert_eq!(
            zed_default_language_servers("TSX", &available, Vec::new(), &[]),
            ["vtsls", "custom-typescript-server"].map(str::to_owned)
        );
    }

    #[test]
    fn zed_language_server_defaults_keep_opt_in_servers_dormant() {
        let available =
            ["basedpyright", "basedpyright-langserver", "pyright", "ruff"]
                .map(str::to_owned);
        let opt_in = ["basedpyright", "ruff"].map(str::to_owned);
        assert_eq!(
            zed_default_language_servers("python", &available, Vec::new(), &opt_in),
            ["basedpyright-langserver"].map(str::to_owned)
        );
    }

    #[test]
    fn opt_in_extension_servers_require_explicit_language_routing()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let workspace = root.path().join("workspace");
        let extension = root.path().join("extensions/html");
        std::fs::create_dir_all(workspace.join(".ahead"))?;
        std::fs::create_dir_all(&extension)?;
        std::fs::write(
            extension.join("extension.toml"),
            "id = 'html'\n[lib]\nversion = '0.7.0'\n[language_servers.html-server]\nlanguages = ['HTML']\nopt_in_languages = ['HTML']\n",
        )?;
        std::fs::write(
            extension.join("extension.wasm"),
            test_extension_component(),
        )?;

        let mut catalog = PluginCatalog::new(
            Some(workspace.clone()),
            PluginCatalogRpcHandler::new(ahead_rpc::core::CoreRpcHandler::new()),
        );
        catalog.language_extensions =
            discover_language_server_extensions(&root.path().join("extensions"))?
                .extensions;
        catalog.routing = Ok(LanguageServerRouting::load(Some(&workspace), None)?);

        assert_eq!(catalog.language_server_names("html").0, ["html-server"]);
        assert!(
            catalog
                .selected_servers("html")
                .expect("default routing")
                .is_empty()
        );

        let config = workspace.join(".ahead/config.toml");
        std::fs::write(
            &config,
            "[languages.HTML]\nlanguage_servers = ['html-server']\n",
        )?;
        catalog.routing = Ok(LanguageServerRouting::load(Some(&workspace), None)?);
        assert_eq!(
            catalog
                .selected_servers("html")
                .expect("explicit server routing"),
            ["html-server"]
        );

        std::fs::write(&config, "[languages.HTML]\nlanguage_servers = ['...']\n")?;
        catalog.routing = Ok(LanguageServerRouting::load(Some(&workspace), None)?);
        assert_eq!(
            catalog
                .selected_servers("html")
                .expect("wildcard server routing"),
            Vec::<String>::new()
        );
        Ok(())
    }

    #[test]
    fn proto_defaults_match_zed_and_allow_explicit_server_choice()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let workspace = root.path().join("workspace");
        let extension = root.path().join("extensions/proto");
        std::fs::create_dir_all(workspace.join(".ahead"))?;
        std::fs::create_dir_all(&extension)?;
        std::fs::write(
            extension.join("extension.toml"),
            "id = 'proto'\n[lib]\nversion = '0.7.0'\n[language_servers.buf]\nlanguages = ['Proto']\n[language_servers.protols]\nlanguages = ['Proto']\n[language_servers.protobuf-language-server]\nlanguages = ['Proto']\n",
        )?;
        std::fs::write(
            extension.join("extension.wasm"),
            test_extension_component(),
        )?;

        let mut catalog = PluginCatalog::new(
            Some(workspace.clone()),
            PluginCatalogRpcHandler::new(ahead_rpc::core::CoreRpcHandler::new()),
        );
        catalog.language_extensions =
            discover_language_server_extensions(&root.path().join("extensions"))?
                .extensions;
        catalog.routing = Ok(LanguageServerRouting::load(Some(&workspace), None)?);

        assert_eq!(
            catalog
                .selected_servers("Proto")
                .map_err(|error| anyhow::anyhow!("{error:?}"))?,
            ["buf"]
        );

        let config = workspace.join(".ahead/config.toml");
        std::fs::write(
            &config,
            "[languages.Proto]\nlanguage_servers = ['protols']\n",
        )?;
        catalog.routing = Ok(LanguageServerRouting::load(Some(&workspace), None)?);
        assert_eq!(
            catalog
                .selected_servers("Proto")
                .map_err(|error| anyhow::anyhow!("{error:?}"))?,
            ["protols"]
        );
        Ok(())
    }

    #[test]
    fn configured_server_order_and_feature_routes_preserve_explicit_owners()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join(".ahead"))?;
        std::fs::write(
            root.path().join(".ahead/config.toml"),
            "[languages.Python]\nlanguage_servers = ['ruff', 'basedpyright-langserver']\n[languages.Python.requests]\n\"textDocument/hover\" = 'basedpyright-langserver'\n",
        )?;
        let core = ahead_rpc::core::CoreRpcHandler::new();
        let mut catalog = PluginCatalog::new(
            Some(root.path().into()),
            PluginCatalogRpcHandler::new(core),
        );
        catalog.routing = Ok(LanguageServerRouting::load(Some(root.path()), None)?);
        let mut receivers = Vec::new();
        for (id, name) in [(1, "basedpyright-langserver"), (2, "ruff")] {
            let (sender, _receiver) = crossbeam_channel::unbounded();
            let server = PluginServerRpcHandler::new(
                ServerId {
                    author: "ahead".into(),
                    name: format!("lsp-{name}"),
                },
                Some(PluginId(id)),
                sender,
            );
            let requests = server.test_receiver();
            requests.recv()?;
            receivers.push(requests);
            catalog.plugins.insert(PluginId(id), server);
        }
        let ids = |servers: VecDeque<PluginServerRpcHandler>| {
            servers
                .into_iter()
                .map(|server| server.plugin_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(catalog
                .request_servers(Some("python"), "textDocument/completion")
                .expect("configured order"))
            .as_slice(),
            [PluginId(2), PluginId(1)]
        );
        assert_eq!(
            ids(catalog
                .request_servers(Some("Python"), "textDocument/hover")
                .expect("hover preference"))
            .as_slice(),
            [PluginId(1)]
        );

        // A resolve/command owner is not redirected by a feature preference.
        let (sender, receiver) = crossbeam_channel::bounded(1);
        catalog.handle_server_request(
            Some(PluginId(2)),
            None,
            None,
            "textDocument/hover".into(),
            Value::Null,
            Some("python".into()),
            None,
            true,
            Box::new(move |id, result| {
                sender.send((id, result)).expect("owner response")
            }),
        );
        let PluginServerRpc::ServerRequest { rh, .. } =
            receivers[1].recv_timeout(Duration::from_secs(1))?
        else {
            anyhow::bail!("owned request")
        };
        rh.invoke(Ok(Value::Null));
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(1))?.0,
            PluginId(2)
        );
        assert!(receivers[0].try_recv().is_err());

        std::fs::write(
            root.path().join(".ahead/config.toml"),
            "[languages.Python]\nlanguage_servers = ['!ruff', '...']\n[languages.Python.requests]\n\"textDocument/formatting\" = 'ruff'\n",
        )?;
        catalog.routing = Ok(LanguageServerRouting::load(Some(root.path()), None)?);
        assert_eq!(
            ids(catalog
                .request_servers(Some("python"), "textDocument/completion")
                .expect("disabled formatter")),
            [PluginId(1)]
        );
        assert_eq!(
            catalog
                .request_servers(Some("python"), "textDocument/formatting")
                .err()
                .expect("disabled preferred server")
                .code,
            -32602
        );
        Ok(())
    }

    #[test]
    fn changed_routing_restarts_from_unsaved_documents_and_invalid_settings_keep_last_valid_order()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join(".ahead"))?;
        let config = root.path().join(".ahead/config.toml");
        let core = ahead_rpc::core::CoreRpcHandler::new();
        let mut catalog = PluginCatalog::new(
            Some(root.path().into()),
            PluginCatalogRpcHandler::new(core),
        );
        catalog.routing = Ok(LanguageServerRouting::load(Some(root.path()), None)?);
        let path = root.path().join("main.py");
        let document = TextDocumentItem::new(
            url::Url::from_file_path(&path).expect("URI"),
            "python".into(),
            7,
            "print('unsaved')\n".into(),
        );
        // The fixture remains untrusted, so restarting cannot launch or download a server.
        catalog.handle_did_open_text_document(document);
        let previous_generation = catalog.configuration_generation;
        std::fs::write(&config, "[languages.Python]\nlanguage_servers = []\n")?;
        catalog.refresh_workspace_configurations();
        assert!(
            catalog
                .selected_servers("python")
                .expect("disabled servers")
                .is_empty()
        );
        assert_ne!(catalog.configuration_generation, previous_generation);
        let restored = catalog
            .open_files
            .get(&path)
            .expect("open unsaved document");
        assert_eq!(restored.version, 7);
        assert_eq!(restored.text.to_string(), "print('unsaved')\n");
        assert!(!path.exists(), "routing changes must not save buffers");
        let previous_generation = catalog.configuration_generation;
        std::fs::write(&config, "[languages.Python]\nlanguage_servers = 42\n")?;
        catalog.refresh_workspace_configurations();
        assert!(
            catalog
                .selected_servers("python")
                .expect("last valid settings")
                .is_empty()
        );
        assert_eq!(catalog.configuration_generation, previous_generation);
        Ok(())
    }

    #[test]
    fn file_watchers_notify_only_registered_live_servers_and_remove_retired_owners()
    {
        let project = tempfile::tempdir().expect("project");
        let workspace = project.path().canonicalize().expect("workspace");
        let core = ahead_rpc::core::CoreRpcHandler::new();
        let mut catalog = PluginCatalog::new(
            Some(workspace.clone()),
            PluginCatalogRpcHandler::new(core),
        );
        let mut receivers = Vec::new();
        for id in [PluginId(1), PluginId(2)] {
            let (sender, receiver) = crossbeam_channel::unbounded();
            catalog.plugins.insert(
                id,
                PluginServerRpcHandler::new(
                    ServerId {
                        author: "ahead".into(),
                        name: "test".into(),
                    },
                    Some(id),
                    sender,
                ),
            );
            receivers.push(receiver);
        }
        let register = |catalog: &mut PluginCatalog, plugin_id, pattern| {
            catalog.handle_notification(PluginCatalogNotification::RegisterWatchedFiles {
                plugin_id, id: "config".into(),
                registration: super::super::watched_files::WatchedFileRegistration::new(Some(&workspace), serde_json::from_value(serde_json::json!({ "watchers": [{ "globPattern": pattern }] })).expect("options")).expect("registration"),
            });
        };
        register(&mut catalog, PluginId(1), "**/ruff.toml");
        register(&mut catalog, PluginId(2), "**/pyproject.toml");
        let event = notify::Event::new(notify::EventKind::Modify(
            notify::event::ModifyKind::Any,
        ))
        .add_path(workspace.join("ruff.toml"));
        catalog.handle_notification(PluginCatalogNotification::WatchedFilesChanged(
            event.clone(),
        ));
        let notification = receivers[0]
            .try_recv()
            .expect("owning server notification")
            .expect("frame");
        assert_eq!(
            notification.get_method(),
            Some("workspace/didChangeWatchedFiles")
        );
        let params: lsp_types::DidChangeWatchedFilesParams = serde_json::from_value(
            serde_json::to_value(notification.get_params().expect("params"))
                .expect("serialize"),
        )
        .expect("file events");
        assert_eq!(params.changes.len(), 1);
        assert_eq!(params.changes[0].typ, lsp_types::FileChangeType::CHANGED);
        assert!(receivers[1].try_recv().is_err());
        catalog.handle_notification(
            PluginCatalogNotification::UnregisterWatchedFiles {
                plugin_id: PluginId(1),
                id: "config".into(),
            },
        );
        catalog.handle_notification(PluginCatalogNotification::WatchedFilesChanged(
            event.clone(),
        ));
        assert!(receivers[0].try_recv().is_err());
        register(&mut catalog, PluginId(1), "**/ruff.toml");
        catalog.remove_language_server(PluginId(1));
        // A delayed registration from a stopped process cannot recreate its watches.
        register(&mut catalog, PluginId(1), "**/ruff.toml");
        assert!(catalog.watched_files.changes(&event).is_empty());
        catalog.handle_notification(PluginCatalogNotification::WatchedFilesChanged(
            notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Any,
            ))
            .add_path(workspace.join("pyproject.toml")),
        ));
        assert!(receivers[1].try_recv().is_ok());
        catalog.watched_files.clear();
    }

    fn discard_initial_extension_issues(core: &ahead_rpc::core::CoreRpcHandler) {
        use ahead_rpc::core::{CoreNotification, CoreRpc};

        if let Ok(message) = core.rx().try_recv() {
            assert!(matches!(
                message,
                CoreRpc::Notification(notification)
                    if matches!(&*notification, CoreNotification::LanguageExtensionIssues { .. })
            ));
        }
    }

    #[test]
    fn extension_issues_are_not_server_statuses() {
        use ahead_rpc::core::{CoreNotification, CoreRpc, CoreRpcHandler};

        let core = CoreRpcHandler::new();
        let rpc = PluginCatalogRpcHandler::new(core.clone());
        report_extension_issues(
            &rpc,
            vec![
                (
                    PathBuf::from("broken-extension"),
                    anyhow::anyhow!("bad manifest"),
                ),
                (
                    PathBuf::from("Language extension host"),
                    anyhow::anyhow!("invalid user settings"),
                ),
            ],
        );
        let CoreRpc::Notification(notification) =
            core.rx().try_recv().expect("issue")
        else {
            panic!("expected notification");
        };
        let CoreNotification::LanguageExtensionIssues { issues } = *notification
        else {
            panic!("extension issue must not be a language-server status");
        };
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].name, "broken-extension");
        assert!(issues[0].message.contains("bad manifest"));
        assert_eq!(issues[1].name, "Language extension host");
        assert!(issues[1].message.contains("invalid user settings"));
        assert!(core.rx().try_recv().is_err());

        report_extension_issues(&rpc, Vec::new());
        assert!(matches!(
            core.rx().try_recv(),
            Ok(CoreRpc::Notification(notification))
                if matches!(&*notification, CoreNotification::LanguageExtensionIssues { issues } if issues.is_empty())
        ));
    }

    #[test]
    fn restart_clears_old_language_server_statuses() {
        use ahead_rpc::core::{CoreNotification, CoreRpc, CoreRpcHandler};

        let core = CoreRpcHandler::new();
        let rpc = PluginCatalogRpcHandler::new(core.clone());
        let mut catalog = PluginCatalog::new(None, rpc);
        catalog.handle_notification(
            PluginCatalogNotification::RestartLanguageServers {
                documents: Vec::new(),
                extension_id: None,
            },
        );
        assert!(core.rx().try_iter().any(|event| matches!(
            event,
            CoreRpc::Notification(notification)
                if matches!(&*notification, CoreNotification::LanguageServerStatusesCleared)
        )));
    }

    #[test]
    fn restart_invalidates_pending_server_resolution() {
        use ahead_rpc::core::CoreRpcHandler;

        let core = CoreRpcHandler::new();
        let rpc = PluginCatalogRpcHandler::new(core);
        let mut catalog = PluginCatalog::new(None, rpc);
        let previous_generation = catalog.configuration_generation;
        catalog
            .pending_extension_resolutions
            .insert("html".into(), previous_generation);
        catalog
            .pending_builtin_resolutions
            .insert("python".into(), previous_generation);

        catalog.handle_notification(
            PluginCatalogNotification::RestartLanguageServers {
                documents: Vec::new(),
                extension_id: None,
            },
        );

        assert_ne!(catalog.configuration_generation, previous_generation);
        assert!(catalog.pending_extension_resolutions.is_empty());
        assert!(catalog.pending_builtin_resolutions.is_empty());
    }

    #[test]
    fn managed_npm_server_repair_only_removes_the_selected_package()
    -> anyhow::Result<()> {
        let cache = tempfile::tempdir()?;
        let root = cache.path().join("language-servers");
        let basedpyright = root.join("basedpyright");
        let vtsls = root.join("vtsls");
        std::fs::create_dir_all(&basedpyright)?;
        std::fs::create_dir_all(&vtsls)?;
        std::fs::write(basedpyright.join("broken.marker"), "remove")?;
        std::fs::write(vtsls.join("keep.marker"), "keep")?;
        std::fs::write(root.join("npm-cache-marker"), "keep")?;

        remove_cached_builtin_npm_package(
            cache.path(),
            builtin_npm_language_server("basedpyright-langserver")
                .expect("managed BasedPyright"),
            &AtomicBool::new(false),
        )?;

        assert!(!basedpyright.exists());
        assert_eq!(std::fs::read_to_string(vtsls.join("keep.marker"))?, "keep");
        assert_eq!(
            std::fs::read_to_string(root.join("npm-cache-marker"))?,
            "keep"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn managed_npm_server_repair_refuses_a_symlinked_cache_root()
    -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;

        let cache = tempfile::tempdir()?;
        let external = tempfile::tempdir()?;
        let external_package = external.path().join("basedpyright");
        std::fs::create_dir(&external_package)?;
        std::fs::write(external_package.join("keep.marker"), "keep")?;
        symlink(external.path(), cache.path().join("language-servers"))?;

        let result = remove_cached_builtin_npm_package(
            cache.path(),
            builtin_npm_language_server("basedpyright-langserver")
                .expect("managed BasedPyright"),
            &AtomicBool::new(false),
        );

        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(external_package.join("keep.marker"))?,
            "keep"
        );
        Ok(())
    }

    #[test]
    fn only_ahead_managed_npm_servers_are_repairable() {
        assert_eq!(
            builtin_npm_language_server("basedpyright-langserver")
                .map(|server| server.command),
            Some("basedpyright-langserver")
        );
        assert_eq!(
            builtin_npm_language_server("vtsls").map(|server| server.command),
            Some("vtsls")
        );
        for server_id in ["rust-analyzer", "ruff", "unknown"] {
            assert!(builtin_npm_language_server(server_id).is_none());
        }
    }

    #[test]
    fn npm_stderr_tail_is_bounded_and_retains_the_error_code() -> anyhow::Result<()>
    {
        let mut output = vec![b'x'; MAX_NPM_STDERR_BYTES + 64];
        output.extend_from_slice(b"\nnpm error code EAI_AGAIN\n");
        let stderr = read_npm_stderr_tail(Cursor::new(output))?;
        assert!(stderr.len() <= MAX_NPM_STDERR_BYTES);
        assert_eq!(npm_error_code(&stderr), Some("EAI_AGAIN"));
        Ok(())
    }

    #[test]
    fn npm_version_output_accepts_json_and_plain_semver() -> anyhow::Result<()> {
        assert_eq!(
            parse_npm_package_version(br#""1.2.3""#)?,
            semver::Version::new(1, 2, 3)
        );
        assert_eq!(
            parse_npm_package_version(b"2.0.0\n")?,
            semver::Version::new(2, 0, 0)
        );
        assert!(parse_npm_package_version(b"not-a-version").is_err());

        let (output, oversized) =
            read_bounded_npm_version_output(std::io::Cursor::new(vec![
                b'x';
                MAX_NPM_VERSION_OUTPUT_BYTES
                    + 1
            ]))?;
        assert_eq!(output.len(), MAX_NPM_VERSION_OUTPUT_BYTES);
        assert!(oversized);

        let current = semver::Version::new(1, 2, 3);
        assert!(npm_update_available(
            &current,
            &semver::Version::new(1, 2, 4)
        ));
        assert!(!npm_update_available(
            &current,
            &semver::Version::new(1, 2, 3)
        ));
        assert!(!npm_update_available(
            &semver::Version::new(2, 0, 0),
            &current
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn latest_npm_version_lookup_uses_the_bounded_process_path() -> anyhow::Result<()>
    {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir()?;
        let npm = directory.path().join("fake-npm");
        std::fs::write(&npm, "#!/bin/sh\nprintf '\"2.4.1\"\\n'\n")?;
        std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o700))?;
        let invocation = NpmInvocation {
            program: npm,
            prefix_arguments: Vec::new(),
        };
        assert_eq!(
            latest_npm_package_version(
                &invocation,
                "@vtsls/language-server",
                &AtomicBool::new(false),
            )?,
            semver::Version::new(2, 4, 1)
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn builtin_npm_install_publishes_only_a_complete_cached_package()
    -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let cache = tempfile::tempdir()?;
        let npm = cache.path().join("fake-npm");
        std::fs::write(
            &npm,
            r#"#!/bin/sh
mkdir -p "$3/node_modules/basedpyright"
printf '%s' "$6" > "$3/../npm-cache-path"
printf '{"name":"basedpyright","version":"1.0.0"}' > "$3/node_modules/basedpyright/package.json"
: > "$3/node_modules/basedpyright/langserver.index.js"
"#,
        )?;
        std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o700))?;
        let builtin = builtin_language_server("python").expect("Python server");
        let cancelled = AtomicBool::new(false);
        install_builtin_npm_package(cache.path(), &npm, builtin, &cancelled)?;
        assert_eq!(
            std::fs::read_to_string(
                cache.path().join("language-servers/npm-cache-path")
            )?,
            cache
                .path()
                .join("language-servers/npm-cache")
                .to_string_lossy()
                .into_owned()
        );
        let cached =
            builtin.cached_npm_command(cache.path(), Path::new("/usr/bin/node"));
        assert!(cached.is_some());

        std::fs::write(
            &npm,
            r#"#!/bin/sh
mkdir -p "$3/node_modules/basedpyright"
printf '{}' > "$3/node_modules/basedpyright/package.json"
: > "$3/node_modules/basedpyright/langserver.index.js"
"#,
        )?;
        assert!(
            install_builtin_npm_package(cache.path(), &npm, builtin, &cancelled)
                .is_err()
        );
        assert!(
            builtin
                .cached_npm_command(cache.path(), Path::new("/usr/bin/node"))
                .is_some()
        );

        std::fs::write(
            &npm,
            "#!/bin/sh\nprintf '%s\\n' 'npm error code E401' >&2\nprintf '%s\\n' 'https://test-user:private-sentinel@example.invalid/' >&2\nexit 7\n",
        )?;
        let error =
            install_builtin_npm_package(cache.path(), &npm, builtin, &cancelled)
                .expect_err("failed npm install should include a safe error code")
                .to_string();
        assert!(error.contains("E401"), "diagnostic: {error}");
        assert!(!error.contains("test-user"), "diagnostic: {error}");
        assert!(!error.contains("private-sentinel"), "diagnostic: {error}");
        assert!(
            builtin
                .cached_npm_command(cache.path(), Path::new("/usr/bin/node"))
                .is_some()
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_builtin_npm_installs_serialize_cache_publication()
    -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Barrier;

        let cache = tempfile::tempdir()?;
        let npm = cache.path().join("fake-npm");
        std::fs::write(
            &npm,
            r#"#!/bin/sh
mkdir "$3/../busy" || exit 27
sleep 1
mkdir -p "$3/node_modules/basedpyright"
printf '{"name":"basedpyright","version":"1.0.0"}' > "$3/node_modules/basedpyright/package.json"
: > "$3/node_modules/basedpyright/langserver.index.js"
rmdir "$3/../busy"
"#,
        )?;
        std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o700))?;
        let start = Arc::new(Barrier::new(3));
        let workers = (0..2)
            .map(|_| {
                let cache = cache.path().to_path_buf();
                let npm = npm.clone();
                let start = start.clone();
                thread::spawn(move || {
                    start.wait();
                    install_builtin_npm_package(
                        &cache,
                        &npm,
                        builtin_language_server("python").expect("Python server"),
                        &AtomicBool::new(false),
                    )
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        for worker in workers {
            worker.join().expect("installer thread")?;
        }
        assert!(
            builtin_language_server("python")
                .expect("Python server")
                .cached_npm_command(cache.path(), Path::new("/usr/bin/node"))
                .is_some()
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_stops_an_active_builtin_npm_install() -> anyhow::Result<()> {
        use ahead_rpc::core::CoreRpcHandler;
        use std::os::unix::fs::PermissionsExt;

        let cache = tempfile::tempdir()?;
        let npm = cache.path().join("fake-npm");
        std::fs::write(&npm, "#!/bin/sh\n: > \"$3/../started\"\nexec sleep 10\n")?;
        std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o700))?;
        let builtin = builtin_language_server("python").expect("Python server");
        let mut catalog = PluginCatalog::new(
            None,
            PluginCatalogRpcHandler::new(CoreRpcHandler::new()),
        );
        let cancelled = catalog.cancel_installs.clone();
        let installing = catalog.installing_servers.clone();
        installing.lock().insert(builtin.command);
        let cache_path = cache.path().to_path_buf();
        let worker = thread::spawn(move || {
            let result =
                install_builtin_npm_package(&cache_path, &npm, builtin, &cancelled);
            installing.lock().remove(builtin.command);
            result
        });
        let started = cache.path().join("language-servers/started");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !started.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let was_running = started.exists();
        let shutdown_started = Instant::now();
        catalog.handle_notification(PluginCatalogNotification::Shutdown);
        let result = worker.join().expect("installer thread");
        assert!(was_running, "fake npm did not start");
        assert!(format!("{result:?}").contains("cancelled"));
        assert!(shutdown_started.elapsed() < Duration::from_secs(5));
        assert!(!cache.path().join("language-servers/basedpyright").exists());
        Ok(())
    }

    #[test]
    fn builtin_servers_use_only_complete_app_cached_packages() -> anyhow::Result<()>
    {
        let cache = tempfile::tempdir()?;
        let node = Path::new("/usr/bin/node");
        for (language, package_name, script) in [
            ("python", "basedpyright", "langserver.index.js"),
            ("typescript", "@vtsls/language-server", "bin/vtsls.js"),
        ] {
            let builtin =
                builtin_language_server(language).expect("built-in server");
            assert!(builtin.cached_npm_command(cache.path(), node).is_none());
            let package = cache
                .path()
                .join("language-servers")
                .join(if language == "python" {
                    "basedpyright"
                } else {
                    "vtsls"
                })
                .join("node_modules")
                .join(package_name);
            std::fs::create_dir_all(&package)?;
            std::fs::write(package.join("package.json"), "{}")?;
            assert!(builtin.cached_npm_command(cache.path(), node).is_none());
            let script = package.join(script);
            std::fs::create_dir_all(script.parent().expect("script directory"))?;
            std::fs::write(&script, "")?;
            assert!(builtin.cached_npm_command(cache.path(), node).is_none());
            std::fs::write(
                package.join("package.json"),
                format!(r#"{{"name":"{package_name}","version":"1.0.0"}}"#),
            )?;
            let command = builtin
                .cached_npm_command(cache.path(), node)
                .expect("complete cached package");
            assert_eq!(command.command, node.to_string_lossy().into_owned());
            assert_eq!(
                command.args,
                vec![script.to_string_lossy().into_owned(), "--stdio".into()]
            );
            assert_eq!(
                cached_npm_package_version(cache.path(), builtin),
                Some(semver::Version::new(1, 0, 0))
            );
        }
        Ok(())
    }

    #[test]
    #[ignore = "set AHEAD_ZED_EXTENSION_SOURCE and isolated temporary AHEAD_DATA_HOME/AHEAD_USER_HOME, and put the matching Zed HTML server on PATH"]
    fn installed_zed_html_extension_refreshes_settings_and_uses_a_real_server()
    -> anyhow::Result<()> {
        use ahead_rpc::core::{CoreNotification, CoreRpc, CoreRpcHandler};
        use anyhow::{Context as _, bail};

        let data_home = PathBuf::from(
            std::env::var_os("AHEAD_DATA_HOME")
                .context("set AHEAD_DATA_HOME to an isolated temporary directory")?,
        );
        let user_home = PathBuf::from(
            std::env::var_os("AHEAD_USER_HOME")
                .context("set AHEAD_USER_HOME to an isolated temporary directory")?,
        );
        let temporary_root = std::env::temp_dir().canonicalize()?;
        anyhow::ensure!(
            data_home.canonicalize()?.starts_with(&temporary_root),
            "AHEAD_DATA_HOME must be an existing temporary directory for this opt-in test"
        );
        anyhow::ensure!(
            user_home.canonicalize()?.starts_with(&temporary_root),
            "AHEAD_USER_HOME must be an existing temporary directory for this opt-in test"
        );
        let source = PathBuf::from(std::env::var("AHEAD_ZED_EXTENSION_SOURCE")?);
        let project = tempfile::tempdir()?;
        let extension_root = ahead_core::directory::Directory::plugins_directory()
            .context("isolated AHEAD plugin directory")?;
        anyhow::ensure!(
            extension_root.canonicalize()?.starts_with(&temporary_root),
            "AHEAD plugin directory must stay inside the temporary test root"
        );
        let installed = extension_root.join("html");
        if installed.exists() {
            std::fs::remove_dir_all(&installed)?;
        }
        std::fs::create_dir_all(&installed)?;
        let manifest = std::fs::read_to_string(source.join("extension.toml"))?;
        let server_section = "[language_servers.vscode-html-language-server]\n";
        let opt_in_manifest = manifest.replace(
            server_section,
            &format!("{server_section}opt_in_languages = [\"HTML\"]\n"),
        );
        anyhow::ensure!(
            opt_in_manifest != manifest,
            "installed HTML extension manifest has no expected server section"
        );
        std::fs::write(installed.join("extension.toml"), opt_in_manifest)?;
        std::fs::copy(
            source.join("extension.wasm"),
            installed.join("extension.wasm"),
        )?;
        let language_directory = installed.join("languages/html");
        std::fs::create_dir_all(&language_directory)?;
        std::fs::copy(
            source.join("languages/html/config.toml"),
            language_directory.join("config.toml"),
        )?;
        let extension = discover_language_server_extensions(&extension_root)?
            .extensions
            .into_iter()
            .find(|extension| extension.server_id == "vscode-html-language-server")
            .context("installed HTML extension has no language server")?;
        let workspace = project.path().join("workspace");
        std::fs::create_dir(&workspace)?;
        ahead_core::workspace_trust::set_trusted(&workspace, true)?;
        let ahead_config = workspace.join(".ahead");
        std::fs::create_dir(&ahead_config)?;
        std::fs::write(
            ahead_config.join("config.toml"),
            "[lsp.vscode-html-language-server.settings.html.validate]\nstyles = true\n",
        )?;
        let file = workspace.join("index.html");
        std::fs::write(&file, "<di")?;
        let language = ahead_extension_host::language_id_for_path(
            &extension_root,
            &file,
            Some("<di"),
        )?
        .context("installed extension did not detect HTML")?;
        assert_eq!(language, "HTML");
        let core = CoreRpcHandler::new();
        let rpc = PluginCatalogRpcHandler::new(core.clone());
        let mut catalog = PluginCatalog::new(Some(workspace.clone()), rpc.clone());
        let server_id = extension.server_id.clone();
        catalog.language_extensions = vec![extension];
        assert_eq!(
            catalog.language_server_names("HTML").2,
            [server_id.clone()],
            "installed extension manifest must mark its HTML server opt-in"
        );
        assert!(
            catalog
                .selected_servers("html")
                .expect("default routing")
                .is_empty(),
            "opt-in HTML server must stay dormant under default routing"
        );
        catalog.handle_did_open_text_document(TextDocumentItem::new(
            url::Url::from_file_path(&file)
                .map_err(|_| anyhow::anyhow!("invalid HTML file path"))?,
            language,
            1,
            "<di".into(),
        ));
        assert!(
            catalog
                .pending_extension_resolutions
                .get(&server_id)
                .is_none(),
            "opening HTML must not start an opt-in server by default"
        );
        let loop_rpc = rpc.clone();
        let loop_thread = std::thread::spawn(move || {
            loop_rpc.mainloop(&mut catalog);
        });
        std::fs::write(
            ahead_config.join("config.toml"),
            "[languages.HTML]\nlanguage_servers = ['vscode-html-language-server']\n[lsp.vscode-html-language-server.settings.html.validate]\nstyles = true\n",
        )?;
        rpc.catalog_notification(
            PluginCatalogNotification::RefreshWorkspaceConfigurations,
        )?;
        let outcome = (|| -> anyhow::Result<()> {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                let timeout =
                    deadline.saturating_duration_since(std::time::Instant::now());
                let CoreRpc::Notification(notification) =
                    core.rx().recv_timeout(timeout).context(
                        "waiting for explicitly opted-in HTML server startup",
                    )?
                else {
                    continue;
                };
                if let CoreNotification::ServerStatus { params } = &*notification
                    && params.server_name.as_deref()
                        == Some("vscode-html-language-server")
                {
                    if params.is_ok() {
                        let diagnostic_path = workspace.join("diagnostics.html");
                        let diagnostic_uri = url::Url::from_file_path(
                            &diagnostic_path,
                        )
                        .map_err(|_| anyhow::anyhow!("invalid diagnostics path"))?;
                        let invalid_css = "<style>body { colr: red; }</style>";
                        std::fs::write(&diagnostic_path, invalid_css)?;
                        rpc.did_open_document(
                            &diagnostic_path,
                            "html".into(),
                            1,
                            invalid_css.into(),
                        );
                        let diagnostic_deadline = std::time::Instant::now()
                            + std::time::Duration::from_secs(20);
                        loop {
                            let notification = match core
                                .rx()
                                .recv_timeout(
                                    diagnostic_deadline.saturating_duration_since(
                                        std::time::Instant::now(),
                                    ),
                                )
                                .context("waiting for embedded CSS diagnostics")?
                            {
                                CoreRpc::Notification(notification) => notification,
                                _ => continue,
                            };
                            let CoreNotification::PublishDiagnostics { diagnostics } =
                                &*notification
                            else {
                                continue;
                            };
                            if diagnostics.uri != diagnostic_uri {
                                continue;
                            }
                            if diagnostics.diagnostics.iter().any(|diagnostic| {
                                diagnostic.message.contains("colr")
                            }) {
                                break;
                            }
                        }

                        std::fs::write(
                            ahead_config.join("config.toml"),
                            "[languages.HTML]\nlanguage_servers = ['vscode-html-language-server']\n[lsp.vscode-html-language-server.settings.html.validate]\nstyles = false\n",
                        )?;
                        rpc.catalog_notification(
                            PluginCatalogNotification::RefreshWorkspaceConfigurations,
                        )?;
                        let mut server_restarted = false;
                        let refresh_deadline = std::time::Instant::now()
                            + std::time::Duration::from_secs(20);
                        loop {
                            let notification = match core
                                .rx()
                                .recv_timeout(
                                    refresh_deadline.saturating_duration_since(
                                        std::time::Instant::now(),
                                    ),
                                )
                                .context("waiting for refreshed HTML diagnostics")?
                            {
                                CoreRpc::Notification(notification) => notification,
                                _ => continue,
                            };
                            if let CoreNotification::ServerStatus { params } =
                                &*notification
                                && params.server_name.as_deref()
                                    == Some("vscode-html-language-server")
                            {
                                server_restarted = true;
                            }
                            let CoreNotification::PublishDiagnostics { diagnostics } =
                                &*notification
                            else {
                                continue;
                            };
                            if diagnostics.uri == diagnostic_uri
                                && diagnostics.diagnostics.is_empty()
                            {
                                break;
                            }
                        }
                        anyhow::ensure!(
                            !server_restarted,
                            "workspace settings change restarted the language server"
                        );

                        std::fs::write(
                            ahead_config.join("config.toml"),
                            "[languages.HTML]\nlanguage_servers = ['vscode-html-language-server']\n[lsp.vscode-html-language-server.initialization_options]\naheadSmokeMarker = 'restart'\n[lsp.vscode-html-language-server.settings.html.validate]\nstyles = false\n",
                        )?;
                        rpc.catalog_notification(
                            PluginCatalogNotification::RefreshWorkspaceConfigurations,
                        )?;
                        let restart_deadline = std::time::Instant::now()
                            + std::time::Duration::from_secs(20);
                        let mut last_restart_event = None;
                        loop {
                            let event = core
                                .rx()
                                .recv_timeout(
                                    restart_deadline.saturating_duration_since(
                                        std::time::Instant::now(),
                                    ),
                                )
                                .map_err(|error| {
                                    anyhow::anyhow!(
                                        "waiting for HTML language-server restart after initialization-options change; last event: {last_restart_event:?}: {error}"
                                    )
                                })?;
                            let CoreRpc::Notification(notification) = event else {
                                last_restart_event =
                                    Some("non-notification RPC event".into());
                                continue;
                            };
                            last_restart_event = Some(format!("{notification:?}"));
                            if let CoreNotification::ServerStatus { params } =
                                &*notification
                                && params.server_name.as_deref()
                                    == Some("vscode-html-language-server")
                            {
                                if params.is_ok() {
                                    break;
                                }
                                if let Some(message) = &params.message {
                                    bail!("restarted HTML server failed: {message}");
                                }
                            }
                            if let CoreNotification::ShowMessage { title, message } =
                                &*notification
                            {
                                bail!(
                                    "HTML server restart reported {title}: {}",
                                    message.message
                                );
                            }
                        }
                        let request_id = 1;
                        let completion_deadline = std::time::Instant::now()
                            + std::time::Duration::from_secs(20);
                        rpc.completion(
                            request_id,
                            &file,
                            "<di".into(),
                            lsp_types::Position::new(0, 3),
                        );
                        loop {
                            let notification = match core.rx().recv_timeout(
                                completion_deadline.saturating_duration_since(
                                    std::time::Instant::now(),
                                ),
                            )? {
                                CoreRpc::Notification(notification) => notification,
                                _ => continue,
                            };
                            let CoreNotification::CompletionResponse {
                                request_id: received_id,
                                input,
                                resp,
                                ..
                            } = &*notification
                            else {
                                continue;
                            };
                            if *received_id != request_id {
                                continue;
                            }
                            anyhow::ensure!(
                                input == "<di",
                                "completion input changed"
                            );
                            let Some(resp) = resp else {
                                anyhow::bail!(
                                    "language server returned null completions"
                                );
                            };
                            let item_summary = match resp {
                                lsp_types::CompletionResponse::Array(items) => items
                                    .iter()
                                    .take(8)
                                    .map(|item| {
                                        (&item.label, item.insert_text_format)
                                    })
                                    .collect::<Vec<_>>(),
                                lsp_types::CompletionResponse::List(list) => list
                                    .items
                                    .iter()
                                    .take(8)
                                    .map(|item| {
                                        (&item.label, item.insert_text_format)
                                    })
                                    .collect::<Vec<_>>(),
                            };
                            let has_html_completion = match resp {
                                lsp_types::CompletionResponse::Array(items) => {
                                    items.iter().any(|item| {
                                        item.label.eq_ignore_ascii_case("html")
                                    })
                                }
                                lsp_types::CompletionResponse::List(list) => {
                                    list.items.iter().any(|item| {
                                        item.label.eq_ignore_ascii_case("html")
                                    })
                                }
                            };
                            anyhow::ensure!(
                                has_html_completion,
                                "installed HTML server returned no html tag completion for <di: {item_summary:?}"
                            );

                            let formatting_path = workspace.join("format.html");
                            let unformatted =
                                "<div><span>Hello</span><p>World</p></div>";
                            std::fs::write(&formatting_path, unformatted)?;
                            rpc.did_open_document(
                                &formatting_path,
                                "html".to_string(),
                                1,
                                unformatted.to_string(),
                            );
                            let (format_tx, format_rx) =
                                crossbeam_channel::bounded(1);
                            rpc.get_document_formatting(
                                &formatting_path,
                                move |_, result| {
                                    if let Err(error) = format_tx.send(result) {
                                        tracing::warn!(
                                            ?error,
                                            "HTML formatting smoke receiver closed"
                                        );
                                    }
                                },
                            );
                            let edits = format_rx
                                .recv_timeout(std::time::Duration::from_secs(20))?
                                .map_err(|error| anyhow::anyhow!(error.message))?;
                            anyhow::ensure!(
                                edits
                                    .iter()
                                    .any(|edit| edit.new_text.contains('\n')),
                                "HTML server did not return multiline formatting edits"
                            );

                            let (rename_tx, rename_rx) =
                                crossbeam_channel::bounded(1);
                            rpc.rename(
                                &formatting_path,
                                lsp_types::Position::new(0, 7),
                                "strong".into(),
                                move |_, result| {
                                    if let Err(error) = rename_tx.send(result) {
                                        tracing::warn!(
                                            ?error,
                                            "HTML rename smoke receiver closed"
                                        );
                                    }
                                },
                            );
                            let renamed = rename_rx
                                .recv_timeout(std::time::Duration::from_secs(20))?
                                .map_err(|error| anyhow::anyhow!(error.message))?;
                            let formatting_uri =
                                url::Url::from_file_path(&formatting_path).map_err(
                                    |_| anyhow::anyhow!("invalid formatting path"),
                                )?;
                            let rename_edits = renamed
                                .changes
                                .and_then(|changes| {
                                    changes.get(&formatting_uri).cloned()
                                })
                                .context("HTML rename returned no edits")?;
                            anyhow::ensure!(
                                rename_edits
                                    .iter()
                                    .filter(|edit| edit.new_text == "strong")
                                    .count()
                                    == 2,
                                "HTML rename did not update both paired tag names: {rename_edits:?}"
                            );
                            return Ok(());
                        }
                    }
                    if let Some(message) = &params.message {
                        bail!("installed HTML server failed: {message}");
                    }
                }
            }
        })();
        rpc.shutdown();
        loop_thread.join().expect("catalog loop");
        outcome
    }

    #[test]
    fn completion_without_a_running_server_finishes_with_an_empty_list() {
        use ahead_rpc::core::{CoreNotification, CoreRpc, CoreRpcHandler};
        let core = CoreRpcHandler::new();
        let rpc = PluginCatalogRpcHandler::new(core.clone());
        let mut catalog = PluginCatalog::new(None, rpc.clone());
        discard_initial_extension_issues(&core);
        let path = std::env::temp_dir().join("ahead-completion-test.ts");
        rpc.completion(3, &path, String::new(), lsp_types::Position::default());
        rpc.shutdown();
        rpc.mainloop(&mut catalog);
        let CoreRpc::Notification(notification) =
            core.rx().try_recv().expect("completion finished")
        else {
            panic!("notification");
        };
        let CoreNotification::CompletionResponse {
            request_id, resp, ..
        } = *notification
        else {
            panic!("completion");
        };
        assert_eq!(request_id, 3);
        assert!(
            matches!(resp, Some(lsp_types::CompletionResponse::Array(items)) if items.is_empty())
        );
    }

    #[test]
    fn closing_a_document_clears_its_diagnostics_and_invalidates_pending_pulls() {
        use ahead_rpc::core::{CoreNotification, CoreRpc, CoreRpcHandler};
        let core_rpc = CoreRpcHandler::new();
        let mut catalog =
            PluginCatalog::new(None, PluginCatalogRpcHandler::new(core_rpc.clone()));
        discard_initial_extension_issues(&core_rpc);
        let uri = url::Url::parse("file:///fixture/main.ts").expect("URI");
        let path = uri.to_file_path().expect("path");
        let generation = Arc::new(AtomicU64::new(7));
        catalog.open_files.insert(
            path.clone(),
            OpenDocument {
                uri: uri.clone(),
                language_id: "typescript".into(),
                version: 7,
                text: Rope::from_str("const value = 1;"),
                generation: generation.clone(),
            },
        );
        catalog.diagnostics.insert(
            uri.clone(),
            HashMap::from([(
                (PluginId(1), DiagnosticSource::Pulled),
                vec![Diagnostic::default()],
            )]),
        );
        catalog.handle_did_close_text_document(path.clone());
        assert!(!catalog.open_files.contains_key(&path));
        assert_eq!(generation.load(Ordering::Relaxed), 8);
        assert!(!catalog.diagnostics.contains_key(&uri));
        let CoreRpc::Notification(notification) =
            core_rpc.rx().try_recv().expect("diagnostic update")
        else {
            panic!("expected notification");
        };
        let CoreNotification::PublishDiagnostics { diagnostics } = *notification
        else {
            panic!("expected diagnostics");
        };
        assert_eq!(diagnostics.uri, uri);
        assert!(diagnostics.diagnostics.is_empty());
        catalog.handle_did_close_text_document(path);
        assert!(core_rpc.rx().try_recv().is_err(), "close is idempotent");
    }

    #[test]
    fn delayed_extension_replays_the_latest_unsaved_document() {
        let core_rpc = ahead_rpc::core::CoreRpcHandler::new();
        let mut catalog =
            PluginCatalog::new(None, PluginCatalogRpcHandler::new(core_rpc));
        let uri = url::Url::parse("file:///fixture/main.ts").expect("URI");
        catalog.handle_did_open_text_document(TextDocumentItem::new(
            uri.clone(),
            "typescript".into(),
            1,
            "const value = 1;".into(),
        ));
        let edited = Rope::from_str("const value = 2; // unsaved");
        catalog.handle_did_change_text_document(
            "typescript".into(),
            VersionedTextDocumentIdentifier::new(uri.clone(), 2),
            AheadDelta::empty(),
            Rope::from_str("const value = 1;"),
            edited,
        );
        let document = catalog
            .open_files
            .get(&uri.to_file_path().expect("path"))
            .expect("open document")
            .latest_item();
        assert_eq!(document.version, 2);
        assert_eq!(document.text, "const value = 2; // unsaved");
    }

    #[test]
    fn closed_document_ignores_a_late_extension_resolution() -> anyhow::Result<()> {
        let temporary = tempfile::tempdir()?;
        let extension_dir = temporary.path().join("extensions/html");
        std::fs::create_dir_all(&extension_dir)?;
        std::fs::write(
            extension_dir.join("extension.toml"),
            "id = 'html'\n[lib]\nversion = '0.7.0'\n[language_servers.html]\nlanguages = ['HTML']\n",
        )?;
        std::fs::write(
            extension_dir.join("extension.wasm"),
            test_extension_component(),
        )?;
        let extension = discover_language_server_extensions(
            &temporary.path().join("extensions"),
        )?
        .extensions
        .into_iter()
        .next()
        .expect("discover test extension");
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace)?;
        let path = workspace.join("index.html");
        let uri = url::Url::from_file_path(&path)
            .map_err(|_| anyhow::anyhow!("invalid test path"))?;
        let core_rpc = ahead_rpc::core::CoreRpcHandler::new();
        let rpc = PluginCatalogRpcHandler::new(core_rpc);
        let mut catalog = PluginCatalog::new(Some(workspace.clone()), rpc.clone());
        catalog.language_extensions = vec![extension.clone()];
        catalog.open_files.insert(
            path.clone(),
            OpenDocument {
                uri,
                language_id: "HTML".into(),
                version: 1,
                text: Rope::from_str("<div>"),
                generation: Arc::new(AtomicU64::new(0)),
            },
        );
        let server_id = extension.server_id.clone();
        catalog.start_extension_resolution("HTML", extension, workspace);
        assert!(
            catalog
                .pending_extension_resolutions
                .contains_key(&server_id)
        );
        let crate::plugin::PluginCatalogRpc::Handler(notification) =
            rpc.test_receiver().recv_timeout(Duration::from_secs(5))?
        else {
            anyhow::bail!("expected extension resolution");
        };
        catalog.handle_did_close_text_document(path);
        catalog.handle_notification(notification);
        assert!(!catalog.lsp_servers.contains_key("HTML"));
        assert!(
            !catalog
                .pending_extension_resolutions
                .contains_key(&server_id)
        );
        Ok(())
    }

    #[test]
    fn complementary_extensions_resolve_independently_for_one_language()
    -> anyhow::Result<()> {
        let temporary = tempfile::tempdir()?;
        let extension_root = temporary.path().join("extensions");
        let extension_dir = extension_root.join("html");
        std::fs::create_dir_all(&extension_dir)?;
        std::fs::write(
            extension_dir.join("extension.toml"),
            "id = 'html'\n[lib]\nversion = '0.7.0'\n[language_servers.html-analyzer]\nlanguages = ['HTML']\n[language_servers.html-formatter]\nlanguages = ['HTML']\n",
        )?;
        std::fs::write(
            extension_dir.join("extension.wasm"),
            test_extension_component(),
        )?;
        let extensions =
            discover_language_server_extensions(&extension_root)?.extensions;
        assert_eq!(extensions.len(), 2);
        let rpc =
            PluginCatalogRpcHandler::new(ahead_rpc::core::CoreRpcHandler::new());
        let mut catalog =
            PluginCatalog::new(Some(temporary.path().into()), rpc.clone());
        catalog.language_extensions = extensions.clone();
        for extension in extensions {
            catalog.start_extension_resolution(
                "HTML",
                extension,
                temporary.path().into(),
            );
        }
        assert_eq!(catalog.pending_extension_resolutions.len(), 2);
        let mut resolved = HashSet::new();
        for _ in 0..2 {
            let super::super::PluginCatalogRpc::Handler(notification) =
                rpc.test_receiver().recv_timeout(Duration::from_secs(5))?
            else {
                anyhow::bail!("expected extension resolution");
            };
            if let PluginCatalogNotification::LanguageServerResolved {
                extension,
                ..
            } = &notification
            {
                resolved.insert(extension.server_id.clone());
            }
            catalog.handle_notification(notification);
        }
        assert_eq!(resolved.len(), 2);
        assert!(catalog.pending_extension_resolutions.is_empty());
        Ok(())
    }

    #[test]
    fn diagnostic_updates_preserve_other_sources_and_servers() {
        use ahead_rpc::core::{CoreNotification, CoreRpc, CoreRpcHandler};

        let core_rpc = CoreRpcHandler::new();
        let mut catalog =
            PluginCatalog::new(None, PluginCatalogRpcHandler::new(core_rpc.clone()));
        discard_initial_extension_issues(&core_rpc);
        let uri = url::Url::parse("file:///fixture/main.rs").expect("file URL");
        for id in [PluginId(1), PluginId(2)] {
            let (sender, _) = crossbeam_channel::unbounded();
            catalog.plugins.insert(
                id,
                PluginServerRpcHandler::new(
                    ServerId {
                        author: "ahead".into(),
                        name: format!("test-{}", id.0),
                    },
                    Some(id),
                    sender,
                ),
            );
        }
        for (plugin_id, source, message, expected) in [
            (
                1,
                DiagnosticSource::Pushed,
                Some("compiler"),
                vec!["compiler"],
            ),
            (1, DiagnosticSource::Pulled, None, vec!["compiler"]),
            (
                1,
                DiagnosticSource::Pulled,
                Some("syntax"),
                vec!["compiler", "syntax"],
            ),
            (
                2,
                DiagnosticSource::Pushed,
                Some("lint"),
                vec!["compiler", "lint", "syntax"],
            ),
            (1, DiagnosticSource::Pushed, None, vec!["lint", "syntax"]),
            (1, DiagnosticSource::Pulled, None, vec!["lint"]),
            (2, DiagnosticSource::Pushed, None, vec![]),
        ] {
            catalog.publish_diagnostics(
                PluginId(plugin_id),
                source,
                PublishDiagnosticsParams {
                    uri: uri.clone(),
                    diagnostics: message
                        .into_iter()
                        .map(|message| Diagnostic {
                            message: message.to_string(),
                            ..Diagnostic::default()
                        })
                        .collect(),
                    version: Some(1),
                },
            );
            let CoreRpc::Notification(notification) =
                core_rpc.rx().try_recv().expect("diagnostic update")
            else {
                panic!("expected notification");
            };
            let CoreNotification::PublishDiagnostics { diagnostics } = *notification
            else {
                panic!("expected merged diagnostics");
            };
            assert_eq!(diagnostics.uri, uri);
            assert_eq!(diagnostics.version, None);
            assert_eq!(
                diagnostics
                    .diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
        }
        assert!(catalog.diagnostics.is_empty());
        for id in [1, 2] {
            catalog.publish_diagnostics(
                PluginId(id),
                DiagnosticSource::Pushed,
                PublishDiagnosticsParams::new(
                    uri.clone(),
                    vec![Diagnostic {
                        message: format!("server {id}"),
                        ..Diagnostic::default()
                    }],
                    None,
                ),
            );
            core_rpc.rx().try_recv().expect("diagnostics");
        }
        catalog.remove_language_server(PluginId(1));
        let CoreRpc::Notification(notification) =
            core_rpc.rx().try_recv().expect("clear stopped server")
        else {
            panic!("notification");
        };
        let CoreNotification::PublishDiagnostics { diagnostics } = *notification
        else {
            panic!("diagnostics");
        };
        assert_eq!(diagnostics.diagnostics.len(), 1);
        assert_eq!(diagnostics.diagnostics[0].message, "server 2");
        catalog.publish_diagnostics(
            PluginId(1),
            DiagnosticSource::Pushed,
            PublishDiagnosticsParams::new(uri, vec![Diagnostic::default()], None),
        );
        catalog.handle_notification(
            PluginCatalogNotification::LanguageServerStatus {
                plugin_id: PluginId(1),
                params: ahead_rpc::core::ServerStatusParams::ready("stale".into()),
            },
        );
        catalog.handle_notification(
            PluginCatalogNotification::LanguageServerStopped {
                plugin_id: PluginId(1),
                message: "stale exit".into(),
            },
        );
        assert!(
            core_rpc.rx().try_recv().is_err(),
            "retired process must not replace live diagnostics or status"
        );
    }

    #[test]
    fn stopping_one_server_preserves_the_other_servers_for_its_languages() {
        let core = ahead_rpc::core::CoreRpcHandler::new();
        let mut catalog =
            PluginCatalog::new(None, PluginCatalogRpcHandler::new(core));
        for (id, name) in [
            (PluginId(1), "basedpyright-langserver"),
            (PluginId(2), "ruff"),
        ] {
            let (sender, _) = crossbeam_channel::unbounded();
            catalog.plugins.insert(
                id,
                PluginServerRpcHandler::new(
                    ServerId {
                        author: "ahead".into(),
                        name: format!("lsp-{name}"),
                    },
                    Some(id),
                    sender,
                ),
            );
        }
        for language in ["python", "Python"] {
            catalog
                .lsp_servers
                .insert(language.into(), vec![PluginId(1), PluginId(2)]);
            assert!(catalog.has_lsp_server(language, "ruff"));
            assert!(catalog.has_lsp_server(language, "basedpyright-langserver"));
        }
        catalog.remove_language_server(PluginId(2));
        for language in ["python", "Python"] {
            assert!(!catalog.has_lsp_server(language, "ruff"));
            assert!(catalog.has_lsp_server(language, "basedpyright-langserver"));
            assert_eq!(catalog.lsp_servers[language], [PluginId(1)]);
        }
        catalog.remove_language_server(PluginId(1));
        assert!(catalog.lsp_servers.is_empty());
    }

    #[test]
    fn extracts_full_report_and_ignores_unchanged() {
        let full: DocumentDiagnosticReportResult = serde_json::from_value(serde_json::json!({
            "kind": "full",
            "items": [{
                "range": {"start": {"line": 4, "character": 7}, "end": {"line": 4, "character": 7}},
                "message": "Syntax Error: expected pattern"
            }]
        }))
        .expect("valid full diagnostic report");
        assert_eq!(full_document_diagnostics(full).unwrap().len(), 1);
        let unchanged: DocumentDiagnosticReportResult =
            serde_json::from_value(serde_json::json!({
                "kind": "unchanged", "resultId": "rust-analyzer"
            }))
            .expect("valid unchanged diagnostic report");
        assert!(full_document_diagnostics(unchanged).is_none());
    }

    #[test]
    fn cancelled_document_diagnostics_retry_and_publish_the_full_report() {
        let plugin_id = PluginId(42);
        let (io_sender, _) = crossbeam_channel::unbounded();
        let plugin = PluginServerRpcHandler::new(
            ServerId {
                author: "ahead".into(),
                name: "diagnostic-retry-test".into(),
            },
            Some(plugin_id),
            io_sender,
        );
        let server_requests = plugin.test_receiver();
        assert!(matches!(
            server_requests.try_recv(),
            Ok(PluginServerRpc::Handler(_))
        ));

        let core_rpc = ahead_rpc::core::CoreRpcHandler::new();
        let catalog_rpc = PluginCatalogRpcHandler::new(core_rpc);
        let notifications = catalog_rpc.test_receiver();
        let uri = url::Url::parse("file:///fixture/main.rs").expect("file URI");
        let generation = Arc::new(AtomicU64::new(7));
        request_document_diagnostics(
            plugin,
            plugin_id,
            PathBuf::from("/fixture/main.rs"),
            uri.clone(),
            "rust".into(),
            catalog_rpc,
            7,
            generation,
            0,
        );

        let PluginServerRpc::ServerRequest {
            method,
            rh: cancelled_response,
            ..
        } = server_requests
            .recv_timeout(Duration::from_secs(1))
            .expect("initial diagnostics request")
        else {
            panic!("expected a server request");
        };
        assert_eq!(method, DocumentDiagnosticRequest::METHOD);
        cancelled_response.invoke(Err(RpcError {
            code: lsp_types::error_codes::SERVER_CANCELLED,
            message: "diagnostic refresh superseded the pull".into(),
        }));

        let PluginServerRpc::ServerRequest {
            method,
            rh: successful_response,
            ..
        } = server_requests
            .recv_timeout(Duration::from_secs(1))
            .expect("retry diagnostics request")
        else {
            panic!("expected a retry server request");
        };
        assert_eq!(method, DocumentDiagnosticRequest::METHOD);
        successful_response.invoke(Ok(serde_json::json!({
            "kind": "full",
            "items": []
        })));

        let PluginCatalogRpc::PublishDiagnostics {
            plugin_id: published_plugin,
            source,
            diagnostics,
        } = notifications
            .recv_timeout(Duration::from_secs(1))
            .expect("successful retry publishes diagnostics")
        else {
            panic!("expected published diagnostics");
        };
        assert_eq!(published_plugin, plugin_id);
        assert_eq!(source, DiagnosticSource::Pulled);
        assert_eq!(diagnostics.uri, uri);
        assert!(diagnostics.diagnostics.is_empty());
    }

    #[test]
    fn cancelled_document_diagnostics_do_not_retry_after_document_changes() {
        let plugin_id = PluginId(43);
        let (io_sender, _) = crossbeam_channel::unbounded();
        let plugin = PluginServerRpcHandler::new(
            ServerId {
                author: "ahead".into(),
                name: "stale-diagnostic-retry-test".into(),
            },
            Some(plugin_id),
            io_sender,
        );
        let server_requests = plugin.test_receiver();
        assert!(matches!(
            server_requests.try_recv(),
            Ok(PluginServerRpc::Handler(_))
        ));

        let core_rpc = ahead_rpc::core::CoreRpcHandler::new();
        let catalog_rpc = PluginCatalogRpcHandler::new(core_rpc);
        let notifications = catalog_rpc.test_receiver();
        let generation = Arc::new(AtomicU64::new(7));
        request_document_diagnostics(
            plugin,
            plugin_id,
            PathBuf::from("/fixture/main.rs"),
            url::Url::parse("file:///fixture/main.rs").expect("file URI"),
            "rust".into(),
            catalog_rpc,
            7,
            generation.clone(),
            0,
        );

        let PluginServerRpc::ServerRequest {
            rh: cancelled_response,
            ..
        } = server_requests
            .recv_timeout(Duration::from_secs(1))
            .expect("initial diagnostics request")
        else {
            panic!("expected a server request");
        };
        generation.store(8, Ordering::Relaxed);
        cancelled_response.invoke(Err(RpcError {
            code: lsp_types::error_codes::SERVER_CANCELLED,
            message: "document changed".into(),
        }));

        assert!(
            server_requests.try_recv().is_err(),
            "stale pull must not retry"
        );
        assert!(
            notifications.try_recv().is_err(),
            "stale pull must not publish"
        );
    }
}

/// Builds the debug-adapter command from the user's run config, falling back
/// to the workspace when no working directory is configured.
fn resolve_dap_server(
    config: &RunDebugConfig,
    workspace: Option<PathBuf>,
) -> DapServer {
    let (program, args) = match config.debug_adapter.as_ref() {
        Some(program) => (
            program.clone(),
            config.debug_adapter_args.clone().unwrap_or_default(),
        ),
        None => (
            config.program.clone(),
            config.args.clone().unwrap_or_default(),
        ),
    };
    DapServer {
        program,
        args,
        cwd: config.cwd.clone().map(PathBuf::from).or(workspace),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_server_restarts_only_when_launch_configuration_changes() {
        let original = LanguageServerCommand {
            command: "server".into(),
            args: vec!["--stdio".into()],
            env: vec![("MODE".into(), "one".into())],
            initialization_options: Some(serde_json::json!({"init": 1})),
            initialization_options_schema: None,
            workspace_configuration: Some(serde_json::json!({"setting": 1})),
            workspace_configuration_schema: None,
        };
        let mut changed = original.clone();
        changed.workspace_configuration = Some(serde_json::json!({"setting": 2}));
        assert!(!language_server_command_requires_restart(
            &original, &changed
        ));

        changed = original.clone();
        changed.command = "other-server".into();
        assert!(language_server_command_requires_restart(
            &original, &changed
        ));
        changed = original.clone();
        changed.args.push("--verbose".into());
        assert!(language_server_command_requires_restart(
            &original, &changed
        ));
        changed = original.clone();
        changed.env[0].1 = "two".into();
        assert!(language_server_command_requires_restart(
            &original, &changed
        ));
        changed = original.clone();
        changed.initialization_options = Some(serde_json::json!({"init": 2}));
        assert!(language_server_command_requires_restart(
            &original, &changed
        ));
    }

    #[test]
    fn builtin_servers_cover_rust_python_and_the_shared_typescript_family() {
        assert_eq!(
            builtin_language_server("Rust").expect("Rust").command,
            "rust-analyzer"
        );
        let python = builtin_language_server("Python").expect("Python");
        assert_eq!(python.command, "basedpyright-langserver");
        assert_eq!(python.args, ["--stdio"]);
        for language in [
            "JavaScript",
            "TypeScript",
            "JSX",
            "TSX",
            "typescriptreact",
            "javascriptreact",
        ] {
            let server =
                builtin_language_server(language).expect("TypeScript family");
            assert_eq!(server.command, "vtsls");
            assert_eq!(server.args, ["--stdio"]);
            assert_eq!(
                server.languages,
                [
                    "javascript",
                    "javascriptreact",
                    "typescript",
                    "typescriptreact",
                    "JavaScript",
                    "JSX",
                    "TypeScript",
                    "TSX"
                ]
            );
            assert!(server.languages.contains(
                &builtin_protocol_language_id(language).expect("protocol ID")
            ));
        }
        assert!(builtin_language_server("Gleam").is_none());
        let python_servers = builtin_language_servers("Python");
        assert_eq!(python_servers.len(), 2);
        assert_eq!(python_servers[0].command, "basedpyright-langserver");
        assert_eq!(python_servers[1].command, "ruff");
        assert_eq!(python_servers[1].args, ["server"]);
    }

    #[test]
    fn rust_analyzer_release_asset_matches_the_current_target() {
        let (asset, is_zip) =
            rust_analyzer_release_asset_name().expect("supported host target");
        assert!(
            asset.starts_with(&format!("rust-analyzer-{}-", std::env::consts::ARCH))
        );
        #[cfg(target_os = "windows")]
        assert!(is_zip && asset.ends_with(".zip"));
        #[cfg(not(target_os = "windows"))]
        assert!(!is_zip && asset.ends_with(".gz"));
    }

    #[test]
    fn ruff_release_asset_matches_the_current_target() {
        let (asset, is_zip) =
            ruff_release_asset_name().expect("supported host target");
        let architecture = match std::env::consts::ARCH {
            "x86" => "i686",
            architecture => architecture,
        };
        assert!(asset.starts_with(&format!("ruff-{architecture}-")));
        assert_eq!(is_zip, cfg!(windows));
        assert!(asset.ends_with(if cfg!(windows) { ".zip" } else { ".tar.gz" }));
    }

    #[test]
    fn ruff_archive_extracts_only_its_regular_binary() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let binary = directory.path().join("ruff");
        let asset = "ruff-aarch64-apple-darwin.tar.gz";
        let make_archive =
            |member: &str, symlink: bool| -> anyhow::Result<Vec<u8>> {
                let encoder = flate2::write::GzEncoder::new(
                    Vec::new(),
                    flate2::Compression::default(),
                );
                let mut archive = tar::Builder::new(encoder);
                let mut header = tar::Header::new_gnu();
                header.set_size(if symlink { 0 } else { 6 });
                header.set_mode(0o755);
                if symlink {
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_link_name("/outside/ruff")?;
                }
                header.set_cksum();
                archive.append_data(
                    &mut header,
                    member,
                    if symlink { &b""[..] } else { &b"binary"[..] },
                )?;
                Ok(archive.into_inner()?.finish()?)
            };
        for (member, symlink) in [
            ("other/ruff", false),
            ("ruff-aarch64-apple-darwin/ruff", true),
        ] {
            assert!(
                extract_ruff_binary(&make_archive(member, symlink)?, asset, &binary)
                    .is_err()
            );
            assert!(!binary.exists());
        }
        let archive = make_archive("ruff-aarch64-apple-darwin/ruff", false)?;
        extract_ruff_binary(&archive, asset, &binary)?;
        assert_eq!(std::fs::read(&binary)?, b"binary");
        assert!(
            extract_ruff_binary(&archive, asset, &binary).is_err(),
            "existing files must not be overwritten"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn ruff_resolution_prefers_the_worktree_virtualenv_and_honors_cancellation()
    -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir()?;
        let binary = directory.path().join(".venv/bin/ruff");
        std::fs::create_dir_all(binary.parent().context("binary parent")?)?;
        std::fs::write(&binary, "#!/bin/sh\n[ \"$1\" = --version ]\n")?;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))?;
        let command = resolve_ruff_command(
            &directory.path().join("cache"),
            directory.path(),
            &AtomicBool::new(false),
        )?;
        assert_eq!(command.command, binary.to_string_lossy());
        assert_eq!(command.args, ["server"]);
        assert!(!directory.path().join("cache").exists());
        assert!(
            resolve_ruff_command(
                &directory.path().join("cache"),
                directory.path(),
                &AtomicBool::new(true)
            )
            .is_err()
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rust_analyzer_probe_rejects_a_broken_path_shim() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir()?;
        let working = directory.path().join("working-server");
        std::fs::write(&working, "#!/bin/sh\nexit 0\n")?;
        std::fs::set_permissions(&working, std::fs::Permissions::from_mode(0o700))?;
        let broken = directory.path().join("broken-rustup-shim");
        std::fs::write(&broken, "#!/bin/sh\nexit 1\n")?;
        std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o700))?;
        let cancelled = AtomicBool::new(false);

        assert!(native_server_accepts(&working, "--help", &cancelled)?);
        assert!(!native_server_accepts(&broken, "--help", &cancelled)?);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rust_analyzer_uses_rustup_override() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir()?;
        let workspace = directory.path().join("workspace");
        std::fs::create_dir_all(&workspace)?;
        let workspace = std::fs::canonicalize(workspace)?;
        let rustup = directory.path().join("rustup");
        std::fs::write(
            &rustup,
            "#!/bin/sh\n[ \"$1\" = which ] && [ \"$2\" = rust-analyzer ] || exit 2\nprintf '%s\\n' \"$(pwd)/rust-analyzer\"\n",
        )?;
        std::fs::set_permissions(&rustup, std::fs::Permissions::from_mode(0o700))?;
        let cancelled = AtomicBool::new(false);

        assert!(!workspace_has_rust_toolchain_override(&workspace));

        let selected_binary = workspace.join("rust-analyzer");
        for toolchain_file in ["rust-toolchain.toml", "rust-toolchain"] {
            let contents = if toolchain_file.ends_with(".toml") {
                "channel = 'stable'\n"
            } else {
                "stable\n"
            };
            std::fs::write(workspace.join(toolchain_file), contents)?;
            assert!(workspace_has_rust_toolchain_override(&workspace));
            assert_eq!(
                rustup_rust_analyzer_for_workspace(&workspace, &rustup, &cancelled)?,
                Some(selected_binary.clone())
            );
            std::fs::remove_file(workspace.join(toolchain_file))?;
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rustup_rust_analyzer_resolution_honors_cancellation() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir()?;
        let workspace = directory.path().join("workspace");
        std::fs::create_dir_all(&workspace)?;
        let rustup = directory.path().join("rustup");
        std::fs::write(&rustup, "#!/bin/sh\nwhile :; do :; done\n")?;
        std::fs::set_permissions(&rustup, std::fs::Permissions::from_mode(0o700))?;
        let cancelled = AtomicBool::new(false);

        let result = thread::scope(|scope| {
            let cancellation_signal = scope.spawn(|| {
                thread::sleep(Duration::from_millis(50));
                cancelled.store(true, Ordering::Relaxed);
            });
            let result =
                rustup_rust_analyzer_for_workspace(&workspace, &rustup, &cancelled);
            cancellation_signal
                .join()
                .expect("cancellation signal should complete");
            result
        });
        let error = result.expect_err("cancellation must stop rustup resolution");
        assert!(error.to_string().contains("cancelled"));
        Ok(())
    }

    fn test_config() -> RunDebugConfig {
        RunDebugConfig {
            ty: Some("lldb".to_string()),
            debug_adapter: None,
            debug_adapter_args: None,
            name: "Run".to_string(),
            program: "/usr/bin/codelldb".to_string(),
            args: Some(vec!["--port".to_string()]),
            cwd: Some("/work".to_string()),
            env: None,
            prelaunch: None,
            dap_id: DapId(0),
            tracing_output: false,
            config_source: Default::default(),
        }
    }

    #[test]
    fn resolve_dap_server_prefers_config_values() {
        let server = resolve_dap_server(&test_config(), Some(PathBuf::from("/ws")));
        assert_eq!(server.program, "/usr/bin/codelldb");
        assert_eq!(server.args, vec!["--port".to_string()]);
        assert_eq!(server.cwd, Some(PathBuf::from("/work")));
    }

    #[test]
    fn resolve_dap_server_keeps_target_args_out_of_adapter_command() {
        let mut config = test_config();
        config.debug_adapter = Some("lldb-dap".to_string());
        config.debug_adapter_args = Some(vec!["--stdio".to_string()]);
        config.args = Some(vec!["target-argument".to_string()]);

        let server = resolve_dap_server(&config, None);
        assert_eq!(server.program, "lldb-dap");
        assert_eq!(server.args, vec!["--stdio".to_string()]);
    }

    #[test]
    fn resolve_dap_server_falls_back_to_workspace() {
        let mut config = test_config();
        config.cwd = None;
        config.args = None;
        let server = resolve_dap_server(&config, Some(PathBuf::from("/ws")));
        assert_eq!(server.args, Vec::<String>::new());
        assert_eq!(server.cwd, Some(PathBuf::from("/ws")));
    }

    #[test]
    fn debugger_disconnect_retains_ownership_until_cleanup_then_prunes_the_session()
    {
        let plugin_rpc = super::PluginCatalogRpcHandler::new(
            ahead_rpc::core::CoreRpcHandler::new(),
        );
        let mut catalog = super::PluginCatalog::new(None, plugin_rpc.clone());
        let config = test_config();
        let client = super::DapClient::new(
            resolve_dap_server(&config, None),
            config.clone(),
            std::collections::HashMap::new(),
            plugin_rpc,
        )
        .expect("client");
        let rpc = client.dap_rpc.clone();
        catalog.daps.insert(config.dap_id, rpc.clone());
        catalog.handle_notification(
            super::PluginCatalogNotification::DapDisconnect {
                dap_id: config.dap_id,
            },
        );
        assert!(catalog.daps.contains_key(&config.dap_id));
        assert!(rpc.wait_for_shutdown_until(
            std::time::Instant::now() + std::time::Duration::from_secs(3)
        ));
        let mut next_config = config.clone();
        next_config.dap_id = DapId(4312);
        next_config.debug_adapter = Some("ahead-missing-debug-adapter-test".into());
        catalog.handle_notification(super::PluginCatalogNotification::DapStart {
            config: next_config.clone(),
            breakpoints: std::collections::HashMap::new(),
        });
        assert!(!catalog.daps.contains_key(&config.dap_id));
        assert!(catalog.daps.contains_key(&next_config.dap_id));
        catalog.handle_notification(super::PluginCatalogNotification::Shutdown);
        drop(client);
    }
}
