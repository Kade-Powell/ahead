//! Storage boundary between AHEAD's harness and the durable session store.
//!
//! The agent layer owns the *using* side of conversation persistence; `ahead-proxy`
//! owns the libSQL implementation. Defining the trait here keeps the dependency
//! direction one-way (`ahead-proxy` → `ahead-agent`) and lets the agent layer be
//! tested with a lightweight fake.

use ahead_rpc::ahead::{
    AgentRuntimeState, AgentTurnRequestDto, CodeAnchor, ConversationMessage,
    ConversationMessageCursor, ConversationMessagePage, RepoPath, TaskIntent,
};
use anyhow::Result;
use serde_json::Value;
use std::path::PathBuf;

/// The native loop state required to restore one managed AHEAD thread.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeThreadSnapshot {
    pub create_params: Value,
    pub metadata_patches: Vec<Value>,
    pub rollout_items: Vec<Value>,
    pub archived: bool,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// One legacy rollout imported as a single durable thread-store transaction.
#[derive(Clone, Debug, PartialEq)]
pub struct LegacyNativeThreadImport {
    pub thread_id: String,
    pub create_params: Value,
    pub rollout_items: Vec<Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Fields needed to list a native thread without loading its replay items.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeThreadHeader {
    pub thread_id: String,
    pub create_params: Value,
    pub metadata_patches: Vec<Value>,
    pub archived: bool,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Workspace-relative instruction sources captured for one managed turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstructionFileSource {
    pub path: RepoPath,
    pub content_sha256: String,
    pub targets: Vec<RepoPath>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeThreadTimestampSort {
    CreatedAt,
    UpdatedAt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeThreadSortDirection {
    Asc,
    Desc,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeThreadRelationFilter {
    DirectChildrenOf(String),
    DescendantsOf(String),
}

/// A database-ordered page of lightweight native-thread headers.
#[derive(Clone, Debug)]
pub struct NativeThreadHeaderPageRequest {
    pub archived: bool,
    pub sort: NativeThreadTimestampSort,
    pub direction: NativeThreadSortDirection,
    pub cursor: Option<String>,
    pub allowed_sources: Vec<Value>,
    pub model_providers: Vec<String>,
    pub cwd_filters: Option<Vec<PathBuf>>,
    /// Case-sensitive substring match over current thread search metadata.
    pub search_term: Option<String>,
    pub relation_filter: Option<NativeThreadRelationFilter>,
    /// Maximum rows to return, including the extra row used to detect another page.
    pub limit: usize,
}

/// A reverse-ordered page of native replay rows. `next_before_ordinal` is set
/// only when older rows remain.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeThreadReplayPage {
    pub items: Vec<Value>,
    pub next_before_ordinal: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeAgentEdgeStatus {
    Open,
    Closed,
}

impl NativeAgentEdgeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
}

/// Durable conversation + harness-binding operations the harness needs.
pub trait HarnessStore: Send + Sync {
    /// Task intent governing the current harness conversation.
    fn get_task_intent(&self, session_id: &str) -> Result<Option<TaskIntent>>;

    /// Next monotonically increasing message sequence for a session.
    fn next_message_sequence(&self, session_id: &str) -> Result<i64>;

    /// Appends or replaces one durable conversation message.
    fn upsert_message(&self, message: &ConversationMessage) -> Result<()>;

    /// Appends streamed text to a message and returns the new body.
    fn append_message_delta(&self, message_id: &str, delta: &str) -> Result<String>;

    /// Marks a message lifecycle state (`complete`, `cancelled`, `failed`).
    fn set_message_status(&self, message_id: &str, status: &str) -> Result<()>;

    /// Loads the durable non-sensitive UI projection for a streamed turn.
    fn get_agent_runtime_state(
        &self,
        session_id: &str,
    ) -> Result<Option<AgentRuntimeState>>;

    /// Replaces the durable UI projection for a work session.
    fn set_agent_runtime_state(
        &self,
        session_id: &str,
        state: &AgentRuntimeState,
    ) -> Result<()>;

    /// Persists the request needed to offer an explicit retry after a process
    /// restart interrupts a streamed turn.
    fn save_turn_request(
        &self,
        turn_id: &str,
        request: &AgentTurnRequestDto,
        instruction_sources: &[InstructionFileSource],
    ) -> Result<()>;

    /// Loads a persisted request for an interrupted or failed turn.
    fn get_turn_request(&self, turn_id: &str)
    -> Result<Option<AgentTurnRequestDto>>;

    /// Removes retry state after a turn completes successfully.
    fn remove_turn_request(&self, turn_id: &str) -> Result<()>;

    /// Full readable conversation in order.
    fn list_messages(&self, session_id: &str) -> Result<Vec<ConversationMessage>>;

    /// Marks streamed messages from turns that cannot still be active as failed.
    /// Implementations with a database should update these rows in one query.
    fn recover_stale_streaming_messages(
        &self,
        session_id: &str,
        active_turn_id: Option<&str>,
    ) -> Result<()> {
        for message in self.list_messages(session_id)? {
            if message.role == "agent"
                && message.status == "streaming"
                && active_turn_id != Some(message.turn_id.as_str())
            {
                self.set_message_status(&message.id, "failed")?;
            }
        }
        Ok(())
    }

    /// Loads the newest page or the page immediately before `before_sequence`.
    /// The default keeps test stores source-compatible; durable stores should
    /// override it with an indexed keyset query.
    fn list_messages_page(
        &self,
        session_id: &str,
        before: Option<ConversationMessageCursor>,
        limit: usize,
    ) -> Result<ConversationMessagePage> {
        let limit = limit.clamp(1, 100);
        let before_key = before
            .as_ref()
            .map(|cursor| (cursor.sequence, cursor.message_id.as_str()));
        let messages = self
            .list_messages(session_id)?
            .into_iter()
            .filter(|message| {
                before_key.is_none_or(|before| {
                    (message.sequence, message.id.as_str()) < before
                })
            })
            .collect::<Vec<_>>();
        let start = messages.len().saturating_sub(limit);
        Ok(ConversationMessagePage {
            has_older: start > 0,
            messages: messages.into_iter().skip(start).collect(),
        })
    }

    /// Current durable title for the work session.
    fn session_title(&self, session_id: &str) -> Result<Option<String>>;

    /// Updates the durable work-session title.
    fn update_session_title(&self, session_id: &str, title: &str) -> Result<()>;

    /// Records the harness conversation bound to a work session.
    fn set_harness_binding(
        &self,
        session_id: &str,
        acp_session_id: &str,
        backend: &str,
    ) -> Result<()>;

    /// The harness conversation bound to a work session, if any.
    fn get_harness_binding(
        &self,
        session_id: &str,
    ) -> Result<Option<(String, String)>>;

    /// Persists an agent-authored region after the native runtime applies it.
    fn record_edit_anchor(&self, anchor: &CodeAnchor) -> Result<()>;

    /// Creates the durable native-thread record in the same store as AHEAD chat.
    fn create_native_thread(
        &self,
        _thread_id: &str,
        _create_params: Value,
    ) -> Result<()> {
        anyhow::bail!("native thread persistence is unavailable")
    }

    /// Imports a legacy rollout once; the header and replay rows must commit atomically.
    fn import_legacy_native_thread(
        &self,
        _import: LegacyNativeThreadImport,
    ) -> Result<bool> {
        anyhow::bail!("native thread persistence is unavailable")
    }

    /// Appends model-loop history in replay order.
    fn append_native_thread_items(
        &self,
        _thread_id: &str,
        _items: Vec<Value>,
    ) -> Result<()> {
        anyhow::bail!("native thread persistence is unavailable")
    }

    /// Persists a metadata patch applied to the native thread.
    fn append_native_thread_metadata(
        &self,
        _thread_id: &str,
        _patch: Value,
    ) -> Result<()> {
        anyhow::bail!("native thread persistence is unavailable")
    }

    /// Loads the native loop state needed to restore a managed thread.
    fn load_native_thread(
        &self,
        _thread_id: &str,
    ) -> Result<Option<NativeThreadSnapshot>> {
        anyhow::bail!("native thread persistence is unavailable")
    }

    /// Loads one thread header without reading its replay rows.
    ///
    /// The compatibility default derives it from a full snapshot; durable
    /// stores should override this to keep metadata reads proportional to the
    /// header size.
    fn load_native_thread_header(
        &self,
        thread_id: &str,
    ) -> Result<Option<NativeThreadHeader>> {
        Ok(self
            .load_native_thread(thread_id)?
            .map(|snapshot| NativeThreadHeader {
                thread_id: thread_id.to_string(),
                create_params: snapshot.create_params,
                metadata_patches: snapshot.metadata_patches,
                archived: snapshot.archived,
                archived_at: snapshot.archived_at,
                created_at: None,
                updated_at: None,
            }))
    }

    /// Loads replay rows newest-first using an exclusive ordinal cursor.
    fn load_native_thread_item_page(
        &self,
        _thread_id: &str,
        _before_ordinal: Option<i64>,
        _limit: usize,
    ) -> Result<NativeThreadReplayPage> {
        anyhow::bail!("native thread replay paging is unavailable")
    }

    /// Lists durable native thread headers without reading replay items.
    fn list_native_thread_headers(&self) -> Result<Vec<NativeThreadHeader>> {
        anyhow::bail!("native thread persistence is unavailable")
    }

    /// Lists a simple timestamp-ordered header page without loading all threads.
    /// Stores without an indexed implementation can return `None` to use the
    /// complete header listing instead.
    fn list_native_thread_headers_page(
        &self,
        _request: &NativeThreadHeaderPageRequest,
    ) -> Result<Option<Vec<NativeThreadHeader>>> {
        Ok(None)
    }

    /// Marks a native thread archived or active.
    fn set_native_thread_archived(
        &self,
        _thread_id: &str,
        _archived: bool,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        anyhow::bail!("native thread persistence is unavailable")
    }

    /// Removes native loop state when its owning thread is deleted.
    fn delete_native_thread(&self, _thread_id: &str) -> Result<()> {
        anyhow::bail!("native thread persistence is unavailable")
    }

    fn upsert_native_agent_edge(
        &self,
        _parent_thread_id: &str,
        _child_thread_id: &str,
        _status: NativeAgentEdgeStatus,
    ) -> Result<()> {
        anyhow::bail!("native agent graph persistence is unavailable")
    }

    fn set_native_agent_edge_status(
        &self,
        _child_thread_id: &str,
        _status: NativeAgentEdgeStatus,
    ) -> Result<()> {
        anyhow::bail!("native agent graph persistence is unavailable")
    }

    fn list_native_agent_children(
        &self,
        _parent_thread_id: &str,
        _status: Option<NativeAgentEdgeStatus>,
    ) -> Result<Vec<String>> {
        anyhow::bail!("native agent graph persistence is unavailable")
    }
}
