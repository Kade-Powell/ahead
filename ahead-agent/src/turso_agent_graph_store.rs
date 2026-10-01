use std::{collections::HashSet, sync::Arc};

use codex_agent_graph_store::{
    AgentGraphStore, AgentGraphStoreError, AgentGraphStoreFuture,
    ThreadSpawnEdgeStatus,
};
use codex_protocol::ThreadId;

use crate::{HarnessStore, NativeAgentEdgeStatus};

pub(crate) struct TursoAgentGraphStore {
    store: Arc<dyn HarnessStore>,
}

impl TursoAgentGraphStore {
    pub(crate) fn new(store: Arc<dyn HarnessStore>) -> Self {
        Self { store }
    }
}

fn native_status(status: ThreadSpawnEdgeStatus) -> NativeAgentEdgeStatus {
    match status {
        ThreadSpawnEdgeStatus::Open => NativeAgentEdgeStatus::Open,
        ThreadSpawnEdgeStatus::Closed => NativeAgentEdgeStatus::Closed,
    }
}

fn store_error(error: impl std::fmt::Display) -> AgentGraphStoreError {
    AgentGraphStoreError::Internal {
        message: error.to_string(),
    }
}

fn parse_thread_ids(
    ids: Vec<String>,
) -> Result<Vec<ThreadId>, AgentGraphStoreError> {
    ids.into_iter()
        .map(|id| ThreadId::from_string(&id).map_err(store_error))
        .collect()
}

impl AgentGraphStore for TursoAgentGraphStore {
    fn upsert_thread_spawn_edge(
        &self,
        parent_thread_id: ThreadId,
        child_thread_id: ThreadId,
        status: ThreadSpawnEdgeStatus,
    ) -> AgentGraphStoreFuture<'_, ()> {
        Box::pin(async move {
            self.store
                .upsert_native_agent_edge(
                    &parent_thread_id.to_string(),
                    &child_thread_id.to_string(),
                    native_status(status),
                )
                .map_err(store_error)
        })
    }

    fn set_thread_spawn_edge_status(
        &self,
        child_thread_id: ThreadId,
        status: ThreadSpawnEdgeStatus,
    ) -> AgentGraphStoreFuture<'_, ()> {
        Box::pin(async move {
            self.store
                .set_native_agent_edge_status(
                    &child_thread_id.to_string(),
                    native_status(status),
                )
                .map_err(store_error)
        })
    }

    fn list_thread_spawn_children(
        &self,
        parent_thread_id: ThreadId,
        status_filter: Option<ThreadSpawnEdgeStatus>,
    ) -> AgentGraphStoreFuture<'_, Vec<ThreadId>> {
        Box::pin(async move {
            parse_thread_ids(
                self.store
                    .list_native_agent_children(
                        &parent_thread_id.to_string(),
                        status_filter.map(native_status),
                    )
                    .map_err(store_error)?,
            )
        })
    }

    fn list_thread_spawn_descendants(
        &self,
        root_thread_id: ThreadId,
        status_filter: Option<ThreadSpawnEdgeStatus>,
    ) -> AgentGraphStoreFuture<'_, Vec<ThreadId>> {
        Box::pin(async move {
            let mut seen = HashSet::from([root_thread_id.to_string()]);
            let mut frontier = vec![root_thread_id.to_string()];
            let mut descendants = Vec::new();
            while !frontier.is_empty() {
                let mut next = Vec::new();
                for parent in frontier {
                    next.extend(
                        self.store
                            .list_native_agent_children(
                                &parent,
                                status_filter.map(native_status),
                            )
                            .map_err(store_error)?,
                    );
                }
                next.sort_unstable();
                next.dedup();
                next.retain(|id| seen.insert(id.clone()));
                descendants.extend(next.iter().cloned());
                frontier = next;
            }
            parse_thread_ids(descendants)
        })
    }
}
