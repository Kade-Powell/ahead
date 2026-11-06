//! Events from managed and external agents, normalized for AHEAD storage and UI.

use std::sync::Arc;

use ahead_rpc::ahead::{AgentCommand, AgentConfigOption, AgentPresentationAction};

/// One streamed update from the harness, normalized for AHEAD storage and UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessEvent {
    AgentDelta {
        acp_session_id: String,
        text: String,
    },
    AgentThought {
        acp_session_id: String,
        text: String,
    },
    Warning {
        acp_session_id: String,
        message: String,
    },
    ContextCompacted {
        acp_session_id: String,
    },
    ToolCall {
        acp_session_id: String,
        call_id: String,
        title: String,
        status: String,
        kind: String,
    },
    PatchApplyProgress {
        acp_session_id: String,
        preview: HarnessPatchPreview,
    },
    FileChange {
        acp_session_id: String,
        status: String,
        changes: Vec<HarnessFileChange>,
    },
    Plan {
        acp_session_id: String,
        entries: Vec<HarnessPlanEntry>,
    },
    Usage {
        acp_session_id: String,
        used_tokens: u64,
        context_window: Option<u64>,
    },
    UserInputRequested {
        acp_session_id: String,
        request_id: String,
        is_blocking: bool,
        questions: Vec<HarnessUserInputQuestion>,
    },
    UserInputCancelled {
        acp_session_id: String,
        request_id: String,
    },
    BufferSnapshotsRequested {
        acp_session_id: String,
        request_id: String,
        include_document_symbols: bool,
    },
    EditorPresentationRequested {
        acp_session_id: String,
        request_id: String,
        action: AgentPresentationAction,
    },
    ModeChanged {
        acp_session_id: String,
        mode_id: String,
    },
    SessionTitle {
        acp_session_id: String,
        title: String,
    },
    AvailableCommands {
        acp_session_id: String,
        commands: Vec<AgentCommand>,
    },
    ConfigOptions {
        acp_session_id: String,
        options: Vec<AgentConfigOption>,
    },
}

/// A single plan step reported by the harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessPlanEntry {
    pub content: String,
    pub status: String,
    pub priority: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessUserInputOption {
    pub value: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessUserInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    pub external_url: Option<String>,
    pub options: Vec<HarnessUserInputOption>,
    pub default_answers: Vec<String>,
    pub allows_other: bool,
    pub is_secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessFileChange {
    pub path: String,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessPatchPreview {
    pub call_id: String,
    pub changes: Vec<HarnessFileChange>,
    pub truncated: bool,
}

/// Callback invoked on the reader thread for every streamed event.
pub type HarnessSink = Arc<dyn Fn(HarnessEvent) + Send + Sync>;
