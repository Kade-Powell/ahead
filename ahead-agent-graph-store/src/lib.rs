//! Storage-neutral parent/child topology for thread-spawned agents.

mod error;
mod store;
mod types;

pub use error::AgentGraphStoreError;
pub use error::AgentGraphStoreResult;
pub use store::AgentGraphStore;
pub use store::AgentGraphStoreFuture;
pub use types::ThreadSpawnEdgeStatus;
