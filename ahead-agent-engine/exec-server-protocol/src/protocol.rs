use std::collections::HashMap;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use codex_file_system::FileSystemSandboxContext;
pub use codex_file_system::WalkOptions;
pub use codex_file_system::WalkOutcome;
use codex_protocol::config_types::ShellEnvironmentPolicyInherit;
use codex_shell_command::shell_detect::DetectedShell;
use codex_utils_path_uri::PathUri;
use serde::Deserialize;
use serde::Serialize;

use crate::ProcessId;

pub const FS_READ_FILE_METHOD: &str = "fs/readFile";
pub const FS_OPEN_METHOD: &str = "fs/open";
pub const FS_READ_BLOCK_METHOD: &str = "fs/readBlock";
pub const FS_CLOSE_METHOD: &str = "fs/close";
pub const FS_WRITE_FILE_METHOD: &str = "fs/writeFile";
pub const FS_CREATE_DIRECTORY_METHOD: &str = "fs/createDirectory";
pub const FS_GET_METADATA_METHOD: &str = "fs/getMetadata";
pub const FS_CANONICALIZE_METHOD: &str = "fs/canonicalize";
pub const FS_READ_DIRECTORY_METHOD: &str = "fs/readDirectory";
pub const FS_WALK_METHOD: &str = "fs/walk";
pub const FS_REMOVE_METHOD: &str = "fs/remove";
pub const FS_COPY_METHOD: &str = "fs/copy";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ByteChunk(#[serde(with = "base64_bytes")] pub Vec<u8>);

impl ByteChunk {
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

impl From<Vec<u8>> for ByteChunk {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

/// Information about an execution/filesystem environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentInfo {
    pub shell: ShellInfo,
    /// Working directory inherited by the exec-server process.
    pub cwd: Option<PathUri>,
    /// Executor user home used to expand `~` in path-bearing values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_home_dir: Option<PathUri>,
    /// Operating system reported by the executor, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_os: Option<String>,
    /// Executor-local default directories for resolving `:tmpdir`, when reported.
    /// On Windows, a command's `TEMP` or `TMP` overrides take precedence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporary_directories: Option<Vec<PathUri>>,
    /// Executor-native temporary directory for child-visible sidecars.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temp_dir: Option<PathUri>,
    /// Features provided by this executor.
    pub capabilities: EnvironmentCapabilities,
}

/// Features supported by the selected exec-server environment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentCapabilities {
    /// Whether capability discovery applies the filesystem sandbox sent with each root.
    pub capability_discovery_sandbox: bool,
    /// Whether filesystem streams can use the requested platform sandbox.
    pub sandboxed_file_streaming: bool,
    /// Whether shell state can be cached and restored entirely inside the executor.
    pub shell_snapshot_v2: bool,
}

impl EnvironmentInfo {
    /// Returns executor-local default directories used to resolve `:tmpdir`.
    ///
    /// This is separate from `local` so orchestrator startup can cache the
    /// directories without repeating local shell detection.
    pub fn local_temporary_directories() -> Vec<PathUri> {
        let cwd = std::env::current_dir().ok();
        Self::local_temporary_directories_with_cwd(cwd.as_deref())
    }

    fn local_temporary_directories_with_cwd(cwd: Option<&std::path::Path>) -> Vec<PathUri> {
        let temporary_directory_env_vars: &[&str] = if cfg!(windows) {
            &["TEMP", "TMP"]
        } else {
            &["TMPDIR"]
        };
        let normalize_temp_path = |path: std::ffi::OsString| {
            PathUri::from_host_native_path(&path).ok().or_else(|| {
                if cfg!(unix) {
                    PathUri::from_host_native_path(cwd.as_ref()?.join(path)).ok()
                } else {
                    None
                }
            })
        };
        let mut temporary_directories = Vec::new();
        for name in temporary_directory_env_vars {
            if let Some(path) = std::env::var_os(name)
                .filter(|path| !path.is_empty())
                .filter(|path| cfg!(unix) || std::path::Path::new(path).is_absolute())
                .and_then(&normalize_temp_path)
                && !temporary_directories.contains(&path)
            {
                temporary_directories.push(path);
            }
        }
        temporary_directories
    }

    /// Returns information about the current local exec-server process.
    pub fn local() -> Self {
        let cwd = std::env::current_dir().ok();
        let temporary_directories = Self::local_temporary_directories_with_cwd(cwd.as_deref());
        let normalize_temp_path = |path: std::ffi::OsString| {
            PathUri::from_host_native_path(&path).ok().or_else(|| {
                if cfg!(unix) {
                    PathUri::from_host_native_path(cwd.as_ref()?.join(path)).ok()
                } else {
                    None
                }
            })
        };
        let temp_dir = normalize_temp_path(std::env::temp_dir().into_os_string());

        Self {
            shell: codex_shell_command::shell_detect::default_user_shell().into(),
            cwd: cwd.and_then(|cwd| PathUri::from_host_native_path(cwd).ok()),
            user_home_dir: PathUri::from_host_native_path("~").ok(),
            platform_os: Some(std::env::consts::OS.to_string()),
            temporary_directories: Some(temporary_directories),
            temp_dir,
            capabilities: EnvironmentCapabilities {
                capability_discovery_sandbox: true,
                sandboxed_file_streaming: true,
                shell_snapshot_v2: cfg!(unix),
            },
        }
    }
}

/// Shell detected for an execution/filesystem environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellInfo {
    /// Stable shell name, for example `zsh`, `bash`, `powershell`, `sh`, or `cmd`.
    pub name: String,
    /// Target-native shell executable path or command name. Fallbacks such as `cmd.exe` need not
    /// be absolute, so this is not a [`PathUri`].
    pub path: String,
}

