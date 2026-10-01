//! AHEAD-owned AI harnessing.
//!
//! This crate is AHEAD's agent integration layer. It is deliberately separate
//! from `ahead-proxy` so the editor can depend on it directly and so the
//! harness can evolve without touching the file/LSP/terminal proxy.
//!
//! Two tiers are represented here:
//!
//! - **External ACP agents** (`adapters`, `acp_client`, `session`): AHEAD offers
//!   curated installs for Pi, Codex and Claude Code, then speaks ACP v1 over
//!   stdio for streaming, cancellation, resume and conversation rendering. It
//!   does *not* enforce Learn read-only, edit scope or attribution inside an
//!   external process; these sessions are presented as external.
//! - **Native AHEAD agent** (`native_client`, default): AHEAD embeds its
//!   hard-forked model/tool loop and consumes events in-process. No ACP or
//!   app-server transport sits between the editor and its built-in agent.
//!
//! Naming is AHEAD-first: the crate and types describe AHEAD's harnessing
//! needs, not any single vendor. The installed external adapter is selected by
//! the user, and the native loop remains an AHEAD implementation detail rather
//! than the crate's identity.

pub mod acp_client;
pub mod adapters;
pub mod editor_tools;
mod instructions;
pub mod native_client;
mod runtime_support;
pub mod session;
pub mod store;
mod turso_agent_graph_store;
mod turso_thread_store;

pub use acp_client::{
    ACP_PROTOCOL_VERSION, HarnessClient, HarnessClientConfig, HarnessEvent,
    HarnessFileChange, HarnessPlanEntry, HarnessSink, run_editor_mcp_stdio,
};
pub use adapters::{
    external_acp_adapters, external_agent_config, set_external_acp_adapter_installed,
};
pub use instructions::{
    MemoryScope, MemorySource, memory_sources,
    project_instruction_prompt_for_targets,
};
pub use native_client::{NativeClient, NativeClientConfig};
pub use runtime_support::{
    mcp_server_declarations, path_is_allowed, set_mcp_server_approval,
};
pub use session::{HarnessController, HarnessNotificationSink};
pub use store::{
    HarnessStore, InstructionFileSource, NativeAgentEdgeStatus, NativeThreadHeader,
    NativeThreadHeaderPageRequest, NativeThreadRelationFilter,
    NativeThreadReplayPage, NativeThreadSnapshot, NativeThreadSortDirection,
    NativeThreadTimestampSort,
};
