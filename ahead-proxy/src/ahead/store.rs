//! AHEAD Local Session Persistence
//!
//! Turso / libSQL-backed transactional store for sessions, workflow revisions,
//! session event logs, and durable code anchors.
//! Uses local embedded databases through the Turso/libSQL client.
//! Corresponds to Section 7.1 of `ahead-editor-mvp.md`.

use ahead_agent::{InstructionFileSource, LegacyNativeThreadImport};
use ahead_rpc::ahead::{
    AgentRuntimeState, ApprovalRecord, CodeAnchor, ConversationMessage,
    ConversationMessageCursor, ConversationMessagePage, ConversationSummary,
    DisplayPosition, DisplayRange, GithubIssueRef, Id, LearningArc, LearningRecord,
    Participant, Revision, SessionLifecycle, SessionListItem,
    SessionParticipantRecord, SessionPolicySnapshot, SessionRole, SessionTask,
    SessionView, TaskIntent, WorkItem, WorkItemCloseout, WorkItemEvent,
    WorkItemStatus, WorkKind, WorkSession, WorkflowPhase, WorkflowState,
};
use ahead_rpc::file::{
    EditorRecoverySnapshot, EditorRecoverySummary, MAX_EDITOR_RECOVERY_BYTES,
};
use anyhow::{Context, Result, bail};
use libsql::{Builder, Connection, Value as SqlValue, params, params_from_iter};
use parking_lot::Mutex;
use serde_json::Value;
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::fs;
use std::{
    collections::{BTreeSet, HashMap},
    fs::OpenOptions,
    io::{ErrorKind, Read},
    path::Path,
    sync::Arc,
};

const RECENT_WORKSPACE_FILE_LIMIT: i64 = 20;
const SESSION_APPLICATION_ID: i64 = 0x41484544;
const SESSION_SCHEMA_VERSION: i64 = 2;
// A raw descriptor closed during another local open could release its POSIX locks.
static SESSION_STORE_OPEN_LOCK: Mutex<()> = Mutex::new(());