impl From<DetectedShell> for ShellInfo {
    fn from(shell: DetectedShell) -> Self {
        Self {
            name: shell.name().to_string(),
            path: shell.shell_path.to_string_lossy().into_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecParams {
    /// Client-chosen logical process handle scoped to this connection/session.
    /// This is a protocol key, not an OS pid.
    pub process_id: ProcessId,
    pub argv: Vec<String>,
    /// Working directory URI, interpreted using the exec-server host's path rules at launch time.
    pub cwd: PathUri,
    pub env_policy: Option<ExecEnvPolicy>,
    /// Optional request to restore executor-owned, attachment-scoped shell state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell_snapshot: Option<ShellSnapshotRequest>,
    pub env: HashMap<String, String>,
    pub tty: bool,
    /// Keep non-tty stdin writable through `process/write`.
    pub pipe_stdin: bool,
    /// Optional process-visible argv0 override. Values such as `codex-linux-sandbox` are command
    /// names rather than paths, so this is not a [`PathUri`].
    pub arg0: Option<String>,
    /// Portable sandbox intent. Concrete wrapper argv is resolved by the exec-server.
    pub sandbox: Option<FileSystemSandboxContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecEnvPolicy {
    pub inherit: ShellEnvironmentPolicyInherit,
    pub ignore_default_excludes: bool,
    pub exclude: Vec<String>,
    pub r#set: HashMap<String, String>,
    pub include_only: Vec<String>,
}

/// Identifies shell state owned by one attachment within an executor session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellSnapshotRequest {
    /// Attachment identity; executor sessions independently scope every cache.
    pub scope_id: String,
    /// Executor-native shell used to capture and restore the snapshot.
    pub shell: ShellInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadParams {
    pub process_id: ProcessId,
    pub after_seq: Option<u64>,
    pub max_bytes: Option<usize>,
    pub wait_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessOutputChunk {
    pub seq: u64,
    pub stream: ExecOutputStream,
    pub chunk: ByteChunk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResponse {
    pub chunks: Vec<ProcessOutputChunk>,
    pub next_seq: u64,
    pub exited: bool,
    pub exit_code: Option<i32>,
    pub closed: bool,
    pub failure: Option<String>,
    /// Whether the executor classified the process failure as a sandbox denial.
    pub sandbox_denied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteParams {
    pub process_id: ProcessId,
    pub chunk: ByteChunk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WriteStatus {
    Accepted,
    UnknownProcess,
    StdinClosed,
    Starting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteResponse {
    pub status: WriteStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessSignal {
    Interrupt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalParams {
    pub process_id: ProcessId,
    pub signal: ProcessSignal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminateParams {
    pub process_id: ProcessId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminateResponse {
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsReadFileParams {
    pub path: PathUri,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_symlinks: Option<bool>,
    pub sandbox: Option<FileSystemSandboxContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsReadFileResponse {
    pub data_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsOpenParams {
    pub handle_id: String,
    pub path: PathUri,
    pub sandbox: Option<FileSystemSandboxContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsOpenResponse {
    pub handle_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsReadBlockParams {
    pub handle_id: String,
    pub offset: u64,
    pub len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsReadBlockResponse {
    pub chunk: ByteChunk,
    pub eof: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsCloseParams {
    pub handle_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsCloseResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsWriteFileParams {
    pub path: PathUri,
    pub data_base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_symlinks: Option<bool>,
    pub sandbox: Option<FileSystemSandboxContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsWriteFileResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsCreateDirectoryParams {
    pub path: PathUri,
    pub recursive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_symlinks: Option<bool>,
    pub sandbox: Option<FileSystemSandboxContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsCreateDirectoryResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsGetMetadataParams {
    pub path: PathUri,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_symlinks: Option<bool>,
    pub sandbox: Option<FileSystemSandboxContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsGetMetadataResponse {
    pub is_directory: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub created_at_ms: i64,
    pub modified_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsCanonicalizeParams {
    pub path: PathUri,
    pub sandbox: Option<FileSystemSandboxContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsCanonicalizeResponse {
    pub path: PathUri,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsReadDirectoryParams {
    pub path: PathUri,
    pub sandbox: Option<FileSystemSandboxContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsReadDirectoryEntry {
    pub file_name: String,
    pub is_directory: bool,
    pub is_file: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsReadDirectoryResponse {
    pub entries: Vec<FsReadDirectoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsWalkParams {
    pub path: PathUri,
    pub options: WalkOptions,
    pub sandbox: Option<FileSystemSandboxContext>,
}

pub type FsWalkResponse = WalkOutcome;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsRemoveParams {
    pub path: PathUri,
    pub recursive: Option<bool>,
    pub force: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_symlinks: Option<bool>,
    pub sandbox: Option<FileSystemSandboxContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsRemoveResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsCopyParams {
    pub source_path: PathUri,
    pub destination_path: PathUri,
    pub recursive: bool,
    pub sandbox: Option<FileSystemSandboxContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsCopyResponse {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecOutputStream {
    Stdout,
    Stderr,
    Pty,
}

mod base64_bytes {
    use super::BASE64_STANDARD;
    use base64::Engine as _;
    use serde::Deserialize;
    use serde::Deserializer;
    use serde::Serializer;

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&BASE64_STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        BASE64_STANDARD
            .decode(encoded)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::EnvironmentInfo;
    use super::ExecParams;
    use super::FsReadFileParams;
    use super::ProcessId;
    use super::ShellInfo;
    use codex_file_system::FileSystemSandboxContext;
    use codex_protocol::models::ManagedFileSystemPermissions;
    use codex_protocol::models::PermissionProfile;
    use codex_protocol::permissions::FileSystemAccessMode;
    use codex_protocol::permissions::FileSystemPath;
    use codex_protocol::permissions::FileSystemSandboxEntry;
    use codex_protocol::permissions::FileSystemSandboxPolicy;
    use codex_protocol::permissions::FileSystemSpecialPath;
    use codex_protocol::permissions::NetworkSandboxPolicy;
    use codex_utils_path_uri::PathUri;
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;

    #[test]
    fn environment_info_preserves_executor_temporary_directories() {
        let expected = serde_json::json!({
            "shell": { "name": "powershell", "path": "powershell.exe" },
            "cwd": null,
            "userHomeDir": "file:///C:/Users/remote",
            "platformOs": "windows",
            "temporaryDirectories": ["file:///C:/Temp", "file:///D:/Temp"],
            "capabilities": {
                "capabilityDiscoverySandbox": false,
                "sandboxedFileStreaming": false,
                "shellSnapshotV2": false,
            },
        });
        let info: EnvironmentInfo = serde_json::from_value(expected.clone())
            .expect("environment info with executor temporary directories should deserialize");

        assert_eq!(
            serde_json::to_value(info).expect("environment info should serialize"),
            expected,
        );
    }

    #[test]
    fn local_environment_info_reads_platform_temporary_directories() {
        let cwd = std::env::current_dir().expect("current directory");
        let names: &[&str] = if cfg!(windows) {
            &["TEMP", "TMP"]
        } else {
            &["TMPDIR"]
        };
        let mut expected = names
            .iter()
            .filter_map(std::env::var_os)
            .filter(|path| !path.is_empty())
            .filter(|path| cfg!(unix) || std::path::Path::new(path).is_absolute())
            .filter_map(|path| {
                PathUri::from_host_native_path(&path).ok().or_else(|| {
                    if cfg!(unix) {
                        PathUri::from_host_native_path(cwd.join(path)).ok()
                    } else {
                        None
                    }
                })
            })
            .collect::<Vec<_>>();
        expected.dedup();

        let info = EnvironmentInfo::local();
        assert_eq!(info.temporary_directories, Some(expected));
        assert_eq!(info.user_home_dir, PathUri::from_host_native_path("~").ok());
    }

    #[cfg(unix)]
    #[test]
    fn local_environment_info_resolves_relative_temporary_directory() {
        if std::env::var_os("CODEX_TEST_RELATIVE_TMPDIR").is_none() {
            let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .arg("--exact")
                .arg(
                    "protocol::tests::local_environment_info_resolves_relative_temporary_directory",
                )
                .env("CODEX_TEST_RELATIVE_TMPDIR", "1")
                .env("TMPDIR", "relative-temp")
                .status()
                .expect("run relative TMPDIR subprocess");
            assert!(status.success(), "relative TMPDIR subprocess failed");
            return;
        }

        let expected = PathUri::from_host_native_path(
            std::env::current_dir()
                .expect("current directory")
                .join("relative-temp"),
        )
        .expect("absolute temporary directory URI");
        let info = EnvironmentInfo::local();
        assert_eq!(info.temporary_directories, Some(vec![expected.clone()]));
        assert_eq!(info.temp_dir, Some(expected));
    }

    #[test]
    fn filesystem_protocol_rejects_native_absolute_paths() {
        let native_path = std::env::current_dir()
            .expect("current directory")
            .join("native-file.txt");
        let native_cwd = std::env::current_dir().expect("current directory");

        serde_json::from_value::<FsReadFileParams>(serde_json::json!({
            "path": native_path.to_string_lossy(),
            "sandbox": null,
        }))
        .expect_err("native absolute path should not deserialize as a URI");

        let sandbox = FileSystemSandboxContext::from_permission_profile_with_cwd(
            PermissionProfile::default(),
            PathUri::from_host_native_path(&native_cwd).expect("cwd URI"),
        );
        let mut native_path_sandbox =
            serde_json::to_value(sandbox).expect("sandbox should serialize");
        native_path_sandbox["cwd"] = serde_json::json!(native_cwd.to_string_lossy());

        serde_json::from_value::<FsReadFileParams>(serde_json::json!({
            "path": PathUri::from_host_native_path(native_path)
                .expect("path URI")
                .to_string(),
            "sandbox": native_path_sandbox,
        }))
        .expect_err("native absolute sandbox cwd should not deserialize as a URI");
    }

    #[test]
    fn filesystem_protocol_round_trips_permission_entries() {
        let native_cwd = std::env::current_dir().expect("current directory");
        let cwd = PathUri::from_host_native_path(&native_cwd).expect("cwd URI");
        let file_system = ManagedFileSystemPermissions::Restricted {
            entries: vec![
                FileSystemSandboxEntry {
                    path: FileSystemPath::Path { path: cwd.clone() },
                    access: FileSystemAccessMode::Read,
                    missing_path_behavior: None,
                },
                FileSystemSandboxEntry::skip_missing_path(
                    FileSystemPath::Path {
                        path: PathUri::from_host_native_path(native_cwd.join(".git"))
                            .expect("absolute path"),
                    },
                    FileSystemAccessMode::Read,
                ),
                FileSystemSandboxEntry::skip_missing_path(
                    FileSystemPath::Special {
                        value: FileSystemSpecialPath::ProjectRoots {
                            subpath: Some(".ahead".into()),
                        },
                    },
                    FileSystemAccessMode::Read,
                ),
            ],
            glob_scan_max_depth: Some(2.try_into().expect("non-zero depth")),
        };
        let permissions = PermissionProfile::Managed {
            file_system,
            network: NetworkSandboxPolicy::Restricted,
        };
        let mut sandbox =
            FileSystemSandboxContext::from_permission_profile_with_cwd(permissions, cwd.clone());
        sandbox.user_home_dir = Some(cwd.clone());

        let serialized = serde_json::to_value(&sandbox).expect("serialize sandbox");

        assert_eq!(
            serialized["userHomeDir"],
            serde_json::json!(cwd.to_string())
        );
        assert_eq!(
            serialized["permissions"]["file_system"]["entries"][0]["path"]["path"],
            serde_json::json!(cwd.to_string())
        );
        assert_eq!(
            serialized["permissions"]["file_system"]["entries"][1]["path"]["type"],
            serde_json::json!("path")
        );
        assert_eq!(
            serialized["permissions"]["file_system"]["entries"][1]["missing_path_behavior"],
            serde_json::json!("skip")
        );
        assert_eq!(
            serialized["permissions"]["file_system"]["entries"][2]["path"]["type"],
            serde_json::json!("special")
        );
        assert_eq!(
            serialized["permissions"]["file_system"]["entries"][2]["missing_path_behavior"],
            serde_json::json!("skip")
        );
        assert!(!serialized.to_string().contains("generated_default_path"));
        assert!(!serialized.to_string().contains("generated_default_special"));
        assert_eq!(
            serde_json::from_value::<FileSystemSandboxContext>(serialized)
                .expect("deserialize sandbox"),
            sandbox
        );
    }

    #[test]
    fn filesystem_protocol_round_trips_policy_paths_as_uris() {
        let native_cwd = std::env::current_dir().expect("current directory");
        let cwd = PathUri::from_host_native_path(&native_cwd).expect("cwd URI");
        let mut file_system_policy =
            FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
                path: FileSystemPath::Path { path: cwd.clone() },
                access: FileSystemAccessMode::Read,
                missing_path_behavior: None,
            }]);
        file_system_policy.glob_scan_max_depth = Some(2);
        let permissions = PermissionProfile::from_runtime_permissions(
            &file_system_policy,
            NetworkSandboxPolicy::Restricted,
        );
        let sandbox =
            FileSystemSandboxContext::from_permission_profile_with_cwd(permissions, cwd.clone());

        let serialized = serde_json::to_value(&sandbox).expect("serialize sandbox");

        assert_eq!(
            serialized["permissions"]["file_system"]["entries"][0]["path"]["path"],
            serde_json::json!(cwd.to_string())
        );
        assert_eq!(
            serde_json::from_value::<FileSystemSandboxContext>(serialized)
                .expect("deserialize sandbox"),
            sandbox
        );
    }
}
