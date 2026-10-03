//! AHEAD Session Host & Core Systems
//!
//! Submodules:
//! - `store`: Turso/libSQL transactional persistence
//! - `policy`: Capability evaluator & sandbox
//! - `voice`: Full-duplex streaming voice runtime probe
//! - `prediction`: Edit prediction context assembler
//! - `host`: Session host lifecycle and RPC dispatch
//!
//! Agent integration lives in the separate `ahead-agent` crate so the editor
//! can integrate deeply with it without depending on the file/LSP proxy.

pub mod auth;
pub mod collab;
pub mod host;
pub mod policy;
pub mod prediction;
mod recovery;
pub mod scenarios;
pub(crate) mod share;
pub mod store;
pub mod tracker;
pub mod voice;

pub use ahead_agent::{
    HarnessClient, HarnessClientConfig, HarnessController, HarnessEvent,
    HarnessNotificationSink, HarnessPlanEntry, HarnessSink, HarnessStore,
};
pub use auth::{AuthRecord, GitHubAuthManager, PollTokenResult};
pub use collab::{
    CollabParticipant, CollabSession, ParticipantStatus, ReviewSnapshot,
    StickyAnchorIndex,
};
pub use host::AheadSessionHost;
pub use policy::PolicyEvaluator;
pub use prediction::PredictionEngine;
pub use store::SessionStore;
pub use tracker::{
    OutboxStatus, TrackerAdapter, TrackerOutboxItem, TrackerUpdatePayload,
};
pub use voice::VoiceSession;
