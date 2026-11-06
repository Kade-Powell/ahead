use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Cursor,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use fs4::fs_std::FileExt;
use sha2::{Digest, Sha256};

const MANAGED_NODE_VERSION: &str = "v24.22.1";
const MAX_CHECKSUM_BYTES: u64 = 64 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_UNPACKED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Debug)]
pub struct NpmInvocation {
    pub program: PathBuf,
    pub prefix_arguments: Vec<OsString>,
}

#[derive(Clone, Debug)]
struct NodeRuntimePaths {
    version: String,
    node: PathBuf,
    npm_cli: PathBuf,
}

#[derive(Clone)]
pub struct NodeRuntime {
    cache_directory: Option<PathBuf>,
    client: Arc<tokio::sync::OnceCell<reqwest::Client>>,
    cached: Arc<tokio::sync::Mutex<Option<NodeRuntimePaths>>>,
    install_lock: Arc<tokio::sync::Mutex<()>>,
    cancelled: Arc<AtomicBool>,
}

impl NodeRuntime {
    pub fn new(
        cache_directory: Option<PathBuf>,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            cache_directory,
            client: Arc::new(tokio::sync::OnceCell::new()),
            cached: Arc::new(tokio::sync::Mutex::new(None)),
            install_lock: Arc::new(tokio::sync::Mutex::new(())),
            cancelled,
        }
    }

    pub fn cached_node_binary(&self) -> Option<PathBuf> {
        which::which("node").ok().or_else(|| {
            latest_cached_runtime(self.cache_directory.as_deref())
                .map(|paths| paths.node)
        })
    }

    pub fn cached_npm_invocation(&self) -> Option<NpmInvocation> {
        system_npm_invocation().or_else(|| {
            let runtime = latest_cached_runtime(self.cache_directory.as_deref())?;
            Some(NpmInvocation {
                program: runtime.node,
                prefix_arguments: vec![runtime.npm_cli.into_os_string()],
            })
        })
    }

    pub async fn node_binary_path(&self) -> Result<PathBuf> {
        if let Ok(node) = which::which("node") {
            return Ok(node);
        }
        Ok(self.managed_runtime().await?.node)
    }

    pub async fn npm_invocation(&self) -> Result<NpmInvocation> {
        if let Some(invocation) = system_npm_invocation() {
            return Ok(invocation);
        }
        let runtime = self.managed_runtime().await?;
        Ok(NpmInvocation {
            program: runtime.node,
            prefix_arguments: vec![runtime.npm_cli.into_os_string()],
        })
    }

    async fn managed_runtime(&self) -> Result<NodeRuntimePaths> {
        if self.cancelled.load(Ordering::Relaxed) {
            bail!("Node.js runtime setup was cancelled");
        }
        if let Some(paths) = self.cached.lock().await.clone() {
            return Ok(paths);
        }

        let _process_lock = tokio::select! {
            guard = self.install_lock.lock() => guard,
            _ = self.wait_until_cancelled() => bail!("Node.js runtime setup was cancelled"),
        };
        if let Some(paths) = self.cached.lock().await.clone() {
            return Ok(paths);
        }

        let root = self.runtime_directory()?;
        prepare_runtime_directory(&root)?;
        let _file_lock = acquire_runtime_lock(&root, &self.cancelled).await?;
        let cache_directory = self.cache_directory_path()?;
        let cached = latest_cached_runtime(Some(&cache_directory));
        if let Some(paths) = cached.as_ref()
            && version_key(&paths.version) >= version_key(MANAGED_NODE_VERSION)
        {
            *self.cached.lock().await = Some(paths.clone());
            return Ok(paths.clone());
        }

        let paths = match self.download_runtime(&root, MANAGED_NODE_VERSION).await {
            Ok(paths) => paths,
            Err(error) => match cached {
                Some(paths) => {
                    *self.cached.lock().await = Some(paths.clone());
                    return Ok(paths);
                }
                None => return Err(error),
            },
        };
        *self.cached.lock().await = Some(paths.clone());
        Ok(paths)
    }

    fn cache_directory_path(&self) -> Result<PathBuf> {
        self.cache_directory
            .clone()
            .ok_or_else(|| anyhow!("AHEAD cache directory is unavailable"))
    }

    fn runtime_directory(&self) -> Result<PathBuf> {
        Ok(self.cache_directory_path()?.join("node-runtime"))
    }

    async fn download_runtime(
        &self,
        root: &Path,
        version: &str,
    ) -> Result<NodeRuntimePaths> {
        let target = node_target()?;
        let filename = node_archive_filename(version, &target);
        let base_url = format!("https://nodejs.org/dist/{version}/");
        let checksum_url = format!("{base_url}SHASUMS256.txt");
        let checksums = self
            .fetch_bounded(&checksum_url, MAX_CHECKSUM_BYTES)
            .await?;
        let expected_checksum = checksum_for_file(&checksums, &filename)
            .ok_or_else(|| {
                anyhow!("Node.js checksum manifest omitted {filename}")
            })?;
        let archive_url = format!("{base_url}{filename}");
        let archive = self.fetch_bounded(&archive_url, MAX_ARCHIVE_BYTES).await?;
        ensure!(
            sha256_hex(&archive) == expected_checksum,
            "Node.js archive checksum did not match the official manifest"
        );

        let version_directory = root.join(format!("node-{version}-{}", target.name));
        if let Some(paths) = runtime_paths(&version_directory, version) {
            return Ok(paths);
        }

        let staging = tempfile::Builder::new()
            .prefix(".node-runtime-install-")
            .tempdir_in(root)
            .context("creating Node.js staging directory")?;
        let staging_path = staging.path().to_path_buf();
        let final_directory = version_directory.clone();
        let expected_root = format!("node-{version}-{}", target.name);
        let unpacked_limit = MAX_UNPACKED_BYTES;
        let entries_limit = MAX_ARCHIVE_ENTRIES;
        let archive_kind = target.archive_kind;
        let extraction_version = version.to_owned();
        let cancelled = self.cancelled.clone();
        tokio::select! {
            result = tokio::task::spawn_blocking(move || {
                extract_node_archive(
                    &archive,
                    &staging_path,
                    &expected_root,
                    &final_directory,
                    &extraction_version,
                    archive_kind,
                    unpacked_limit,
                    entries_limit,
                    &cancelled,
                )
            }) => result.context("extracting Node.js runtime")??,
            _ = self.wait_until_cancelled() => bail!("Node.js runtime setup was cancelled"),
        }

        runtime_paths(&version_directory, version)
            .ok_or_else(|| anyhow!("downloaded Node.js runtime was incomplete"))
    }

    async fn fetch_bounded(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
        let client = self
            .client
            .get_or_try_init(|| async { reqwest::Client::builder().build() })
            .await?;
        let mut response = tokio::select! {
            response = client.get(url).timeout(DOWNLOAD_TIMEOUT).send() => {
                response.with_context(|| format!("requesting {url}"))?
            }
            _ = self.wait_until_cancelled() => bail!("Node.js runtime setup was cancelled"),
        };
        ensure!(
            response.url().host_str() == Some("nodejs.org"),
            "Node.js download redirected to an unexpected host"
        );
        ensure!(
            response.status().is_success(),
            "Node.js server returned {} for {url}",
            response.status()
        );
        if let Some(length) = response.content_length() {
            ensure!(length <= limit, "Node.js download exceeds its size limit");
        }

        let mut bytes = Vec::with_capacity(
            response
                .content_length()
                .and_then(|length| usize::try_from(length).ok())
                .unwrap_or(0),
        );
        loop {
            let chunk = tokio::select! {
                chunk = response.chunk() => chunk.context("reading Node.js download")?,
                _ = self.wait_until_cancelled() => bail!("Node.js runtime setup was cancelled"),
            };
            let Some(chunk) = chunk else {
                break;
            };
            ensure!(
                u64::try_from(bytes.len())
                    .unwrap_or(u64::MAX)
                    .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX))
                    <= limit,
                "Node.js download exceeds its size limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn wait_until_cancelled(&self) {
        while !self.cancelled.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl Default for NodeRuntime {
    fn default() -> Self {
        Self::new(
            ahead_core::directory::Directory::cache_directory(),
            Arc::new(AtomicBool::new(false)),
        )
    }
}

#[derive(Clone, Copy)]
enum ArchiveKind {
    TarGz,
    Zip,
}

struct NodeTarget {
    name: String,
    archive_kind: ArchiveKind,
}

fn node_target() -> Result<NodeTarget> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "win",
        other => bail!("managed Node.js is not supported on {other}"),
    };
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => bail!("managed Node.js is not supported on {other}"),
    };
    Ok(NodeTarget {
        name: format!("{os}-{architecture}"),
        archive_kind: if os == "win" {
            ArchiveKind::Zip
        } else {
            ArchiveKind::TarGz
        },
    })
}

