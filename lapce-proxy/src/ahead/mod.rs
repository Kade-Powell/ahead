//! AHEAD Session Host & Core Systems
//!
//! Submodules:
//! - `store`: SQLite transactional persistence
//! - `policy`: Capability evaluator & sandbox
//! - `voice`: Full-duplex streaming voice runtime probe
//! - `prediction`: Edit prediction context assembler
//! - `host`: Session host lifecycle and RPC dispatch

pub mod host;
pub mod policy;
pub mod prediction;
pub mod store;
pub mod voice;

pub use host::AheadSessionHost;
pub use policy::PolicyEvaluator;
pub use prediction::PredictionEngine;
pub use store::SessionStore;
pub use voice::VoiceSession;
