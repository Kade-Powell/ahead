//! AHEAD Local Session Persistence
//!
//! SQLite-backed transactional store for sessions, workflow revisions,
//! session event logs, durable code anchors, and proposals.
//! Corresponds to Section 7.1 of `ahead-editor-mvp.md`.

use std::path::Path;
use anyhow::{Context, Result, bail};
use lapce_rpc::ahead::{
    ApprovalRecord, CodeAnchor, DisplayPosition, DisplayRange, GithubIssueRef, Id,
    Revision, SessionLifecycle, SessionPolicySnapshot,
    SessionView, WorkKind, WorkSession, WorkflowPhase, WorkflowState,
    AssistanceMode, ChangeProposal, Participant, SessionRole, SessionParticipantRecord,
};
use rusqlite::{params, Connection};

pub struct SessionStore {
    conn: Connection,
}

impl SessionStore {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
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
        ).context("Failed to initialize AHEAD SQLite schema")?;
        Ok(())
    }

    pub fn insert_session(&mut self, view: &SessionView) -> Result<()> {
        let tx = self.conn.transaction()?;
        let session = &view.session;
        let lifecycle_json = serde_json::to_string(&session.lifecycle)?;
        let policy_json = serde_json::to_string(&session.policy)?;
        let work_kind_str = serde_json::to_string(&session.work_kind)?.replace('\"', "");
        let mode_str = match session.mode {
            AssistanceMode::Learn => "learn",
            AssistanceMode::Assist => "assist",
        };

        tx.execute(
            "INSERT INTO sessions (id, project_id, worktree_id, work_kind, mode, title, owner_id, lifecycle_json, policy_json, revision, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                session.id,
                session.project_id,
                session.worktree_id,
                work_kind_str,
                mode_str,
                session.title,
                session.owner_id,
                lifecycle_json,
                policy_json,
                session.revision,
                session.created_at,
            ],
        )?;

        let wf = &view.workflow;
        let item_json = wf.primary_work_item.as_ref().map(serde_json::to_string).transpose()?;
        let artifacts_json = serde_json::to_string(&wf.current_artifact_ids)?;
        let approvals_json = serde_json::to_string(&wf.approvals)?;

        tx.execute(
            "INSERT INTO workflow_state (session_id, revision, definition_version, phase_id, phase_title, phase_visit, primary_work_item_json, current_artifact_ids_json, approvals_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                session.id,
                wf.revision,
                wf.definition_version,
                wf.phase.id,
                wf.phase.title,
                wf.phase.visit,
                item_json,
                artifacts_json,
                approvals_json,
            ],
        )?;

        for p_rec in &view.participants {
            let p_json = serde_json::to_string(&p_rec.participant)?;
            let role_str = serde_json::to_string(&p_rec.role)?.replace('\"', "");
            tx.execute(
                "INSERT INTO participants (id, session_id, participant_json, role) VALUES (?1, ?2, ?3, ?4)",
                params![p_rec.participant.id(), session.id, p_json, role_str],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionView>> {
        let session_res = self.conn.query_row(
            "SELECT id, project_id, worktree_id, work_kind, mode, title, owner_id, lifecycle_json, policy_json, revision, created_at
             FROM sessions WHERE id = ?1",
            params![session_id],
            |row| {
                let id: String = row.get(0)?;
                let project_id: String = row.get(1)?;
                let worktree_id: String = row.get(2)?;
                let work_kind_str: String = row.get(3)?;
                let mode_str: String = row.get(4)?;
                let title: String = row.get(5)?;
                let owner_id: String = row.get(6)?;
                let lifecycle_json: String = row.get(7)?;
                let policy_json: String = row.get(8)?;
                let revision: u64 = row.get(9)?;
                let created_at: String = row.get(10)?;

                Ok((id, project_id, worktree_id, work_kind_str, mode_str, title, owner_id, lifecycle_json, policy_json, revision, created_at))
            },
        );

        let row = match session_res {
            Ok(r) => r,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        let work_kind: WorkKind = serde_json::from_str(&format!("\"{}\"", row.3))?;
        let mode = match row.4.as_str() {
            "learn" => AssistanceMode::Learn,
            _ => AssistanceMode::Assist,
        };
        let lifecycle: SessionLifecycle = serde_json::from_str(&row.7)?;
        let policy: SessionPolicySnapshot = serde_json::from_str(&row.8)?;

        let session = WorkSession {
            id: row.0,
            project_id: row.1,
            worktree_id: row.2,
            work_kind,
            mode,
            title: row.5,
            owner_id: row.6,
            lifecycle,
            policy,
            revision: row.9,
            created_at: row.10,
        };

        let wf_row = self.conn.query_row(
            "SELECT revision, definition_version, phase_id, phase_title, phase_visit, primary_work_item_json, current_artifact_ids_json, approvals_json
             FROM workflow_state WHERE session_id = ?1",
            params![session_id],
            |row| {
                let rev: u64 = row.get(0)?;
                let def_ver: String = row.get(1)?;
                let phase_id: String = row.get(2)?;
                let phase_title: String = row.get(3)?;
                let phase_visit: u32 = row.get(4)?;
                let item_json: Option<String> = row.get(5)?;
                let artifacts_json: String = row.get(6)?;
                let approvals_json: String = row.get(7)?;
                Ok((rev, def_ver, phase_id, phase_title, phase_visit, item_json, artifacts_json, approvals_json))
            },
        )?;

        let primary_work_item: Option<GithubIssueRef> = wf_row.5.as_deref().map(serde_json::from_str).transpose()?;
        let current_artifact_ids: Vec<Id> = serde_json::from_str(&wf_row.6)?;
        let approvals: Vec<ApprovalRecord> = serde_json::from_str(&wf_row.7)?;

        let workflow = WorkflowState {
            revision: wf_row.0,
            definition_version: wf_row.1,
            phase: WorkflowPhase {
                id: wf_row.2,
                title: wf_row.3,
                visit: wf_row.4,
            },
            primary_work_item,
            current_artifact_ids,
            approvals,
        };

        let mut stmt = self.conn.prepare("SELECT participant_json, role FROM participants WHERE session_id = ?1")?;
        let participant_rows = stmt.query_map(params![session_id], |row| {
            let p_json: String = row.get(0)?;
            let role_str: String = row.get(1)?;
            Ok((p_json, role_str))
        })?;

        let mut participants = Vec::new();
        for p_res in participant_rows {
            let (p_json, role_str) = p_res?;
            let participant: Participant = serde_json::from_str(&p_json)?;
            let role: SessionRole = serde_json::from_str(&format!("\"{}\"", role_str))?;
            participants.push(SessionParticipantRecord { participant, role });
        }

        Ok(Some(SessionView {
            session,
            workflow,
            participants,
        }))
    }

    pub fn advance_workflow_phase(
        &mut self,
        session_id: &str,
        expected_revision: Revision,
        new_phase: WorkflowPhase,
    ) -> Result<WorkflowState> {
        let tx = self.conn.transaction()?;

        let (cur_rev, def_ver, item_json, artifacts_json, approvals_json): (u64, String, Option<String>, String, String) = tx.query_row(
            "SELECT revision, definition_version, primary_work_item_json, current_artifact_ids_json, approvals_json
             FROM workflow_state WHERE session_id = ?1",
            params![session_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;

        if cur_rev != expected_revision {
            bail!("Revision mismatch: expected {}, found {}", expected_revision, cur_rev);
        }

        let next_rev = cur_rev + 1;

        tx.execute(
            "UPDATE workflow_state SET revision = ?1, phase_id = ?2, phase_title = ?3, phase_visit = ?4 WHERE session_id = ?5",
            params![next_rev, new_phase.id, new_phase.title, new_phase.visit, session_id],
        )?;

        tx.execute(
            "UPDATE sessions SET revision = revision + 1 WHERE id = ?1",
            params![session_id],
        )?;

        let primary_work_item = item_json.as_deref().map(serde_json::from_str).transpose()?;
        let current_artifact_ids = serde_json::from_str(&artifacts_json)?;
        let approvals = serde_json::from_str(&approvals_json)?;

        tx.commit()?;

        Ok(WorkflowState {
            revision: next_rev,
            definition_version: def_ver,
            phase: new_phase,
            primary_work_item,
            current_artifact_ids,
            approvals,
        })
    }

    pub fn set_assistance_mode(&mut self, session_id: &str, mode: AssistanceMode) -> Result<()> {
        let mode_str = match mode {
            AssistanceMode::Learn => "learn",
            AssistanceMode::Assist => "assist",
        };
        self.conn.execute(
            "UPDATE sessions SET mode = ?1, revision = revision + 1 WHERE id = ?2",
            params![mode_str, session_id],
        )?;
        Ok(())
    }

    pub fn insert_anchor(&mut self, anchor: &CodeAnchor) -> Result<()> {
        self.conn.execute(
            "INSERT INTO anchors (id, session_id, path, start_line, start_col, end_line, end_col, quote_hash, surrounding_context, created_at_commit)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                anchor.id,
                anchor.session_id,
                anchor.path,
                anchor.range.start.line,
                anchor.range.start.col,
                anchor.range.end.line,
                anchor.range.end.col,
                anchor.quote_hash,
                anchor.surrounding_context,
                anchor.created_at_commit,
            ],
        )?;
        Ok(())
    }

    pub fn get_anchors_for_session(&self, session_id: &str) -> Result<Vec<CodeAnchor>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, path, start_line, start_col, end_line, end_col, quote_hash, surrounding_context, created_at_commit
             FROM anchors WHERE session_id = ?1",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            let id: String = row.get(0)?;
            let session_id: String = row.get(1)?;
            let path: String = row.get(2)?;
            let start_line: u32 = row.get(3)?;
            let start_col: u32 = row.get(4)?;
            let end_line: u32 = row.get(5)?;
            let end_col: u32 = row.get(6)?;
            let quote_hash: String = row.get(7)?;
            let surrounding_context: Option<String> = row.get(8)?;
            let created_at_commit: Option<String> = row.get(9)?;

            Ok(CodeAnchor {
                id,
                session_id,
                path,
                range: DisplayRange {
                    start: DisplayPosition { line: start_line, col: start_col },
                    end: DisplayPosition { line: end_line, col: end_col },
                },
                quote_hash,
                surrounding_context,
                created_at_commit,
            })
        })?;

        let mut list = Vec::new();
        for item in rows {
            list.push(item?);
        }
        Ok(list)
    }

    pub fn insert_proposal(&mut self, proposal: &ChangeProposal) -> Result<()> {
        let (target_line, target_col) = match proposal.recommended_cursor {
            Some(pos) => (Some(pos.line), Some(pos.col)),
            None => (None, None),
        };
        self.conn.execute(
            "INSERT INTO proposals (id, session_id, path, original_sha256, patch, is_mechanical, description, status, target_line, target_col)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8, ?9)",
            params![
                proposal.id,
                proposal.session_id,
                proposal.path,
                proposal.original_sha256,
                proposal.patch,
                if proposal.is_mechanical { 1 } else { 0 },
                proposal.description,
                target_line,
                target_col,
            ],
        )?;
        Ok(())
    }

    pub fn accept_proposal(&mut self, proposal_id: &str) -> Result<()> {
        let count = self.conn.execute(
            "UPDATE proposals SET status = 'accepted' WHERE id = ?1 AND status = 'pending'",
            params![proposal_id],
        )?;
        if count == 0 {
            bail!("Proposal not found or already resolved: {}", proposal_id);
        }
        Ok(())
    }
}