fn node_archive_filename(version: &str, target: &NodeTarget) -> String {
    let extension = match target.archive_kind {
        ArchiveKind::TarGz => "tar.gz",
        ArchiveKind::Zip => "zip",
    };
    format!("node-{version}-{}.{extension}", target.name)
}

fn version_key(version: &str) -> (u64, u64, u64) {
    let Some(version_without_prefix) = version.strip_prefix('v') else {
        return (0, 0, 0);
    };
    let mut components = version_without_prefix.split('.');
    let Some(Ok(major)) = components.next().map(str::parse) else {
        return (0, 0, 0);
    };
    let Some(Ok(minor)) = components.next().map(str::parse) else {
        return (0, 0, 0);
    };
    let Some(Ok(patch)) = components.next().map(str::parse) else {
        return (0, 0, 0);
    };
    if components.next().is_some() || format!("v{major}.{minor}.{patch}") != version
    {
        return (0, 0, 0);
    }
    (major, minor, patch)
}

fn checksum_for_file(manifest: &[u8], filename: &str) -> Option<String> {
    let manifest = std::str::from_utf8(manifest).ok()?;
    manifest.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let checksum = fields.next()?;
        let listed_filename = fields.next()?.trim_start_matches('*');
        (listed_filename == filename
            && checksum.len() == 64
            && checksum.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| checksum.to_ascii_lowercase())
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn system_npm_invocation() -> Option<NpmInvocation> {
    let node = which::which("node").ok()?;
    let npm = which::which("npm").ok()?;
    #[cfg(windows)]
    {
        let npm_cli = npm
            .parent()?
            .join("node_modules")
            .join("npm")
            .join("bin")
            .join("npm-cli.js");
        return regular_file(&npm_cli).then_some(NpmInvocation {
            program: node,
            prefix_arguments: vec![npm_cli.into_os_string()],
        });
    }
    #[cfg(not(windows))]
    {
        if let Ok(npm_cli) = npm.canonicalize()
            && npm_cli.file_name().is_some_and(|name| name == "npm-cli.js")
        {
            return Some(NpmInvocation {
                program: node,
                prefix_arguments: vec![npm_cli.into_os_string()],
            });
        }
        Some(NpmInvocation {
            program: npm,
            prefix_arguments: Vec::new(),
        })
    }
}

fn latest_cached_runtime(
    cache_directory: Option<&Path>,
) -> Option<NodeRuntimePaths> {
    let target = node_target().ok()?;
    let root = cache_directory?.join("node-runtime");
    let entries = fs::read_dir(root).ok()?;
    entries
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().ok().is_some_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let path = entry.path();
            let version = path
                .file_name()?
                .to_str()?
                .strip_prefix("node-")?
                .strip_suffix(&format!("-{}", target.name))?
                .to_string();
            let key = version_key(&version);
            (key != (0, 0, 0))
                .then(|| runtime_paths(&path, &version))
                .flatten()
                .map(|paths| (key, paths))
        })
        .max_by_key(|(key, _)| *key)
        .map(|(_, paths)| paths)
}

