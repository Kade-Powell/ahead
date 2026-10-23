use crate::agents_md::LoadedAgentsMd;
use crate::agents_md::load_project_instructions;
use crate::config::Config;
use crate::environment_selection::TurnEnvironmentSnapshot;
use ahead_runtime_api::Instructions;
use codex_protocol::protocol::TurnEnvironmentSelection;
use std::io;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Owns the inputs and cached result of AGENTS.md discovery for a session.
pub(crate) struct AgentsMdManager {
    user_instructions: Option<Instructions>,
    cache: Mutex<AgentsMdCache>,
}

#[derive(Default)]
struct AgentsMdCache {
    selections: Option<Vec<TurnEnvironmentSelection>>,
    refresh_scope: InstructionRefreshScope,
    loaded: Option<Arc<LoadedAgentsMd>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum InstructionRefreshScope {
    #[default]
    SessionInitialization,
    Turn(String),
}

impl AgentsMdCache {
    fn matches(
        &self,
        selections: &[TurnEnvironmentSelection],
        refresh_scope: &InstructionRefreshScope,
    ) -> bool {
        self.selections.as_deref() == Some(selections) && self.refresh_scope == *refresh_scope
    }
}

impl AgentsMdManager {
    pub(crate) fn new(user_instructions: Option<Instructions>) -> Self {
        Self {
            user_instructions: user_instructions
                .filter(|instructions| !instructions.text.trim().is_empty()),
            cache: Mutex::new(AgentsMdCache::default()),
        }
    }

    #[tracing::instrument(name = "agents_md.refresh", skip_all)]
    pub(crate) async fn refresh(
        &self,
        config: &Config,
        environments: &TurnEnvironmentSnapshot,
    ) -> io::Result<()> {
        self.refresh_with_scope(
            config,
            environments,
            InstructionRefreshScope::SessionInitialization,
        )
        .await
    }

    /// Refreshes project instructions once per model turn so edits made between
    /// turns are visible without re-reading them for every tool step.
    pub(crate) async fn refresh_for_turn(
        &self,
        config: &Config,
        environments: &TurnEnvironmentSnapshot,
        turn_id: &str,
    ) -> io::Result<()> {
        self.refresh_with_scope(
            config,
            environments,
            InstructionRefreshScope::Turn(turn_id.to_string()),
        )
        .await
    }

    async fn refresh_with_scope(
        &self,
        config: &Config,
        environments: &TurnEnvironmentSnapshot,
        refresh_scope: InstructionRefreshScope,
    ) -> io::Result<()> {
        let selections = environments
            .turn_environments()
            .map(|environment| environment.selection.clone())
            .collect::<Vec<_>>();
        {
            let mut cache = self.cache.lock().await;
            if cache.matches(&selections, &refresh_scope) {
                return Ok(());
            }
            cache.selections = None;
            cache.loaded = None;
        }

        let loaded =
            load_project_instructions(config, self.user_instructions.clone(), environments)
                .await?
                .map(Arc::new);
        let mut cache = self.cache.lock().await;
        cache.selections = Some(selections);
        cache.refresh_scope = refresh_scope;
        cache.loaded = loaded;
        Ok(())
    }

    pub(crate) async fn get_loaded(&self) -> Option<Arc<LoadedAgentsMd>> {
        self.cache.lock().await.loaded.clone()
    }

    /// Distinguishes a refreshed turn with no project files from an uncaptured turn.
    pub(crate) async fn project_source_hashes_for_turn(
        &self,
        turn_id: &str,
    ) -> Option<Vec<(codex_utils_path_uri::PathUri, String)>> {
        let cache = self.cache.lock().await;
        if cache.selections.is_none()
            || !matches!(&cache.refresh_scope, InstructionRefreshScope::Turn(id) if id == turn_id)
        {
            return None;
        }
        Some(
            cache
                .loaded
                .as_ref()
                .map(|loaded| {
                    loaded
                        .project_source_hashes()
                        .map(|(path, hash)| (path.clone(), hash.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
        )
    }

    pub(crate) fn user_instructions(&self) -> Option<Instructions> {
        self.user_instructions.clone()
    }
}

#[cfg(test)]
#[path = "agents_md_manager_tests.rs"]
mod tests;
