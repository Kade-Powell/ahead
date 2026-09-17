//! AHEAD Local Session Persistence
//!
//! Turso / libSQL-backed transactional store for sessions, workflow revisions,
//! session event logs, durable code anchors, and proposals.
//! Supports local offline embedded databases and Turso embedded replicas.
//! Corresponds to Section 7.1 of `ahead-editor-mvp.md`.

use std::{path::Path, sync::Arc};
use anyhow::{Context, Result, bail};
use lapce_rpc::ahead::{
    ApprovalRecord, CodeAnchor, DisplayPosition, DisplayRange, GithubIssueRef, Id,
    Revision, SessionLifecycle, SessionPolicySnapshot,
    SessionView, WorkKind, WorkSession, WorkflowPhase, WorkflowState,
    AssistanceMode, ChangeProposal, Participant, SessionRole, SessionParticipantRecord,
};
use libsql::{params, Connection, Builder};

pub struct SessionStore {
    conn: Connection,
    rt: Arc<tokio::runtime::Runtime>,
}

impl SessionStore {
    /// Opens or creates a local Turso/libSQL database file
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("Failed to create tokio runtime for SessionStore")?,
        );
        let conn = rt.block_on(async {
            let db = Builder::new_local(&path_str).build().await?;
            db.connect()
        }).context("Failed to initialize local Turso/libsql database")?;
        let store = Self { conn, rt };
        store.init_schema()?;
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
        let conn = rt.block_on(async {
            let db = Builder::new_local(":memory:").build().await?;
            db.connect()
        }).context("Failed to initialize in-memory Turso/libsql database")?;
        let store = Self { conn, rt };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        self.rt.block_on(async {
            self.conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS sessions (
                    id TEXT PRIMARY KEY,
                    project_id TEXT NOT NULL,
                    worktree_id TEXT NOT NULL,
                    work_kind TEXT NOT NULL,
                    mode TEXT NOT NULL,
                    title TEXT NOT NULL,
                    owner_id TEXT NOT NULL,
                    lifecycle_json TEXT NOT NULL,
                    policy_json TEXT NOT NULL,
                    revision INTEGER NOT NULL,
                    created_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS workflow_state (
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

                CREATE TABLE IF NOT EXISTS participants (
                    id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    participant_json TEXT NOT NULL,
                    role TEXT NOT NULL,
                    PRIMARY KEY (id, session_id),
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE IF NOT EXISTS session_events (
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

                CREATE TABLE IF NOT EXISTS anchors (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    path TEXT NOT NULL,
                    start_line INTEGER NOT NULL,
                    start_col INTEGER NOT NULL,
                    end_line INTEGER NOT NULL,
                    end_col INTEGER NOT NULL,
                    quote_hash TEXT NOT NULL,
                    surrounding_context TEXT,
                    created_at_commit TEXT,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE TABLE IF NOT EXISTS proposals (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    path TEXT NOT NULL,
                    original_sha256 TEXT NOT NULL,
                    patch TEXT NOT NULL,
                    is_mechanical INTEGER NOT NULL,
                    description TEXT NOT NULL,
                    status TEXT NOT NULL,
                    target_line INTEGER,
                    target_col INTEGER,
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE INDEX IF NOT EXISTS idx_events_session_seq ON session_events(session_id, sequence);
                CREATE INDEX IF NOT EXISTS idx_anchors_session ON anchors(session_id);
                "
            ).await
        }).context("Failed to initialize AHEAD Turso/libsql schema")?;
        Ok(())
    }
    pub fn list_sessions(&self) -> Result<Vec<SessionView>> {
        let ids: Vec<String> = self.rt.block_on(async {
            let mut rows = self.conn.query("SELECT id FROM sessions ORDER BY created_at DESC", ()).await?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().await? {
                let id: String = row.get(0)?;
                out.push(id);
            }
            Ok::<Vec<String>, anyhow::Error>(out)
        })?;
        let mut views = Vec::new();
        for id in ids {
            if let Some(view) = self.get_session(&id)? {
                views.push(view);
            }
        }
        Ok(views)
    }

    pub fn insert_session(&mut self, view: &SessionView) -> Result<()> {
        let session = &view.session;
        let lifecycle_json = serde_json::to_string(&session.lifecycle)?;
        let policy_json = serde_json::to_string(&session.policy)?;
        let work_kind_str = serde_json::to_string(&session.work_kind)?.replace('\"', "");
        let mode_str = match session.mode {
            AssistanceMode::Learn => "learn",
            AssistanceMode::Assist => "assist",
        };

        let wf = &view.workflow;
        let item_json = wf.primary_work_item.as_ref().map(serde_json::to_string).transpose()?;
        let artifacts_json = serde_json::to_string(&wf.current_artifact_ids)?;
        let approvals_json = serde_json::to_string(&wf.approvals)?;

        let mut participant_tuples = Vec::new();
        for p_rec in &view.participants {
            let p_json = serde_json::to_string(&p_rec.participant)?;
            let role_str = serde_json::to_string(&p_rec.role)?.replace('\"', "");
            participant_tuples.push((p_rec.participant.id(), p_json, role_str));
        }

        self.rt.block_on(async {
            self.conn.execute("BEGIN TRANSACTION", ()).await?;

            self.conn.execute(
                "INSERT INTO sessions (id, project_id, worktree_id, work_kind, mode, title, owner_id, lifecycle_json, policy_json, revision, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    session.id.clone(),
                    session.project_id.clone(),
                    session.worktree_id.clone(),
                    work_kind_str,
                    mode_str,
                    session.title.clone(),
                    session.owner_id.clone(),
                    lifecycle_json,
                    policy_json,
                    session.revision as i64,
                    session.created_at.clone(),
                ],
            ).await?;

            self.conn.execute(
                "INSERT INTO workflow_state (session_id, revision, definition_version, phase_id, phase_title, phase_visit, primary_work_item_json, current_artifact_ids_json, approvals_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    session.id.clone(),
                    wf.revision as i64,
                    wf.definition_version.clone(),
                    wf.phase.id.clone(),
                    wf.phase.title.clone(),
                    wf.phase.visit as i64,
                    item_json,
                    artifacts_json,
                    approvals_json,
                ],
            ).await?;

            for (p_id, p_json, role_str) in participant_tuples {
                self.conn.execute(
                    "INSERT INTO participants (id, session_id, participant_json, role) VALUES (?1, ?2, ?3, ?4)",
                    params![p_id, session.id.clone(), p_json, role_str],
                ).await?;
            }

            self.conn.execute("COMMIT", ()).await?;
            Ok::<(), anyhow::Error>(())
        })?;

        Ok(())
    }

    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionView>> {
        self.rt.block_on(async {
            let mut rows = self.conn.query(
                "SELECT id, project_id, worktree_id, work_kind, mode, title, owner_id, lifecycle_json, policy_json, revision, created_at
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
            let mode_str: String = row.get(4)?;
            let title: String = row.get(5)?;
            let owner_id: String = row.get(6)?;
            let lifecycle_json: String = row.get(7)?;
            let policy_json: String = row.get(8)?;
            let revision: i64 = row.get(9)?;
            let created_at: String = row.get(10)?;

            let work_kind: WorkKind = serde_json::from_str(&format!("\"{}\"", work_kind_str))?;
            let mode = match mode_str.as_str() {
                "learn" => AssistanceMode::Learn,
                _ => AssistanceMode::Assist,
            };
            let lifecycle: SessionLifecycle = serde_json::from_str(&lifecycle_json)?;
            let policy: SessionPolicySnapshot = serde_json::from_str(&policy_json)?;

            let session = WorkSession {
                id,
                project_id,
                worktree_id,
                work_kind,
                mode,
                title,
                owner_id,
                lifecycle,
                policy,
                revision: revision as u64,
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
                revision: wf_rev as u64,
                definition_version: def_ver,
                phase: WorkflowPhase {
                    id: phase_id,
                    title: phase_title,
                    visit: phase_visit as u32,
                },
                primary_work_item,
                current_artifact_ids,
                approvals,
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
                let role: SessionRole = serde_json::from_str(&format!("\"{}\"", role_str))?;
                participants.push(SessionParticipantRecord { participant, role });
            }

            Ok(Some(SessionView {
                session,
                workflow,
                participants,
            }))
        })
    }

    pub fn advance_workflow_phase(
        &mut self,
        session_id: &str,
        expected_revision: Revision,
        new_phase: WorkflowPhase,
    ) -> Result<WorkflowState> {
        self.rt.block_on(async {
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

            if (cur_rev as u64) != expected_revision {
                self.conn.execute("ROLLBACK", ()).await?;
                bail!("Revision mismatch: expected {}, found {}", expected_revision, cur_rev);
            }

            let next_rev = cur_rev + 1;

            self.conn.execute(
                "UPDATE workflow_state SET revision = ?1, phase_id = ?2, phase_title = ?3, phase_visit = ?4 WHERE session_id = ?5",
                params![next_rev, new_phase.id.clone(), new_phase.title.clone(), new_phase.visit as i64, session_id],
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
                revision: next_rev as u64,
                definition_version: def_ver,
                phase: new_phase,
                primary_work_item,
                current_artifact_ids,
                approvals,
            })
        })
    }

    pub fn set_assistance_mode(&mut self, session_id: &str, mode: AssistanceMode) -> Result<()> {
        let mode_str = match mode {
            AssistanceMode::Learn => "learn",
            AssistanceMode::Assist => "assist",
        };
        self.rt.block_on(async {
            self.conn.execute(
                "UPDATE sessions SET mode = ?1, revision = revision + 1 WHERE id = ?2",
                params![mode_str, session_id],
            ).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    pub fn insert_anchor(&mut self, anchor: &CodeAnchor) -> Result<()> {
        self.rt.block_on(async {
            self.conn.execute(
                "INSERT INTO anchors (id, session_id, path, start_line, start_col, end_line, end_col, quote_hash, surrounding_context, created_at_commit)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    anchor.id.clone(),
                    anchor.session_id.clone(),
                    anchor.path.clone(),
                    anchor.range.start.line as i64,
                    anchor.range.start.col as i64,
                    anchor.range.end.line as i64,
                    anchor.range.end.col as i64,
                    anchor.quote_hash.clone(),
                    anchor.surrounding_context.clone(),
                    anchor.created_at_commit.clone(),
                ],
            ).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    pub fn list_anchors(&self, session_id: &str) -> Result<Vec<CodeAnchor>> {
        self.rt.block_on(async {
            let mut rows = self.conn.query(
                "SELECT id, session_id, path, start_line, start_col, end_line, end_col, quote_hash, surrounding_context, created_at_commit
                 FROM anchors WHERE session_id = ?1 ORDER BY start_line ASC",
                params![session_id],
            ).await?;

            let mut list = Vec::new();
            while let Some(row) = rows.next().await? {
                let id: String = row.get(0)?;
                let s_id: String = row.get(1)?;
                let path: String = row.get(2)?;
                let start_line: i64 = row.get(3)?;
                let start_col: i64 = row.get(4)?;
                let end_line: i64 = row.get(5)?;
                let end_col: i64 = row.get(6)?;
                let quote_hash: String = row.get(7)?;
                let surrounding_context: Option<String> = row.get(8)?;
                let created_at_commit: Option<String> = row.get(9)?;

                list.push(CodeAnchor {
                    id,
                    session_id: s_id,
                    path,
                    range: DisplayRange {
                        start: DisplayPosition { line: start_line as u32, col: start_col as u32 },
                        end: DisplayPosition { line: end_line as u32, col: end_col as u32 },
                    },
                    quote_hash,
                    surrounding_context,
                    created_at_commit,
                });
            }
            Ok(list)
        })
    }

    pub fn insert_proposal(&mut self, proposal: &ChangeProposal) -> Result<()> {
        let (target_line, target_col) = match proposal.recommended_cursor {
            Some(pos) => (Some(pos.line as i64), Some(pos.col as i64)),
            None => (None, None),
        };
        self.rt.block_on(async {
            self.conn.execute(
                "INSERT INTO proposals (id, session_id, path, original_sha256, patch, is_mechanical, description, status, target_line, target_col)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8, ?9)",
                params![
                    proposal.id.clone(),
                    proposal.session_id.clone(),
                    proposal.path.clone(),
                    proposal.original_sha256.clone(),
                    proposal.patch.clone(),
                    if proposal.is_mechanical { 1i64 } else { 0i64 },
                    proposal.description.clone(),
                    target_line,
                    target_col,
                ],
            ).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    pub fn accept_proposal(&mut self, proposal_id: &str) -> Result<()> {
        self.rt.block_on(async {
            let count = self.conn.execute(
                "UPDATE proposals SET status = 'accepted' WHERE id = ?1 AND status = 'pending'",
                params![proposal_id],
            ).await?;
            if count == 0 {
                bail!("Proposal not found or already resolved: {}", proposal_id);
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_turso_session_store_crud() -> Result<()> {
        let mut store = SessionStore::in_memory()?;

        let view = SessionView {
            session: WorkSession {
                id: "sess-turso-1".into(),
                project_id: "proj-alpha".into(),
                worktree_id: "wt-main".into(),
                work_kind: WorkKind::CorrectiveDebugging,
                mode: AssistanceMode::Assist,
                title: "Fix null pointer in parser".into(),
                owner_id: "dev-42".into(),
                lifecycle: SessionLifecycle::Active,
                policy: SessionPolicySnapshot::default(),
                revision: 1,
                created_at: "2026-09-17T12:00:00Z".into(),
            },
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
            participants: vec![
                SessionParticipantRecord {
                    participant: Participant::Human {
                        id: "dev-42".into(),
                        subject: "kade@example.com".into(),
                        display_name: "Kade".into(),
                    },
                    role: SessionRole::Owner,
                },
            ],
        };

        // Insert
        store.insert_session(&view)?;

        // Retrieve
        let retrieved = store.get_session("sess-turso-1")?.expect("Session should exist");
        assert_eq!(retrieved.session.title, "Fix null pointer in parser");
        assert_eq!(retrieved.workflow.phase.id, "phase-reproduce");
        assert_eq!(retrieved.participants.len(), 1);

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
            path: "src/parser.rs".into(),
            range: DisplayRange {
                start: DisplayPosition { line: 42, col: 1 },
                end: DisplayPosition { line: 42, col: 15 },
            },
            quote_hash: "abc123hash".into(),
            surrounding_context: Some("fn parse() {".into()),
            created_at_commit: Some("commit-sha".into()),
        };
        store.insert_anchor(&anchor)?;
        let anchors = store.list_anchors("sess-turso-1")?;
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].id, "anc-1");

        // Verify proposal
        let proposal = ChangeProposal {
            id: "prop-1".into(),
            session_id: "sess-turso-1".into(),
            path: "src/parser.rs".into(),
            original_sha256: "abc".into(),
            patch: "+ if input.is_empty() { return None; }".into(),
            is_mechanical: true,
            description: "Guard against empty slice".into(),
            recommended_cursor: Some(DisplayPosition { line: 43, col: 8 }),
        };
        store.insert_proposal(&proposal)?;
        store.accept_proposal("prop-1")?;

        Ok(())
    }
}