async fn session_schema_is_current(connection: &Connection) -> Result<bool> {
    let mut rows = connection
        .query(
            "SELECT application_id, user_version,
                EXISTS(SELECT 1 FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*')
         FROM pragma_application_id, pragma_user_version",
            (),
        )
        .await?;
    let row = rows
        .next()
        .await?
        .context("missing session schema header")?;
    let application_id = row.get::<i64>(0)?;
    let version = row.get::<i64>(1)?;
    let has_schema = row.get::<i64>(2)? != 0;
    if application_id == 0 && version == 0 && !has_schema {
        return Ok(false);
    }
    validate_session_version(application_id, version)?;
    Ok(true)
}

fn validate_session_version(application_id: i64, version: i64) -> Result<()> {
    anyhow::ensure!(
        application_id == SESSION_APPLICATION_ID
            && version == SESSION_SCHEMA_VERSION,
        "Unsupported AHEAD session database (application {application_id:#x}, schema {version}). \
         This build requires AHEAD schema {SESSION_SCHEMA_VERSION}; no migration is provided. \
         The database was not changed."
    );
    Ok(())
}

#[cfg(unix)]
fn session_database_files(
    path: &Path,
    restrict: bool,
) -> Result<std::path::PathBuf> {
    use rustix::fs::{
        AtFlags, FileType, Mode, OFlags, chmodat, fstat, openat, statat,
    };

    let absolute = std::path::absolute(path)?;
    let parent = absolute
        .parent()
        .context("session database needs a parent directory")?;
    anyhow::ensure!(
        std::fs::symlink_metadata(parent)?.is_dir(),
        "session database directory must not be a symlink"
    );
    let parent = parent.canonicalize()?;
    let directory = ahead_core::secure_fs::open_canonical_directory(&parent)?;
    let owner = rustix::process::geteuid().as_raw();
    let metadata = fstat(&directory)?;
    anyhow::ensure!(
        metadata.st_uid == owner && metadata.st_mode & 0o022 == 0,
        "session database directory must be owned by the current user and not writable by other users"
    );
    let name = absolute
        .file_name()
        .context("session database needs a filename")?;
    let path = parent.join(name);
    path.to_str()
        .context("session database path must be UTF-8")?;

    let mut names = vec![name.to_owned()];
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut sidecar = name.to_owned();
        sidecar.push(suffix);
        names.push(sidecar);
    }
    let mut existing = Vec::new();
    for name in &names {
        match statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(metadata) => {
                anyhow::ensure!(
                    FileType::from_raw_mode(metadata.st_mode)
                        == FileType::RegularFile
                        && metadata.st_nlink == 1
                        && metadata.st_uid == owner,
                    "session database and sidecars must be regular, singly-linked files owned by the current user: {}",
                    name.to_string_lossy()
                );
                existing.push((name, metadata.st_mode));
            }
            Err(rustix::io::Errno::NOENT) => {}
            Err(error) => return Err(error.into()),
        }
    }
    let main_exists = existing.iter().any(|(entry, _)| *entry == name);
    anyhow::ensure!(
        main_exists || existing.is_empty(),
        "session database is missing but recovery sidecars remain; keep these files for recovery"
    );
    if restrict {
        anyhow::ensure!(main_exists, "session database disappeared during startup");
        for (name, mode) in existing {
            if mode & 0o7777 != 0o600 {
                // The directory is owner-only writable. Use its descriptor so a
                // renamed parent cannot redirect chmod; do not reopen the DB and
                // accidentally release libSQL's process-wide POSIX file locks.
                chmodat(
                    &directory,
                    name,
                    Mode::RUSR | Mode::WUSR,
                    AtFlags::empty(),
                )?;
            }
        }
    } else if !main_exists {
        match openat(
            &directory,
            name,
            OFlags::WRONLY
                | OFlags::CREATE
                | OFlags::EXCL
                | OFlags::NOFOLLOW
                | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(file) => drop(file),
            Err(rustix::io::Errno::EXIST) => {
                return session_database_files(&path, false);
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path)
}

#[cfg(not(unix))]
fn session_database_files(
    path: &Path,
    _restrict: bool,
) -> Result<std::path::PathBuf> {
    let path = std::path::absolute(path)?;
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => anyhow::ensure!(
            metadata.is_file(),
            "session database must be a regular file"
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            drop(
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)?,
            );
        }
        Err(error) => return Err(error.into()),
    }
    Ok(path)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySearchHit {
    pub scope: String,
    pub path: String,
    pub content_sha256: String,
    pub line: usize,
    pub excerpt: String,
    pub relevance: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRevision {
    pub scope: String,
    pub path: String,
    pub content_sha256: String,
    pub indexed_at: String,
}

pub struct SessionStore {
    conn: Connection,
    rt: Arc<tokio::runtime::Runtime>,
    access: Mutex<()>,
}

impl SessionStore {
    /// Opens or creates a local Turso/libSQL database file
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let _opening = SESSION_STORE_OPEN_LOCK.lock();
        let path = session_database_files(path.as_ref(), false)?;
        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("Failed to create tokio runtime for SessionStore")?,
        );
        let conn = rt
            .block_on(async {
                // The pinned client omits a named flag for SQLITE_OPEN_NOFOLLOW.
                // Keep the VFS check enabled as well as our file-kind validation.
                let nofollow = libsql::OpenFlags::from_bits_retain(0x01000000);
                if std::fs::metadata(&path)?.len() > 0 {
                    // Schema markers never change in this hard fork. Inspect the
                    // main header without recovering journals or creating SHM;
                    // a supported store can then use normal crash recovery.
                    let mut uri = url::Url::from_file_path(&path).map_err(|_| anyhow::anyhow!("invalid session database path"))?;
                    uri.query_pairs_mut().append_pair("immutable", "1");
                    let database = Builder::new_local(uri.as_str())
                        .flags(libsql::OpenFlags::SQLITE_OPEN_READ_ONLY | nofollow | libsql::OpenFlags::from_bits_retain(0x00000040))
                        .build().await?;
                    let connection = database.connect()?;
                    let mut header = connection.query("SELECT application_id, user_version FROM pragma_application_id, pragma_user_version", ()).await
                        .context("Read-only session validation failed; keep the database and its recovery sidecars intact")?;
                    let row = header.next().await?.context("missing session database header")?;
                    validate_session_version(row.get(0)?, row.get(1)?)?;
                }
                let flags = libsql::OpenFlags::SQLITE_OPEN_READ_WRITE | nofollow;
                let db = Builder::new_local(&path).flags(flags).build().await?;
                Ok::<_, anyhow::Error>(db.connect()?)
            })
            .context("Failed to initialize local Turso/libsql database")?;
        let store = Self {
            conn,
            rt,
            access: Mutex::new(()),
        };
        store.init_schema()?;
        session_database_files(&path, true)?;
        Ok(store)
    }

    /// Creates an in-memory Turso/libSQL database
    pub fn in_memory() -> Result<Self> {
        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("Failed to create tokio runtime for SessionStore")?,
        );
        let conn = rt
            .block_on(async {
                let db = Builder::new_local(":memory:").build().await?;
                db.connect()
            })
            .context("Failed to initialize in-memory Turso/libsql database")?;
        let store = Self {
            conn,
            rt,
            access: Mutex::new(()),
        };
        store.init_schema()?;
        Ok(store)
    }

    pub fn record_recent_workspace_file(
        &self,
        path: &str,
        opened_at: i64,
    ) -> Result<()> {
        if path.is_empty() || path.contains('\0') {
            bail!("recent workspace file path must not be empty");
        }
        let path = path.to_string();
        self.block_on(async {
            self.conn.execute("BEGIN IMMEDIATE", ()).await?;
            let update = async {
                self.conn
                    .execute(
                        "INSERT INTO recent_workspace_files(path, opened_at)
                         VALUES (?1, ?2)
                         ON CONFLICT(path) DO UPDATE SET opened_at =
                             MAX(recent_workspace_files.opened_at, excluded.opened_at)",
                        params![path, opened_at],
                    )
                    .await?;
                self.conn
                    .execute(
                        "DELETE FROM recent_workspace_files
                         WHERE path NOT IN (
                             SELECT path FROM recent_workspace_files
                             ORDER BY opened_at DESC, path ASC LIMIT ?1
                         )",
                        params![RECENT_WORKSPACE_FILE_LIMIT],
                    )
                    .await?;
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = update {
                self.conn
                    .execute("ROLLBACK", ())
                    .await
                    .context("rolling back recent workspace file update")?;
                return Err(error);
            }
            self.conn.execute("COMMIT", ()).await?;
            Ok::<(), anyhow::Error>(())
        })
        .context("Failed to record recent workspace file")
    }

    pub fn recent_workspace_files(&self) -> Result<Vec<String>> {
        self.block_on(async {
            let mut rows = self
                .conn
                .query(
                    "SELECT path FROM recent_workspace_files
                     ORDER BY opened_at DESC, path ASC LIMIT ?1",
                    params![RECENT_WORKSPACE_FILE_LIMIT],
                )
                .await?;
            let mut paths = Vec::new();
            while let Some(row) = rows.next().await? {
                paths.push(row.get::<String>(0)?);
            }
            Ok::<Vec<String>, anyhow::Error>(paths)
        })
        .context("Failed to read recent workspace files")
    }

    pub fn write_editor_recovery(
        &self,
        owner: &str,
        snapshot: &EditorRecoverySnapshot,
    ) -> Result<bool> {
        uuid::Uuid::parse_str(owner).context("invalid editor recovery owner")?;
        uuid::Uuid::parse_str(&snapshot.buffer_id)
            .context("invalid editor recovery buffer ID")?;
        let revision = i64::try_from(snapshot.revision)
            .context("editor recovery revision exceeds storage range")?;
        anyhow::ensure!(revision > 0, "editor recovery revision must be positive");
        let path = snapshot
            .path
            .to_str()
            .context("recovery path is not UTF-8")?;
        anyhow::ensure!(
            !path.is_empty()
                && !path.contains(['\0', '\\'])
                && snapshot
                    .path
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_))),
            "editor recovery requires a workspace-relative file path"
        );
        anyhow::ensure!(
            snapshot
                .contents
                .as_ref()
                .is_none_or(|text| text.len() <= MAX_EDITOR_RECOVERY_BYTES),
            "editor recovery exceeds the 32 MiB per-buffer limit"
        );
        anyhow::ensure!(
            snapshot
                .saved_sha256
                .as_ref()
                .is_none_or(|hash| hash.len() == 64
                    && hash.bytes().all(|byte| byte.is_ascii_hexdigit())),
            "invalid saved-file digest"
        );
        self.block_on(async {
            let changed = self.conn.execute(
                "INSERT INTO editor_recoveries(owner_id, buffer_id, revision, path, contents, saved_sha256)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(buffer_id) DO UPDATE SET
                    revision = excluded.revision, path = excluded.path,
                    contents = excluded.contents, saved_sha256 = excluded.saved_sha256
                 WHERE editor_recoveries.owner_id = excluded.owner_id
                   AND (excluded.revision > editor_recoveries.revision
                     OR (excluded.revision = editor_recoveries.revision
                       AND excluded.path = editor_recoveries.path
                       AND excluded.contents IS editor_recoveries.contents
                       AND excluded.saved_sha256 IS editor_recoveries.saved_sha256))",
                params![owner, snapshot.buffer_id.clone(), revision, path, snapshot.contents.clone(), snapshot.saved_sha256.clone()],
            ).await?;
            Ok::<_, anyhow::Error>(changed == 1)
        }).context("Failed to persist editor recovery")
    }

    pub fn editor_recovery_owners(&self) -> Result<Vec<String>> {
        self.block_on(async {
            let mut rows = self.conn.query(
                "SELECT DISTINCT owner_id FROM editor_recoveries ORDER BY owner_id", (),
            ).await?;
            let mut owners = Vec::new();
            while let Some(row) = rows.next().await? {
                owners.push(row.get::<String>(0)?);
            }
            Ok::<_, anyhow::Error>(owners)
        })
    }

    /// Caller must hold the abandoned owner's OS lock until this transaction commits.
    pub fn claim_editor_recoveries(
        &self,
        previous_owner: &str,
        owner: &str,
    ) -> Result<()> {
        anyhow::ensure!(
            previous_owner != owner,
            "cannot reclaim the current editor owner"
        );
        uuid::Uuid::parse_str(previous_owner)
            .context("invalid previous editor owner")?;
        uuid::Uuid::parse_str(owner).context("invalid editor owner")?;
        self.block_on(async {
            let transaction = self.conn.transaction_with_behavior(libsql::TransactionBehavior::Immediate).await?;
            transaction.execute(
                "DELETE FROM editor_recoveries WHERE owner_id = ?1 AND contents IS NULL",
                params![previous_owner],
            ).await?;
            transaction.execute(
                "UPDATE editor_recoveries SET owner_id = ?1 WHERE owner_id = ?2",
                params![owner, previous_owner],
            ).await?;
            transaction.commit().await?;
            Ok::<_, anyhow::Error>(())
        }).context("Failed to claim abandoned editor recovery")
    }

    pub fn editor_recovery_summaries(
        &self,
        owner: &str,
    ) -> Result<Vec<EditorRecoverySummary>> {
        self.block_on(async {
            let mut rows = self.conn.query(
                "SELECT buffer_id, path, revision FROM editor_recoveries
                 WHERE owner_id = ?1 AND contents IS NOT NULL ORDER BY path, buffer_id",
                params![owner],
            ).await?;
            let mut summaries = Vec::new();
            while let Some(row) = rows.next().await? {
                summaries.push(EditorRecoverySummary {
                    buffer_id: row.get(0)?,
                    path: row.get::<String>(1)?.into(),
                    revision: u64::try_from(row.get::<i64>(2)?)?,
                });
            }
            Ok::<_, anyhow::Error>(summaries)
        })
    }

    pub fn editor_recovery(
        &self,
        owner: &str,
        buffer_id: &str,
    ) -> Result<Option<EditorRecoverySnapshot>> {
        self.block_on(async {
            let mut rows = self.conn.query(
                "SELECT revision, path, contents, saved_sha256 FROM editor_recoveries
                 WHERE owner_id = ?1 AND buffer_id = ?2 AND contents IS NOT NULL",
                params![owner, buffer_id],
            ).await?;
            let Some(row) = rows.next().await? else {
                return Ok(None);
            };
            Ok::<_, anyhow::Error>(Some(EditorRecoverySnapshot {
                buffer_id: buffer_id.to_owned(),
                revision: u64::try_from(row.get::<i64>(0)?)?,
                path: row.get::<String>(1)?.into(),
                contents: row.get(2)?,
                saved_sha256: row.get(3)?,
            }))
        })
    }

    fn block_on<F>(&self, future: F) -> F::Output
    where
        F: std::future::Future + Send,
        F::Output: Send,
    {
        // AHEAD has one libSQL connection; a write must not enter another
        // thread's multi-statement transaction on that same connection.
        // ponytail: serialize reads too until measured contention warrants
        // separate read connections.
        let _access = self.access.lock();
        match tokio::runtime::Handle::try_current() {
            Ok(handle)
                if handle.runtime_flavor()
                    == tokio::runtime::RuntimeFlavor::MultiThread =>
            {
                // Native agent turns call this store from a Tokio worker.
                tokio::task::block_in_place(|| self.rt.block_on(future))
            }
            Ok(_) => std::thread::scope(|scope| {
                match scope.spawn(move || self.rt.block_on(future)).join() {
                    Ok(output) => output,
                    Err(panic) => std::panic::resume_unwind(panic),
                }
            }),
            Err(_) => self.rt.block_on(future),
        }
    }

    fn init_schema(&self) -> Result<()> {
        self.block_on(async {
            // Lock before inspecting the header so two first opens cannot both
            // decide to initialize the same database.
            let transaction = self.conn
                .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
                .await?;
            match session_schema_is_current(&transaction).await {
                Ok(true) => { transaction.rollback().await?; return Ok(()); }
                Ok(false) => {}
                Err(error) => { transaction.rollback().await?; return Err(error); }
            }
            transaction.execute_batch(
                "
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    project_id TEXT NOT NULL,
                    worktree_id TEXT NOT NULL,
                    work_kind TEXT NOT NULL,
                    title TEXT NOT NULL,
                    owner_id TEXT NOT NULL,
                    lifecycle_json TEXT NOT NULL,
                    policy_json TEXT NOT NULL,
                    revision INTEGER NOT NULL,
                    created_at TEXT NOT NULL
                );

                CREATE TABLE recent_workspace_files (
                    path TEXT PRIMARY KEY,
                    opened_at INTEGER NOT NULL
                );

                CREATE TABLE editor_recoveries (
                    buffer_id TEXT PRIMARY KEY,
                    owner_id TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK(revision > 0),
                    path TEXT NOT NULL,
                    contents TEXT,
                    saved_sha256 TEXT
                );
                CREATE INDEX editor_recoveries_owner ON editor_recoveries(owner_id);

                CREATE TABLE archived_sessions (
                    session_id TEXT PRIMARY KEY,
                    archived_at TEXT NOT NULL,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE session_tasks (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    intent TEXT NOT NULL,
                    work_kind TEXT NOT NULL,
                    title TEXT NOT NULL,
                    objective TEXT NOT NULL DEFAULT '',
                    parent_task_id TEXT,
                    learning_arc_id TEXT,
                    created_at TEXT NOT NULL,
                    completed_at TEXT,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE learning_arcs (
                    id TEXT PRIMARY KEY,
                    task_id TEXT NOT NULL,
                    mission TEXT NOT NULL,
                    current_concept_id TEXT,
                    state TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    FOREIGN KEY (task_id) REFERENCES session_tasks(id)
                );

                CREATE TABLE learning_records (
                    id TEXT PRIMARY KEY,
                    arc_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    content TEXT NOT NULL,
                    source_refs_json TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY (arc_id) REFERENCES learning_arcs(id)
                );

                CREATE TABLE workflow_state (
                    session_id TEXT PRIMARY KEY,
                    revision INTEGER NOT NULL,
                    definition_version TEXT NOT NULL,
                    phase_id TEXT NOT NULL,
                    phase_title TEXT NOT NULL,
                    phase_visit INTEGER NOT NULL,
                    primary_work_item_json TEXT,
                    current_artifact_ids_json TEXT NOT NULL,
                    approvals_json TEXT NOT NULL,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE participants (
                    id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    participant_json TEXT NOT NULL,
                    role TEXT NOT NULL,
                    PRIMARY KEY (id, session_id),
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE session_events (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    sequence INTEGER NOT NULL,
                    request_id TEXT NOT NULL,
                    event_type TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    actor_id TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE anchors (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    actor_id TEXT NOT NULL DEFAULT 'human',
                    path TEXT NOT NULL,
                    start_line INTEGER NOT NULL,
                    start_col INTEGER NOT NULL,
                    end_line INTEGER NOT NULL,
                    end_col INTEGER NOT NULL,
                    quote_hash TEXT NOT NULL,
                    surrounding_context TEXT,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE work_items (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    status TEXT NOT NULL,
                    position INTEGER NOT NULL,
                    created_by TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    closed_at TEXT,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE work_item_events (
                    id TEXT PRIMARY KEY,
                    item_id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    body_markdown TEXT NOT NULL,
                    actor_id TEXT NOT NULL,
                    anchor_id TEXT,
                    artifact_id TEXT,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE work_item_closeouts (
                    item_id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    summary_markdown TEXT NOT NULL,
                    issue_ref TEXT,
                    follow_ups_json TEXT NOT NULL,
                    conversation_summary_id TEXT,
                    closed_at TEXT NOT NULL,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE conversation_summaries (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    phase TEXT NOT NULL,
                    summary_markdown TEXT NOT NULL,
                    message_id_range TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE conversation_messages (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    turn_id TEXT NOT NULL,
                    sequence INTEGER NOT NULL,
                    role TEXT NOT NULL,
                    actor_id TEXT NOT NULL,
                    content TEXT NOT NULL,
                    status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE harness_runtime_state (
                    session_id TEXT PRIMARY KEY,
                    state_json TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE harness_bindings (
                    session_id TEXT PRIMARY KEY,
                    acp_session_id TEXT NOT NULL,
                    backend TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE agent_runtime_threads (
                    thread_id TEXT PRIMARY KEY,
                    session_id TEXT,
                    create_params_json TEXT NOT NULL,
                    archived INTEGER NOT NULL DEFAULT 0,
                    archived_at TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE agent_runtime_thread_items (
                    thread_id TEXT NOT NULL,
                    ordinal INTEGER NOT NULL,
                    item_json TEXT NOT NULL,
                    PRIMARY KEY (thread_id, ordinal),
                    FOREIGN KEY (thread_id) REFERENCES agent_runtime_threads(thread_id) ON DELETE CASCADE
                );

                CREATE TABLE agent_runtime_thread_metadata (
                    thread_id TEXT NOT NULL,
                    ordinal INTEGER NOT NULL,
                    patch_json TEXT NOT NULL,
                    PRIMARY KEY (thread_id, ordinal),
                    FOREIGN KEY (thread_id) REFERENCES agent_runtime_threads(thread_id) ON DELETE CASCADE
                );

                CREATE TABLE agent_spawn_edges (
                    child_thread_id TEXT PRIMARY KEY,
                    parent_thread_id TEXT NOT NULL,
                    status TEXT NOT NULL CHECK (status IN ('open', 'closed'))
                );

                CREATE TABLE harness_turn_requests (
                    turn_id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    request_json TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE turn_instruction_sources (
                    session_id TEXT NOT NULL,
                    turn_id TEXT NOT NULL,
                    ordinal INTEGER NOT NULL,
                    source_path TEXT NOT NULL,
                    content_sha256 TEXT NOT NULL,
                    targets_json TEXT NOT NULL,
                    PRIMARY KEY (turn_id, ordinal),
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE memory_revisions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    scope TEXT NOT NULL,
                    path TEXT NOT NULL,
                    content_sha256 TEXT NOT NULL,
                    content TEXT NOT NULL,
                    indexed_at TEXT NOT NULL,
                    UNIQUE(scope, path, content_sha256)
                );

                CREATE TABLE memory_sources (
                    scope TEXT NOT NULL,
                    path TEXT NOT NULL,
                    current_sha256 TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (scope, path)
                );

                CREATE TABLE memory_search_lines (
                    scope TEXT NOT NULL,
                    path TEXT NOT NULL,
                    content_sha256 TEXT NOT NULL,
                    line INTEGER NOT NULL,
                    content TEXT NOT NULL,
                    PRIMARY KEY (scope, path, content_sha256, line)
                );

                CREATE TABLE memory_search_terms (
                    scope TEXT NOT NULL,
                    path TEXT NOT NULL,
                    content_sha256 TEXT NOT NULL,
                    line INTEGER NOT NULL,
                    term TEXT NOT NULL,
                    PRIMARY KEY (scope, path, content_sha256, line, term)
                );

                CREATE INDEX idx_items_session ON work_items(session_id, position);
                CREATE INDEX idx_item_events ON work_item_events(item_id);
                CREATE INDEX idx_summaries_session ON conversation_summaries(session_id);
                CREATE INDEX idx_messages_session_sequence_id ON conversation_messages(session_id, sequence, id);
                CREATE INDEX idx_messages_session_created_at ON conversation_messages(session_id, created_at DESC);
                CREATE INDEX idx_memory_revisions_source ON memory_revisions(scope, path, id);
                CREATE INDEX idx_memory_search_terms_term
                    ON memory_search_terms(term, scope, path, content_sha256, line);
                CREATE INDEX idx_agent_runtime_threads_session ON agent_runtime_threads(session_id);
                CREATE INDEX idx_agent_runtime_threads_archived_created ON agent_runtime_threads(archived, created_at, thread_id);
                CREATE INDEX idx_agent_runtime_threads_archived_updated ON agent_runtime_threads(archived, updated_at, thread_id);
                CREATE INDEX idx_agent_runtime_threads_source_created
                    ON agent_runtime_threads(archived, json_extract(create_params_json, '$.source'), created_at, thread_id);
                CREATE INDEX idx_agent_runtime_threads_source_updated
                    ON agent_runtime_threads(archived, json_extract(create_params_json, '$.source'), updated_at, thread_id);
                CREATE INDEX idx_agent_runtime_threads_provider_created
                    ON agent_runtime_threads(archived, json_extract(create_params_json, '$.metadata.model_provider'), created_at, thread_id);
                CREATE INDEX idx_agent_runtime_threads_provider_updated
                    ON agent_runtime_threads(archived, json_extract(create_params_json, '$.metadata.model_provider'), updated_at, thread_id);
                CREATE INDEX idx_agent_runtime_threads_cwd_created
                    ON agent_runtime_threads(archived, json_extract(create_params_json, '$.metadata.cwd'), created_at, thread_id);
                CREATE INDEX idx_agent_runtime_threads_cwd_updated
                    ON agent_runtime_threads(archived, json_extract(create_params_json, '$.metadata.cwd'), updated_at, thread_id);
                CREATE INDEX idx_agent_runtime_threads_parent
                    ON agent_runtime_threads(json_extract(create_params_json, '$.parent_thread_id'), thread_id);
                CREATE INDEX idx_agent_spawn_edges_parent ON agent_spawn_edges(parent_thread_id, status, child_thread_id);
                "
            ).await?;
            transaction.execute_batch(&format!(
                "PRAGMA application_id = {SESSION_APPLICATION_ID};
                 PRAGMA user_version = {SESSION_SCHEMA_VERSION};"
            )).await?;
            transaction.commit().await?;
            Ok::<(), anyhow::Error>(())
        }).context("Failed to initialize AHEAD Turso/libsql schema")
    }

    /// Synchronizes one human-readable AHEAD memory file into the local search
    /// index. The Markdown file remains authoritative; the database retains
    /// immutable content-hash revisions and a pointer to the current one.
    pub fn sync_memory_file(
        &self,
        scope: &str,
        path: &Path,
    ) -> Result<Option<String>> {
        if !matches!(scope, "project" | "user") {
            bail!("memory scope must be `project` or `user`");
        }
        let source_path = path;
        let path = path.to_string_lossy().to_string();
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = match options.open(source_path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                self.block_on(async {
                    self.conn
                        .execute(
                            "DELETE FROM memory_sources WHERE scope = ?1 AND path = ?2",
                            params![scope.to_string(), path],
                        )
                        .await
                })?;
                return Ok(None);
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to read memory file {path}"));
            }
        };
        if !file.metadata()?.file_type().is_file() {
            bail!("AHEAD memory source must be a regular file");
        }
        const MEMORY_DOCUMENT_LIMIT: u64 =
            ahead_rpc::ahead::MEMORY_DOCUMENT_MAX_BYTES as u64;
        if file.metadata()?.len() > MEMORY_DOCUMENT_LIMIT {
            bail!("AHEAD memory document exceeds the 32 KiB limit");
        }
        let mut content = String::new();
        (&mut file)
            .take(MEMORY_DOCUMENT_LIMIT + 1)
            .read_to_string(&mut content)
            .with_context(|| format!("Failed to read memory file {path}"))?;
        if content.len() as u64 > MEMORY_DOCUMENT_LIMIT {
            bail!("AHEAD memory document exceeds the 32 KiB limit");
        }
        let content_sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
        let indexed_at = chrono::Utc::now().to_rfc3339();
        let lines = content
            .lines()
            .enumerate()
            .map(|(index, line)| {
                let terms = if line.trim().starts_with("<!-- AHEAD memory source:") {
                    BTreeSet::new()
                } else {
                    line.to_lowercase()
                        .split(|character: char| !character.is_alphanumeric())
                        .filter(|term| !term.is_empty())
                        .map(str::to_string)
                        .collect::<BTreeSet<_>>()
                };
                (index as i64 + 1, line.to_string(), terms)
            })
            .collect::<Vec<_>>();
        self.block_on(async {
            self.conn.execute("BEGIN IMMEDIATE", ()).await?;
            let index_memory = async {
                self.conn.execute(
                    "INSERT OR IGNORE INTO memory_revisions
                     (scope, path, content_sha256, content, indexed_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        scope.to_string(),
                        path.clone(),
                        content_sha256.clone(),
                        content,
                        indexed_at.clone(),
                    ],
                )
                .await?;
                for (line_number, line, terms) in &lines {
                    self.conn.execute(
                        "INSERT OR IGNORE INTO memory_search_lines
                         (scope, path, content_sha256, line, content)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            scope.to_string(),
                            path.clone(),
                            content_sha256.clone(),
                            *line_number,
                            line.clone(),
                        ],
                    ).await?;
                    for term in terms {
                        self.conn.execute(
                            "INSERT OR IGNORE INTO memory_search_terms
                             (scope, path, content_sha256, line, term)
                             VALUES (?1, ?2, ?3, ?4, ?5)",
                            params![
                                scope.to_string(),
                                path.clone(),
                                content_sha256.clone(),
                                *line_number,
                                term.clone(),
                            ],
                        ).await?;
                    }
                }
                self.conn.execute(
                    "INSERT INTO memory_sources (scope, path, current_sha256, updated_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(scope, path) DO UPDATE SET
                        current_sha256 = excluded.current_sha256,
                        updated_at = excluded.updated_at",
                    params![
                        scope.to_string(),
                        path,
                        content_sha256.clone(),
                        indexed_at,
                    ],
                )
                .await?;
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = index_memory {
                self.conn
                    .execute("ROLLBACK", ())
                    .await
                    .context("rolling back AHEAD memory indexing")?;
                return Err(error);
            }
            self.conn.execute("COMMIT", ()).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(Some(content_sha256))
    }

    pub fn search_memory(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemorySearchHit>> {
        let query = query.trim().to_lowercase();
        let terms: Vec<String> = query
            .split(|character: char| !character.is_alphanumeric())
            .filter(|term| !term.is_empty())
            .map(str::to_string)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let limit = limit.min(50);
        if terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let current = self.block_on(async {
            let mut matches =
                HashMap::<(String, String, String, usize), (String, usize)>::new();
            for term in &terms {
                let allow_prefix = term.chars().count() >= 3;
                let prefix = format!("{term}%");
                let mut rows = self
                    .conn
                    .query(
                        "SELECT lines.scope, lines.path, lines.content_sha256,
                            lines.line, lines.content, MAX(terms.term = ?1)
                     FROM memory_search_lines AS lines
                     JOIN memory_sources AS sources
                       ON sources.scope = lines.scope
                      AND sources.path = lines.path
                      AND sources.current_sha256 = lines.content_sha256
                     JOIN memory_search_terms AS terms
                       ON terms.scope = lines.scope
                      AND terms.path = lines.path
                      AND terms.content_sha256 = lines.content_sha256
                      AND terms.line = lines.line
                     WHERE terms.term = ?1
                        OR (?2 = 1 AND terms.term LIKE ?3)
                     GROUP BY lines.scope, lines.path, lines.content_sha256,
                              lines.line, lines.content",
                        params![
                            term.clone(),
                            if allow_prefix { 1_i64 } else { 0_i64 },
                            prefix
                        ],
                    )
                    .await?;
                while let Some(row) = rows.next().await? {
                    let key = (
                        row.get::<String>(0)?,
                        row.get::<String>(1)?,
                        row.get::<String>(2)?,
                        usize::try_from(row.get::<i64>(3)?)?,
                    );
                    let exact_match = row.get::<i64>(5)? != 0;
                    let relevance = if exact_match { 10 } else { 6 };
                    let entry =
                        matches.entry(key).or_insert((row.get::<String>(4)?, 0));
                    entry.1 += relevance;
                }
            }
            Ok::<_, anyhow::Error>(matches)
        })?;

        let mut hits = current
            .into_iter()
            .map(
                |((scope, path, content_sha256, line), (content, relevance))| {
                    let normalized_line = content.to_lowercase();
                    let phrase_bonus = if normalized_line.contains(query.as_str()) {
                        terms.len()
                    } else {
                        0
                    };
                    let trimmed_line = content.trim();
                    let mut excerpt: String =
                        trimmed_line.chars().take(320).collect();
                    if trimmed_line.chars().count() > 320 {
                        excerpt.push('…');
                    }
                    MemorySearchHit {
                        scope,
                        path,
                        content_sha256,
                        line,
                        excerpt,
                        relevance: relevance + phrase_bonus,
                    }
                },
            )
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .relevance
                .cmp(&left.relevance)
                .then_with(|| left.scope.cmp(&right.scope))
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.line.cmp(&right.line))
        });
        hits.truncate(limit);
        Ok(hits)
    }

    pub fn list_memory_revisions(
        &self,
        scope: &str,
        path: &Path,
    ) -> Result<Vec<MemoryRevision>> {
        let path = path.to_string_lossy().to_string();
        self.block_on(async {
            let mut rows = self
                .conn
                .query(
                    "SELECT scope, path, content_sha256, indexed_at
                 FROM memory_revisions
                 WHERE scope = ?1 AND path = ?2
                 ORDER BY id",
                    params![scope.to_string(), path],
                )
                .await?;
            let mut revisions = Vec::new();
            while let Some(row) = rows.next().await? {
                revisions.push(MemoryRevision {
                    scope: row.get(0)?,
                    path: row.get(1)?,
                    content_sha256: row.get(2)?,
                    indexed_at: row.get(3)?,
                });
            }
            Ok::<Vec<MemoryRevision>, anyhow::Error>(revisions)
        })
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionListItem>> {
        self.block_on(async {
            let mut rows = self
                .conn
                .query(
                    "SELECT sessions.id, sessions.title, sessions.lifecycle_json,
                            sessions.created_at, harness_bindings.backend,
                            MAX(sessions.created_at, COALESCE((
                                SELECT MAX(messages.created_at)
                                FROM conversation_messages AS messages
                                WHERE messages.session_id = sessions.id
                            ), sessions.created_at)) AS updated_at,
                            (SELECT parent.session_id
                             FROM session_tasks AS child
                             JOIN session_tasks AS parent ON parent.id = child.parent_task_id
                             WHERE child.session_id = sessions.id LIMIT 1) AS parent_session_id
                     FROM sessions
                     JOIN workflow_state
                       ON workflow_state.session_id = sessions.id
                     LEFT JOIN harness_bindings
                       ON harness_bindings.session_id = sessions.id
                     LEFT JOIN archived_sessions
                       ON archived_sessions.session_id = sessions.id
                     WHERE archived_sessions.session_id IS NULL
                       AND EXISTS (
                           SELECT 1 FROM session_tasks
                           WHERE session_tasks.session_id = sessions.id
                       )
                     ORDER BY updated_at DESC, sessions.id DESC",
                    (),
                )
                .await?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().await? {
                let id: String = row.get(0)?;
                let item = (|| -> Result<SessionListItem> {
                    Ok(SessionListItem {
                        id: id.clone(),
                        title: row.get(1)?,
                        lifecycle: serde_json::from_str(&row.get::<String>(2)?)?,
                        created_at: row.get(3)?,
                        backend: row.get(4)?,
                        updated_at: row.get(5)?,
                        parent_session_id: row.get(6)?,
                    })
                })();
                match item {
                    Ok(item) => out.push(item),
                    Err(error) => eprintln!(
                        "Skipping unreadable durable AHEAD session {id}: {error:#}"
                    ),
                }
            }
            Ok::<Vec<SessionListItem>, anyhow::Error>(out)
        })
    }

    pub fn archive_session(&self, session_id: &str) -> Result<()> {
        let archived_at = chrono::Utc::now().to_rfc3339();
        let changed = self.block_on(async {
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO archived_sessions (session_id, archived_at)
                     SELECT id, ?1 FROM sessions WHERE id = ?2",
                    params![archived_at, session_id.to_string()],
                )
                .await
        })?;
        if changed == 0 && self.get_session(session_id)?.is_none() {
            bail!("Session not found: {session_id}");
        }
        Ok(())
    }

    pub fn insert_session(&mut self, view: &SessionView) -> Result<()> {
        self.insert_session_inner(view, None)
    }

    pub(crate) fn insert_session_with_harness_binding(
        &mut self,
        view: &SessionView,
        backend: &str,
    ) -> Result<()> {
        self.insert_session_inner(view, Some(backend))
    }

    #[cfg(test)]
    pub(crate) fn set_harness_binding_failure_for_test(
        &self,
        fail: bool,
    ) -> Result<()> {
        let sql = if fail {
            "CREATE TRIGGER reject_test_harness_binding
             BEFORE INSERT ON harness_bindings
             BEGIN SELECT RAISE(ABORT, 'injected harness binding failure'); END;"
        } else {
            "DROP TRIGGER IF EXISTS reject_test_harness_binding;"
        };
        self.block_on(self.conn.execute_batch(sql))?;
        Ok(())
    }

    fn insert_session_inner(
        &mut self,
        view: &SessionView,
        initial_backend: Option<&str>,
    ) -> Result<()> {
        let session = &view.session;
        let lifecycle_json = serde_json::to_string(&session.lifecycle)?;
        let policy_json = serde_json::to_string(&session.policy)?;
        let work_kind_str =
            serde_json::to_string(&session.work_kind)?.replace('\"', "");
        let task = &view.task;
        let task_intent_str = serde_json::to_string(&task.intent)?.replace('\"', "");
        let task_work_kind_str =
            serde_json::to_string(&task.work_kind)?.replace('\"', "");
        let wf = &view.workflow;
        let item_json = wf
            .primary_work_item
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let artifacts_json = serde_json::to_string(&wf.current_artifact_ids)?;
        let approvals_json = serde_json::to_string(&wf.approvals)?;

        let mut participant_tuples = Vec::new();
        for p_rec in &view.participants {
            let p_json = serde_json::to_string(&p_rec.participant)?;
            let role_str = serde_json::to_string(&p_rec.role)?.replace('\"', "");
            participant_tuples.push((p_rec.participant.id(), p_json, role_str));
        }

        self.block_on(async {
            let transaction = self
                .conn
                .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
                .await?;
            let result = async {
                transaction.execute(
                "INSERT INTO sessions (id, project_id, worktree_id, work_kind, title, owner_id, lifecycle_json, policy_json, revision, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    session.id.clone(),
                    session.project_id.clone(),
                    session.worktree_id.clone(),
                    work_kind_str,
                    session.title.clone(),
                    session.owner_id.clone(),
                    lifecycle_json,
                    policy_json,
                    session.revision.cast_signed(),
                    session.created_at.clone(),
                ],
            ).await?;

                transaction.execute(
                "INSERT INTO session_tasks (id, session_id, intent, work_kind, title, objective, parent_task_id, learning_arc_id, created_at, completed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    task.id.clone(),
                    task.session_id.clone(),
                    task_intent_str,
                    task_work_kind_str,
                    task.title.clone(),
                    task.objective.clone(),
                    task.parent_task_id.clone(),
                    task.learning_arc_id.clone(),
                    task.created_at.clone(),
                    task.completed_at.clone(),
                ],
            ).await?;

            if let Some(arc) = &view.learning_arc {
                    transaction.execute(
                    "INSERT INTO learning_arcs (id, task_id, mission, current_concept_id, state, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        arc.id.clone(),
                        arc.task_id.clone(),
                        arc.mission.clone(),
                        arc.current_concept_id.clone(),
                        arc.state.clone(),
                        arc.created_at.clone(),
                        arc.updated_at.clone(),
                    ],
                ).await?;
                for record in &arc.records {
                        transaction.execute(
                        "INSERT INTO learning_records (id, arc_id, kind, content, source_refs_json, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            record.id.clone(),
                            record.arc_id.clone(),
                            record.kind.clone(),
                            record.content.clone(),
                            serde_json::to_string(&record.source_refs)?,
                            record.created_at.clone(),
                        ],
                    ).await?;
                }
            }

                transaction.execute(
                "INSERT INTO workflow_state (session_id, revision, definition_version, phase_id, phase_title, phase_visit, primary_work_item_json, current_artifact_ids_json, approvals_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    session.id.clone(),
                    wf.revision.cast_signed(),
                    wf.definition_version.clone(),
                    wf.phase.id.clone(),
                    wf.phase.title.clone(),
                    i64::from(wf.phase.visit),
                    item_json,
                    artifacts_json,
                    approvals_json,
                ],
            ).await?;

                for (p_id, p_json, role_str) in participant_tuples {
                    transaction.execute(
                    "INSERT INTO participants (id, session_id, participant_json, role) VALUES (?1, ?2, ?3, ?4)",
                    params![p_id, session.id.clone(), p_json, role_str],
                ).await?;
            }

                if let Some(backend) = initial_backend {
                    transaction
                        .execute(
                        "INSERT INTO harness_bindings
                            (session_id, acp_session_id, backend, updated_at)
                         VALUES (?1, '', ?2, ?3)",
                        params![
                            session.id.clone(),
                            backend.to_string(),
                            chrono::Utc::now().to_rfc3339(),
                        ],
                        )
                        .await?;
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;
            match result {
                Ok(()) => transaction.commit().await?,
                Err(error) => {
                    transaction
                        .rollback()
                        .await
                        .context("rolling back AHEAD session creation")?;
                    return Err(error);
                }
            }
            Ok::<(), anyhow::Error>(())
        })?;

        Ok(())
    }

    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionView>> {
        self.block_on(async {
            let mut rows = self.conn.query(
                "SELECT id, project_id, worktree_id, work_kind, title, owner_id, lifecycle_json, policy_json, revision, created_at
                 FROM sessions WHERE id = ?1",
                params![session_id],
            ).await?;

            let Some(row) = rows.next().await? else {
                return Ok(None);
            };

            let id: String = row.get(0)?;
            let project_id: String = row.get(1)?;
            let worktree_id: String = row.get(2)?;
            let work_kind_str: String = row.get(3)?;
            let title: String = row.get(4)?;
            let owner_id: String = row.get(5)?;
            let lifecycle_json: String = row.get(6)?;
            let policy_json: String = row.get(7)?;
            let revision: i64 = row.get(8)?;
            let created_at: String = row.get(9)?;

            let work_kind: WorkKind = serde_json::from_str(&format!("\"{work_kind_str}\""))?;
            let lifecycle: SessionLifecycle = serde_json::from_str(&lifecycle_json)?;
            let policy: SessionPolicySnapshot = serde_json::from_str(&policy_json)?;

            let session = WorkSession {
                id,
                project_id,
                worktree_id,
                work_kind,
                title,
                owner_id,
                lifecycle,
                policy,
                revision: revision.cast_unsigned(),
                created_at,
            };

            let mut wf_rows = self.conn.query(
                "SELECT revision, definition_version, phase_id, phase_title, phase_visit, primary_work_item_json, current_artifact_ids_json, approvals_json
                 FROM workflow_state WHERE session_id = ?1",
                params![session_id],
            ).await?;

            let Some(wf_row) = wf_rows.next().await? else {
                bail!("Session exists but workflow_state is missing: {}", session_id);
            };

            let wf_rev: i64 = wf_row.get(0)?;
            let def_ver: String = wf_row.get(1)?;
            let phase_id: String = wf_row.get(2)?;
            let phase_title: String = wf_row.get(3)?;
            let phase_visit: i64 = wf_row.get(4)?;
            let item_json: Option<String> = wf_row.get(5)?;
            let artifacts_json: String = wf_row.get(6)?;
            let approvals_json: String = wf_row.get(7)?;

            let primary_work_item: Option<GithubIssueRef> = item_json.as_deref().map(serde_json::from_str).transpose()?;
            let current_artifact_ids: Vec<Id> = serde_json::from_str(&artifacts_json)?;
            let approvals: Vec<ApprovalRecord> = serde_json::from_str(&approvals_json)?;

            let workflow = WorkflowState {
                revision: wf_rev.cast_unsigned(),
                definition_version: def_ver,
                phase: WorkflowPhase {
                    id: phase_id,
                    title: phase_title,
                    visit: u32::try_from(phase_visit).unwrap_or(u32::MAX),
                },
                primary_work_item,
                current_artifact_ids,
                approvals,
            };

            let mut task_rows = self.conn.query(
                "SELECT id, session_id, intent, work_kind, title, objective, parent_task_id, learning_arc_id, created_at, completed_at
                 FROM session_tasks WHERE session_id = ?1 ORDER BY created_at DESC LIMIT 1",
                params![session_id],
            ).await?;
            let Some(task_row) = task_rows.next().await? else {
                bail!("Session exists but session_tasks is missing: {}", session_id);
            };
            let task = SessionTask {
                id: task_row.get(0)?,
                session_id: task_row.get(1)?,
                intent: serde_json::from_str(&format!("\"{}\"", task_row.get::<String>(2)?))?,
                work_kind: serde_json::from_str(&format!("\"{}\"", task_row.get::<String>(3)?))?,
                title: task_row.get(4)?,
                objective: task_row.get(5)?,
                parent_task_id: task_row.get(6)?,
                learning_arc_id: task_row.get(7)?,
                created_at: task_row.get(8)?,
                completed_at: task_row.get(9)?,
            };

            let learning_arc = if let Some(arc_id) = &task.learning_arc_id {
                let mut arc_rows = self.conn.query(
                    "SELECT id, task_id, mission, current_concept_id, state, created_at, updated_at
                     FROM learning_arcs WHERE id = ?1",
                    params![arc_id.clone()],
                ).await?;
                let Some(arc_row) = arc_rows.next().await? else {
                    bail!("Learning arc {} is missing", arc_id);
                };
                let mut record_rows = self.conn.query(
                    "SELECT id, arc_id, kind, content, source_refs_json, created_at
                     FROM learning_records WHERE arc_id = ?1 ORDER BY created_at ASC",
                    params![arc_id.clone()],
                ).await?;
                let mut records = Vec::new();
                while let Some(record_row) = record_rows.next().await? {
                    records.push(LearningRecord {
                        id: record_row.get(0)?,
                        arc_id: record_row.get(1)?,
                        kind: record_row.get(2)?,
                        content: record_row.get(3)?,
                        source_refs: serde_json::from_str(&record_row.get::<String>(4)?)?,
                        created_at: record_row.get(5)?,
                    });
                }
                Some(LearningArc {
                    id: arc_row.get(0)?,
                    task_id: arc_row.get(1)?,
                    mission: arc_row.get(2)?,
                    current_concept_id: arc_row.get(3)?,
                    state: arc_row.get(4)?,
                    records,
                    created_at: arc_row.get(5)?,
                    updated_at: arc_row.get(6)?,
                })
            } else {
                None
            };

            let mut p_rows = self.conn.query(
                "SELECT participant_json, role FROM participants WHERE session_id = ?1",
                params![session_id],
            ).await?;

            let mut participants = Vec::new();
            while let Some(p_row) = p_rows.next().await? {
                let p_json: String = p_row.get(0)?;
                let role_str: String = p_row.get(1)?;
                let participant: Participant = serde_json::from_str(&p_json)?;
                let role: SessionRole = serde_json::from_str(&format!("\"{role_str}\""))?;
                participants.push(SessionParticipantRecord { participant, role });
            }

            Ok(Some(SessionView {
                session,
                task,
                learning_arc,
                workflow,
                participants,
            }))
        })
    }

    pub fn session_title(&self, session_id: &str) -> Result<Option<String>> {
        self.block_on(async {
            let mut rows = self
                .conn
                .query(
                    "SELECT title FROM sessions WHERE id = ?1",
                    params![session_id],
                )
                .await?;
            rows.next()
                .await?
                .map(|row| row.get(0))
                .transpose()
                .map_err(Into::into)
        })
    }

    pub fn update_session_title(&self, session_id: &str, title: &str) -> Result<()> {
        let title = title.trim();
        if title.is_empty() {
            bail!("session title cannot be empty");
        }
        self.block_on(async {
            self.conn.execute("BEGIN TRANSACTION", ()).await?;
            self.conn.execute(
                "UPDATE sessions SET title = ?1, revision = revision + 1 WHERE id = ?2",
                params![title.to_string(), session_id],
            ).await?;
            self.conn.execute(
                "UPDATE session_tasks SET title = ?1 WHERE session_id = ?2",
                params![title.to_string(), session_id],
            ).await?;
            self.conn.execute("COMMIT", ()).await?;
            Ok::<(), anyhow::Error>(())
        })
    }

    pub fn advance_workflow_phase(
        &mut self,
        session_id: &str,
        expected_revision: Revision,
        new_phase: WorkflowPhase,
    ) -> Result<WorkflowState> {
        self.block_on(async {
            self.conn.execute("BEGIN TRANSACTION", ()).await?;

            let mut rows = self.conn.query(
                "SELECT revision, definition_version, primary_work_item_json, current_artifact_ids_json, approvals_json
                 FROM workflow_state WHERE session_id = ?1",
                params![session_id],
            ).await?;

            let Some(row) = rows.next().await? else {
                self.conn.execute("ROLLBACK", ()).await?;
                bail!("Workflow state not found for session: {}", session_id);
            };

            let cur_rev: i64 = row.get(0)?;
            let def_ver: String = row.get(1)?;
            let item_json: Option<String> = row.get(2)?;
            let artifacts_json: String = row.get(3)?;
            let approvals_json: String = row.get(4)?;

            if (cur_rev.cast_unsigned()) != expected_revision {
                self.conn.execute("ROLLBACK", ()).await?;
                bail!("Revision mismatch: expected {}, found {}", expected_revision, cur_rev);
            }

            let next_rev = cur_rev + 1;

            self.conn.execute(
                "UPDATE workflow_state SET revision = ?1, phase_id = ?2, phase_title = ?3, phase_visit = ?4 WHERE session_id = ?5",
                params![next_rev, new_phase.id.clone(), new_phase.title.clone(), i64::from(new_phase.visit), session_id],
            ).await?;

            self.conn.execute(
                "UPDATE sessions SET revision = revision + 1 WHERE id = ?1",
                params![session_id],
            ).await?;

            let primary_work_item = item_json.as_deref().map(serde_json::from_str).transpose()?;
            let current_artifact_ids = serde_json::from_str(&artifacts_json)?;
            let approvals = serde_json::from_str(&approvals_json)?;

            self.conn.execute("COMMIT", ()).await?;

            Ok(WorkflowState {
                revision: next_rev.cast_unsigned(),
                definition_version: def_ver,
                phase: new_phase,
                primary_work_item,
                current_artifact_ids,
                approvals,
            })
        })
    }

    pub fn insert_anchor(&mut self, anchor: &CodeAnchor) -> Result<()> {
        self.block_on(async {
            self.conn.execute(
                "INSERT INTO anchors (id, session_id, actor_id, path, start_line, start_col, end_line, end_col, quote_hash, surrounding_context)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    anchor.id.clone(),
                    anchor.session_id.clone(),
                    anchor.actor_id.clone(),
                    anchor.path.clone(),
                    i64::from(anchor.range.start.line),
                    i64::from(anchor.range.start.col),
                    i64::from(anchor.range.end.line),
                    i64::from(anchor.range.end.col),
                    anchor.quote_hash.clone(),
                    anchor.surrounding_context.clone(),
                ],
            ).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    /// Records an attribution anchor, coalescing it into an existing
    /// uncommitted region owned by the same actor on the same path.
    ///
    /// Edits arrive as many small ranges; without coalescing the table would
    /// grow with every keystroke-batch instead of with the number of regions.
    /// Only forward extension is merged, so the surviving anchor keeps the
    /// `quote_hash` of its original start and stays a valid content anchor.
    pub fn record_edit_anchor(&mut self, anchor: &CodeAnchor) -> Result<()> {
        let existing = self.block_on(async {
            let mut rows = self
                .conn
                .query(
                    "SELECT id, end_line, end_col FROM anchors
                     WHERE session_id = ?1 AND path = ?2 AND actor_id = ?3
                       AND start_line <= ?4
                     ORDER BY start_line ASC",
                    params![
                        anchor.session_id.clone(),
                        anchor.path.clone(),
                        anchor.actor_id.clone(),
                        i64::from(anchor.range.start.line),
                    ],
                )
                .await?;

            let mut found: Option<(String, i64, i64)> = None;
            while let Some(row) = rows.next().await? {
                let id: String = row.get(0)?;
                let end_line: i64 = row.get(1)?;
                let end_col: i64 = row.get(2)?;
                // Adjacent or overlapping: the new region starts at or
                // before one line past the existing end.
                if i64::from(anchor.range.start.line) <= end_line + 1 {
                    found = Some((id, end_line, end_col));
                }
            }
            Ok::<Option<(String, i64, i64)>, anyhow::Error>(found)
        })?;

        if let Some((id, end_line, end_col)) = existing {
            let new_end_line = i64::from(anchor.range.end.line);
            let new_end_col = i64::from(anchor.range.end.col);
            let merged_end_line = new_end_line.max(end_line);
            let merged_end_col = match new_end_line.cmp(&end_line) {
                std::cmp::Ordering::Greater => new_end_col,
                std::cmp::Ordering::Less => end_col,
                std::cmp::Ordering::Equal => new_end_col.max(end_col),
            };
            self.block_on(async {
                self.conn.execute(
                    "UPDATE anchors SET end_line = ?1, end_col = ?2 WHERE id = ?3",
                    params![merged_end_line, merged_end_col, id],
                ).await?;
                Ok::<(), anyhow::Error>(())
            })?;
            return Ok(());
        }

        self.insert_anchor(anchor)
    }

    pub fn list_anchors(&self, session_id: &str) -> Result<Vec<CodeAnchor>> {
        self.query_anchors(
            "SELECT id, session_id, actor_id, path, start_line, start_col, end_line, end_col, quote_hash, surrounding_context
             FROM anchors WHERE session_id = ?1 ORDER BY start_line ASC",
            vec![session_id.to_string()],
        )
    }

    /// Uncommitted attribution anchors on the given paths, across sessions.
    pub fn list_anchors_for_paths(
        &self,
        paths: &[String],
    ) -> Result<Vec<CodeAnchor>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (0..paths.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, session_id, actor_id, path, start_line, start_col, end_line, end_col, quote_hash, surrounding_context
             FROM anchors WHERE path IN ({placeholders}) ORDER BY start_line ASC"
        );
        self.query_anchors(&sql, paths.to_vec())
    }

    /// Drops only uncommitted anchors whose quoted content is still present in
    /// the committed file. A human edit must leave its anchor intact rather
    /// than silently turning stale attribution into agent attribution.
    pub fn clear_anchors_for_paths(
        &mut self,
        paths: &[String],
        head_contents: &std::collections::HashMap<String, String>,
    ) -> Result<usize> {
        if paths.is_empty() {
            return Ok(0);
        }
        let placeholders = (0..paths.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, path, quote_hash, surrounding_context
             FROM anchors WHERE path IN ({placeholders})"
        );
        let candidates: Vec<(String, String, String, String)> =
            self.block_on(async {
                let mut rows = self
                    .conn
                    .query(&sql, params_from_iter(paths.to_vec()))
                    .await?;
                let mut out = Vec::new();
                while let Some(row) = rows.next().await? {
                    out.push((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get::<Option<String>>(3)?.unwrap_or_default(),
                    ));
                }
                Ok::<_, anyhow::Error>(out)
            })?;
        let ids: Vec<String> = candidates
            .into_iter()
            .filter(|(_, path, quote_hash, quote)| {
                let Some(content) = head_contents.get(path) else {
                    return false;
                };
                !quote.is_empty()
                    && content.contains(quote)
                    && format!(
                        "{:x}",
                        <sha2::Sha256 as sha2::Digest>::digest(quote.as_bytes())
                    ) == *quote_hash
            })
            .map(|(id, _, _, _)| id)
            .collect();
        let mut cleared = 0usize;
        for id in ids {
            cleared += usize::try_from(self.block_on(async {
                self.conn
                    .execute("DELETE FROM anchors WHERE id = ?1", params![id])
                    .await
            })?)
            .unwrap_or(usize::MAX);
        }
        Ok(cleared)
    }

    fn query_anchors(
        &self,
        sql: &str,
        args: Vec<String>,
    ) -> Result<Vec<CodeAnchor>> {
        self.block_on(async {
            let mut rows =
                self.conn.query(sql, params_from_iter(args.clone())).await?;

            let mut list = Vec::new();
            while let Some(row) = rows.next().await? {
                let id: String = row.get(0)?;
                let s_id: String = row.get(1)?;
                let actor_id: String = row.get(2)?;
                let path: String = row.get(3)?;
                let start_line: i64 = row.get(4)?;
                let start_col: i64 = row.get(5)?;
                let end_line: i64 = row.get(6)?;
                let end_col: i64 = row.get(7)?;
                let quote_hash: String = row.get(8)?;
                let surrounding_context: Option<String> = row.get(9)?;

                list.push(CodeAnchor {
                    id,
                    session_id: s_id,
                    actor_id,
                    path,
                    range: DisplayRange {
                        start: DisplayPosition {
                            line: u32::try_from(start_line).unwrap_or(u32::MAX),
                            col: u32::try_from(start_col).unwrap_or(u32::MAX),
                        },
                        end: DisplayPosition {
                            line: u32::try_from(end_line).unwrap_or(u32::MAX),
                            col: u32::try_from(end_col).unwrap_or(u32::MAX),
                        },
                    },
                    quote_hash,
                    surrounding_context,
                });
            }
            Ok(list)
        })
    }

    fn work_item_status_str(status: WorkItemStatus) -> &'static str {
        match status {
            WorkItemStatus::Open => "open",
            WorkItemStatus::InProgress => "in_progress",
            WorkItemStatus::Done => "done",
            WorkItemStatus::Dropped => "dropped",
        }
    }

    fn parse_work_item_status(raw: &str) -> Result<WorkItemStatus> {
        match raw {
            "open" => Ok(WorkItemStatus::Open),
            "in_progress" => Ok(WorkItemStatus::InProgress),
            "done" => Ok(WorkItemStatus::Done),
            "dropped" => Ok(WorkItemStatus::Dropped),
            other => bail!("Unknown work item status: {}", other),
        }
    }

    pub fn create_work_item(
        &self,
        session_id: &str,
        title: &str,
        created_by: &str,
    ) -> Result<WorkItem> {
        let title = title.trim();
        if title.is_empty() {
            bail!("Work item title cannot be empty");
        }
        let now = chrono::Utc::now().to_rfc3339();
        let id = uuid::Uuid::new_v4().to_string();
        let position: i64 = self.block_on(async {
            let mut rows = self.conn.query(
                "SELECT COALESCE(MAX(position), -1) FROM work_items WHERE session_id = ?1",
                params![session_id],
            ).await?;
            let row = rows.next().await?.context("Missing max(position) row")?;
            let max: i64 = row.get(0)?;
            Ok::<i64, anyhow::Error>(max + 1)
        })?;
        self.block_on(async {
            self.conn.execute(
                "INSERT INTO work_items (id, session_id, title, status, position, created_by, created_at, closed_at)
                 VALUES (?1, ?2, ?3, 'open', ?4, ?5, ?6, NULL)",
                params![id.clone(), session_id, title.to_string(), position, created_by, now.clone()],
            ).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(WorkItem {
            id,
            session_id: session_id.to_string(),
            title: title.to_string(),
            status: WorkItemStatus::Open,
            position,
            created_by: created_by.to_string(),
            created_at: now,
            closed_at: None,
        })
    }

    pub fn list_work_items(&self, session_id: &str) -> Result<Vec<WorkItem>> {
        self.block_on(async {
            let mut rows = self.conn.query(
                "SELECT id, session_id, title, status, position, created_by, created_at, closed_at
                 FROM work_items WHERE session_id = ?1 ORDER BY position ASC",
                params![session_id],
            ).await?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().await? {
                let status_raw: String = row.get(3)?;
                out.push(WorkItem {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    title: row.get(2)?,
                    status: Self::parse_work_item_status(&status_raw)?,
                    position: row.get(4)?,
                    created_by: row.get(5)?,
                    created_at: row.get(6)?,
                    closed_at: row.get(7)?,
                });
            }
            Ok(out)
        })
    }

    pub fn work_item_session(&self, item_id: &str) -> Result<Option<String>> {
        self.block_on(async {
            let mut rows = self
                .conn
                .query(
                    "SELECT work_items.session_id FROM work_items
                     JOIN sessions ON sessions.id = work_items.session_id
                     JOIN workflow_state ON workflow_state.session_id = sessions.id
                     LEFT JOIN archived_sessions
                       ON archived_sessions.session_id = sessions.id
                     WHERE work_items.id = ?1
                       AND archived_sessions.session_id IS NULL
                       AND EXISTS (
                           SELECT 1 FROM session_tasks
                           WHERE session_tasks.session_id = sessions.id
                       )",
                    params![item_id],
                )
                .await?;
            match rows.next().await? {
                Some(row) => Ok(Some(row.get(0)?)),
                None => Ok(None),
            }
        })
    }

    pub fn set_work_item_status(
        &self,
        item_id: &str,
        status: WorkItemStatus,
    ) -> Result<WorkItem> {
        let now = chrono::Utc::now().to_rfc3339();
        let closed: Option<String> = match status {
            WorkItemStatus::Done | WorkItemStatus::Dropped => Some(now),
            _ => None,
        };
        self.block_on(async {
            let count = self.conn.execute(
                "UPDATE work_items SET status = ?1, closed_at = COALESCE(?2, closed_at) WHERE id = ?3",
                params![Self::work_item_status_str(status), closed, item_id],
            ).await?;
            if count == 0 {
                bail!("Work item not found: {}", item_id);
            }
            Ok::<(), anyhow::Error>(())
        })?;
        // Reopening clears a stale closed_at.
        if matches!(status, WorkItemStatus::Open | WorkItemStatus::InProgress) {
            self.block_on(async {
                self.conn
                    .execute(
                        "UPDATE work_items SET closed_at = NULL WHERE id = ?1",
                        params![item_id],
                    )
                    .await?;
                Ok::<(), anyhow::Error>(())
            })?;
        }
        self.get_work_item(item_id)?
            .context("Work item vanished after update")
    }

    fn get_work_item(&self, item_id: &str) -> Result<Option<WorkItem>> {
        self.block_on(async {
            let mut rows = self.conn.query(
                "SELECT id, session_id, title, status, position, created_by, created_at, closed_at
                 FROM work_items WHERE id = ?1",
                params![item_id],
            ).await?;
            let Some(row) = rows.next().await? else {
                return Ok(None);
            };
            let status_raw: String = row.get(3)?;
            Ok(Some(WorkItem {
                id: row.get(0)?,
                session_id: row.get(1)?,
                title: row.get(2)?,
                status: Self::parse_work_item_status(&status_raw)?,
                position: row.get(4)?,
                created_by: row.get(5)?,
                created_at: row.get(6)?,
                closed_at: row.get(7)?,
            }))
        })
    }

    pub fn add_work_item_event(
        &self,
        item_id: &str,
        session_id: &str,
        kind: &str,
        body_markdown: &str,
        actor_id: &str,
    ) -> Result<WorkItemEvent> {
        let event = WorkItemEvent {
            id: uuid::Uuid::new_v4().to_string(),
            item_id: item_id.to_string(),
            session_id: session_id.to_string(),
            kind: kind.to_string(),
            body_markdown: body_markdown.to_string(),
            actor_id: actor_id.to_string(),
            anchor_id: None,
            artifact_id: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.block_on(async {
            self.conn.execute(
                "INSERT INTO work_item_events (id, item_id, session_id, kind, body_markdown, actor_id, anchor_id, artifact_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, ?7)",
                params![
                    event.id.clone(),
                    event.item_id.clone(),
                    event.session_id.clone(),
                    event.kind.clone(),
                    event.body_markdown.clone(),
                    event.actor_id.clone(),
                    event.created_at.clone(),
                ],
            ).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(event)
    }

    pub fn list_work_item_events(
        &self,
        item_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<WorkItemEvent>> {
        let limit = limit
            .map(i64::try_from)
            .transpose()
            .context("work item event limit exceeds database integer range")?
            .unwrap_or(-1);
        self.block_on(async {
            let mut rows = self.conn.query(
                "SELECT id, item_id, session_id, kind, body_markdown, actor_id, anchor_id, artifact_id, created_at
                 FROM work_item_events WHERE item_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2",
                params![item_id, limit],
            ).await?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().await? {
                out.push(WorkItemEvent {
                    id: row.get(0)?,
                    item_id: row.get(1)?,
                    session_id: row.get(2)?,
                    kind: row.get(3)?,
                    body_markdown: row.get(4)?,
                    actor_id: row.get(5)?,
                    anchor_id: row.get(6)?,
                    artifact_id: row.get(7)?,
                    created_at: row.get(8)?,
                });
            }
            out.reverse();
            Ok(out)
        })
    }

    pub fn close_work_item(
        &self,
        item_id: &str,
        summary_markdown: &str,
        issue_ref: Option<String>,
        follow_ups_json: Option<String>,
        skip_reason: Option<String>,
    ) -> Result<WorkItemCloseout> {
        let summary = summary_markdown.trim();
        let effective_summary = if summary.is_empty() {
            match skip_reason
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(reason) => format!("Close-out skipped: {reason}"),
                None => {
                    bail!("Closing requires a summary or an explicit skip reason")
                }
            }
        } else {
            summary.to_string()
        };
        let Some(item) = self.get_work_item(item_id)? else {
            bail!("Work item not found: {}", item_id);
        };
        let now = chrono::Utc::now().to_rfc3339();
        let closeout = WorkItemCloseout {
            item_id: item.id.clone(),
            session_id: item.session_id.clone(),
            summary_markdown: effective_summary,
            issue_ref,
            follow_ups_json: follow_ups_json.unwrap_or_else(|| "[]".to_string()),
            conversation_summary_id: None,
            closed_at: now.clone(),
        };
        self.block_on(async {
            self.conn.execute(
                "INSERT INTO work_item_closeouts (item_id, session_id, summary_markdown, issue_ref, follow_ups_json, conversation_summary_id, closed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)
                 ON CONFLICT(item_id) DO UPDATE SET summary_markdown = excluded.summary_markdown,
                    issue_ref = excluded.issue_ref, follow_ups_json = excluded.follow_ups_json,
                    closed_at = excluded.closed_at",
                params![
                    closeout.item_id.clone(),
                    closeout.session_id.clone(),
                    closeout.summary_markdown.clone(),
                    closeout.issue_ref.clone(),
                    closeout.follow_ups_json.clone(),
                    closeout.closed_at.clone(),
                ],
            ).await?;
            self.conn.execute(
                "UPDATE work_items SET status = 'done', closed_at = ?1 WHERE id = ?2",
                params![now, item_id],
            ).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        self.add_work_item_event(
            &item.id,
            &item.session_id,
            "close",
            &closeout.summary_markdown,
            "host",
        )?;
        Ok(closeout)
    }

    pub fn get_work_item_closeout(
        &self,
        item_id: &str,
    ) -> Result<Option<WorkItemCloseout>> {
        self.block_on(async {
            let mut rows = self.conn.query(
                "SELECT item_id, session_id, summary_markdown, issue_ref, follow_ups_json, conversation_summary_id, closed_at
                 FROM work_item_closeouts WHERE item_id = ?1",
                params![item_id],
            ).await?;
            let Some(row) = rows.next().await? else {
                return Ok(None);
            };
            Ok(Some(WorkItemCloseout {
                item_id: row.get(0)?,
                session_id: row.get(1)?,
                summary_markdown: row.get(2)?,
                issue_ref: row.get(3)?,
                follow_ups_json: row.get(4)?,
                conversation_summary_id: row.get(5)?,
                closed_at: row.get(6)?,
            }))
        })
    }

    pub fn save_conversation_summary(
        &self,
        session_id: &str,
        phase: &str,
        summary_markdown: &str,
        message_id_range: &str,
    ) -> Result<ConversationSummary> {
        if summary_markdown.trim().is_empty() {
            bail!("Conversation summary cannot be empty");
        }
        let summary = ConversationSummary {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            phase: phase.to_string(),
            summary_markdown: summary_markdown.to_string(),
            message_id_range: message_id_range.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.block_on(async {
            self.conn.execute(
                "INSERT INTO conversation_summaries (id, session_id, phase, summary_markdown, message_id_range, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    summary.id.clone(),
                    summary.session_id.clone(),
                    summary.phase.clone(),
                    summary.summary_markdown.clone(),
                    summary.message_id_range.clone(),
                    summary.created_at.clone(),
                ],
            ).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(summary)
    }

    pub fn list_conversation_summaries(
        &self,
        session_id: &str,
    ) -> Result<Vec<ConversationSummary>> {
        self.block_on(async {
            let mut rows = self.conn.query(
                "SELECT id, session_id, phase, summary_markdown, message_id_range, created_at
                 FROM conversation_summaries WHERE session_id = ?1 ORDER BY created_at ASC",
                params![session_id],
            ).await?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().await? {
                out.push(ConversationSummary {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    phase: row.get(2)?,
                    summary_markdown: row.get(3)?,
                    message_id_range: row.get(4)?,
                    created_at: row.get(5)?,
                });
            }
            Ok(out)
        })
    }

    /// Direct row insert for export restore (bypasses id generation).
    pub fn insert_work_item_row(&self, item: &WorkItem) -> Result<()> {
        self.block_on(async {
            self.conn.execute(
                "INSERT INTO work_items (id, session_id, title, status, position, created_by, created_at, closed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    item.id.clone(),
                    item.session_id.clone(),
                    item.title.clone(),
                    Self::work_item_status_str(item.status),
                    item.position,
                    item.created_by.clone(),
                    item.created_at.clone(),
                    item.closed_at.clone(),
                ],
            ).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    pub fn insert_work_item_event_row(&self, event: &WorkItemEvent) -> Result<()> {
        self.block_on(async {
            self.conn.execute(
                "INSERT INTO work_item_events (id, item_id, session_id, kind, body_markdown, actor_id, anchor_id, artifact_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    event.id.clone(),
                    event.item_id.clone(),
                    event.session_id.clone(),
                    event.kind.clone(),
                    event.body_markdown.clone(),
                    event.actor_id.clone(),
                    event.anchor_id.clone(),
                    event.artifact_id.clone(),
                    event.created_at.clone(),
                ],
            ).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    pub fn insert_work_item_closeout_row(
        &self,
        closeout: &WorkItemCloseout,
    ) -> Result<()> {
        self.block_on(async {
            self.conn.execute(
                "INSERT INTO work_item_closeouts (item_id, session_id, summary_markdown, issue_ref, follow_ups_json, conversation_summary_id, closed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(item_id) DO UPDATE SET summary_markdown = excluded.summary_markdown,
                    issue_ref = excluded.issue_ref, follow_ups_json = excluded.follow_ups_json,
                    conversation_summary_id = excluded.conversation_summary_id, closed_at = excluded.closed_at",
                params![
                    closeout.item_id.clone(),
                    closeout.session_id.clone(),
                    closeout.summary_markdown.clone(),
                    closeout.issue_ref.clone(),
                    closeout.follow_ups_json.clone(),
                    closeout.conversation_summary_id.clone(),
                    closeout.closed_at.clone(),
                ],
            ).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    pub fn insert_conversation_summary_row(
        &self,
        summary: &ConversationSummary,
    ) -> Result<()> {
        self.block_on(async {
            self.conn.execute(
                "INSERT INTO conversation_summaries (id, session_id, phase, summary_markdown, message_id_range, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    summary.id.clone(),
                    summary.session_id.clone(),
                    summary.phase.clone(),
                    summary.summary_markdown.clone(),
                    summary.message_id_range.clone(),
                    summary.created_at.clone(),
                ],
            ).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    /// Next monotonically increasing message sequence for a session.
    pub fn next_message_sequence(&self, session_id: &str) -> Result<i64> {
        self.block_on(async {
            let mut rows = self.conn.query(
                "SELECT COALESCE(MAX(sequence), 0) FROM conversation_messages WHERE session_id = ?1",
                params![session_id.to_string()],
            ).await?;
            let max: i64 = match rows.next().await? {
                Some(row) => row.get(0)?,
                None => 0,
            };
            Ok::<i64, anyhow::Error>(max + 1)
        })
    }

    /// Appends or replaces one durable conversation message.
    pub fn upsert_message(&self, message: &ConversationMessage) -> Result<()> {
        self.block_on(async {
            self.conn.execute(
                "INSERT INTO conversation_messages
                    (id, session_id, turn_id, sequence, role, actor_id, content, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(id) DO UPDATE SET
                    content = excluded.content,
                    status = excluded.status",
                params![
                    message.id.clone(),
                    message.session_id.clone(),
                    message.turn_id.clone(),
                    message.sequence,
                    message.role.clone(),
                    message.actor_id.clone(),
                    message.content.clone(),
                    message.status.clone(),
                    message.created_at.clone(),
                ],
            ).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    /// Appends streamed text to an existing message and returns the new body.
    pub fn append_message_delta(
        &self,
        message_id: &str,
        delta: &str,
    ) -> Result<String> {
        let existing: Option<String> = self.block_on(async {
            let mut rows = self
                .conn
                .query(
                    "SELECT content FROM conversation_messages WHERE id = ?1",
                    params![message_id.to_string()],
                )
                .await?;
            match rows.next().await? {
                Some(row) => Ok::<Option<String>, anyhow::Error>(Some(row.get(0)?)),
                None => Ok(None),
            }
        })?;
        let updated = match existing {
            Some(mut content) => {
                content.push_str(delta);
                self.block_on(async {
                    self.conn.execute(
                        "UPDATE conversation_messages SET content = ?1 WHERE id = ?2",
                        params![content.clone(), message_id.to_string()],
                    ).await?;
                    Ok::<(), anyhow::Error>(())
                })?;
                content
            }
            None => String::new(),
        };
        Ok(updated)
    }

    /// Marks a message's lifecycle state (`complete`, `cancelled`, `failed`).
    pub fn set_message_status(&self, message_id: &str, status: &str) -> Result<()> {
        self.block_on(async {
            self.conn
                .execute(
                    "UPDATE conversation_messages SET status = ?1 WHERE id = ?2",
                    params![status.to_string(), message_id.to_string()],
                )
                .await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    pub fn get_agent_runtime_state(
        &self,
        session_id: &str,
    ) -> Result<Option<AgentRuntimeState>> {
        self.block_on(async {
            let mut rows = self.conn.query(
                "SELECT state_json FROM harness_runtime_state WHERE session_id = ?1",
                params![session_id.to_string()],
            ).await?;
            let Some(row) = rows.next().await? else {
                return Ok(None);
            };
            let state_json: String = row.get(0)?;
            Ok(Some(serde_json::from_str(&state_json)?))
        })
    }

    pub fn set_agent_runtime_state(
        &self,
        session_id: &str,
        state: &AgentRuntimeState,
    ) -> Result<()> {
        let state_json = serde_json::to_string(state)?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        self.block_on(async {
            self.conn.execute(
                "INSERT INTO harness_runtime_state (session_id, state_json, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(session_id) DO UPDATE SET
                    state_json = excluded.state_json,
                    updated_at = excluded.updated_at",
                params![session_id.to_string(), state_json, updated_at],
            ).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    pub fn save_turn_request(
        &self,
        turn_id: &str,
        request: &ahead_rpc::ahead::AgentTurnRequestDto,
        instruction_sources: &[InstructionFileSource],
    ) -> Result<()> {
        let request_json = serde_json::to_string(request)?;
        let instruction_rows = instruction_sources
            .iter()
            .enumerate()
            .map(|(ordinal, source)| {
                Ok((
                    i64::try_from(ordinal)
                        .context("too many instruction sources for one turn")?,
                    source.path.clone(),
                    source.content_sha256.clone(),
                    serde_json::to_string(&source.targets)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        self.block_on(async {
            self.conn.execute("BEGIN IMMEDIATE", ()).await?;
            let result = async {
                self.conn
                    .execute(
                        "INSERT INTO harness_turn_requests
                        (turn_id, session_id, request_json, created_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(turn_id) DO UPDATE SET
                        request_json = excluded.request_json",
                        params![
                            turn_id.to_string(),
                            request.session_id.clone(),
                            request_json,
                            chrono::Utc::now().to_rfc3339(),
                        ],
                    )
                    .await?;
                self.conn
                    .execute(
                        "DELETE FROM turn_instruction_sources WHERE turn_id = ?1",
                        params![turn_id.to_string()],
                    )
                    .await?;
                for (ordinal, source_path, content_sha256, targets_json) in
                    &instruction_rows
                {
                    self.conn
                        .execute(
                            "INSERT INTO turn_instruction_sources
                                (session_id, turn_id, ordinal, source_path,
                                 content_sha256, targets_json)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            params![
                                request.session_id.clone(),
                                turn_id.to_string(),
                                *ordinal,
                                source_path.clone(),
                                content_sha256.clone(),
                                targets_json.clone(),
                            ],
                        )
                        .await?;
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                self.conn
                    .execute("ROLLBACK", ())
                    .await
                    .context("rolling back turn instruction provenance")?;
                return Err(error);
            }
            self.conn.execute("COMMIT", ()).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    pub fn list_turn_instruction_sources(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Vec<InstructionFileSource>> {
        self.block_on(async {
            let mut rows = self
                .conn
                .query(
                    "SELECT source_path, content_sha256, targets_json
                     FROM turn_instruction_sources
                     WHERE session_id = ?1 AND turn_id = ?2
                     ORDER BY ordinal",
                    params![session_id.to_string(), turn_id.to_string()],
                )
                .await?;
            let mut sources = Vec::new();
            while let Some(row) = rows.next().await? {
                let targets_json: String = row.get(2)?;
                sources.push(InstructionFileSource {
                    path: row.get(0)?,
                    content_sha256: row.get(1)?,
                    targets: serde_json::from_str(&targets_json)?,
                });
            }
            Ok::<Vec<InstructionFileSource>, anyhow::Error>(sources)
        })
    }

    pub fn get_turn_request(
        &self,
        turn_id: &str,
    ) -> Result<Option<ahead_rpc::ahead::AgentTurnRequestDto>> {
        let request_json: Option<String> = self.block_on(async {
            let mut rows = self
                .conn
                .query(
                    "SELECT request_json FROM harness_turn_requests WHERE turn_id = ?1",
                    params![turn_id.to_string()],
                )
                .await?;
            match rows.next().await? {
                Some(row) => Ok::<Option<String>, anyhow::Error>(Some(row.get(0)?)),
                None => Ok(None),
            }
        })?;
        request_json
            .map(|json| {
                serde_json::from_str(&json).context("parsing persisted turn request")
            })
            .transpose()
    }

    pub fn remove_turn_request(&self, turn_id: &str) -> Result<()> {
        self.block_on(async {
            self.conn
                .execute(
                    "DELETE FROM harness_turn_requests WHERE turn_id = ?1",
                    params![turn_id.to_string()],
                )
                .await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    pub fn create_native_thread(
        &self,
        thread_id: &str,
        create_params: &Value,
    ) -> Result<()> {
        let create_params_json = serde_json::to_string(create_params)?;
        let now = chrono::Utc::now().to_rfc3339();
        let changed = self.block_on(async {
            self.conn
                .execute(
                    "INSERT INTO agent_runtime_threads
                        (thread_id, create_params_json, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?3)",
                    params![thread_id.to_string(), create_params_json, now,],
                )
                .await
                .map_err(anyhow::Error::from)
        })?;
        anyhow::ensure!(changed == 1, "native thread {thread_id} was not created");
        Ok(())
    }

    pub fn import_legacy_native_thread(
        &self,
        import: &LegacyNativeThreadImport,
    ) -> Result<bool> {
        anyhow::ensure!(
            import
                .create_params
                .get("thread_id")
                .and_then(Value::as_str)
                == Some(import.thread_id.as_str()),
            "legacy native thread id does not match its create parameters"
        );
        let create_params_json = serde_json::to_string(&import.create_params)?;
        let item_json = import
            .rollout_items
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let thread_id = import.thread_id.clone();
        let created_at = import.created_at.to_rfc3339();
        let updated_at = import.updated_at.to_rfc3339();
        let archived_at = import
            .archived_at
            .as_ref()
            .map(chrono::DateTime::to_rfc3339);
        self.block_on(async {
            self.conn.execute("BEGIN TRANSACTION", ()).await?;
            let result = async {
                let inserted = self
                    .conn
                    .execute(
                        "INSERT INTO agent_runtime_threads
                            (thread_id, create_params_json, archived, archived_at,
                             created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT(thread_id) DO NOTHING",
                        params![
                            thread_id.clone(),
                            create_params_json,
                            if import.archived_at.is_some() {
                                1_i64
                            } else {
                                0_i64
                            },
                            archived_at,
                            created_at,
                            updated_at,
                        ],
                    )
                    .await?;
                if inserted == 0 {
                    self.conn.execute("COMMIT", ()).await?;
                    return Ok::<bool, anyhow::Error>(false);
                }
                for (ordinal, item) in item_json.iter().enumerate() {
                    let ordinal = i64::try_from(ordinal)
                        .context("legacy rollout has too many records")?;
                    self.conn
                        .execute(
                            "INSERT INTO agent_runtime_thread_items
                                (thread_id, ordinal, item_json)
                             VALUES (?1, ?2, ?3)",
                            params![thread_id.clone(), ordinal, item.clone()],
                        )
                        .await?;
                }
                self.conn.execute("COMMIT", ()).await?;
                Ok(true)
            }
            .await;
            match result {
                Ok(imported) => Ok(imported),
                Err(error) => {
                    if let Err(rollback_error) =
                        self.conn.execute("ROLLBACK", ()).await
                    {
                        return Err(error.context(format!(
                            "rollback also failed: {rollback_error}"
                        )));
                    }
                    Err(error)
                }
            }
        })
    }

    pub fn append_native_thread_items(
        &self,
        thread_id: &str,
        items: &[Value],
    ) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let serialized_items = items
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let thread_id = thread_id.to_string();
        self.block_on(async {
            self.conn.execute("BEGIN TRANSACTION", ()).await?;
            let result = async {
                let mut rows = self
                    .conn
                    .query(
                        "SELECT COALESCE(MAX(ordinal), -1)
                         FROM agent_runtime_thread_items WHERE thread_id = ?1",
                        params![thread_id.clone()],
                    )
                    .await?;
                let last_ordinal = match rows.next().await? {
                    Some(row) => row.get::<i64>(0)?,
                    None => -1,
                };
                for (offset, item_json) in serialized_items.iter().enumerate() {
                    let ordinal_offset = i64::try_from(offset)
                        .context("native thread item batch is too large")?
                        .checked_add(1)
                        .context("native thread item batch is too large")?;
                    let ordinal = last_ordinal
                        .checked_add(ordinal_offset)
                        .context("native thread item ordinal overflow")?;
                    self.conn
                        .execute(
                            "INSERT INTO agent_runtime_thread_items
                                (thread_id, ordinal, item_json)
                             VALUES (?1, ?2, ?3)",
                            params![thread_id.clone(), ordinal, item_json.clone()],
                        )
                        .await?;
                }
                let changed = self
                    .conn
                    .execute(
                        "UPDATE agent_runtime_threads SET updated_at = ?1
                         WHERE thread_id = ?2",
                        params![chrono::Utc::now().to_rfc3339(), thread_id.clone()],
                    )
                    .await?;
                anyhow::ensure!(
                    changed == 1,
                    "native thread {thread_id} was not found"
                );
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                self.conn
                    .execute("ROLLBACK", ())
                    .await
                    .context("rolling back native thread item append")?;
                return Err(error);
            }
            self.conn.execute("COMMIT", ()).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    pub fn append_native_thread_metadata(
        &self,
        thread_id: &str,
        patch: &Value,
    ) -> Result<()> {
        let patch_json = serde_json::to_string(patch)?;
        let thread_id = thread_id.to_string();
        self.block_on(async {
            self.conn.execute("BEGIN TRANSACTION", ()).await?;
            let result = async {
                let mut rows = self
                    .conn
                    .query(
                        "SELECT COALESCE(MAX(ordinal), -1)
                         FROM agent_runtime_thread_metadata WHERE thread_id = ?1",
                        params![thread_id.clone()],
                    )
                    .await?;
                let last_ordinal = match rows.next().await? {
                    Some(row) => row.get::<i64>(0)?,
                    None => -1,
                };
                let ordinal = last_ordinal
                    .checked_add(1)
                    .context("native thread metadata ordinal overflow")?;
                let changed = self
                    .conn
                    .execute(
                        "INSERT INTO agent_runtime_thread_metadata
                            (thread_id, ordinal, patch_json)
                         SELECT ?1, ?2, ?3
                         WHERE EXISTS (
                            SELECT 1 FROM agent_runtime_threads WHERE thread_id = ?1
                         )",
                        params![thread_id.clone(), ordinal, patch_json],
                    )
                    .await?;
                anyhow::ensure!(
                    changed == 1,
                    "native thread {thread_id} was not found"
                );
                self.conn
                    .execute(
                        "UPDATE agent_runtime_threads SET updated_at = ?1
                         WHERE thread_id = ?2",
                        params![chrono::Utc::now().to_rfc3339(), thread_id.clone()],
                    )
                    .await?;
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                self.conn
                    .execute("ROLLBACK", ())
                    .await
                    .context("rolling back native thread metadata append")?;
                return Err(error);
            }
            self.conn.execute("COMMIT", ()).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    pub fn load_native_thread(
        &self,
        thread_id: &str,
    ) -> Result<Option<ahead_agent::NativeThreadSnapshot>> {
        let thread_id = thread_id.to_string();
        self.block_on(async {
            let mut rows = self
                .conn
                .query(
                    "SELECT create_params_json, archived, archived_at
                     FROM agent_runtime_threads WHERE thread_id = ?1",
                    params![thread_id.clone()],
                )
                .await?;
            let Some(row) = rows.next().await? else {
                return Ok(None);
            };
            let create_params: Value = serde_json::from_str(&row.get::<String>(0)?)?;
            let archived = row.get::<i64>(1)? != 0;
            let archived_at = row
                .get::<Option<String>>(2)?
                .map(|time| {
                    chrono::DateTime::parse_from_rfc3339(&time)
                        .map(|time| time.with_timezone(&chrono::Utc))
                        .with_context(|| {
                            format!("invalid archive timestamp for native thread {thread_id}")
                        })
                })
                .transpose()?;
            anyhow::ensure!(
                archived == archived_at.is_some(),
                "native thread {thread_id} archive flag and timestamp disagree"
            );

            let mut item_rows = self
                .conn
                .query(
                    "SELECT item_json FROM agent_runtime_thread_items
                     WHERE thread_id = ?1 ORDER BY ordinal ASC",
                    params![thread_id.clone()],
                )
                .await?;
            let mut rollout_items = Vec::new();
            while let Some(row) = item_rows.next().await? {
                rollout_items.push(serde_json::from_str(&row.get::<String>(0)?)?);
            }

            let mut metadata_rows = self
                .conn
                .query(
                    "SELECT patch_json FROM agent_runtime_thread_metadata
                     WHERE thread_id = ?1 ORDER BY ordinal ASC",
                    params![thread_id],
                )
                .await?;
            let mut metadata_patches = Vec::new();
            while let Some(row) = metadata_rows.next().await? {
                metadata_patches.push(serde_json::from_str(&row.get::<String>(0)?)?);
            }
            Ok(Some(ahead_agent::NativeThreadSnapshot {
                create_params,
                metadata_patches,
                rollout_items,
                archived,
                archived_at,
            }))
        })
    }

    pub fn load_native_thread_header(
        &self,
        thread_id: &str,
    ) -> Result<Option<ahead_agent::NativeThreadHeader>> {
        let thread_id = thread_id.to_string();
        self.block_on(async {
            let mut rows = self
                .conn
                .query(
                    "SELECT create_params_json, archived, archived_at,
                            created_at, updated_at
                     FROM agent_runtime_threads WHERE thread_id = ?1",
                    params![thread_id.clone()],
                )
                .await?;
            let Some(row) = rows.next().await? else {
                return Ok(None);
            };
            let archived = row.get::<i64>(1)? != 0;
            let archived_at = row
                .get::<Option<String>>(2)?
                .map(|time| {
                    chrono::DateTime::parse_from_rfc3339(&time)
                        .map(|time| time.with_timezone(&chrono::Utc))
                        .with_context(|| {
                            format!("invalid archive timestamp for native thread {thread_id}")
                        })
                })
                .transpose()?;
            anyhow::ensure!(
                archived == archived_at.is_some(),
                "native thread {thread_id} archive flag and timestamp disagree"
            );
            let create_params = serde_json::from_str(&row.get::<String>(0)?)?;
            let created_at = chrono::DateTime::parse_from_rfc3339(&row.get::<String>(3)?)
                .context("invalid native thread creation timestamp")?
                .with_timezone(&chrono::Utc);
            let updated_at = chrono::DateTime::parse_from_rfc3339(&row.get::<String>(4)?)
                .context("invalid native thread update timestamp")?
                .with_timezone(&chrono::Utc);
            drop(rows);

            let mut metadata_rows = self
                .conn
                .query(
                    "SELECT patch_json FROM agent_runtime_thread_metadata
                     WHERE thread_id = ?1 ORDER BY ordinal ASC",
                    params![thread_id.clone()],
                )
                .await?;
            let mut metadata_patches = Vec::new();
            while let Some(row) = metadata_rows.next().await? {
                metadata_patches.push(serde_json::from_str(&row.get::<String>(0)?)?);
            }
            Ok(Some(ahead_agent::NativeThreadHeader {
                thread_id,
                create_params,
                metadata_patches,
                archived,
                archived_at,
                created_at: Some(created_at),
                updated_at: Some(updated_at),
            }))
        })
    }

    pub fn load_native_thread_item_page(
        &self,
        thread_id: &str,
        before_ordinal: Option<i64>,
        limit: usize,
    ) -> Result<ahead_agent::NativeThreadReplayPage> {
        anyhow::ensure!(
            (1..=256).contains(&limit),
            "native thread replay page limit must be between 1 and 256"
        );
        anyhow::ensure!(
            before_ordinal.is_none_or(|ordinal| ordinal >= 0),
            "native thread replay cursor must be non-negative"
        );
        let thread_id = thread_id.to_string();
        let fetch_limit = i64::try_from(limit + 1).context(
            "native thread replay page limit exceeds database integer range",
        )?;
        self.block_on(async {
            let mut rows = match before_ordinal {
                Some(before_ordinal) => self
                    .conn
                    .query(
                        "SELECT ordinal, item_json FROM agent_runtime_thread_items
                             WHERE thread_id = ?1 AND ordinal < ?2
                             ORDER BY ordinal DESC LIMIT ?3",
                        params![thread_id.clone(), before_ordinal, fetch_limit],
                    )
                    .await?,
                None => self
                    .conn
                    .query(
                        "SELECT ordinal, item_json FROM agent_runtime_thread_items
                             WHERE thread_id = ?1
                             ORDER BY ordinal DESC LIMIT ?2",
                        params![thread_id.clone(), fetch_limit],
                    )
                    .await?,
            };
            let mut page_rows = Vec::new();
            while let Some(row) = rows.next().await? {
                page_rows.push((
                    row.get::<i64>(0)?,
                    serde_json::from_str::<Value>(&row.get::<String>(1)?)?,
                ));
            }
            let has_more = page_rows.len() > limit;
            if has_more {
                page_rows.truncate(limit);
            }
            let next_before_ordinal = has_more
                .then(|| page_rows.last().map(|(ordinal, _)| *ordinal))
                .flatten();
            Ok(ahead_agent::NativeThreadReplayPage {
                items: page_rows.into_iter().map(|(_, item)| item).collect(),
                next_before_ordinal,
            })
        })
    }

    pub fn list_native_thread_headers(
        &self,
    ) -> Result<Vec<ahead_agent::NativeThreadHeader>> {
        self.block_on(async {
            let mut rows = self
                .conn
                .query(
                    "SELECT thread_id, create_params_json, archived, archived_at,
                            created_at, updated_at
                     FROM agent_runtime_threads ORDER BY thread_id",
                    (),
                )
                .await?;
            let mut headers = Vec::new();
            let mut positions = HashMap::new();
            while let Some(row) = rows.next().await? {
                let thread_id: String = row.get(0)?;
                let archived = row.get::<i64>(2)? != 0;
                let archived_at = row
                    .get::<Option<String>>(3)?
                    .map(|time| {
                        chrono::DateTime::parse_from_rfc3339(&time)
                            .map(|time| time.with_timezone(&chrono::Utc))
                            .with_context(|| {
                                format!("invalid archive timestamp for native thread {thread_id}")
                            })
                    })
                    .transpose()?;
                anyhow::ensure!(
                    archived == archived_at.is_some(),
                    "native thread {thread_id} archive flag and timestamp disagree"
                );
                positions.insert(thread_id.clone(), headers.len());
                headers.push(ahead_agent::NativeThreadHeader {
                    thread_id,
                    create_params: serde_json::from_str(&row.get::<String>(1)?)?,
                    metadata_patches: Vec::new(),
                    archived,
                    archived_at,
                    created_at: Some(
                        chrono::DateTime::parse_from_rfc3339(&row.get::<String>(4)?)?
                            .with_timezone(&chrono::Utc),
                    ),
                    updated_at: Some(
                        chrono::DateTime::parse_from_rfc3339(&row.get::<String>(5)?)?
                            .with_timezone(&chrono::Utc),
                    ),
                });
            }
            drop(rows);
            let mut rows = self
                .conn
                .query(
                    "SELECT thread_id, patch_json FROM agent_runtime_thread_metadata
                     ORDER BY thread_id, ordinal",
                    (),
                )
                .await?;
            while let Some(row) = rows.next().await? {
                let thread_id: String = row.get(0)?;
                let position = positions
                    .get(&thread_id)
                    .context("native thread metadata has no owning thread")?;
                headers[*position]
                    .metadata_patches
                    .push(serde_json::from_str(&row.get::<String>(1)?)?);
            }
            Ok::<Vec<ahead_agent::NativeThreadHeader>, anyhow::Error>(headers)
        })
    }

    pub fn list_native_thread_headers_page(
        &self,
        request: &ahead_agent::NativeThreadHeaderPageRequest,
    ) -> Result<Option<Vec<ahead_agent::NativeThreadHeader>>> {
        anyhow::ensure!(
            request.limit > 0,
            "native thread page limit must be positive"
        );
        let sort_column = match request.sort {
            ahead_agent::NativeThreadTimestampSort::CreatedAt => "created_at",
            ahead_agent::NativeThreadTimestampSort::UpdatedAt => "updated_at",
        };
        let (direction, after) = match request.direction {
            ahead_agent::NativeThreadSortDirection::Asc => ("ASC", ">"),
            ahead_agent::NativeThreadSortDirection::Desc => ("DESC", "<"),
        };
        let cursor = match request.cursor.as_deref() {
            None => None,
            Some(cursor) => {
                let Some((timestamp, thread_id)) = cursor.rsplit_once('|') else {
                    return Ok(None);
                };
                let Ok(timestamp) = chrono::DateTime::parse_from_rfc3339(timestamp)
                else {
                    return Ok(None);
                };
                Some((
                    timestamp.with_timezone(&chrono::Utc).to_rfc3339(),
                    thread_id.to_string(),
                ))
            }
        };
        let archived = if request.archived { 1_i64 } else { 0_i64 };
        let limit = i64::try_from(request.limit)
            .context("native thread page limit exceeds database integer range")?;
        let mut predicates = vec!["archived = ?1".to_string()];
        let mut query_params = vec![SqlValue::Integer(archived)];
        let mut next_parameter = 2;
        if !request.allowed_sources.is_empty() {
            predicates.push(format!(
                "COALESCE((
                    SELECT json_extract(patch_json, '$.source')
                    FROM agent_runtime_thread_metadata
                    WHERE thread_id = agent_runtime_threads.thread_id
                      AND json_type(patch_json, '$.source') IS NOT NULL
                    ORDER BY ordinal DESC LIMIT 1
                 ), json_extract(create_params_json, '$.source')) IN
                 (SELECT value FROM json_each(?{next_parameter}))"
            ));
            query_params.push(SqlValue::Text(serde_json::to_string(
                &request.allowed_sources,
            )?));
            next_parameter += 1;
        }
        if !request.model_providers.is_empty() {
            predicates.push(format!(
                "COALESCE((
                    SELECT json_extract(patch_json, '$.model_provider')
                    FROM agent_runtime_thread_metadata
                    WHERE thread_id = agent_runtime_threads.thread_id
                      AND json_type(patch_json, '$.model_provider') = 'text'
                    ORDER BY ordinal DESC LIMIT 1
                 ), json_extract(create_params_json, '$.metadata.model_provider')) IN
                 (SELECT value FROM json_each(?{next_parameter}))"
            ));
            query_params.push(SqlValue::Text(serde_json::to_string(
                &request.model_providers,
            )?));
            next_parameter += 1;
        }
        if let Some(cwd_filters) = request.cwd_filters.as_ref() {
            predicates.push(format!(
                "COALESCE((
                    SELECT json_extract(patch_json, '$.cwd')
                    FROM agent_runtime_thread_metadata
                    WHERE thread_id = agent_runtime_threads.thread_id
                      AND json_type(patch_json, '$.cwd') = 'text'
                    ORDER BY ordinal DESC LIMIT 1
                 ), json_extract(create_params_json, '$.metadata.cwd')) IN
                 (SELECT value FROM json_each(?{next_parameter}))"
            ));
            query_params.push(SqlValue::Text(serde_json::to_string(cwd_filters)?));
            next_parameter += 1;
        }
        if let Some(search_term) = request.search_term.as_deref() {
            predicates.push(format!(
                "(
                    instr((
                        SELECT json_extract(patch_json, '$.name')
                        FROM agent_runtime_thread_metadata
                        WHERE thread_id = agent_runtime_threads.thread_id
                          AND json_type(patch_json, '$.name') IS NOT NULL
                        ORDER BY ordinal DESC LIMIT 1
                    ), ?{next_parameter}) > 0
                    OR instr(COALESCE((
                        SELECT json_extract(patch_json, '$.preview')
                        FROM agent_runtime_thread_metadata
                        WHERE thread_id = agent_runtime_threads.thread_id
                          AND json_type(patch_json, '$.preview') = 'text'
                        ORDER BY ordinal DESC LIMIT 1
                    ), ''), ?{next_parameter}) > 0
                    OR instr(COALESCE((
                        SELECT json_extract(patch_json, '$.first_user_message')
                        FROM agent_runtime_thread_metadata
                        WHERE thread_id = agent_runtime_threads.thread_id
                          AND json_type(patch_json, '$.first_user_message') = 'text'
                        ORDER BY ordinal DESC LIMIT 1
                    ), json_extract(create_params_json, '$.metadata.first_user_message')), ?{next_parameter}) > 0
                    OR instr((
                        SELECT json_extract(patch_json, '$.title')
                        FROM agent_runtime_thread_metadata
                        WHERE thread_id = agent_runtime_threads.thread_id
                          AND json_type(patch_json, '$.title') = 'text'
                        ORDER BY ordinal DESC LIMIT 1
                    ), ?{next_parameter}) > 0
                )"
            ));
            query_params.push(SqlValue::Text(search_term.to_string()));
            next_parameter += 1;
        }
        let mut with_clause = String::new();
        if let Some(relation_filter) = request.relation_filter.as_ref() {
            match relation_filter {
                ahead_agent::NativeThreadRelationFilter::DirectChildrenOf(
                    parent,
                ) => {
                    predicates.push(format!(
                        "json_extract(create_params_json, '$.parent_thread_id') = ?{next_parameter}"
                    ));
                    query_params.push(SqlValue::Text(parent.clone()));
                    next_parameter += 1;
                }
                ahead_agent::NativeThreadRelationFilter::DescendantsOf(ancestor) => {
                    let relation_parameter = next_parameter;
                    with_clause = format!(
                        "WITH RECURSIVE thread_descendants(thread_id) AS (
                            SELECT thread_id FROM agent_runtime_threads
                            WHERE json_extract(create_params_json, '$.parent_thread_id') = ?{relation_parameter}
                            UNION
                            SELECT child.thread_id FROM agent_runtime_threads AS child
                            JOIN thread_descendants AS parent
                              ON json_extract(child.create_params_json, '$.parent_thread_id') = parent.thread_id
                        ) "
                    );
                    predicates.push(format!(
                        "thread_id IN (
                            SELECT thread_id FROM thread_descendants
                            WHERE thread_id != ?{relation_parameter}
                        )"
                    ));
                    query_params.push(SqlValue::Text(ancestor.clone()));
                    next_parameter += 1;
                }
            }
        }
        if let Some((timestamp, thread_id)) = cursor {
            predicates.push(format!(
                "({sort_column} {after} ?{next_parameter} OR
                 ({sort_column} = ?{next_parameter} AND
                  thread_id {after} ?{}))",
                next_parameter + 1
            ));
            query_params.push(SqlValue::Text(timestamp));
            query_params.push(SqlValue::Text(thread_id));
            next_parameter += 2;
        }
        query_params.push(SqlValue::Integer(limit));

        self.block_on(async {
            let sql = format!(
                "{with_clause}SELECT thread_id, create_params_json, archived, archived_at,
                        created_at, updated_at
                 FROM agent_runtime_threads
                 WHERE {}
                 ORDER BY {sort_column} {direction}, thread_id {direction}
                 LIMIT ?{next_parameter}",
                predicates.join(" AND ")
            );
            let mut rows = self
                .conn
                .query(&sql, params_from_iter(query_params))
                .await?;
            let mut headers = Vec::new();
            let mut positions = HashMap::new();
            while let Some(row) = rows.next().await? {
                let thread_id: String = row.get(0)?;
                let archived = row.get::<i64>(2)? != 0;
                let archived_at = row
                    .get::<Option<String>>(3)?
                    .map(|time| {
                        chrono::DateTime::parse_from_rfc3339(&time)
                            .map(|time| time.with_timezone(&chrono::Utc))
                            .with_context(|| {
                                format!("invalid archive timestamp for native thread {thread_id}")
                            })
                    })
                    .transpose()?;
                anyhow::ensure!(
                    archived == archived_at.is_some(),
                    "native thread {thread_id} archive flag and timestamp disagree"
                );
                positions.insert(thread_id.clone(), headers.len());
                headers.push(ahead_agent::NativeThreadHeader {
                    thread_id,
                    create_params: serde_json::from_str(&row.get::<String>(1)?)?,
                    metadata_patches: Vec::new(),
                    archived,
                    archived_at,
                    created_at: Some(
                        chrono::DateTime::parse_from_rfc3339(&row.get::<String>(4)?)?
                            .with_timezone(&chrono::Utc),
                    ),
                    updated_at: Some(
                        chrono::DateTime::parse_from_rfc3339(&row.get::<String>(5)?)?
                            .with_timezone(&chrono::Utc),
                    ),
                });
            }
            drop(rows);
            if !headers.is_empty() {
                let placeholders = (1..=headers.len())
                    .map(|index| format!("?{index}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let metadata_sql = format!(
                    "SELECT thread_id, patch_json FROM agent_runtime_thread_metadata
                     WHERE thread_id IN ({placeholders})
                     ORDER BY thread_id, ordinal"
                );
                let thread_ids = headers
                    .iter()
                    .map(|header| header.thread_id.clone())
                    .collect::<Vec<_>>();
                let mut metadata_rows = self
                    .conn
                    .query(&metadata_sql, params_from_iter(thread_ids))
                    .await?;
                while let Some(row) = metadata_rows.next().await? {
                    let thread_id: String = row.get(0)?;
                    let position = positions
                        .get(&thread_id)
                        .context("native thread metadata has no owning thread")?;
                    headers[*position]
                        .metadata_patches
                        .push(serde_json::from_str(&row.get::<String>(1)?)?);
                }
            }
            Ok::<_, anyhow::Error>(Some(headers))
        })
    }

    pub fn set_native_thread_archived(
        &self,
        thread_id: &str,
        archived: bool,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let now = chrono::Utc::now();
        let updated_at = now.to_rfc3339();
        let archived_at = archived.then_some(now);
        let archived_at_text =
            archived_at.as_ref().map(chrono::DateTime::to_rfc3339);
        self.block_on(async {
            let changed = self
                .conn
                .execute(
                    "UPDATE agent_runtime_threads
                     SET archived = ?1, archived_at = ?2, updated_at = ?3
                     WHERE thread_id = ?4",
                    params![
                        if archived { 1_i64 } else { 0_i64 },
                        archived_at_text,
                        updated_at,
                        thread_id.to_string(),
                    ],
                )
                .await?;
            anyhow::ensure!(changed == 1, "native thread {thread_id} was not found");
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(archived_at)
    }

    pub fn delete_native_thread(&self, thread_id: &str) -> Result<()> {
        self.block_on(async {
            self.conn.execute("BEGIN TRANSACTION", ()).await?;
            let result = async {
                self.conn
                    .execute(
                        "DELETE FROM agent_spawn_edges WHERE parent_thread_id = ?1 OR child_thread_id = ?1",
                        params![thread_id.to_string()],
                    )
                    .await?;
                self.conn
                    .execute(
                        "DELETE FROM agent_runtime_thread_items WHERE thread_id = ?1",
                        params![thread_id.to_string()],
                    )
                    .await?;
                self.conn
                    .execute(
                        "DELETE FROM agent_runtime_thread_metadata WHERE thread_id = ?1",
                        params![thread_id.to_string()],
                    )
                    .await?;
                self.conn
                    .execute(
                        "DELETE FROM agent_runtime_threads WHERE thread_id = ?1",
                        params![thread_id.to_string()],
                    )
                    .await?;
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                self.conn
                    .execute("ROLLBACK", ())
                    .await
                    .context("rolling back native thread deletion")?;
                return Err(error);
            }
            self.conn.execute("COMMIT", ()).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    pub fn upsert_native_agent_edge(
        &self,
        parent_thread_id: &str,
        child_thread_id: &str,
        status: ahead_agent::NativeAgentEdgeStatus,
    ) -> Result<()> {
        anyhow::ensure!(
            parent_thread_id != child_thread_id,
            "agent thread cannot spawn itself"
        );
        self.block_on(async {
            self.conn
                .execute(
                    "INSERT INTO agent_spawn_edges (parent_thread_id, child_thread_id, status)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(child_thread_id) DO UPDATE SET
                         parent_thread_id = excluded.parent_thread_id,
                         status = excluded.status",
                    params![
                        parent_thread_id.to_string(),
                        child_thread_id.to_string(),
                        status.as_str(),
                    ],
                )
                .await?;
            Ok::<(), anyhow::Error>(())
        })
    }

    pub fn set_native_agent_edge_status(
        &self,
        child_thread_id: &str,
        status: ahead_agent::NativeAgentEdgeStatus,
    ) -> Result<()> {
        self.block_on(async {
            self.conn
                .execute(
                    "UPDATE agent_spawn_edges SET status = ?1 WHERE child_thread_id = ?2",
                    params![status.as_str(), child_thread_id.to_string()],
                )
                .await?;
            Ok::<(), anyhow::Error>(())
        })
    }

    pub fn list_native_agent_children(
        &self,
        parent_thread_id: &str,
        status: Option<ahead_agent::NativeAgentEdgeStatus>,
    ) -> Result<Vec<String>> {
        self.block_on(async {
            let mut rows = self
                .conn
                .query(
                    "SELECT child_thread_id FROM agent_spawn_edges
                     WHERE parent_thread_id = ?1 AND (?2 IS NULL OR status = ?2)
                     ORDER BY child_thread_id",
                    params![
                        parent_thread_id.to_string(),
                        status.map(|status| status.as_str().to_string()),
                    ],
                )
                .await?;
            let mut children = Vec::new();
            while let Some(row) = rows.next().await? {
                children.push(row.get(0)?);
            }
            Ok::<Vec<String>, anyhow::Error>(children)
        })
    }

    pub fn recover_stale_streaming_messages(
        &self,
        session_id: &str,
        active_turn_id: Option<&str>,
    ) -> Result<()> {
        self.block_on(async {
            if let Some(active_turn_id) = active_turn_id {
                self.conn
                    .execute(
                        "UPDATE conversation_messages SET status = 'failed'
                         WHERE session_id = ?1 AND role = 'agent'
                           AND status = 'streaming' AND turn_id <> ?2",
                        params![session_id.to_string(), active_turn_id.to_string()],
                    )
                    .await?;
            } else {
                self.conn
                    .execute(
                        "UPDATE conversation_messages SET status = 'failed'
                         WHERE session_id = ?1 AND role = 'agent'
                           AND status = 'streaming'",
                        params![session_id.to_string()],
                    )
                    .await?;
            }
            Ok::<(), anyhow::Error>(())
        })
    }

    /// Bounded newest-first keyset scan, returned chronologically for the UI.
    pub fn list_messages_page(
        &self,
        session_id: &str,
        before: Option<ConversationMessageCursor>,
        limit: usize,
    ) -> Result<ConversationMessagePage> {
        let limit = limit.clamp(1, 100);
        let fetch_limit = i64::try_from(limit + 1)
            .context("conversation page limit exceeds database integer range")?;
        self.block_on(async {
            let mut rows = if let Some(cursor) = before {
                self.conn
                    .query(
                        "SELECT id, session_id, turn_id, sequence, role, actor_id,
                                content, status, created_at
                         FROM conversation_messages
                         WHERE session_id = ?1
                           AND (sequence < ?2 OR (sequence = ?2 AND id < ?3))
                         ORDER BY sequence DESC, id DESC LIMIT ?4",
                        params![
                            session_id.to_string(),
                            cursor.sequence,
                            cursor.message_id,
                            fetch_limit,
                        ],
                    )
                    .await?
            } else {
                self.conn
                    .query(
                        "SELECT id, session_id, turn_id, sequence, role, actor_id,
                                content, status, created_at
                         FROM conversation_messages WHERE session_id = ?1
                         ORDER BY sequence DESC, id DESC LIMIT ?2",
                        params![session_id.to_string(), fetch_limit],
                    )
                    .await?
            };
            let mut messages = Vec::new();
            while let Some(row) = rows.next().await? {
                messages.push(ConversationMessage {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    turn_id: row.get(2)?,
                    sequence: row.get(3)?,
                    role: row.get(4)?,
                    actor_id: row.get(5)?,
                    content: row.get(6)?,
                    status: row.get(7)?,
                    created_at: row.get(8)?,
                });
            }
            let has_older = messages.len() > limit;
            if has_older {
                messages.pop();
            }
            messages.reverse();
            Ok::<ConversationMessagePage, anyhow::Error>(ConversationMessagePage {
                messages,
                has_older,
            })
        })
    }

    /// Full readable conversation in order.
    pub fn list_messages(
        &self,
        session_id: &str,
    ) -> Result<Vec<ConversationMessage>> {
        self.block_on(async {
            let mut rows = self.conn.query(
                "SELECT id, session_id, turn_id, sequence, role, actor_id, content, status, created_at
                 FROM conversation_messages WHERE session_id = ?1 ORDER BY sequence ASC",
                params![session_id.to_string()],
            ).await?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().await? {
                out.push(ConversationMessage {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    turn_id: row.get(2)?,
                    sequence: row.get(3)?,
                    role: row.get(4)?,
                    actor_id: row.get(5)?,
                    content: row.get(6)?,
                    status: row.get(7)?,
                    created_at: row.get(8)?,
                });
            }
            Ok::<Vec<ConversationMessage>, anyhow::Error>(out)
        })
    }

    /// Records the harness (ACP) conversation bound to a work session so a
    /// reopen can `session/load` the real runtime conversation.
    pub fn set_harness_binding(
        &self,
        session_id: &str,
        acp_session_id: &str,
        backend: &str,
    ) -> Result<()> {
        self.block_on(async {
            self.conn.execute(
                "INSERT INTO harness_bindings (session_id, acp_session_id, backend, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(session_id) DO UPDATE SET
                    acp_session_id = excluded.acp_session_id,
                    backend = excluded.backend,
                    updated_at = excluded.updated_at",
                params![
                    session_id.to_string(),
                    acp_session_id.to_string(),
                    backend.to_string(),
                    chrono::Utc::now().to_rfc3339(),
                ],
            ).await?;
            self.conn.execute(
                "UPDATE agent_runtime_threads SET session_id = ?1
                 WHERE thread_id = ?2",
                params![session_id.to_string(), acp_session_id.to_string()],
            ).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    /// The ACP session bound to a work session, if any.
    pub fn get_harness_binding(
        &self,
        session_id: &str,
    ) -> Result<Option<(String, String)>> {
        self.block_on(async {
            let mut rows = self.conn.query(
                "SELECT acp_session_id, backend FROM harness_bindings WHERE session_id = ?1",
                params![session_id.to_string()],
            ).await?;
            match rows.next().await? {
                Some(row) => Ok::<Option<(String, String)>, anyhow::Error>(Some((row.get(0)?, row.get(1)?))),
                None => Ok(None),
            }
        })
    }
}

/// Adapts the shared `Arc<RwLock<SessionStore>>` the host already owns to the
/// `HarnessStore` trait, so the harness controller and the host use one store.
pub struct SharedSessionStore(pub Arc<parking_lot::RwLock<SessionStore>>);

/// Lets `ahead-agent::HarnessController` drive conversation persistence and
/// harness bindings without depending on this concrete libSQL store.
impl ahead_agent::HarnessStore for SharedSessionStore {
    fn get_task_intent(&self, session_id: &str) -> Result<Option<TaskIntent>> {
        Ok(self
            .0
            .read()
            .get_session(session_id)?
            .map(|v| v.task.intent))
    }

    fn next_message_sequence(&self, session_id: &str) -> Result<i64> {
        self.0.read().next_message_sequence(session_id)
    }

    fn upsert_message(&self, message: &ConversationMessage) -> Result<()> {
        self.0.read().upsert_message(message)
    }

    fn append_message_delta(&self, message_id: &str, delta: &str) -> Result<String> {
        self.0.read().append_message_delta(message_id, delta)
    }

    fn set_message_status(&self, message_id: &str, status: &str) -> Result<()> {
        self.0.read().set_message_status(message_id, status)
    }

    fn get_agent_runtime_state(
        &self,
        session_id: &str,
    ) -> Result<Option<AgentRuntimeState>> {
        self.0.read().get_agent_runtime_state(session_id)
    }

    fn set_agent_runtime_state(
        &self,
        session_id: &str,
        state: &AgentRuntimeState,
    ) -> Result<()> {
        self.0.read().set_agent_runtime_state(session_id, state)
    }

    fn save_turn_request(
        &self,
        turn_id: &str,
        request: &ahead_rpc::ahead::AgentTurnRequestDto,
        instruction_sources: &[InstructionFileSource],
    ) -> Result<()> {
        self.0
            .read()
            .save_turn_request(turn_id, request, instruction_sources)
    }

    fn get_turn_request(
        &self,
        turn_id: &str,
    ) -> Result<Option<ahead_rpc::ahead::AgentTurnRequestDto>> {
        self.0.read().get_turn_request(turn_id)
    }

    fn remove_turn_request(&self, turn_id: &str) -> Result<()> {
        self.0.read().remove_turn_request(turn_id)
    }

    fn list_messages(&self, session_id: &str) -> Result<Vec<ConversationMessage>> {
        self.0.read().list_messages(session_id)
    }

    fn recover_stale_streaming_messages(
        &self,
        session_id: &str,
        active_turn_id: Option<&str>,
    ) -> Result<()> {
        self.0
            .read()
            .recover_stale_streaming_messages(session_id, active_turn_id)
    }

    fn list_messages_page(
        &self,
        session_id: &str,
        before: Option<ConversationMessageCursor>,
        limit: usize,
    ) -> Result<ConversationMessagePage> {
        self.0.read().list_messages_page(session_id, before, limit)
    }

    fn session_title(&self, session_id: &str) -> Result<Option<String>> {
        self.0.read().session_title(session_id)
    }

    fn update_session_title(&self, session_id: &str, title: &str) -> Result<()> {
        self.0.read().update_session_title(session_id, title)
    }

    fn set_harness_binding(
        &self,
        session_id: &str,
        acp_session_id: &str,
        backend: &str,
    ) -> Result<()> {
        self.0
            .read()
            .set_harness_binding(session_id, acp_session_id, backend)
    }

    fn get_harness_binding(
        &self,
        session_id: &str,
    ) -> Result<Option<(String, String)>> {
        self.0.read().get_harness_binding(session_id)
    }

    fn create_native_thread(
        &self,
        thread_id: &str,
        create_params: Value,
    ) -> Result<()> {
        self.0
            .read()
            .create_native_thread(thread_id, &create_params)
    }

    fn import_legacy_native_thread(
        &self,
        import: LegacyNativeThreadImport,
    ) -> Result<bool> {
        self.0.read().import_legacy_native_thread(&import)
    }

    fn append_native_thread_items(
        &self,
        thread_id: &str,
        items: Vec<Value>,
    ) -> Result<()> {
        self.0.read().append_native_thread_items(thread_id, &items)
    }

    fn append_native_thread_metadata(
        &self,
        thread_id: &str,
        patch: Value,
    ) -> Result<()> {
        self.0
            .read()
            .append_native_thread_metadata(thread_id, &patch)
    }

    fn load_native_thread(
        &self,
        thread_id: &str,
    ) -> Result<Option<ahead_agent::NativeThreadSnapshot>> {
        self.0.read().load_native_thread(thread_id)
    }

    fn load_native_thread_header(
        &self,
        thread_id: &str,
    ) -> Result<Option<ahead_agent::NativeThreadHeader>> {
        self.0.read().load_native_thread_header(thread_id)
    }

    fn load_native_thread_item_page(
        &self,
        thread_id: &str,
        before_ordinal: Option<i64>,
        limit: usize,
    ) -> Result<ahead_agent::NativeThreadReplayPage> {
        self.0
            .read()
            .load_native_thread_item_page(thread_id, before_ordinal, limit)
    }

    fn list_native_thread_headers(
        &self,
    ) -> Result<Vec<ahead_agent::NativeThreadHeader>> {
        self.0.read().list_native_thread_headers()
    }

    fn list_native_thread_headers_page(
        &self,
        request: &ahead_agent::NativeThreadHeaderPageRequest,
    ) -> Result<Option<Vec<ahead_agent::NativeThreadHeader>>> {
        self.0.read().list_native_thread_headers_page(request)
    }

    fn set_native_thread_archived(
        &self,
        thread_id: &str,
        archived: bool,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        self.0
            .read()
            .set_native_thread_archived(thread_id, archived)
    }

    fn delete_native_thread(&self, thread_id: &str) -> Result<()> {
        self.0.read().delete_native_thread(thread_id)
    }

    fn upsert_native_agent_edge(
        &self,
        parent_thread_id: &str,
        child_thread_id: &str,
        status: ahead_agent::NativeAgentEdgeStatus,
    ) -> Result<()> {
        self.0.read().upsert_native_agent_edge(
            parent_thread_id,
            child_thread_id,
            status,
        )
    }

    fn set_native_agent_edge_status(
        &self,
        child_thread_id: &str,
        status: ahead_agent::NativeAgentEdgeStatus,
    ) -> Result<()> {
        self.0
            .read()
            .set_native_agent_edge_status(child_thread_id, status)
    }

    fn list_native_agent_children(
        &self,
        parent_thread_id: &str,
        status: Option<ahead_agent::NativeAgentEdgeStatus>,
    ) -> Result<Vec<String>> {
        self.0
            .read()
            .list_native_agent_children(parent_thread_id, status)
    }

    fn record_edit_anchor(&self, anchor: &CodeAnchor) -> Result<()> {
        self.0.write().record_edit_anchor(anchor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ahead_rpc::ahead::{
        AgentCommand, AgentPlanEntry, AgentToolCall, AgentTurnRequestDto,
        AgentUsage, AheadNotification, AheadRequest, HarnessKind, TurnEditorContext,
        TurnMechanicalScope,
    };
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    #[test]
    fn agent_runtime_state_survives_file_backed_turso_reopen() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let session = super::super::host::AheadSessionHost::in_memory()?
            .start_work(
                Some(WorkKind::ProductChange),
                "Durable agent state".to_string(),
                "Restore streamed cards".to_string(),
                None,
            )?;
        let database_path = temporary.path().join("session.db");
        let mut store = SessionStore::open(&database_path)?;
        store.insert_session(&session)?;

        let expected = AgentRuntimeState {
            turn_id: Some("turn-state-1".to_string()),
            plan: vec![AgentPlanEntry {
                content: "Inspect storage".to_string(),
                status: "completed".to_string(),
                priority: "medium".to_string(),
            }],
            tool_calls: vec![AgentToolCall {
                id: "tool-state-1".to_string(),
                title: "Search workspace".to_string(),
                status: "completed".to_string(),
                kind: "search".to_string(),
            }],
            usage: Some(AgentUsage {
                total_tokens: 128,
                context_window: Some(4096),
            }),
            commands: vec![AgentCommand {
                name: "review".to_string(),
                description: "Review current changes".to_string(),
                input: None,
            }],
            warnings: vec!["Code Mode host unavailable".to_string()],
        };
        store.set_agent_runtime_state(&session.session.id, &expected)?;
        drop(store);

        let reopened = SessionStore::open(&database_path)?;
        assert_eq!(
            reopened.get_agent_runtime_state(&session.session.id)?,
            Some(expected)
        );
        Ok(())
    }

    #[test]
    fn recent_workspace_files_keep_mru_order_and_survive_reopen() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let database_path = temporary.path().join("session.db");
        let store = SessionStore::open(&database_path)?;
        for index in 0..25 {
            store.record_recent_workspace_file(&format!("src/{index}.rs"), index)?;
        }
        store.record_recent_workspace_file("src/10.rs", 30)?;
        store.record_recent_workspace_file("src/10.rs", 2)?;

        let expected = store.recent_workspace_files()?;
        assert_eq!(expected.len(), 20);
        assert_eq!(expected.first().map(String::as_str), Some("src/10.rs"));
        assert_eq!(expected.get(1).map(String::as_str), Some("src/24.rs"));
        assert!(!expected.iter().any(|path| path == "src/4.rs"));
        drop(store);

        let reopened = SessionStore::open(&database_path)?;
        assert_eq!(reopened.recent_workspace_files()?, expected);
        Ok(())
    }

    #[test]
    fn instruction_sources_survive_turso_reopen_after_retry_state_is_removed()
    -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let database_path = temporary.path().join("session.db");
        let session = super::super::host::AheadSessionHost::in_memory()?
            .start_work(
                Some(WorkKind::ProductChange),
                "Instruction provenance".to_string(),
                "Persist project guidance".to_string(),
                None,
            )?;
        let request = AgentTurnRequestDto {
            session_id: session.session.id.clone(),
            thread_id: "thread-instruction-sources".into(),
            harness: HarnessKind::Ahead,
            external_agent_id: None,
            model: Some("test-model".into()),
            model_provider: Some("test-provider".into()),
            user_message: "Inspect the source".into(),
            session_context: String::new(),
            context: TurnEditorContext {
                active_path: "src/lib.rs".into(),
                caret: ahead_rpc::ahead::DisplayPosition { line: 0, col: 0 },
                selection: None,
                file_content: String::new(),
                visible_end: None,
                attached_anchor_ids: Vec::new(),
                attached_files: Vec::new(),
                attached_memories: Vec::new(),
            },
            invariants: Vec::new(),
            cwd: None,
            expected_policy_sha256: session.session.policy.sha256.clone(),
            read_only: false,
            scope: None,
        };
        let expected_sources = vec![InstructionFileSource {
            path: "src/AGENTS.md".into(),
            content_sha256: "a".repeat(64),
            targets: vec!["src/lib.rs".into()],
        }];
        let mut store = SessionStore::open(&database_path)?;
        store.insert_session(&session)?;
        store.save_turn_request(
            "turn-instruction-sources",
            &request,
            &expected_sources,
        )?;
        drop(store);

        let reopened = SessionStore::open(&database_path)?;
        assert_eq!(
            reopened.list_turn_instruction_sources(
                &session.session.id,
                "turn-instruction-sources",
            )?,
            expected_sources
        );
        reopened.remove_turn_request("turn-instruction-sources")?;
        assert!(
            reopened
                .get_turn_request("turn-instruction-sources")?
                .is_none()
        );
        assert_eq!(
            reopened.list_turn_instruction_sources(
                &session.session.id,
                "turn-instruction-sources",
            )?,
            expected_sources
        );
        Ok(())
    }

    #[test]
    fn turso_store_can_be_called_from_a_current_thread_runtime() -> Result<()> {
        let store = SessionStore::in_memory()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            assert_eq!(store.next_message_sequence("no-messages")?, 1);
            Ok::<(), anyhow::Error>(())
        })
    }

    #[test]
    fn editor_recovery_reopens_and_fences_stale_snapshots_and_dismissals()
    -> Result<()> {
        let workspace = tempfile::tempdir()?;
        let path = workspace.path().join("session.db");
        let owner = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        let store = SessionStore::open(&path)?;
        let snapshot = EditorRecoverySnapshot {
            buffer_id: uuid::Uuid::new_v4().to_string(),
            revision: 2,
            path: "src/main.ts".into(),
            contents: Some("unsaved α\n".into()),
            saved_sha256: Some(format!("{:x}", Sha256::digest(b"disk version"))),
        };
        assert!(store.write_editor_recovery(&owner, &snapshot)?);
        assert!(
            store.write_editor_recovery(&owner, &snapshot)?,
            "an identical replay is idempotent"
        );
        assert!(!store.write_editor_recovery(
            &owner,
            &EditorRecoverySnapshot {
                contents: Some("different same revision".into()),
                ..snapshot.clone()
            }
        )?);
        assert!(!store.write_editor_recovery(
            &owner,
            &EditorRecoverySnapshot {
                revision: 1,
                ..snapshot.clone()
            }
        )?);
        assert!(!store.write_editor_recovery(
            &other,
            &EditorRecoverySnapshot {
                revision: 3,
                ..snapshot.clone()
            }
        )?);
        let dismissal = EditorRecoverySnapshot {
            revision: 3,
            contents: None,
            ..snapshot.clone()
        };
        assert!(store.write_editor_recovery(&owner, &dismissal)?);
        assert!(
            !store.write_editor_recovery(&owner, &snapshot)?,
            "an old reply cannot resurrect discarded text"
        );
        assert!(store.editor_recovery_summaries(&owner)?.is_empty());
        let edited = EditorRecoverySnapshot {
            revision: 4,
            contents: Some("new typing after the prompt".into()),
            ..snapshot
        };
        assert!(store.write_editor_recovery(&owner, &edited)?);
        assert!(
            !store.write_editor_recovery(&owner, &dismissal)?,
            "a delayed dismissal cannot erase newer typing"
        );
        drop(store);
        let reopened = SessionStore::open(&path)?;
        assert_eq!(
            reopened.editor_recovery(&owner, &edited.buffer_id)?,
            Some(edited.clone())
        );
        assert_eq!(
            reopened.editor_recovery_summaries(&owner)?,
            vec![EditorRecoverySummary {
                buffer_id: edited.buffer_id,
                path: edited.path,
                revision: 4,
            }]
        );
        assert!(
            !workspace.path().join("src/main.ts").exists(),
            "recovery never writes project source files"
        );
        Ok(())
    }

    #[test]
    fn editor_recovery_rejects_invalid_or_unbounded_records() -> Result<()> {
        let store = SessionStore::in_memory()?;
        let owner = uuid::Uuid::new_v4().to_string();
        let snapshot = EditorRecoverySnapshot {
            buffer_id: uuid::Uuid::new_v4().to_string(),
            revision: 1,
            path: "src/main.py".into(),
            contents: Some("text".into()),
            saved_sha256: None,
        };
        for path in [
            "",
            "/outside.py",
            "../outside.py",
            "src/../../outside.py",
            "src\\outside.py",
            "nul\0.py",
        ] {
            assert!(
                store
                    .write_editor_recovery(
                        &owner,
                        &EditorRecoverySnapshot {
                            path: path.into(),
                            ..snapshot.clone()
                        }
                    )
                    .is_err(),
                "{path}"
            );
        }
        for revision in [0, u64::MAX] {
            assert!(
                store
                    .write_editor_recovery(
                        &owner,
                        &EditorRecoverySnapshot {
                            revision,
                            ..snapshot.clone()
                        }
                    )
                    .is_err()
            );
        }
        assert!(store.write_editor_recovery("invalid", &snapshot).is_err());
        assert!(
            store
                .write_editor_recovery(
                    &owner,
                    &EditorRecoverySnapshot {
                        buffer_id: "../bad".into(),
                        ..snapshot.clone()
                    }
                )
                .is_err()
        );
        assert!(
            store
                .write_editor_recovery(
                    &owner,
                    &EditorRecoverySnapshot {
                        saved_sha256: Some("not a digest".into()),
                        ..snapshot.clone()
                    }
                )
                .is_err()
        );
        assert!(
            store
                .write_editor_recovery(
                    &owner,
                    &EditorRecoverySnapshot {
                        contents: Some("x".repeat(MAX_EDITOR_RECOVERY_BYTES + 1)),
                        ..snapshot
                    }
                )
                .is_err()
        );
        assert!(store.editor_recovery_owners()?.is_empty());
        Ok(())
    }

    #[test]
    fn session_storage_creates_current_schema_and_reopens_without_rewriting()
    -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("session.db");
        let store = SessionStore::open(&path)?;
        let (application_id, version, obsolete_columns) = store.block_on(async {
            let mut rows = store.conn.query(
                "SELECT application_id, user_version,
                        (SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'mode')
                        + (SELECT COUNT(*) FROM pragma_table_info('anchors') WHERE name = 'created_at_commit')
                 FROM pragma_application_id, pragma_user_version",
                (),
            ).await?;
            let row = rows.next().await?.context("schema header")?;
            Ok::<_, anyhow::Error>((row.get::<i64>(0)?, row.get::<i64>(1)?, row.get::<i64>(2)?))
        })?;
        assert_eq!(application_id, SESSION_APPLICATION_ID);
        assert_eq!(version, SESSION_SCHEMA_VERSION);
        assert_eq!(obsolete_columns, 0);
        store.record_recent_workspace_file("src/main.rs", 42)?;
        drop(store);
        let before = fs::read(&path)?;
        let reopened = SessionStore::open(&path)?;
        assert_eq!(reopened.recent_workspace_files()?, ["src/main.rs"]);
        drop(reopened);
        assert_eq!(
            fs::read(path)?,
            before,
            "current-schema reopen must not rewrite the database"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn session_storage_restricts_database_journal_wal_and_shared_memory()
    -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir()?;
        for (filename, journal_mode, suffixes) in [
            ("journal.db", "PERSIST", vec!["", "-journal"]),
            ("wal.db", "WAL", vec!["", "-wal", "-shm"]),
        ] {
            let path = directory.path().join(filename);
            let store = SessionStore::open(&path)?;
            assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
            store.block_on(store.conn.execute_batch(&format!(
                "PRAGMA journal_mode={journal_mode}; PRAGMA wal_autocheckpoint=0;"
            )))?;
            store.record_recent_workspace_file("src/private.py", 1)?;
            for suffix in &suffixes {
                let file = directory.path().join(format!("{filename}{suffix}"));
                assert_eq!(
                    fs::metadata(&file)?.permissions().mode() & 0o777,
                    0o600,
                    "{}",
                    file.display()
                );
                fs::set_permissions(file, fs::Permissions::from_mode(0o644))?;
            }
            let reopened = SessionStore::open(&path)?;
            assert_eq!(reopened.recent_workspace_files()?, ["src/private.py"]);
            for suffix in &suffixes {
                let file = directory.path().join(format!("{filename}{suffix}"));
                assert_eq!(
                    fs::metadata(&file)?.permissions().mode() & 0o777,
                    0o600,
                    "{}",
                    file.display()
                );
            }
            // The original connection must keep working after another open hardens permissions.
            store.record_recent_workspace_file("src/second.ts", 2)?;
            assert_eq!(
                reopened.recent_workspace_files()?,
                ["src/second.ts", "src/private.py"]
            );
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn session_storage_rejects_linked_or_special_database_and_sidecar_paths()
    -> Result<()> {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = tempfile::tempdir()?;
        let original = directory.path().join("original.db");
        drop(SessionStore::open(&original)?);
        let original_bytes = fs::read(&original)?;
        let target = directory.path().join("must-not-change");
        fs::write(&target, "unchanged")?;
        let target_mode = fs::metadata(&target)?.permissions().mode();

        for (index, suffix) in
            ["", "-journal", "-wal", "-shm"].into_iter().enumerate()
        {
            for kind in ["symlink", "hardlink", "directory", "fifo"] {
                let name = format!("{kind}-{index}.db");
                let path = directory.path().join(&name);
                if !suffix.is_empty() {
                    fs::copy(&original, &path)?;
                }
                let entry = directory.path().join(format!("{name}{suffix}"));
                match kind {
                    "symlink" => symlink(&target, &entry)?,
                    "hardlink" => fs::hard_link(&target, &entry)?,
                    "directory" => fs::create_dir(&entry)?,
                    "fifo" => {
                        use std::os::unix::ffi::OsStrExt;
                        let name =
                            std::ffi::CString::new(entry.as_os_str().as_bytes())?;
                        // SAFETY: name is a live, NUL-terminated test-fixture path.
                        if unsafe { libc::mkfifo(name.as_ptr(), 0o600) } != 0 {
                            return Err(std::io::Error::last_os_error().into());
                        }
                    }
                    _ => unreachable!(),
                }
                assert!(
                    SessionStore::open(&path).is_err(),
                    "accepted {kind} {suffix}"
                );
                assert_eq!(fs::read(&target)?, b"unchanged");
                assert_eq!(fs::metadata(&target)?.permissions().mode(), target_mode);
                if !suffix.is_empty() {
                    assert_eq!(fs::read(&path)?, original_bytes);
                }
            }
        }
        let linked_directory = directory.path().join("linked-directory");
        symlink(directory.path(), &linked_directory)?;
        assert!(SessionStore::open(linked_directory.join("original.db")).is_err());

        let shared_directory = directory.path().join("shared");
        fs::create_dir(&shared_directory)?;
        fs::set_permissions(&shared_directory, fs::Permissions::from_mode(0o770))?;
        assert!(SessionStore::open(shared_directory.join("session.db")).is_err());
        assert!(!shared_directory.join("session.db").exists());

        let missing = directory.path().join("missing.db");
        fs::write(
            directory.path().join("missing.db-wal"),
            "preserve orphaned recovery",
        )?;
        assert!(SessionStore::open(&missing).is_err());
        assert!(!missing.exists());
        assert_eq!(
            fs::read(directory.path().join("missing.db-wal"))?,
            b"preserve orphaned recovery"
        );
        Ok(())
    }

    #[test]
    fn session_storage_rejects_unsupported_files_without_modifying_them()
    -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        for (application_id, version) in [
            (0, 0),
            (0, 1),
            (SESSION_APPLICATION_ID, 1),
            (SESSION_APPLICATION_ID, SESSION_SCHEMA_VERSION + 1),
        ] {
            let path = temporary
                .path()
                .join(format!("schema-{application_id}-{version}.db"));
            runtime.block_on(async {
                let database = Builder::new_local(&path).build().await?;
                let connection = database.connect()?;
                connection
                    .execute_batch(&format!(
                        "CREATE TABLE retained_user_data (value TEXT NOT NULL);
                     INSERT INTO retained_user_data VALUES ('do not alter');
                     PRAGMA application_id = {application_id};
                     PRAGMA user_version = {version};"
                    ))
                    .await?;
                Ok::<(), anyhow::Error>(())
            })?;
            let before = fs::read(&path)?;
            #[cfg(unix)]
            let before_mode = {
                use std::os::unix::fs::PermissionsExt;
                fs::metadata(&path)?.permissions().mode()
            };
            let error = SessionStore::open(&path)
                .err()
                .context("unsupported database was accepted")?;
            assert!(
                format!("{error:#}").contains("Unsupported AHEAD session database")
            );
            assert_eq!(
                fs::read(&path)?,
                before,
                "unsupported database was modified"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(&path)?.permissions().mode(),
                    before_mode,
                    "rejected database permissions must not change"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn session_storage_crash_fixture_child() -> Result<()> {
        let Some(path) = std::env::var_os("AHEAD_STORAGE_CRASH_FIXTURE") else {
            return Ok(());
        };
        let path = std::path::PathBuf::from(path);
        anyhow::ensure!(
            !path.exists(),
            "crash fixture must be a new disposable file"
        );
        let version: i64 = std::env::var("AHEAD_STORAGE_CRASH_VERSION")?.parse()?;
        if version == SESSION_SCHEMA_VERSION {
            drop(SessionStore::open(&path)?);
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let database = Builder::new_local(&path).build().await?;
            let connection = database.connect()?;
            connection
                .execute_batch(&format!(
                    "PRAGMA application_id = {SESSION_APPLICATION_ID};
                 PRAGMA user_version = {version};
                 PRAGMA journal_mode = DELETE;
                 PRAGMA synchronous = FULL;
                 CREATE TABLE crash_probe(id INTEGER PRIMARY KEY, payload BLOB);
                 INSERT INTO crash_probe VALUES (1, zeroblob(32768));
                 PRAGMA cache_size = 1;
                 BEGIN IMMEDIATE;
                 UPDATE crash_probe SET payload = zeroblob(65536) WHERE id = 1;"
                ))
                .await?;
            // Leave the real rollback journal without running connection destructors.
            std::process::exit(0);
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        })
    }

    #[test]
    fn session_storage_recovers_current_hot_journal_but_preserves_unsupported_files()
    -> Result<()> {
        let directory = tempfile::tempdir()?;
        for version in [1, SESSION_SCHEMA_VERSION] {
            let path = directory
                .path()
                .join(format!("crashed-#{version} space.db"));
            let output = std::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "ahead::store::tests::session_storage_crash_fixture_child",
                    "--nocapture",
                ])
                .env("AHEAD_STORAGE_CRASH_FIXTURE", &path)
                .env("AHEAD_STORAGE_CRASH_VERSION", version.to_string())
                .output()?;
            anyhow::ensure!(
                output.status.success(),
                "fixture child failed: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let journal = directory
                .path()
                .join(format!("crashed-#{version} space.db-journal"));
            let before = fs::read(&path)?;
            let journal_before = fs::read(&journal)?;
            assert!(
                journal_before.len() > 512
                    && journal_before[..8].iter().any(|byte| *byte != 0),
                "fixture must leave a hot rollback journal"
            );
            if version == SESSION_SCHEMA_VERSION {
                let store = SessionStore::open(&path)?;
                let size = store.block_on(async {
                    let mut rows = store
                        .conn
                        .query(
                            "SELECT length(payload) FROM crash_probe WHERE id = 1",
                            (),
                        )
                        .await?;
                    Ok::<_, anyhow::Error>(
                        rows.next()
                            .await?
                            .context("recovered row")?
                            .get::<i64>(0)?,
                    )
                })?;
                assert_eq!(size, 32768, "uncommitted changes must roll back");
            } else {
                let error = SessionStore::open(&path)
                    .err()
                    .context("accepted unsupported crashed store")?;
                assert!(
                    format!("{error:#}")
                        .contains("Unsupported AHEAD session database")
                );
                assert_eq!(fs::read(&path)?, before);
                assert_eq!(fs::read(&journal)?, journal_before);
            }
        }
        Ok(())
    }

    #[test]
    fn concurrent_agent_threads_append_without_interleaving_transactions()
    -> Result<()> {
        let store = Arc::new(SessionStore::in_memory()?);
        store.create_native_thread(
            "parallel",
            &serde_json::json!({"thread_id": "parallel"}),
        )?;
        let barrier = Arc::new(std::sync::Barrier::new(8));
        std::thread::scope(|scope| -> Result<()> {
            let mut workers = Vec::new();
            for worker in 0..8 {
                let store = store.clone();
                let barrier = barrier.clone();
                workers.push(scope.spawn(move || -> Result<()> {
                    barrier.wait();
                    for batch in 0..4 {
                        store.append_native_thread_items(
                            "parallel",
                            &[serde_json::json!({"worker": worker, "batch": batch})],
                        )?;
                    }
                    Ok(())
                }));
            }
            for worker in workers {
                worker
                    .join()
                    .map_err(|_| anyhow::anyhow!("agent store worker panicked"))??;
            }
            Ok(())
        })?;

        let snapshot = store
            .load_native_thread("parallel")?
            .context("concurrent thread missing")?;
        assert_eq!(snapshot.rollout_items.len(), 32);
        let unique_items = snapshot
            .rollout_items
            .iter()
            .map(ToString::to_string)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique_items.len(), 32);
        Ok(())
    }

    #[test]
    fn native_thread_headers_keep_metadata_order_and_archive_state() -> Result<()> {
        let store = SessionStore::in_memory()?;
        store.create_native_thread("first", &serde_json::json!({"id": "first"}))?;
        store
            .create_native_thread("second", &serde_json::json!({"id": "second"}))?;
        store.append_native_thread_items(
            "first",
            &[serde_json::json!({"replay": "only loaded on demand"})],
        )?;
        store.append_native_thread_metadata(
            "first",
            &serde_json::json!({"name": "before"}),
        )?;
        store.append_native_thread_metadata(
            "first",
            &serde_json::json!({"name": "after"}),
        )?;
        let archived_at = store
            .set_native_thread_archived("second", true)?
            .context("archive timestamp missing")?;

        let headers = store.list_native_thread_headers()?;
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0].thread_id, "first");
        assert_eq!(headers[0].create_params, serde_json::json!({"id": "first"}));
        assert_eq!(
            headers[0].metadata_patches,
            vec![
                serde_json::json!({"name": "before"}),
                serde_json::json!({"name": "after"}),
            ]
        );
        assert!(!headers[0].archived);
        assert!(headers[0].created_at.is_some());
        assert!(headers[0].updated_at >= headers[0].created_at);
        assert_eq!(headers[1].thread_id, "second");
        assert!(headers[1].archived);
        assert_eq!(headers[1].archived_at, Some(archived_at));
        assert_eq!(
            store
                .load_native_thread("first")?
                .context("full thread missing")?
                .rollout_items
                .len(),
            1
        );
        Ok(())
    }

    #[test]
    fn conversation_message_pages_use_exclusive_keyset_and_recover_stale_streams()
    -> Result<()> {
        let session = super::super::host::AheadSessionHost::in_memory()?
            .start_work(
                Some(WorkKind::ProductChange),
                "Conversation paging".to_string(),
                "Keep history bounded".to_string(),
                None,
            )?;
        let mut store = SessionStore::in_memory()?;
        store.insert_session(&session)?;

        for (id, sequence, role, turn_id, status) in [
            ("a", 1, "human", "turn-1", "complete"),
            ("b", 2, "agent", "active-turn", "streaming"),
            ("c", 2, "agent", "stale-turn", "streaming"),
            ("d", 3, "human", "turn-2", "complete"),
        ] {
            store.upsert_message(&ConversationMessage {
                id: id.to_string(),
                session_id: session.session.id.clone(),
                turn_id: turn_id.to_string(),
                sequence,
                role: role.to_string(),
                actor_id: role.to_string(),
                content: id.to_string(),
                status: status.to_string(),
                created_at: "2026-09-24T00:00:00Z".to_string(),
            })?;
        }

        store.recover_stale_streaming_messages(
            &session.session.id,
            Some("active-turn"),
        )?;

        let latest = store.list_messages_page(&session.session.id, None, 2)?;
        assert!(latest.has_older);
        assert_eq!(
            latest
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["c", "d"]
        );
        let cursor = ConversationMessageCursor {
            sequence: latest.messages[0].sequence,
            message_id: latest.messages[0].id.clone(),
        };
        let older =
            store.list_messages_page(&session.session.id, Some(cursor), 2)?;
        assert!(!older.has_older);
        assert_eq!(
            older
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(older.messages[1].status, "streaming");
        let all_messages = store.list_messages(&session.session.id)?;
        assert_eq!(
            all_messages
                .iter()
                .find(|message| message.id == "c")
                .context("stale streaming message missing")?
                .status,
            "failed"
        );
        Ok(())
    }

    #[test]
    fn native_thread_header_pages_use_keyset_order_and_include_only_page_metadata()
    -> Result<()> {
        let store = SessionStore::in_memory()?;
        for thread_id in ["a", "b", "c", "d"] {
            store.create_native_thread(
                thread_id,
                &serde_json::json!({"thread_id": thread_id}),
            )?;
        }
        for (thread_id, created_at, updated_at) in [
            (
                "a",
                "2025-01-01T00:00:00+00:00",
                "2025-01-01T00:00:00+00:00",
            ),
            (
                "b",
                "2025-01-02T00:00:00+00:00",
                "2025-01-02T00:00:00+00:00",
            ),
            (
                "c",
                "2025-01-03T00:00:00+00:00",
                "2025-01-02T00:00:00+00:00",
            ),
            (
                "d",
                "2025-01-04T00:00:00+00:00",
                "2025-01-04T00:00:00+00:00",
            ),
        ] {
            store.block_on(async {
                store
                    .conn
                    .execute(
                        "UPDATE agent_runtime_threads
                         SET created_at = ?1, updated_at = ?2
                         WHERE thread_id = ?3",
                        params![created_at, updated_at, thread_id],
                    )
                    .await?;
                Ok::<_, anyhow::Error>(())
            })?;
        }
        store.append_native_thread_metadata(
            "c",
            &serde_json::json!({"title": "kept with its page row"}),
        )?;
        store.set_native_thread_archived("d", true)?;

        let request = ahead_agent::NativeThreadHeaderPageRequest {
            archived: false,
            sort: ahead_agent::NativeThreadTimestampSort::UpdatedAt,
            direction: ahead_agent::NativeThreadSortDirection::Desc,
            cursor: None,
            allowed_sources: Vec::new(),
            model_providers: Vec::new(),
            cwd_filters: None,
            search_term: None,
            relation_filter: None,
            limit: 2,
        };
        let first_page = store
            .list_native_thread_headers_page(&request)?
            .context("indexed listing was unavailable")?;
        assert_eq!(
            first_page
                .iter()
                .map(|header| header.thread_id.as_str())
                .collect::<Vec<_>>(),
            vec!["c", "b"]
        );
        assert_eq!(
            first_page[0].metadata_patches,
            vec![serde_json::json!({"title": "kept with its page row"})]
        );

        let cursor = format!(
            "{}|{}",
            first_page[1]
                .updated_at
                .as_ref()
                .context("updated timestamp missing")?
                .to_rfc3339(),
            first_page[1].thread_id
        );
        let second_page = store
            .list_native_thread_headers_page(
                &ahead_agent::NativeThreadHeaderPageRequest {
                    cursor: Some(cursor),
                    ..request.clone()
                },
            )?
            .context("indexed listing was unavailable")?;
        assert_eq!(
            second_page
                .iter()
                .map(|header| header.thread_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );

        let archived = store
            .list_native_thread_headers_page(
                &ahead_agent::NativeThreadHeaderPageRequest {
                    archived: true,
                    limit: 2,
                    cursor: None,
                    ..request
                },
            )?
            .context("indexed listing was unavailable")?;
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].thread_id, "d");
        Ok(())
    }

    #[test]
    fn native_thread_header_page_filters_creation_fields_before_keyset_limit()
    -> Result<()> {
        let store = SessionStore::in_memory()?;
        let threads = [
            (
                "source-miss",
                serde_json::json!("exec"),
                "openai",
                "/workspace",
                "2025-01-06T00:00:00Z",
            ),
            (
                "provider-miss",
                serde_json::json!("cli"),
                "anthropic",
                "/workspace",
                "2025-01-05T00:00:00Z",
            ),
            (
                "cwd-miss",
                serde_json::json!("cli"),
                "openai",
                "/other",
                "2025-01-04T00:00:00Z",
            ),
            (
                "matching-new",
                serde_json::json!("cli"),
                "openai",
                "/workspace",
                "2025-01-03T00:00:00Z",
            ),
            (
                "matching-old",
                serde_json::json!({"custom": "custom-source"}),
                "openai",
                "/workspace",
                "2025-01-02T00:00:00Z",
            ),
        ];
        for (thread_id, source, provider, cwd, timestamp) in threads {
            store.create_native_thread(
                thread_id,
                &serde_json::json!({
                    "thread_id": thread_id,
                    "source": source,
                    "metadata": {"model_provider": provider, "cwd": cwd}
                }),
            )?;
            store.block_on(async {
                store
                    .conn
                    .execute(
                        "UPDATE agent_runtime_threads
                         SET created_at = ?1, updated_at = ?1
                         WHERE thread_id = ?2",
                        params![timestamp, thread_id],
                    )
                    .await?;
                Ok::<_, anyhow::Error>(())
            })?;
        }

        let request = ahead_agent::NativeThreadHeaderPageRequest {
            archived: false,
            sort: ahead_agent::NativeThreadTimestampSort::UpdatedAt,
            direction: ahead_agent::NativeThreadSortDirection::Desc,
            cursor: None,
            allowed_sources: vec![
                serde_json::json!("cli"),
                serde_json::json!({"custom": "custom-source"}),
            ],
            model_providers: vec!["openai".to_string()],
            cwd_filters: Some(vec![std::path::PathBuf::from("/workspace")]),
            search_term: None,
            relation_filter: None,
            limit: 1,
        };
        let first_page = store
            .list_native_thread_headers_page(&request)?
            .context("indexed listing was unavailable")?;
        assert_eq!(first_page.len(), 1);
        assert_eq!(first_page[0].thread_id, "matching-new");

        let cursor = format!(
            "{}|{}",
            first_page[0]
                .updated_at
                .as_ref()
                .context("updated timestamp missing")?
                .to_rfc3339(),
            first_page[0].thread_id
        );
        let second_page = store
            .list_native_thread_headers_page(
                &ahead_agent::NativeThreadHeaderPageRequest {
                    cursor: Some(cursor),
                    ..request
                },
            )?
            .context("indexed listing was unavailable")?;
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].thread_id, "matching-old");
        Ok(())
    }

    #[test]
    fn native_thread_header_page_filters_current_metadata_before_keyset_limit()
    -> Result<()> {
        let store = SessionStore::in_memory()?;
        for (thread_id, source, provider, cwd, timestamp) in [
            (
                "demoted",
                "cli",
                "openai",
                "/workspace",
                "2025-01-03T00:00:00Z",
            ),
            (
                "promoted",
                "exec",
                "anthropic",
                "/other",
                "2025-01-02T00:00:00Z",
            ),
            (
                "unchanged",
                "cli",
                "openai",
                "/workspace",
                "2025-01-01T00:00:00Z",
            ),
        ] {
            store.create_native_thread(
                thread_id,
                &serde_json::json!({
                    "thread_id": thread_id,
                    "source": source,
                    "metadata": {"model_provider": provider, "cwd": cwd}
                }),
            )?;
            if thread_id == "demoted" {
                store.append_native_thread_metadata(
                    thread_id,
                    &serde_json::json!({
                        "source": "exec", "model_provider": "anthropic", "cwd": "/other"
                    }),
                )?;
            } else if thread_id == "promoted" {
                for patch in [
                    serde_json::json!({"source": {"custom": "zed"}}),
                    serde_json::json!({"model_provider": "openai"}),
                    serde_json::json!({"cwd": "/workspace"}),
                    serde_json::json!({"title": "latest unrelated patch"}),
                ] {
                    store.append_native_thread_metadata(thread_id, &patch)?;
                }
            }
            store.block_on(async {
                store
                    .conn
                    .execute(
                        "UPDATE agent_runtime_threads SET created_at = ?1, updated_at = ?1 WHERE thread_id = ?2",
                        params![timestamp, thread_id],
                    )
                    .await?;
                Ok::<_, anyhow::Error>(())
            })?;
        }

        let request = ahead_agent::NativeThreadHeaderPageRequest {
            archived: false,
            sort: ahead_agent::NativeThreadTimestampSort::UpdatedAt,
            direction: ahead_agent::NativeThreadSortDirection::Desc,
            cursor: None,
            allowed_sources: vec![
                serde_json::json!("cli"),
                serde_json::json!({"custom": "zed"}),
            ],
            model_providers: vec!["openai".to_string()],
            cwd_filters: Some(vec![std::path::PathBuf::from("/workspace")]),
            search_term: None,
            relation_filter: None,
            limit: 1,
        };
        let first_page = store
            .list_native_thread_headers_page(&request)?
            .context("indexed listing was unavailable")?;
        assert_eq!(first_page.len(), 1);
        assert_eq!(first_page[0].thread_id, "promoted");

        let cursor = format!(
            "{}|{}",
            first_page[0]
                .updated_at
                .as_ref()
                .context("updated timestamp missing")?
                .to_rfc3339(),
            first_page[0].thread_id
        );
        let second_page = store
            .list_native_thread_headers_page(
                &ahead_agent::NativeThreadHeaderPageRequest {
                    cursor: Some(cursor),
                    ..request
                },
            )?
            .context("indexed listing was unavailable")?;
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].thread_id, "unchanged");
        Ok(())
    }

    #[test]
    fn native_thread_header_page_searches_current_metadata_before_keyset_limit()
    -> Result<()> {
        let store = SessionStore::in_memory()?;
        for (thread_id, first_user_message) in [
            ("no-match", None),
            ("stale-preview", None),
            ("cleared-name", None),
            ("name-match", None),
            ("preview-match", None),
            ("message-match", None),
            ("title-match", None),
            (
                "creation-message-match",
                Some("needle in the initial message"),
            ),
        ] {
            store.create_native_thread(
                thread_id,
                &serde_json::json!({
                    "thread_id": thread_id,
                    "metadata": {"first_user_message": first_user_message}
                }),
            )?;
        }
        store.append_native_thread_metadata(
            "stale-preview",
            &serde_json::json!({"preview": "needle, but stale"}),
        )?;
        store.append_native_thread_metadata(
            "stale-preview",
            &serde_json::json!({"preview": "current preview"}),
        )?;
        store.append_native_thread_metadata(
            "cleared-name",
            &serde_json::json!({"name": "needle, but cleared"}),
        )?;
        store.append_native_thread_metadata(
            "cleared-name",
            &serde_json::json!({"name": null}),
        )?;
        store.append_native_thread_metadata(
            "name-match",
            &serde_json::json!({"name": "needle in name"}),
        )?;
        store.append_native_thread_metadata(
            "preview-match",
            &serde_json::json!({"preview": "needle in preview"}),
        )?;
        store.append_native_thread_metadata(
            "message-match",
            &serde_json::json!({"first_user_message": "needle in message"}),
        )?;
        store.append_native_thread_metadata(
            "title-match",
            &serde_json::json!({"title": "needle in title"}),
        )?;
        for (thread_id, timestamp) in [
            ("no-match", "2025-01-08T00:00:00Z"),
            ("stale-preview", "2025-01-07T00:00:00Z"),
            ("cleared-name", "2025-01-06T00:00:00Z"),
            ("name-match", "2025-01-05T00:00:00Z"),
            ("preview-match", "2025-01-04T00:00:00Z"),
            ("message-match", "2025-01-03T00:00:00Z"),
            ("title-match", "2025-01-02T00:00:00Z"),
            ("creation-message-match", "2025-01-01T00:00:00Z"),
        ] {
            store.block_on(async {
                store
                    .conn
                    .execute(
                        "UPDATE agent_runtime_threads
                         SET created_at = ?1, updated_at = ?1
                         WHERE thread_id = ?2",
                        params![timestamp, thread_id],
                    )
                    .await?;
                Ok::<_, anyhow::Error>(())
            })?;
        }

        let request = ahead_agent::NativeThreadHeaderPageRequest {
            archived: false,
            sort: ahead_agent::NativeThreadTimestampSort::UpdatedAt,
            direction: ahead_agent::NativeThreadSortDirection::Desc,
            cursor: None,
            allowed_sources: Vec::new(),
            model_providers: Vec::new(),
            cwd_filters: None,
            search_term: Some("needle".to_string()),
            relation_filter: None,
            limit: 2,
        };
        let first_page = store
            .list_native_thread_headers_page(&request)?
            .context("indexed search listing was unavailable")?;
        assert_eq!(
            first_page
                .iter()
                .map(|header| header.thread_id.as_str())
                .collect::<Vec<_>>(),
            vec!["name-match", "preview-match"]
        );

        let cursor = format!(
            "{}|{}",
            first_page[1]
                .updated_at
                .as_ref()
                .context("updated timestamp missing")?
                .to_rfc3339(),
            first_page[1].thread_id
        );
        let second_page = store
            .list_native_thread_headers_page(
                &ahead_agent::NativeThreadHeaderPageRequest {
                    cursor: Some(cursor),
                    ..request.clone()
                },
            )?
            .context("indexed search listing was unavailable")?;
        assert_eq!(
            second_page
                .iter()
                .map(|header| header.thread_id.as_str())
                .collect::<Vec<_>>(),
            vec!["message-match", "title-match"]
        );

        let cursor = format!(
            "{}|{}",
            second_page[1]
                .updated_at
                .as_ref()
                .context("updated timestamp missing")?
                .to_rfc3339(),
            second_page[1].thread_id
        );
        let third_page = store
            .list_native_thread_headers_page(
                &ahead_agent::NativeThreadHeaderPageRequest {
                    cursor: Some(cursor),
                    ..request.clone()
                },
            )?
            .context("indexed search listing was unavailable")?;
        assert_eq!(third_page.len(), 1);
        assert_eq!(third_page[0].thread_id, "creation-message-match");

        let case_mismatch = store
            .list_native_thread_headers_page(
                &ahead_agent::NativeThreadHeaderPageRequest {
                    search_term: Some("Needle".to_string()),
                    ..request
                },
            )?
            .context("indexed search listing was unavailable")?;
        assert!(case_mismatch.is_empty());
        Ok(())
    }

    #[test]
    fn native_thread_header_page_filters_spawn_relations_before_keyset_limit()
    -> Result<()> {
        let store = SessionStore::in_memory()?;
        let threads = [
            ("root", None, "2025-01-06T00:00:00Z"),
            ("unrelated", None, "2025-01-05T00:00:00Z"),
            ("child-a", Some("root"), "2025-01-04T00:00:00Z"),
            ("grandchild", Some("child-a"), "2025-01-03T00:00:00Z"),
            ("child-b", Some("root"), "2025-01-02T00:00:00Z"),
        ];
        for (thread_id, parent_thread_id, timestamp) in threads {
            store.create_native_thread(
                thread_id,
                &serde_json::json!({
                    "thread_id": thread_id,
                    "parent_thread_id": parent_thread_id,
                }),
            )?;
            store.block_on(async {
                store
                    .conn
                    .execute(
                        "UPDATE agent_runtime_threads
                         SET created_at = ?1, updated_at = ?1
                         WHERE thread_id = ?2",
                        params![timestamp, thread_id],
                    )
                    .await?;
                Ok::<_, anyhow::Error>(())
            })?;
        }

        let direct_request = ahead_agent::NativeThreadHeaderPageRequest {
            archived: false,
            sort: ahead_agent::NativeThreadTimestampSort::UpdatedAt,
            direction: ahead_agent::NativeThreadSortDirection::Desc,
            cursor: None,
            allowed_sources: Vec::new(),
            model_providers: Vec::new(),
            cwd_filters: None,
            search_term: None,
            relation_filter: Some(
                ahead_agent::NativeThreadRelationFilter::DirectChildrenOf(
                    "root".to_string(),
                ),
            ),
            limit: 1,
        };
        let first_direct_page = store
            .list_native_thread_headers_page(&direct_request)?
            .context("indexed direct-child listing was unavailable")?;
        assert_eq!(
            first_direct_page
                .iter()
                .map(|header| header.thread_id.as_str())
                .collect::<Vec<_>>(),
            vec!["child-a"]
        );
        let first_direct = first_direct_page
            .first()
            .context("direct-child page unexpectedly empty")?;
        let direct_cursor = format!(
            "{}|{}",
            first_direct
                .updated_at
                .as_ref()
                .context("updated timestamp missing")?
                .to_rfc3339(),
            first_direct.thread_id
        );
        let second_direct_page = store
            .list_native_thread_headers_page(
                &ahead_agent::NativeThreadHeaderPageRequest {
                    cursor: Some(direct_cursor),
                    ..direct_request
                },
            )?
            .context("indexed direct-child listing was unavailable")?;
        assert_eq!(
            second_direct_page
                .iter()
                .map(|header| header.thread_id.as_str())
                .collect::<Vec<_>>(),
            vec!["child-b"]
        );

        let descendant_request = ahead_agent::NativeThreadHeaderPageRequest {
            archived: false,
            sort: ahead_agent::NativeThreadTimestampSort::UpdatedAt,
            direction: ahead_agent::NativeThreadSortDirection::Desc,
            cursor: None,
            allowed_sources: Vec::new(),
            model_providers: Vec::new(),
            cwd_filters: None,
            search_term: None,
            relation_filter: Some(
                ahead_agent::NativeThreadRelationFilter::DescendantsOf(
                    "root".to_string(),
                ),
            ),
            limit: 2,
        };
        let first_descendant_page = store
            .list_native_thread_headers_page(&descendant_request)?
            .context("indexed descendant listing was unavailable")?;
        assert_eq!(
            first_descendant_page
                .iter()
                .map(|header| header.thread_id.as_str())
                .collect::<Vec<_>>(),
            vec!["child-a", "grandchild"]
        );
        let last_descendant = first_descendant_page
            .get(1)
            .context("descendant page did not contain its expected second row")?;
        let descendant_cursor = format!(
            "{}|{}",
            last_descendant
                .updated_at
                .as_ref()
                .context("updated timestamp missing")?
                .to_rfc3339(),
            last_descendant.thread_id
        );
        let second_descendant_page = store
            .list_native_thread_headers_page(
                &ahead_agent::NativeThreadHeaderPageRequest {
                    cursor: Some(descendant_cursor),
                    ..descendant_request
                },
            )?
            .context("indexed descendant listing was unavailable")?;
        assert_eq!(
            second_descendant_page
                .iter()
                .map(|header| header.thread_id.as_str())
                .collect::<Vec<_>>(),
            vec!["child-b"]
        );
        Ok(())
    }

    #[test]
    fn native_thread_replay_pages_use_exclusive_ordinal_cursors() -> Result<()> {
        let store = SessionStore::in_memory()?;
        store.create_native_thread(
            "thread",
            &serde_json::json!({"thread_id": "thread"}),
        )?;
        store.append_native_thread_items(
            "thread",
            &(0..5)
                .map(|ordinal| serde_json::json!(ordinal))
                .collect::<Vec<_>>(),
        )?;

        let first = store.load_native_thread_item_page("thread", None, 2)?;
        assert_eq!(
            first.items,
            vec![serde_json::json!(4), serde_json::json!(3)]
        );
        assert_eq!(first.next_before_ordinal, Some(3));

        let second = store.load_native_thread_item_page(
            "thread",
            first.next_before_ordinal,
            2,
        )?;
        assert_eq!(
            second.items,
            vec![serde_json::json!(2), serde_json::json!(1)]
        );
        assert_eq!(second.next_before_ordinal, Some(1));

        let final_page = store.load_native_thread_item_page(
            "thread",
            second.next_before_ordinal,
            2,
        )?;
        assert_eq!(final_page.items, vec![serde_json::json!(0)]);
        assert_eq!(final_page.next_before_ordinal, None);
        assert!(
            store
                .load_native_thread_item_page("thread", Some(-1), 2)
                .is_err()
        );
        assert!(
            store
                .load_native_thread_item_page("thread", None, 0)
                .is_err()
        );

        let header = store
            .load_native_thread_header("thread")?
            .context("native thread header missing")?;
        assert_eq!(header.thread_id, "thread");
        assert!(header.create_params.get("thread_id").is_some());
        Ok(())
    }

    #[test]
    fn native_agent_model_issued_spawn_and_resume_survives_turso_reopen()
    -> Result<()> {
        std::thread::Builder::new()
            .name("ahead-agent-durable-spawn-test".to_string())
            .stack_size(8 * 1024 * 1024)
            .spawn(native_agent_model_issued_spawn_and_resume_survives_turso_reopen_inner)
            .context("failed to start the durable subagent test thread")?
            .join()
            .map_err(|_| anyhow::anyhow!("durable subagent test thread panicked"))?
    }

    fn native_agent_model_issued_spawn_and_resume_survives_turso_reopen_inner()
    -> Result<()> {
        let workspace = tempfile::tempdir()?;
        fs::create_dir_all(workspace.path().join(".ahead"))?;
        fs::write(
            workspace.path().join("AGENTS.md"),
            "AHEAD_OPEN_STANDARD_INSTRUCTIONS_20260928",
        )?;
        fs::write(
            workspace.path().join(".rules"),
            "AHEAD_ZED_RULES_MUST_NOT_LOAD_20260928",
        )?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        fs::write(
            workspace.path().join(".ahead/settings.toml"),
            format!(
                "[ai]\nactive_connection = \"Mock\"\n[[ai.connections]]\nname = \"Mock\"\nprovider_id = \"mock\"\nbase_url = \"http://{address}/v1\"\nmodel = \"ahead-test\"\n"
            ),
        )?;
        let model_requests = Arc::new(Mutex::new(Vec::new()));
        let captured_requests = model_requests.clone();
        let child_id_for_resume = Arc::new(Mutex::new(None::<String>));
        let server_child_id = child_id_for_resume.clone();
        let model_server = std::thread::spawn(move || {
            for response_index in 0..76 {
                let (mut stream, _) =
                    listener.accept().expect("accept mock model request");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 8192];
                loop {
                    let read = stream.read(&mut buffer).expect("read model request");
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let Some(header_end) = request
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .map(|index| index + 4)
                    else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + content_length {
                        captured_requests.lock().push(
                            String::from_utf8_lossy(
                                &request[header_end..header_end + content_length],
                            )
                            .into_owned(),
                        );
                        break;
                    }
                }
                let tool_arguments = match response_index {
                    0 => Some(
                        serde_json::json!({
                            "label": "Initial durable child",
                            "message": "Remember AHEAD_TURSO_CHILD_CONTEXT_20260925"
                        })
                        .to_string(),
                    ),
                    73 => {
                        let child_thread_id = server_child_id
                            .lock()
                            .clone()
                            .expect("capture child ID before the resume request");
                        Some(
                            serde_json::json!({
                                "label": "Resume durable child",
                                "message": "AHEAD_TURSO_CHILD_RESUME_20260925",
                                "session_id": child_thread_id
                            })
                            .to_string(),
                        )
                    }
                    _ => None,
                };
                let events: Vec<(&str, serde_json::Value)> = if let Some(arguments) =
                    tool_arguments
                {
                    let call_id = format!("call-ahead-turso-{response_index}");
                    let item_id = format!("function-ahead-turso-{response_index}");
                    vec![
                        (
                            "response.output_item.added",
                            serde_json::json!({
                                "type": "response.output_item.added",
                                "output_index": 0,
                                "item": {
                                    "id": item_id.clone(),
                                    "type": "function_call",
                                    "call_id": call_id.clone(),
                                    "name": "spawn_agent",
                                    "arguments": ""
                                }
                            }),
                        ),
                        (
                            "response.output_item.done",
                            serde_json::json!({
                                "type": "response.output_item.done",
                                "output_index": 0,
                                "item": {
                                    "id": item_id,
                                    "type": "function_call",
                                    "call_id": call_id,
                                    "name": "spawn_agent",
                                    "arguments": arguments
                                }
                            }),
                        ),
                        (
                            "response.completed",
                            serde_json::json!({
                                "type": "response.completed",
                                "response": {
                                    "id": format!("resp-ahead-turso-{response_index}"),
                                    "end_turn": false
                                }
                            }),
                        ),
                    ]
                } else {
                    let message_id = format!("msg-ahead-turso-{response_index}");
                    vec![
                        (
                            "response.output_item.added",
                            serde_json::json!({
                                "type": "response.output_item.added",
                                "output_index": 0,
                                "item": {
                                    "id": message_id.clone(),
                                    "type": "message",
                                    "role": "assistant",
                                    "content": []
                                }
                            }),
                        ),
                        (
                            "response.output_text.delta",
                            serde_json::json!({
                                "type": "response.output_text.delta",
                                "item_id": message_id,
                                "output_index": 0,
                                "content_index": 0,
                                "delta": "PONG"
                            }),
                        ),
                        (
                            "response.output_item.done",
                            serde_json::json!({
                                "type": "response.output_item.done",
                                "output_index": 0,
                                "item": {
                                    "id": format!("msg-ahead-turso-{response_index}"),
                                    "type": "message",
                                    "role": "assistant",
                                    "content": [{"type": "output_text", "text": "PONG"}]
                                }
                            }),
                        ),
                        (
                            "response.completed",
                            serde_json::json!({
                                "type": "response.completed",
                                "response": {
                                    "id": format!("resp-ahead-turso-{response_index}"),
                                    "end_turn": true
                                }
                            }),
                        ),
                    ]
                };
                let body = events
                    .into_iter()
                    .map(|(event, payload)| {
                        format!("event: {event}\ndata: {payload}\n\n")
                    })
                    .collect::<String>();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write mock model response");
            }
        });
        let session = super::super::host::AheadSessionHost::in_memory()?
            .start_work(
                Some(WorkKind::ProductChange),
                "Turso chat".to_string(),
                "Test the shared store".to_string(),
                None,
            )?;
        let database_path = workspace.path().join("session.db");
        let mut database = SessionStore::open(&database_path)?;
        database.insert_session(&session)?;
        let store = Arc::new(parking_lot::RwLock::new(database));
        let harness_store: Arc<dyn ahead_agent::HarnessStore> =
            Arc::new(SharedSessionStore(store.clone()));
        let worker = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        worker.block_on(async {
            harness_store.upsert_message(&ConversationMessage {
                id: "chat-1".to_string(),
                session_id: session.session.id.clone(),
                turn_id: "turn-1".to_string(),
                sequence: 1,
                role: "agent".to_string(),
                actor_id: "ahead".to_string(),
                content: "Hello".to_string(),
                status: "streaming".to_string(),
                created_at: "2026-09-23T00:00:00Z".to_string(),
            })?;
            assert_eq!(
                harness_store.append_message_delta("chat-1", " from worker")?,
                "Hello from worker"
            );
            harness_store.set_message_status("chat-1", "complete")?;
            Ok::<(), anyhow::Error>(())
        })?;
        let config =
            ahead_agent::NativeClientConfig::ahead(workspace.path().to_path_buf());

        let client = ahead_agent::NativeClient::spawn(
            &config,
            harness_store.clone(),
            Arc::new(|_| {}),
        )?;
        let thread_id = client.new_session(
            workspace.path(),
            "read-only",
            Some("ahead-test"),
            Some("mock"),
        )?;
        client.set_scope(&thread_id, workspace.path(), &[], true)?;
        client.prompt(&thread_id, "AHEAD_TURSO_PARENT_ONLY_CONTEXT_20260928")?;
        let child_thread_id = store
            .read()
            .list_native_agent_children(&thread_id, None)?
            .into_iter()
            .next()
            .context(
                "model-issued spawn_agent did not persist a Turso child edge",
            )?;
        *child_id_for_resume.lock() = Some(child_thread_id.clone());
        for native_thread_id in [&thread_id, &child_thread_id] {
            let snapshot = store
                .read()
                .load_native_thread(native_thread_id)?
                .context("new native thread snapshot missing")?;
            assert_eq!(
                snapshot.create_params["history_mode"],
                serde_json::json!("paginated")
            );
        }
        assert_eq!(
            store.read().list_native_agent_children(
                &thread_id,
                Some(ahead_agent::NativeAgentEdgeStatus::Closed),
            )?,
            [child_thread_id.as_str()]
        );
        for turn in 0..70 {
            client.prompt(
                &child_thread_id,
                &format!("Additional child turn {turn}"),
            )?;
        }
        assert!(store.read().load_native_thread(&thread_id)?.is_some());
        let child_snapshot = store
            .read()
            .load_native_thread(&child_thread_id)?
            .context("native child snapshot missing after model turns")?;
        assert!(
            child_snapshot.rollout_items.len() > 128,
            "fixture must cross a Turso replay-page boundary"
        );
        for entry in fs::read_dir(workspace.path().join(".ahead/runtime"))? {
            let path = entry?.path();
            assert_ne!(
                path.extension().and_then(std::ffi::OsStr::to_str),
                Some("sqlite"),
                "native agent opened a second database: {}",
                path.display()
            );
        }
        client.shutdown();
        drop(client);
        drop(harness_store);
        drop(store);

        let store = Arc::new(parking_lot::RwLock::new(SessionStore::open(
            &database_path,
        )?));
        let harness_store: Arc<dyn ahead_agent::HarnessStore> =
            Arc::new(SharedSessionStore(store.clone()));
        let messages = store.read().list_messages(&session.session.id)?;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Hello from worker");
        assert_eq!(messages[0].status, "complete");
        let reopened = ahead_agent::NativeClient::spawn(
            &config,
            harness_store,
            Arc::new(|_| {}),
        )?;
        assert!(store.read().load_native_thread(&thread_id)?.is_some());
        reopened.load_session(
            &thread_id,
            workspace.path(),
            "read-only",
            Some("ahead-test"),
            Some("mock"),
        )?;
        reopened.set_scope(&thread_id, workspace.path(), &[], true)?;
        reopened.prompt(&thread_id, "Resume the durable child by session_id")?;
        model_server.join().expect("join mock model");
        let requests = model_requests.lock();
        assert_eq!(requests.len(), 76);
        let parent_request = &requests[0];
        let first_child_request = &requests[1];
        assert!(
            parent_request.contains("AHEAD_TURSO_PARENT_ONLY_CONTEXT_20260928"),
            "parent request omitted its private context sentinel"
        );
        assert!(
            parent_request.contains("AHEAD_OPEN_STANDARD_INSTRUCTIONS_20260928"),
            "native model request omitted project AGENTS.md instructions"
        );
        assert!(
            !parent_request.contains("AHEAD_ZED_RULES_MUST_NOT_LOAD_20260928"),
            "native model request loaded an editor-specific .rules file"
        );
        assert!(
            first_child_request
                .contains("Remember AHEAD_TURSO_CHILD_CONTEXT_20260925"),
            "child request omitted the explicit delegation message: {}",
            first_child_request
        );
        assert!(
            !first_child_request
                .contains("AHEAD_TURSO_PARENT_ONLY_CONTEXT_20260928"),
            "child request inherited parent conversation context: {}",
            first_child_request
        );
        assert!(
            requests[74].contains("AHEAD_TURSO_CHILD_CONTEXT_20260925"),
            "resumed child model request omitted its persisted paginated context: {}",
            requests[74]
        );
        assert!(
            requests[74].contains("AHEAD_TURSO_CHILD_RESUME_20260925"),
            "resumed child model request omitted the new child prompt: {}",
            requests[74]
        );
        assert_eq!(
            store.read().list_native_agent_children(
                &thread_id,
                Some(ahead_agent::NativeAgentEdgeStatus::Closed),
            )?,
            [child_thread_id.as_str()]
        );
        reopened.shutdown();
        Ok(())
    }

    #[test]
    fn native_agent_child_file_edit_records_parent_work_session_anchor() -> Result<()>
    {
        let workspace = tempfile::tempdir()?;
        fs::create_dir_all(workspace.path().join(".ahead"))?;
        fs::create_dir_all(workspace.path().join("src"))?;
        let workspace_path = fs::canonicalize(workspace.path())?;
        let database_path = workspace.path().join("session.db");
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        fs::write(
            workspace.path().join(".ahead/settings.toml"),
            format!(
                "[ai]\nactive_connection = \"Mock\"\n[[ai.connections]]\nname = \"Mock\"\nprovider_id = \"mock\"\nbase_url = \"http://{address}/v1\"\nmodel = \"ahead-test\"\n"
            ),
        )?;
        listener.set_nonblocking(true)?;
        let model_requests = Arc::new(Mutex::new(Vec::new()));
        let captured_requests = model_requests.clone();
        let model_server = std::thread::spawn(move || -> Result<()> {
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            for response_index in 0..4 {
                let (mut stream, _) = loop {
                    match listener.accept() {
                        Ok(connection) => break connection,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && std::time::Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => return Err(error.into()),
                    }
                };
                // macOS accepted sockets inherit the listener's nonblocking mode.
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(Duration::from_secs(10)))?;
                let mut request = Vec::new();
                let mut buffer = [0_u8; 8192];
                loop {
                    let read = stream.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let Some(header_end) = request
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .map(|index| index + 4)
                    else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + content_length {
                        break;
                    }
                }
                let request_body = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .and_then(|header_end| {
                        serde_json::from_slice::<serde_json::Value>(
                            &request[header_end + 4..],
                        )
                        .ok()
                    });
                let tool_names = request_body
                    .as_ref()
                    .and_then(|body| {
                        body.get("tools")
                            .and_then(serde_json::Value::as_array)
                            .cloned()
                    })
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|tool| {
                        tool.get("name")
                            .or_else(|| {
                                tool.get("function")
                                    .and_then(|function| function.get("name"))
                            })
                            .and_then(serde_json::Value::as_str)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let tool_outputs = request_body
                    .as_ref()
                    .and_then(|body| {
                        body.get("input").and_then(serde_json::Value::as_array)
                    })
                    .into_iter()
                    .flatten()
                    .filter(|item| {
                        item.get("type").and_then(serde_json::Value::as_str)
                            == Some("function_call_output")
                    })
                    .filter_map(|item| {
                        item.get("output").map(|output| {
                            output.to_string().chars().take(500).collect::<String>()
                        })
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                let serialized_request = request_body
                    .as_ref()
                    .map(serde_json::Value::to_string)
                    .unwrap_or_default();
                captured_requests.lock().push(format!(
                    "{tool_names}, {serialized_request}; outputs: {tool_outputs}"
                ));

                let tool_call = match response_index {
                    0 => Some((
                        "spawn_agent",
                        serde_json::json!({
                            "label": "Create attributed file",
                            "message": "Create src/agent-authored.txt with the line agent authored"
                        })
                        .to_string(),
                    )),
                    1 => Some((
                        "exec_command",
                        serde_json::json!({
                            "cmd": "apply_patch <<'PATCH'\n*** Begin Patch\n*** Add File: src/agent-authored.txt\n+agent authored\n*** End Patch\nPATCH",
                            "yield_time_ms": 10000
                        })
                        .to_string(),
                    )),
                    _ => None,
                };
                let events = if let Some((tool, arguments)) = tool_call {
                    let call_id = format!("call-ahead-anchor-{response_index}");
                    let item_id = format!("function-ahead-anchor-{response_index}");
                    vec![
                        (
                            "response.output_item.added",
                            serde_json::json!({
                                "type": "response.output_item.added",
                                "output_index": 0,
                                "item": {
                                    "id": item_id,
                                    "type": "function_call",
                                    "call_id": call_id,
                                    "name": tool,
                                    "arguments": ""
                                }
                            }),
                        ),
                        (
                            "response.output_item.done",
                            serde_json::json!({
                                "type": "response.output_item.done",
                                "output_index": 0,
                                "item": {
                                    "id": item_id,
                                    "type": "function_call",
                                    "call_id": call_id,
                                    "name": tool,
                                    "arguments": arguments
                                }
                            }),
                        ),
                        (
                            "response.completed",
                            serde_json::json!({
                                "type": "response.completed",
                                "response": {
                                    "id": format!("resp-ahead-anchor-{response_index}"),
                                    "end_turn": false
                                }
                            }),
                        ),
                    ]
                } else {
                    let message_id = format!("msg-ahead-anchor-{response_index}");
                    vec![
                        (
                            "response.output_item.added",
                            serde_json::json!({
                                "type": "response.output_item.added",
                                "output_index": 0,
                                "item": {
                                    "id": message_id,
                                    "type": "message",
                                    "role": "assistant",
                                    "content": []
                                }
                            }),
                        ),
                        (
                            "response.output_text.delta",
                            serde_json::json!({
                                "type": "response.output_text.delta",
                                "item_id": message_id,
                                "output_index": 0,
                                "content_index": 0,
                                "delta": "Done"
                            }),
                        ),
                        (
                            "response.output_item.done",
                            serde_json::json!({
                                "type": "response.output_item.done",
                                "output_index": 0,
                                "item": {
                                    "id": message_id,
                                    "type": "message",
                                    "role": "assistant",
                                    "content": [{"type": "output_text", "text": "Done"}]
                                }
                            }),
                        ),
                        (
                            "response.completed",
                            serde_json::json!({
                                "type": "response.completed",
                                "response": {
                                    "id": format!("resp-ahead-anchor-{response_index}"),
                                    "end_turn": true
                                }
                            }),
                        ),
                    ]
                };
                let body = events
                    .into_iter()
                    .map(|(event, payload)| {
                        format!("event: {event}\ndata: {payload}\n\n")
                    })
                    .collect::<String>();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )?;
            }
            Ok(())
        });

        let host = super::super::host::AheadSessionHost::new(SessionStore::open(
            &database_path,
        )?);
        host.set_workspace(workspace_path.clone());
        let view = host.start_work(
            Some(WorkKind::ProductChange),
            "Create an attributed file".to_string(),
            "Implement a small file change".to_string(),
            None,
        )?;
        let session_id = view.session.id.clone();
        let work_item = host.handle_request(AheadRequest::WorkItemCreate {
            session_id: session_id.clone(),
            title: "AHEAD_TURN_PLAN_SENTINEL: create one attributed file"
                .to_string(),
        })?;
        let work_item_id = work_item["id"]
            .as_str()
            .context("created work item id")?
            .to_string();
        host.handle_request(AheadRequest::WorkItemNote {
            item_id: work_item_id.clone(),
            kind: "invariant".to_string(),
            body_markdown:
                "AHEAD_TURN_INVARIANT_SENTINEL: do not write any other path."
                    .to_string(),
        })?;
        host.handle_request(AheadRequest::WorkItemNote {
            item_id: work_item_id,
            kind: "decision".to_string(),
            body_markdown:
                "AHEAD_TURN_DECISION_SENTINEL: use the native write tool."
                    .to_string(),
        })?;
        host.handle_request(AheadRequest::ConversationSummarize {
            session_id: session_id.clone(),
            phase: "plan".to_string(),
            summary_markdown:
                "AHEAD_TURN_SUMMARY_SENTINEL: only create the requested file."
                    .to_string(),
            message_id_range: "message-1..message-2".to_string(),
        })?;
        let (notification_sender, notification_receiver) =
            std::sync::mpsc::channel();
        host.set_notification_sink(Arc::new(move |notification| {
            notification_sender
                .send(notification)
                .expect("keep turn notification receiver alive");
        }));
        host.handle_request(AheadRequest::AgentTurnStart {
            request: AgentTurnRequestDto {
                session_id: session_id.clone(),
                thread_id: session_id.clone(),
                harness: HarnessKind::Ahead,
                external_agent_id: None,
                model: Some("ahead-test".to_string()),
                model_provider: Some("mock".to_string()),
                user_message: "Delegate creating one file".to_string(),
                session_context: "UNTRUSTED_CALLER_CONTEXT_MUST_NOT_REACH_MODEL"
                    .to_string(),
                context: TurnEditorContext {
                    active_path: "src/agent-authored.txt".to_string(),
                    caret: DisplayPosition { line: 0, col: 0 },
                    selection: None,
                    file_content: String::new(),
                    visible_end: None,
                    attached_anchor_ids: Vec::new(),
                    attached_files: Vec::new(),
                    attached_memories: Vec::new(),
                },
                invariants: Vec::new(),
                cwd: Some(workspace_path.display().to_string()),
                expected_policy_sha256: view.session.policy.sha256.clone(),
                read_only: false,
                scope: Some(TurnMechanicalScope {
                    scope_id: "test-scope".to_string(),
                    instruction: "Create one test file".to_string(),
                    human_contract_artifact_id: "test-contract".to_string(),
                    allowed_paths: vec!["src".to_string()],
                    approved_by: "test-human".to_string(),
                }),
            },
        })?;

        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut last_activity = "none".to_string();
        loop {
            let remaining =
                deadline.saturating_duration_since(std::time::Instant::now());
            anyhow::ensure!(
                !remaining.is_zero(),
                "native child edit turn timed out"
            );
            match notification_receiver.recv_timeout(remaining).with_context(|| {
                format!(
                    "waiting for native child edit completion (model requests: {}/4; file created: {}; last activity: {last_activity})",
                    model_requests.lock().len(),
                    workspace_path.join("src/agent-authored.txt").is_file(),
                )
            })? {
                AheadNotification::AgentTurnState {
                    session_id: notified_session,
                    state,
                    ..
                } if notified_session == session_id && state != "streaming" => {
                    anyhow::ensure!(
                        state == "complete",
                        "native child edit turn ended as {state}"
                    );
                    break;
                }
                AheadNotification::AgentToolCall { call, .. } => {
                    last_activity = format!("{}: {}", call.kind, call.status);
                }
                AheadNotification::AgentUserInputRequested { .. } => {
                    last_activity = "user input requested".to_string();
                }
                AheadNotification::AgentBufferSnapshotsRequested { .. } => {
                    last_activity = "buffer snapshots requested".to_string();
                }
                AheadNotification::AgentPresentationRequested { .. } => {
                    last_activity = "editor presentation requested".to_string();
                }
                _ => {}
            }
        }
        model_server
            .join()
            .expect("join child edit mock model server")?;
        let requests = model_requests.lock();
        anyhow::ensure!(
            requests.get(1).is_some_and(|tools| tools
                .split(", ")
                .any(|tool| tool == "exec_command")),
            "child model request did not advertise exec_command: {requests:?}"
        );
        for context in [
            "Objective: Implement a small file change",
            "AHEAD_TURN_PLAN_SENTINEL",
            "AHEAD_TURN_INVARIANT_SENTINEL",
            "AHEAD_TURN_DECISION_SENTINEL",
            "AHEAD_TURN_SUMMARY_SENTINEL",
        ] {
            assert!(
                requests[0].contains(context),
                "managed model request omitted {context}: {requests:?}"
            );
        }
        assert!(
            !requests[0].contains("UNTRUSTED_CALLER_CONTEXT_MUST_NOT_REACH_MODEL"),
            "managed host trusted caller-supplied durable context"
        );
        let edited_content =
            fs::read_to_string(workspace.path().join("src/agent-authored.txt"))
                .with_context(|| {
                    format!("child did not create the file; requests: {requests:?}")
                })?;
        assert_eq!(edited_content, "agent authored\n");
        drop(requests);
        host.harness().shutdown_harness();
        drop(host);

        let anchors =
            SessionStore::open(&database_path)?.list_anchors(&session_id)?;
        assert_eq!(anchors.len(), 1);
        let anchor = &anchors[0];
        assert_eq!(anchor.session_id, session_id);
        assert_eq!(anchor.actor_id, ahead_rpc::ahead::AHEAD_ACTOR_ID);
        assert_eq!(anchor.path, "src/agent-authored.txt");
        assert_eq!(anchor.range.start.line, 1);
        assert_eq!(anchor.range.end.line, 1);
        assert_eq!(
            anchor.surrounding_context.as_deref(),
            Some("agent authored")
        );
        assert_eq!(
            anchor.quote_hash,
            format!("{:x}", Sha256::digest(b"agent authored"))
        );
        Ok(())
    }

    #[test]
    fn legacy_rollouts_import_into_turso_before_thread_restore() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        fs::create_dir(workspace.path().join(".ahead"))?;
        let database_path = workspace.path().join(".ahead/session.db");
        let store = Arc::new(parking_lot::RwLock::new(SessionStore::open(
            &database_path,
        )?));
        let config =
            ahead_agent::NativeClientConfig::ahead(workspace.path().to_path_buf());
        let client = ahead_agent::NativeClient::spawn(
            &config,
            Arc::new(SharedSessionStore(store.clone())),
            Arc::new(|_| {}),
        )?;

        for directory in ["sessions", "archived_sessions"] {
            for history_mode in ["legacy", "paginated"] {
                for compressed in [false, true] {
                    let thread_id = uuid::Uuid::new_v4().to_string();
                    let rollout_dir = workspace
                        .path()
                        .join(".ahead/runtime")
                        .join(directory)
                        .join("2025/01/03");
                    fs::create_dir_all(&rollout_dir)?;
                    let rollout_path = rollout_dir.join(format!(
                        "rollout-2025-01-03T12-00-00-{thread_id}.jsonl"
                    ));
                    let meta = serde_json::json!({
                        "timestamp": "2025-01-03T12:00:00Z",
                        "type": "session_meta",
                        "payload": {
                            "session_id": thread_id,
                            "id": thread_id,
                            "timestamp": "2025-01-03T12:00:00Z",
                            "cwd": workspace.path(),
                            "originator": "ahead-test",
                            "cli_version": "test",
                            "source": "cli",
                            "model_provider": "openai",
                            "history_mode": history_mode
                        }
                    });
                    let contents = format!("{meta}\n");
                    let source_path = if compressed {
                        rollout_path.with_extension("jsonl.zst")
                    } else {
                        rollout_path
                    };
                    let source_bytes = if compressed {
                        zstd::stream::encode_all(contents.as_bytes(), 3)?
                    } else {
                        contents.into_bytes()
                    };
                    fs::write(&source_path, &source_bytes)?;

                    client
                        .load_session(
                            &thread_id,
                            workspace.path(),
                            "read-only",
                            None,
                            Some("openai"),
                        )
                        .expect("import and resume legacy JSONL thread");
                    let snapshot = store
                        .read()
                        .load_native_thread(&thread_id)?
                        .context("legacy rollout was not imported")?;
                    assert_eq!(snapshot.archived, directory == "archived_sessions");
                    assert_eq!(snapshot.rollout_items.len(), 1);
                    assert_eq!(snapshot.create_params["thread_id"], thread_id);
                    assert_eq!(snapshot.create_params["history_mode"], history_mode);
                    assert_eq!(fs::read(&source_path)?, source_bytes);
                }
            }
        }
        client.shutdown();
        Ok(())
    }

    #[test]
    fn unsafe_legacy_rollouts_are_rejected_without_modifying_sources() -> Result<()>
    {
        let workspace = tempfile::tempdir()?;
        let other_workspace = tempfile::tempdir()?;
        fs::create_dir_all(workspace.path().join(".ahead"))?;
        let database_path = workspace.path().join(".ahead/session.db");
        let store = Arc::new(parking_lot::RwLock::new(SessionStore::open(
            &database_path,
        )?));
        let config =
            ahead_agent::NativeClientConfig::ahead(workspace.path().to_path_buf());
        let client = ahead_agent::NativeClient::spawn(
            &config,
            Arc::new(SharedSessionStore(store.clone())),
            Arc::new(|_| {}),
        )?;

        for invalid_case in ["ambiguous", "malformed", "out-of-scope"] {
            let thread_id = uuid::Uuid::new_v4().to_string();
            let cwd = if invalid_case == "out-of-scope" {
                other_workspace.path()
            } else {
                workspace.path()
            };
            let meta = serde_json::json!({
                "timestamp": "2025-01-03T12:00:00Z",
                "type": "session_meta",
                "payload": {
                    "session_id": thread_id,
                    "id": thread_id,
                    "timestamp": "2025-01-03T12:00:00Z",
                    "cwd": cwd,
                    "originator": "ahead-test",
                    "cli_version": "test",
                    "source": "cli",
                    "model_provider": "openai"
                }
            });
            let mut sources = Vec::new();
            let directories: &[&str] = if invalid_case == "ambiguous" {
                &["sessions", "archived_sessions"]
            } else {
                &["sessions"]
            };
            for directory in directories {
                let rollout_dir = workspace
                    .path()
                    .join(".ahead/runtime")
                    .join(directory)
                    .join("2025/01/03");
                fs::create_dir_all(&rollout_dir)?;
                let path = rollout_dir
                    .join(format!("rollout-2025-01-03T12-00-00-{thread_id}.jsonl"));
                let contents = if invalid_case == "malformed" {
                    format!("{meta}\nnot valid JSON\n")
                } else {
                    format!("{meta}\n")
                };
                fs::write(&path, &contents)?;
                sources.push((path, contents));
            }
            client
                .load_session(
                    &thread_id,
                    workspace.path(),
                    "read-only",
                    None,
                    Some("openai"),
                )
                .expect_err("unsafe rollout must not be imported");
            assert!(store.read().load_native_thread(&thread_id)?.is_none());
            for (path, contents) in sources {
                assert_eq!(fs::read_to_string(path)?, contents);
            }
        }
        assert!(store.read().list_native_thread_headers()?.is_empty());
        client.shutdown();
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_legacy_session_root_is_rejected_without_import() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        fs::create_dir_all(workspace.path().join(".ahead"))?;
        let database_path = workspace.path().join(".ahead/session.db");
        let store = Arc::new(parking_lot::RwLock::new(SessionStore::open(
            &database_path,
        )?));
        let config =
            ahead_agent::NativeClientConfig::ahead(workspace.path().to_path_buf());
        let client = ahead_agent::NativeClient::spawn(
            &config,
            Arc::new(SharedSessionStore(store.clone())),
            Arc::new(|_| {}),
        )?;

        let thread_id = uuid::Uuid::new_v4().to_string();
        let runtime_home = workspace.path().join(".ahead/runtime");
        let target_directory =
            runtime_home.join("rollout-store/sessions/2025/01/03");
        fs::create_dir_all(&target_directory)?;
        let source = target_directory
            .join(format!("rollout-2025-01-03T12-00-00-{thread_id}.jsonl"));
        let meta = serde_json::json!({
            "timestamp": "2025-01-03T12:00:00Z",
            "type": "session_meta",
            "payload": {
                "session_id": thread_id,
                "id": thread_id,
                "timestamp": "2025-01-03T12:00:00Z",
                "cwd": workspace.path(),
                "originator": "ahead-test",
                "cli_version": "test",
                "source": "cli",
                "model_provider": "openai"
            }
        });
        let contents = format!("{meta}\n");
        fs::write(&source, &contents)?;
        std::os::unix::fs::symlink(
            runtime_home.join("rollout-store/sessions"),
            runtime_home.join("sessions"),
        )?;

        let error = client
            .load_session(
                &thread_id,
                workspace.path(),
                "read-only",
                None,
                Some("openai"),
            )
            .expect_err("symlinked legacy session root must be rejected");
        assert!(format!("{error:#}").contains("must not be symlinks"));
        assert!(store.read().load_native_thread(&thread_id)?.is_none());
        assert_eq!(fs::read_to_string(source)?, contents);
        client.shutdown();
        Ok(())
    }

    #[test]
    fn legacy_native_thread_import_is_atomic_and_idempotent() -> Result<()> {
        let store = SessionStore::in_memory()?;
        let created_at =
            chrono::DateTime::parse_from_rfc3339("2025-01-03T12:00:00Z")?
                .with_timezone(&chrono::Utc);
        let updated_at = created_at + chrono::Duration::minutes(5);
        let import = LegacyNativeThreadImport {
            thread_id: "legacy-thread".into(),
            create_params: serde_json::json!({
                "thread_id": "legacy-thread",
                "history_mode": "legacy"
            }),
            rollout_items: vec![
                serde_json::json!({"type": "user_message", "text": "hello"}),
                serde_json::json!({"type": "assistant_message", "text": "hi"}),
            ],
            created_at: created_at.clone(),
            updated_at: updated_at.clone(),
            archived_at: Some(updated_at),
        };
        store.block_on(async {
            store
                .conn
                .execute(
                    "CREATE TRIGGER reject_legacy_import_item
                     BEFORE INSERT ON agent_runtime_thread_items
                     WHEN NEW.ordinal = 1
                     BEGIN SELECT RAISE(ABORT, 'injected import failure'); END",
                    (),
                )
                .await?;
            Ok::<_, anyhow::Error>(())
        })?;
        assert!(store.import_legacy_native_thread(&import).is_err());
        assert!(store.load_native_thread("legacy-thread")?.is_none());

        store.block_on(async {
            store
                .conn
                .execute("DROP TRIGGER reject_legacy_import_item", ())
                .await?;
            Ok::<_, anyhow::Error>(())
        })?;
        assert!(store.import_legacy_native_thread(&import)?);
        assert!(!store.import_legacy_native_thread(&import)?);
        let snapshot = store
            .load_native_thread("legacy-thread")?
            .context("imported native thread missing")?;
        assert_eq!(snapshot.rollout_items, import.rollout_items);
        assert!(snapshot.archived);
        assert_eq!(snapshot.archived_at, import.archived_at);
        let header = store
            .load_native_thread_header("legacy-thread")?
            .context("imported native thread header missing")?;
        assert_eq!(header.created_at, Some(import.created_at));
        assert_eq!(header.updated_at, Some(import.updated_at));
        Ok(())
    }

    #[test]
    fn native_thread_replay_history_roundtrips_in_turso() -> Result<()> {
        let store = SessionStore::in_memory()?;
        let thread_id = "native-thread-1";
        let create_params = serde_json::json!({
            "thread_id": thread_id,
            "history_mode": "legacy"
        });
        let rollout_items = vec![
            serde_json::json!({"type": "user_message", "text": "hello"}),
            serde_json::json!({"type": "assistant_message", "text": "hi"}),
        ];
        let metadata_patch = serde_json::json!({"name": "test"});

        store.create_native_thread(thread_id, &create_params)?;
        assert_eq!(store.list_native_thread_headers()?[0].thread_id, thread_id);
        store.append_native_thread_items(thread_id, &rollout_items)?;
        store.append_native_thread_metadata(thread_id, &metadata_patch)?;
        let archived_at = store
            .set_native_thread_archived(thread_id, true)?
            .context("archive timestamp missing")?;
        let later_patch = serde_json::json!({"preview": "after archive"});
        store.append_native_thread_metadata(thread_id, &later_patch)?;

        assert_eq!(
            store.load_native_thread(thread_id)?,
            Some(ahead_agent::NativeThreadSnapshot {
                create_params,
                metadata_patches: vec![metadata_patch, later_patch],
                rollout_items,
                archived: true,
                archived_at: Some(archived_at),
            })
        );

        store.delete_native_thread(thread_id)?;
        assert!(store.load_native_thread(thread_id)?.is_none());
        assert!(store.list_native_thread_headers()?.is_empty());

        Ok(())
    }

    #[test]
    fn native_agent_spawn_edges_survive_reopen_and_thread_deletion() -> Result<()> {
        use ahead_agent::NativeAgentEdgeStatus::{Closed, Open};

        let workspace = tempfile::tempdir()?;
        let path = workspace.path().join("session.db");
        let store = SessionStore::open(&path)?;
        for thread_id in ["root", "child-b", "child-a"] {
            store.create_native_thread(
                thread_id,
                &serde_json::json!({"thread_id": thread_id}),
            )?;
        }
        store.upsert_native_agent_edge("root", "child-b", Open)?;
        store.upsert_native_agent_edge("root", "child-a", Open)?;
        assert_eq!(
            store.list_native_agent_children("root", Some(Open))?,
            ["child-a", "child-b"]
        );
        store.set_native_agent_edge_status("child-a", Closed)?;
        drop(store);

        let reopened = SessionStore::open(&path)?;
        assert_eq!(
            reopened.list_native_agent_children("root", Some(Open))?,
            ["child-b"]
        );
        assert_eq!(
            reopened.list_native_agent_children("root", None)?,
            ["child-a", "child-b"]
        );
        reopened.delete_native_thread("root")?;
        assert!(
            reopened
                .list_native_agent_children("root", None)?
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn memory_index_tracks_current_content_and_revisions() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let memory_path = temp.path().join(".ahead/memories/MEMORY.md");
        fs::create_dir_all(memory_path.parent().expect("memory parent"))?;
        fs::write(&memory_path, "# Project memory\n\nfirst decision\n")?;
        let store = SessionStore::in_memory()?;

        let first_hash = store
            .sync_memory_file("project", &memory_path)?
            .expect("indexed memory");
        assert_eq!(
            store.list_memory_revisions("project", &memory_path)?.len(),
            1
        );
        assert_eq!(store.search_memory("FIRST DECISION", 10)?.len(), 1);

        store.sync_memory_file("project", &memory_path)?;
        assert_eq!(
            store.list_memory_revisions("project", &memory_path)?.len(),
            1
        );

        fs::write(&memory_path, "# Project memory\n\nsecond decision\n")?;
        let second_hash = store
            .sync_memory_file("project", &memory_path)?
            .expect("reindexed memory");
        assert_ne!(first_hash, second_hash);
        assert_eq!(
            store.list_memory_revisions("project", &memory_path)?.len(),
            2
        );
        assert!(store.search_memory("first", 10)?.is_empty());
        let hits = store.search_memory("second decision", 10)?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].scope, "project");
        assert_eq!(hits[0].line, 3);

        fs::remove_file(&memory_path)?;
        assert!(store.sync_memory_file("project", &memory_path)?.is_none());
        assert!(store.search_memory("second decision", 10)?.is_empty());
        assert_eq!(
            store.list_memory_revisions("project", &memory_path)?.len(),
            2
        );
        Ok(())
    }

    #[test]
    fn memory_index_rejects_documents_over_the_limit() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let memory_path = temp.path().join("MEMORY.md");
        fs::write(&memory_path, "current memory entry\n")?;
        let store = SessionStore::in_memory()?;
        store.sync_memory_file("project", &memory_path)?;

        let oversized = "x".repeat(ahead_rpc::ahead::MEMORY_DOCUMENT_MAX_BYTES + 1);
        fs::write(&memory_path, oversized)?;
        let error = store
            .sync_memory_file("project", &memory_path)
            .expect_err("oversized memory must not be indexed");
        assert!(error.to_string().contains("32 KiB limit"));
        assert_eq!(store.search_memory("current memory entry", 10)?.len(), 1);
        assert!(store.search_memory("oversized", 10)?.is_empty());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn memory_index_refuses_a_symlinked_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let memory_path = temp.path().join("MEMORY.md");
        let external_path = temp.path().join("outside.md");
        fs::write(&external_path, "do not index external contents")?;
        std::os::unix::fs::symlink(&external_path, &memory_path)?;
        let store = SessionStore::in_memory()?;

        store
            .sync_memory_file("project", &memory_path)
            .expect_err("memory indexing must refuse a symlink");
        assert!(store.search_memory("external contents", 10)?.is_empty());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn memory_index_refuses_a_non_regular_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let memory_path = temp.path().join("MEMORY.md");
        fs::create_dir(&memory_path)?;
        let store = SessionStore::in_memory()?;

        let error = store
            .sync_memory_file("project", &memory_path)
            .expect_err("memory index must refuse a directory");
        assert!(error.to_string().contains("regular file"));
        assert!(store.search_memory("memory", 10)?.is_empty());
        Ok(())
    }

    #[test]
    fn memory_search_ranks_lines_matching_more_query_terms() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let memory_path = temp.path().join("MEMORY.md");
        fs::write(
            &memory_path,
            "# Project memory\nretry policy requires idempotency\nretry remains important\n",
        )?;
        let store = SessionStore::in_memory()?;
        store.sync_memory_file("project", &memory_path)?;

        let hits = store.search_memory("retry idempotency", 10)?;
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].line, 2);
        assert!(hits[0].relevance > hits[1].relevance);
        assert_eq!(hits[1].line, 3);
        assert_eq!(store.search_memory("retry retry idempotency", 10)?, hits);
        assert_eq!(store.search_memory("try", 10)?.len(), 0);
        assert_eq!(store.search_memory("idempot", 10)?[0].line, 2);
        Ok(())
    }

    #[test]
    fn memory_token_index_survives_reopen_and_tracks_current_revision() -> Result<()>
    {
        let temp = tempfile::tempdir()?;
        let database_path = temp.path().join("session.db");
        let memory_path = temp.path().join(".ahead/memories/MEMORY.md");
        fs::create_dir_all(memory_path.parent().expect("memory parent"))?;
        fs::write(&memory_path, "# Project memory\nkeep the retry contract\n")?;

        let first_hash = SessionStore::open(&database_path)?
            .sync_memory_file("project", &memory_path)?
            .expect("indexed memory");
        let reopened = SessionStore::open(&database_path)?;
        let hits = reopened.search_memory("retry contract", 10)?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].content_sha256, first_hash);
        assert_eq!(hits[0].line, 2);

        fs::write(
            &memory_path,
            "# Project memory\nreplace the retry contract\n",
        )?;
        let second_hash = reopened
            .sync_memory_file("project", &memory_path)?
            .expect("updated memory");
        assert_ne!(first_hash, second_hash);
        assert!(reopened.search_memory("keep", 10)?.is_empty());
        let current = reopened.search_memory("replace retry", 10)?;
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].content_sha256, second_hash);
        Ok(())
    }

    #[test]
    fn test_turso_session_store_crud() -> Result<()> {
        let mut store = SessionStore::in_memory()?;

        let view = SessionView {
            session: WorkSession {
                id: "sess-turso-1".into(),
                project_id: "proj-alpha".into(),
                worktree_id: "wt-main".into(),
                work_kind: WorkKind::CorrectiveDebugging,
                title: "Fix null pointer in parser".into(),
                owner_id: "dev-42".into(),
                lifecycle: SessionLifecycle::Active,
                policy: SessionPolicySnapshot::default(),
                revision: 1,
                created_at: "2026-09-17T12:00:00Z".into(),
            },
            task: SessionTask {
                id: "task-turso-1".into(),
                session_id: "sess-turso-1".into(),
                intent: TaskIntent::Assistance,
                work_kind: WorkKind::CorrectiveDebugging,
                title: "Fix null pointer in parser".into(),
                objective: "Fix the parser crash without changing syntax behavior"
                    .into(),
                parent_task_id: None,
                learning_arc_id: None,
                created_at: "2026-09-17T12:00:00Z".into(),
                completed_at: None,
            },
            learning_arc: None,
            workflow: WorkflowState {
                revision: 1,
                definition_version: "2026-09".into(),
                phase: WorkflowPhase {
                    id: "phase-reproduce".into(),
                    title: "Reproduce".into(),
                    visit: 1,
                },
                primary_work_item: Some(GithubIssueRef {
                    host: "github.com".into(),
                    owner: "test".into(),
                    repo: "ahead".into(),
                    issue_number: 101,
                    issue_node_id: "I_kwDOtest".into(),
                    title: "Crash on empty input".into(),
                }),
                current_artifact_ids: vec!["art-1".into()],
                approvals: vec![],
            },
            participants: vec![SessionParticipantRecord {
                participant: Participant::Human {
                    id: "dev-42".into(),
                    subject: "kade@example.com".into(),
                    display_name: "Kade".into(),
                },
                role: SessionRole::Owner,
            }],
        };

        // Insert
        store.insert_session(&view)?;

        let retry_request = AgentTurnRequestDto {
            session_id: "sess-turso-1".into(),
            thread_id: "thread-turso-1".into(),
            harness: HarnessKind::Ahead,
            external_agent_id: None,
            model: Some("test-model".into()),
            model_provider: Some("test-provider".into()),
            user_message: "Retry this turn".into(),
            session_context: "Objective: preserve parser semantics".into(),
            context: TurnEditorContext {
                active_path: "src/parser.rs".into(),
                caret: DisplayPosition { line: 1, col: 0 },
                selection: None,
                file_content: "fn parse() {}".into(),
                visible_end: None,
                attached_anchor_ids: Vec::new(),
                attached_files: Vec::new(),
                attached_memories: Vec::new(),
            },
            invariants: vec!["preserve parser behavior".into()],
            cwd: None,
            expected_policy_sha256: "policy-hash".into(),
            read_only: false,
            scope: None,
        };
        let instruction_sources = vec![InstructionFileSource {
            path: "src/AGENTS.md".into(),
            content_sha256: "instruction-hash".into(),
            targets: vec!["src/parser.rs".into()],
        }];
        store.save_turn_request(
            "turn-turso-1",
            &retry_request,
            &instruction_sources,
        )?;
        assert_eq!(
            store.get_turn_request("turn-turso-1")?,
            Some(retry_request.clone())
        );
        assert_eq!(
            store.list_turn_instruction_sources("sess-turso-1", "turn-turso-1",)?,
            instruction_sources
        );
        store.remove_turn_request("turn-turso-1")?;
        assert!(store.get_turn_request("turn-turso-1")?.is_none());
        assert_eq!(
            store.list_turn_instruction_sources("sess-turso-1", "turn-turso-1",)?,
            instruction_sources
        );

        // Retrieve
        let retrieved = store
            .get_session("sess-turso-1")?
            .expect("Session should exist");
        assert_eq!(retrieved.session.title, "Fix null pointer in parser");
        assert_eq!(
            retrieved.task.objective,
            "Fix the parser crash without changing syntax behavior"
        );
        assert_eq!(retrieved.workflow.phase.id, "phase-reproduce");
        assert_eq!(retrieved.participants.len(), 1);
        store.update_session_title("sess-turso-1", "Investigate parser crash")?;
        assert_eq!(
            store.session_title("sess-turso-1")?.as_deref(),
            Some("Investigate parser crash")
        );
        assert_eq!(
            store
                .get_session("sess-turso-1")?
                .expect("renamed session")
                .task
                .title,
            "Investigate parser crash"
        );

        // Advance phase
        let next_wf = store.advance_workflow_phase(
            "sess-turso-1",
            1,
            WorkflowPhase {
                id: "phase-fix".into(),
                title: "Fix".into(),
                visit: 1,
            },
        )?;
        assert_eq!(next_wf.revision, 2);
        assert_eq!(next_wf.phase.id, "phase-fix");

        // Verify anchor
        let anchor = CodeAnchor {
            id: "anc-1".into(),
            session_id: "sess-turso-1".into(),
            actor_id: "ahead".into(),
            path: "src/parser.rs".into(),
            range: DisplayRange {
                start: DisplayPosition { line: 42, col: 1 },
                end: DisplayPosition { line: 42, col: 15 },
            },
            quote_hash: "abc123hash".into(),
            surrounding_context: Some("fn parse() {".into()),
        };
        store.insert_anchor(&anchor)?;
        let anchors = store.list_anchors("sess-turso-1")?;
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].id, "anc-1");
        assert_eq!(anchors[0].actor_id, "ahead");

        // Work item lifecycle: create, order, note, close with summary.
        let item_a =
            store.create_work_item("sess-turso-1", "Fix retry backoff", "dev-42")?;
        let item_b = store.create_work_item(
            "sess-turso-1",
            "Add regression test",
            "dev-42",
        )?;
        assert!(item_a.position < item_b.position);
        let items = store.list_work_items("sess-turso-1")?;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, item_a.id);
        assert_eq!(
            store.work_item_session(&item_a.id)?.as_deref(),
            Some("sess-turso-1")
        );
        assert!(store.work_item_session("missing-item")?.is_none());
        assert!(
            store
                .create_work_item("sess-turso-1", "   ", "dev-42")
                .is_err()
        );
        store.add_work_item_event(
            &item_a.id,
            "sess-turso-1",
            "note",
            "Reproduced locally",
            "dev-42",
        )?;
        let events = store.list_work_item_events(&item_a.id, None)?;
        assert_eq!(events.len(), 1);
        // Close requires summary or explicit skip.
        assert!(
            store
                .close_work_item(&item_a.id, "", None, None, None)
                .is_err()
        );
        let closeout = store.close_work_item(
            &item_a.id,
            "Fixed backoff; verified with retry test",
            Some("owner/repo#142".to_string()),
            None,
            None,
        )?;
        assert!(closeout.summary_markdown.contains("Fixed backoff"));
        let stored = store
            .get_work_item_closeout(&item_a.id)?
            .expect("closeout stored");
        assert_eq!(stored.issue_ref.as_deref(), Some("owner/repo#142"));
        let updated =
            store.set_work_item_status(&item_b.id, WorkItemStatus::InProgress)?;
        assert_eq!(updated.status, WorkItemStatus::InProgress);

        // Conversation summaries retained alongside messages.
        let summary = store.save_conversation_summary(
            "sess-turso-1",
            "plan",
            "Agreed on backoff invariant",
            "msg-1..msg-4",
        )?;
        assert!(!summary.id.is_empty());
        let summaries = store.list_conversation_summaries("sess-turso-1")?;
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].phase, "plan");
        assert!(
            store
                .save_conversation_summary("sess-turso-1", "plan", "  ", "msg-1")
                .is_err()
        );

        let mut incomplete = view.clone();
        incomplete.session.id = "sess-incomplete".into();
        incomplete.session.title = "Legacy incomplete session".into();
        incomplete.task.id = "task-incomplete".into();
        incomplete.task.session_id = incomplete.session.id.clone();
        store.insert_session(&incomplete)?;
        store.rt.block_on(async {
            store
                .conn
                .execute(
                    "DELETE FROM session_tasks WHERE session_id = ?1",
                    params![incomplete.session.id.clone()],
                )
                .await
        })?;
        store.set_harness_binding(
            "sess-turso-1",
            "acp-thread",
            "external-agent:pi-acp",
        )?;
        let mut newer = view.clone();
        newer.session.id = "sess-turso-newer".into();
        newer.session.created_at = "2026-09-19T12:00:00Z".into();
        newer.task.id = "task-turso-newer".into();
        newer.task.session_id = newer.session.id.clone();
        store.insert_session(&newer)?;
        let listed = store.list_sessions()?;
        assert_eq!(listed[0].id, newer.session.id);
        assert_eq!(listed[1].updated_at, view.session.created_at);

        store.upsert_message(&ConversationMessage {
            id: "msg-turso-activity".into(),
            session_id: view.session.id.clone(),
            turn_id: "turn-turso-activity".into(),
            sequence: 1,
            role: "human".into(),
            actor_id: "human".into(),
            content: "Follow up on the parser".into(),
            status: "complete".into(),
            created_at: "2026-09-20T12:00:00Z".into(),
        })?;
        let listed = store.list_sessions()?;
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, "sess-turso-1");
        assert_eq!(listed[0].title, "Investigate parser crash");
        assert_eq!(listed[0].updated_at, "2026-09-20T12:00:00Z");
        assert_eq!(listed[0].backend.as_deref(), Some("external-agent:pi-acp"));
        store.archive_session(&newer.session.id)?;
        store.archive_session("sess-turso-1")?;
        assert!(store.list_sessions()?.is_empty());
        assert!(store.work_item_session(&item_a.id)?.is_none());
        assert!(store.get_session("sess-turso-1")?.is_some());

        Ok(())
    }

    /// Attribution anchors are coalesced per actor/path region and dropped
    /// once committed, so the table tracks the working delta rather than
    /// growing with every edit.
    #[test]
    fn test_anchor_attribution_coalesces_and_clears() -> Result<()> {
        let mut store = SessionStore::in_memory()?;
        let session = "sess-attr".to_string();

        store.insert_session(&SessionView {
            session: WorkSession {
                id: session.clone(),
                project_id: "proj".into(),
                worktree_id: "wt".into(),
                work_kind: WorkKind::CorrectiveDebugging,
                title: "Attribution".into(),
                owner_id: "dev".into(),
                lifecycle: SessionLifecycle::Active,
                policy: SessionPolicySnapshot::default(),
                revision: 1,
                created_at: "2026-09-17T12:00:00Z".into(),
            },
            task: SessionTask {
                id: "task-attr".into(),
                session_id: session.clone(),
                intent: TaskIntent::Assistance,
                work_kind: WorkKind::CorrectiveDebugging,
                title: "Attribution".into(),
                objective: "Preserve authorship when applying an edit".into(),
                parent_task_id: None,
                learning_arc_id: None,
                created_at: "2026-09-17T12:00:00Z".into(),
                completed_at: None,
            },
            learning_arc: None,
            workflow: WorkflowState {
                revision: 1,
                definition_version: "2026-09".into(),
                phase: WorkflowPhase {
                    id: "plan".into(),
                    title: "Plan".into(),
                    visit: 1,
                },
                primary_work_item: None,
                current_artifact_ids: vec![],
                approvals: vec![],
            },
            participants: vec![],
        })?;

        let anchor =
            |id: &str, actor: &str, path: &str, start: u32, end: u32| CodeAnchor {
                id: id.into(),
                session_id: session.clone(),
                actor_id: actor.into(),
                path: path.into(),
                range: DisplayRange {
                    start: DisplayPosition {
                        line: start,
                        col: 0,
                    },
                    end: DisplayPosition { line: end, col: 0 },
                },
                surrounding_context: Some(format!("q-{id}")),
                quote_hash: format!(
                    "{:x}",
                    <sha2::Sha256 as sha2::Digest>::digest(
                        format!("q-{id}").as_bytes()
                    )
                ),
            };

        // Three contiguous agent regions on one file collapse to a single row.
        store.record_edit_anchor(&anchor("a1", "ahead", "src/lib.rs", 1, 3))?;
        store.record_edit_anchor(&anchor("a2", "ahead", "src/lib.rs", 4, 6))?;
        store.record_edit_anchor(&anchor("a3", "ahead", "src/lib.rs", 7, 9))?;

        // A non-contiguous region and a different actor stay separate.
        store.record_edit_anchor(&anchor("a4", "ahead", "src/lib.rs", 21, 24))?;
        store.record_edit_anchor(&anchor("h1", "human", "src/lib.rs", 4, 6))?;

        let rows = store.list_anchors_for_paths(&["src/lib.rs".to_string()])?;
        assert_eq!(
            rows.len(),
            3,
            "expected coalesced agent region + far region + human region"
        );

        let ahead = rows
            .iter()
            .find(|a| a.id == "a1")
            .expect("coalesced agent row");
        assert_eq!(ahead.range.start.line, 1);
        assert_eq!(ahead.range.end.line, 9);
        assert_eq!(ahead.id, "a1", "merge keeps the earliest anchor id");

        // Unrelated paths are untouched.
        store.record_edit_anchor(&anchor("o1", "ahead", "src/other.rs", 1, 2))?;
        assert_eq!(
            store
                .list_anchors_for_paths(&["src/lib.rs".to_string()])?
                .len(),
            3
        );

        // Committing one path clears only that path's uncommitted anchors.
        let mut head_contents = std::collections::HashMap::new();
        head_contents.insert("src/lib.rs".to_string(), "q-a1 q-a4 q-h1".to_string());
        let cleared = store
            .clear_anchors_for_paths(&["src/lib.rs".to_string()], &head_contents)?;
        assert_eq!(cleared, 3);
        assert!(
            store
                .list_anchors_for_paths(&["src/lib.rs".to_string()])?
                .is_empty()
        );
        assert_eq!(
            store
                .list_anchors_for_paths(&["src/other.rs".to_string()])?
                .len(),
            1
        );

        // A human edit that removed the anchored text leaves attribution intact.
        store.record_edit_anchor(&anchor("stale", "ahead", "src/lib.rs", 30, 31))?;
        assert_eq!(
            store.clear_anchors_for_paths(
                &["src/lib.rs".to_string()],
                &head_contents
            )?,
            0
        );
        assert_eq!(
            store
                .list_anchors_for_paths(&["src/lib.rs".to_string()])?
                .len(),
            1
        );

        Ok(())
    }
}