fn runtime_paths(node_root: &Path, version: &str) -> Option<NodeRuntimePaths> {
    let target = node_target().ok()?;
    let (node, npm_cli) = if target.name.starts_with("win-") {
        (
            node_root.join("node.exe"),
            node_root.join("node_modules/npm/bin/npm-cli.js"),
        )
    } else {
        (
            node_root.join("bin/node"),
            node_root.join("lib/node_modules/npm/bin/npm-cli.js"),
        )
    };
    (regular_file(&node) && executable_file(&node) && regular_file(&npm_cli))
        .then_some(NodeRuntimePaths {
            version: version.to_string(),
            node,
            npm_cli,
        })
}

fn regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}

fn executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        regular_file(path)
    }
}

fn prepare_runtime_directory(root: &Path) -> Result<()> {
    fs::create_dir_all(root)?;
    ensure!(
        fs::symlink_metadata(root)?.is_dir(),
        "Node.js runtime cache is not a regular directory"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

async fn acquire_runtime_lock(
    root: &Path,
    cancelled: &AtomicBool,
) -> Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let path = root.join(".node-runtime.lock");
    let file = options.open(&path)?;
    ensure!(
        fs::symlink_metadata(&path)?.is_file() && file.metadata()?.is_file(),
        "Node.js runtime lock is not a regular file"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        ensure!(
            !cancelled.load(Ordering::Relaxed),
            "Node.js runtime setup was cancelled"
        );
        ensure!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for Node.js runtime installation lock"
        );
        if file.try_lock_exclusive()? {
            return Ok(file);
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            _ = async {
                while !cancelled.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            } => bail!("Node.js runtime setup was cancelled"),
        }
    }
}

fn extract_node_archive(
    archive: &[u8],
    staging_root: &Path,
    expected_root: &str,
    final_directory: &Path,
    version: &str,
    archive_kind: ArchiveKind,
    unpacked_limit: u64,
    entries_limit: usize,
    cancelled: &AtomicBool,
) -> Result<()> {
    fs::create_dir_all(staging_root)?;
    match archive_kind {
        ArchiveKind::TarGz => {
            extract_node_tar_gz(
                archive,
                staging_root,
                unpacked_limit,
                entries_limit,
                cancelled,
            )?;
        }
        ArchiveKind::Zip => {
            extract_node_zip(
                archive,
                staging_root,
                unpacked_limit,
                entries_limit,
                cancelled,
            )?;
        }
    }
    let extracted = staging_root.join(expected_root);
    ensure!(
        !cancelled.load(Ordering::Relaxed),
        "Node.js runtime setup was cancelled"
    );
    ensure!(
        runtime_paths(&extracted, version).is_some(),
        "Node.js archive omitted required runtime files"
    );
    if fs::symlink_metadata(final_directory).is_ok() {
        let metadata = fs::symlink_metadata(final_directory)?;
        if metadata.file_type().is_dir() {
            fs::remove_dir_all(final_directory)
                .context("removing an invalid cached Node.js runtime")?;
        } else {
            fs::remove_file(final_directory)
                .context("removing an invalid cached Node.js runtime")?;
        }
    }
    ensure!(
        !cancelled.load(Ordering::Relaxed),
        "Node.js runtime setup was cancelled"
    );
    fs::rename(extracted, final_directory)
        .context("publishing the verified Node.js runtime")?;
    Ok(())
}

fn extract_node_tar_gz(
    bytes: &[u8],
    destination: &Path,
    unpacked_limit: u64,
    entries_limit: usize,
    cancelled: &AtomicBool,
) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    let mut unpacked = 0_u64;
    for (index, entry) in archive.entries()?.enumerate() {
        ensure!(
            index < entries_limit,
            "Node.js archive exceeds the entry limit"
        );
        let mut entry = entry?;
        ensure!(
            !cancelled.load(Ordering::Relaxed),
            "Node.js runtime setup was cancelled"
        );
        unpacked = unpacked
            .checked_add(entry.size())
            .filter(|size| *size <= unpacked_limit)
            .ok_or_else(|| {
                anyhow!("Node.js archive exceeds the unpacked size limit")
            })?;
        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind.is_hard_link() {
            continue;
        }
        let relative = entry.path()?.into_owned();
        let destination = crate::host::checked_extension_path(
            destination,
            relative
                .to_str()
                .ok_or_else(|| anyhow!("Node.js archive path is not UTF-8"))?,
        )?;
        if kind.is_dir() {
            fs::create_dir_all(destination)?;
        } else if kind.is_file() {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            entry.unpack(destination)?;
        } else {
            bail!("Node.js archive contains an unsupported entry");
        }
    }
    Ok(())
}

fn extract_node_zip(
    bytes: &[u8],
    destination: &Path,
    unpacked_limit: u64,
    entries_limit: usize,
    cancelled: &AtomicBool,
) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    ensure!(
        archive.len() <= entries_limit,
        "Node.js archive exceeds the entry limit"
    );
    let mut unpacked = 0_u64;
    for index in 0..archive.len() {
        ensure!(
            !cancelled.load(Ordering::Relaxed),
            "Node.js runtime setup was cancelled"
        );
        let mut entry = archive.by_index(index)?;
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| {
                anyhow!("Node.js archive path escapes its install directory")
            })?
            .to_path_buf();
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            bail!("Node.js ZIP archive contains an unsupported symbolic link");
        }
        let output = crate::host::checked_extension_path(
            destination,
            relative
                .to_str()
                .ok_or_else(|| anyhow!("Node.js archive path is not UTF-8"))?,
        )?;
        if entry.is_dir() {
            fs::create_dir_all(output)?;
            continue;
        }
        unpacked = unpacked
            .checked_add(entry.size())
            .filter(|size| *size <= unpacked_limit)
            .ok_or_else(|| {
                anyhow!("Node.js archive exceeds the unpacked size limit")
            })?;
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?;
        let copied = std::io::copy(&mut entry, &mut file)?;
        ensure!(copied == entry.size(), "Node.js ZIP entry was incomplete");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_lookup_requires_the_exact_archive_filename() {
        let checksum = "a".repeat(64);
        let manifest = format!(
            "{checksum}  node-v24.21.0-darwin-arm64.tar.gz\n{checksum}  other.tar.gz\n"
        );
        assert_eq!(
            checksum_for_file(
                manifest.as_bytes(),
                "node-v24.21.0-darwin-arm64.tar.gz"
            ),
            Some(checksum)
        );
        assert!(
            checksum_for_file(manifest.as_bytes(), "node-v24.21.0-win-x64.zip")
                .is_none()
        );
    }

    #[test]
    fn rejects_non_version_directory_names() {
        assert_eq!(version_key("v24.21.0"), (24, 21, 0));
        assert_eq!(version_key("../v24.21.0"), (0, 0, 0));
        assert_eq!(version_key("v24.21.0/../../tmp"), (0, 0, 0));
        assert_eq!(version_key("v024.21.0"), (0, 0, 0));
    }

    #[tokio::test]
    async fn cached_runtime_paths_match_the_node_distribution_layout() -> Result<()>
    {
        let cache = tempfile::tempdir()?;
        let version = MANAGED_NODE_VERSION;
        let target = node_target()?;
        let runtime_root = cache
            .path()
            .join("node-runtime")
            .join(format!("node-{version}-{}", target.name));
        let paths = if target.name.starts_with("win-") {
            (
                runtime_root.join("node.exe"),
                runtime_root.join("node_modules/npm/bin/npm-cli.js"),
            )
        } else {
            (
                runtime_root.join("bin/node"),
                runtime_root.join("lib/node_modules/npm/bin/npm-cli.js"),
            )
        };
        std::fs::create_dir_all(paths.0.parent().expect("Node binary directory"))?;
        std::fs::create_dir_all(paths.1.parent().expect("npm CLI directory"))?;
        std::fs::write(&paths.0, b"node")?;
        std::fs::write(&paths.1, b"npm")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                &paths.0,
                std::fs::Permissions::from_mode(0o700),
            )?;
        }

        let runtime = runtime_paths(&runtime_root, version)
            .expect("complete distribution root is accepted");
        assert_eq!(runtime.node, paths.0);
        assert_eq!(runtime.npm_cli, paths.1);
        let manager = NodeRuntime::new(
            Some(cache.path().to_path_buf()),
            Arc::new(AtomicBool::new(false)),
        );
        let managed = manager.managed_runtime().await?;
        assert_eq!(managed.node, paths.0);
        assert!(
            manager.client.get().is_none(),
            "cached runtime must not check the network"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn verified_node_archive_is_published_from_a_staging_directory() -> Result<()> {
        use flate2::{Compression, write::GzEncoder};

        let version = "v24.21.0";
        let target = node_target()?;
        let expected_root = format!("node-{version}-{}", target.name);
        let mut archive =
            tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        for (path, contents, mode) in [
            (
                format!("{expected_root}/bin/node"),
                b"node".as_slice(),
                0o755,
            ),
            (
                format!("{expected_root}/lib/node_modules/npm/bin/npm-cli.js"),
                b"npm".as_slice(),
                0o644,
            ),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(mode);
            header.set_cksum();
            archive.append_data(&mut header, path, contents)?;
        }
        archive.finish()?;
        let compressed = archive.into_inner()?.finish()?;

        let cache = tempfile::tempdir()?;
        let staging = tempfile::tempdir_in(cache.path())?;
        let runtime_root = cache.path().join(&expected_root);
        extract_node_archive(
            &compressed,
            staging.path(),
            &expected_root,
            &runtime_root,
            version,
            ArchiveKind::TarGz,
            MAX_UNPACKED_BYTES,
            MAX_ARCHIVE_ENTRIES,
            &AtomicBool::new(false),
        )?;

        assert!(runtime_paths(&runtime_root, version).is_some());
        assert_eq!(std::fs::read(runtime_root.join("bin/node"))?, b"node");
        Ok(())
    }
}
