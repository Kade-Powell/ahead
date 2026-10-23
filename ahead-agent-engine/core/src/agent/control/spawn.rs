use super::*;
use crate::codex_thread::CodexThread;
use crate::session::turn_context::TurnContext;
use codex_protocol::intersect_effective_permission_profiles;
use codex_protocol::models::PermissionProfileSnapshot;
use codex_protocol::protocol::EnvironmentConfigState;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_thread_store::LoadThreadHistoryParams;
use codex_utils_path_uri::PathUri;

const AGENT_NAMES: &str = include_str!("../../../assets/agent/agent_names.txt");

fn default_agent_nickname_list() -> Vec<&'static str> {
    AGENT_NAMES
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect()
}

pub(super) fn agent_nickname_candidates() -> Vec<String> {
    default_agent_nickname_list()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect()
}

async fn load_agent_model_context(
    state: &ThreadManagerState,
    thread_id: ThreadId,
    history_mode: ThreadHistoryMode,
) -> CodexResult<Option<Vec<RolloutItem>>> {
    match history_mode {
        ThreadHistoryMode::Legacy => Ok(state
            .read_stored_thread(ReadThreadParams {
                thread_id,
                include_archived: true,
                include_history: true,
            })
            .await?
            .history
            .map(|history| history.items)),
        ThreadHistoryMode::Paginated => Ok(Some(
            state
                .load_latest_model_context(LoadThreadHistoryParams {
                    thread_id,
                    include_archived: true,
                })
                .await?
                .items,
        )),
    }
}

impl AgentControl {
    pub(crate) async fn restore_v2_agent_metadata(&self, root_thread_id: ThreadId) {
        self.state.register_root_thread(root_thread_id);

        let Ok(state) = self.upgrade() else {
            return;
        };
        let Some(agent_graph_store) = state.agent_graph_store() else {
            return;
        };
        let descendant_ids = match agent_graph_store
            .list_thread_spawn_descendants(
                root_thread_id,
                Some(ahead_agent_graph_store::ThreadSpawnEdgeStatus::Open),
            )
            .await
        {
            Ok(descendant_ids) => descendant_ids,
            Err(err) => {
                warn!("failed to restore persisted V2 agent metadata for {root_thread_id}: {err}");
                return;
            }
        };

        for thread_id in descendant_ids {
            if self.state.agent_metadata_for_thread(thread_id).is_some() {
                continue;
            }
            let restore_result = async {
                let stored_thread = state
                    .read_stored_thread(ReadThreadParams {
                        thread_id,
                        include_archived: true,
                        include_history: false,
                    })
                    .await?;
                let stored_agent_path = stored_thread
                    .agent_path
                    .as_deref()
                    .map(AgentPath::try_from)
                    .transpose()
                    .map_err(|err| {
                        CodexErr::InvalidRequest(format!("invalid stored agent path: {err}"))
                    })?;
                let mut reservation = self.state.reserve_spawn_slot(/*max_threads*/ None)?;
                let mut metadata = self.prepare_agent_metadata(
                    &mut reservation,
                    stored_agent_path.or_else(|| stored_thread.source.get_agent_path()),
                    stored_thread
                        .agent_nickname
                        .or_else(|| stored_thread.source.get_nickname()),
                )?;
                metadata.agent_id = Some(thread_id);
                reservation.commit(metadata);
                Ok::<(), CodexErr>(())
            }
            .await;
            if let Err(err) = restore_result {
                warn!("failed to restore V2 agent metadata for {thread_id}: {err}");
            }
        }
    }

    fn validate_loaded_v2_child(
        &self,
        thread: &CodexThread,
        parent_thread_id: ThreadId,
    ) -> CodexResult<()> {
        if thread.is_running()
            && thread.multi_agent_version() == Some(MultiAgentVersion::V2)
            && thread.session_source.parent_thread_id() == Some(parent_thread_id)
            && Arc::ptr_eq(&self.state, &thread.session.services.agent_control.state)
        {
            return Ok(());
        }
        Err(CodexErr::InvalidRequest(format!(
            "multi-agent v2 child {} is not owned by its loaded parent",
            thread.session.thread_id
        )))
    }

    /// A provided parent enables owner-validated reloads; `None` preserves sender-driven reloads.
    pub(crate) async fn ensure_v2_agent_loaded(
        &self,
        mut config: Config,
        thread_id: ThreadId,
        parent: Option<Arc<CodexThread>>,
    ) -> CodexResult<()> {
        let state = self.upgrade()?;
        let parent = if let Some(parent) = parent {
            let parent_thread_id = parent.session.thread_id;
            let turn = parent.session.new_default_turn().await;
            config = build_agent_resume_config(&turn).map_err(|_| {
                CodexErr::InvalidRequest(format!(
                    "cannot resume multi-agent v2 child {thread_id} with the current parent settings"
                ))
            })?;
            let registered_parent = state.get_thread(parent_thread_id).await.ok();
            if !registered_parent
                .as_ref()
                .is_some_and(|registered| Arc::ptr_eq(registered, &parent))
                || !parent.is_running()
                || parent.multi_agent_version() != Some(MultiAgentVersion::V2)
                || !Arc::ptr_eq(&self.state, &parent.session.services.agent_control.state)
            {
                return Err(CodexErr::InvalidRequest(format!(
                    "cannot resume multi-agent v2 child {thread_id}: parent ownership is unavailable; resume the parent first"
                )));
            }
            Some((parent, turn.environments.clone()))
        } else {
            None
        };
        let owner_thread_id = parent.as_ref().map(|(parent, _)| parent.session.thread_id);
        if owner_thread_id.is_none() && state.get_thread(thread_id).await.is_ok() {
            self.touch_loaded_v2_residency(&state, thread_id).await;
            return Ok(());
        }
        if self.state.agent_metadata_for_thread(thread_id).is_none() {
            return Err(CodexErr::ThreadNotFound(thread_id));
        }
        let mut environment_selections = self.state.evicted_environments(thread_id);

        let stored_thread = state
            .read_stored_thread(ReadThreadParams {
                thread_id,
                include_archived: true,
                include_history: false,
            })
            .await?;
        let stored_model = stored_thread.model.clone();
        let stored_model_provider = stored_thread.model_provider.clone();
        let stored_reasoning_effort = stored_thread.reasoning_effort.clone();
        let stored_source = stored_thread.source.clone();
        let stored_parent_thread_id = stored_thread.parent_thread_id;
        let history = load_agent_model_context(&state, thread_id, stored_thread.history_mode)
            .await?
            .ok_or(CodexErr::ThreadNotFound(thread_id))?;
        let initial_history = InitialHistory::Resumed(ResumedHistory {
            conversation_id: thread_id,
            history: Arc::new(history),
            rollout_path: stored_thread.rollout_path,
        });
        if initial_history.get_multi_agent_version() != Some(MultiAgentVersion::V2) {
            return Err(CodexErr::ThreadNotFound(thread_id));
        }
        let (session_source, _) = initial_history
            .get_resumed_session_sources()
            .unwrap_or((stored_source, None));
        if let Some(parent_thread_id) = owner_thread_id {
            if session_source.parent_thread_id() != Some(parent_thread_id)
                || initial_history
                    .get_resumed_parent_thread_id()
                    .is_some_and(|recorded_parent| recorded_parent != parent_thread_id)
                || stored_parent_thread_id
                    .is_some_and(|recorded_parent| recorded_parent != parent_thread_id)
            {
                return Err(CodexErr::InvalidRequest(format!(
                    "cannot resume multi-agent v2 child {thread_id}: recorded parent ownership is inconsistent"
                )));
            }
            if let Ok(thread) = state.get_thread(thread_id).await {
                self.validate_loaded_v2_child(&thread, parent_thread_id)?;
                self.touch_loaded_v2_residency(&state, thread_id).await;
                return Ok(());
            }
        }
        config.model_reasoning_effort = stored_reasoning_effort;
        config.service_tier = self.root_service_tier();
        if let Some(model) = stored_model {
            config.model = Some(model);
        }
        if config.model_provider_id != stored_model_provider {
            config.model_provider = config
                .model_providers
                .get(&stored_model_provider)
                .cloned()
                .ok_or_else(|| {
                    CodexErr::InvalidRequest(format!(
                        "Model provider `{stored_model_provider}` not found"
                    ))
                })?;
            config.model_provider_id = stored_model_provider;
        }
        let parent_thread_id = owner_thread_id
            .or_else(|| initial_history.get_resumed_parent_thread_id())
            .or(stored_parent_thread_id);
        let (inherited_environments, inherited_exec_policy, client_mcp_extensions) = if let Some(
            (parent, parent_environments),
        ) =
            parent.as_ref()
        {
            let parent_config = parent.session.get_config().await;
            if !crate::exec_policy::child_uses_parent_exec_policy(&parent_config, &config) {
                return Err(CodexErr::InvalidRequest(format!(
                    "cannot resume multi-agent v2 child {thread_id}: parent execution policy has changed; retry through the parent"
                )));
            }
            if let Some(selections) = environment_selections.as_mut() {
                for selection in selections {
                    let environment_id = &selection.environment_id;
                    let invalid_environment = |reason: &str| {
                        CodexErr::InvalidRequest(format!(
                            "cannot resume multi-agent v2 child {thread_id}: cached environment {environment_id} {reason}"
                        ))
                    };
                    // Matching the attachment also keeps startup on the captured owner executor.
                    let owner_environment = parent_environments
                        .turn_environments()
                        .find(|environment| {
                            let parent_selection = &environment.selection;
                            parent_selection.environment_id == selection.environment_id
                                && parent_selection.cwd == selection.cwd
                                && parent_selection.workspace_roots == selection.workspace_roots
                        })
                        .ok_or_else(|| {
                            invalid_environment("no longer matches a ready parent environment")
                        })?;
                    let owner_config = owner_environment.config();
                    let child_config = match &selection.config {
                        EnvironmentConfigState::FromThread => {
                            // Pin current owner authority instead of re-inferring child settings.
                            selection.config = EnvironmentConfigState::Ready(owner_config.clone());
                            continue;
                        }
                        EnvironmentConfigState::Ready(config) => config,
                        EnvironmentConfigState::Pending | EnvironmentConfigState::Failed(_) => {
                            return Err(invalid_environment("configuration is not ready"));
                        }
                    };
                    let mut bounded_config = child_config.clone();
                    bounded_config.permission_profile = owner_config.permission_profile.clone();
                    if bounded_config != *owner_config {
                        return Err(invalid_environment(
                            "configuration differs from the current parent",
                        ));
                    }
                    if child_config.permission_profile == owner_config.permission_profile {
                        continue;
                    }
                    let cwd = selection.cwd.to_abs_path().map_err(|_| {
                        invalid_environment("working directory is not a local absolute path")
                    })?;
                    let roots = owner_environment
                        .workspace_roots()
                        .iter()
                        .map(PathUri::to_abs_path)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| {
                            invalid_environment("workspace roots are not local absolute paths")
                        })?;
                    let authority = owner_environment
                        .permission_profile()
                        .clone()
                        .materialize_project_roots_with_workspace_roots(&roots);
                    let requested = child_config
                        .permission_profile
                        .permission_profile()
                        .clone()
                        .materialize_project_roots_with_workspace_roots(&roots);
                    let permissions =
                        intersect_effective_permission_profiles(&authority, &requested, &cwd)
                            .map_err(|err| {
                                invalid_environment(&format!(
                                    "permissions cannot be intersected safely: {err}"
                                ))
                            })?;
                    bounded_config.permission_profile =
                        PermissionProfileSnapshot::legacy(permissions);
                    selection.config = EnvironmentConfigState::Ready(bounded_config);
                }
            }
            (
                Some(parent_environments.clone()),
                Some(Arc::clone(&parent.session.services.exec_policy)),
                Some(parent.client_mcp_extensions()),
            )
        } else {
            (
                self.inherited_environments_for_source(&state, Some(&session_source))
                    .await,
                self.inherited_exec_policy_for_source(&state, Some(&session_source), &config)
                    .await,
                None,
            )
        };
        // Reserving a slot can evict an idle nested parent. Keep its authority captured above.
        let residency_slot = self
            .reserve_v2_residency_slot(&state, &config, Some(thread_id))
            .await?;

        match state
            .resume_thread_with_history_with_source(ResumeThreadWithHistoryOptions {
                config,
                initial_history,
                agent_control: self.clone(),
                session_source,
                parent_thread_id,
                environment_selections,
                inherited_environments,
                inherited_exec_policy,
                client_mcp_extensions,
            })
            .await
        {
            Ok(reloaded_thread) => {
                if let Some(parent_thread_id) = owner_thread_id {
                    self.validate_loaded_v2_child(&reloaded_thread.thread, parent_thread_id)?;
                }
                self.state.clear_evicted_environments(thread_id);
                residency_slot.commit(reloaded_thread.thread_id);
                state.notify_thread_created(reloaded_thread.thread_id);
                Ok(())
            }
            Err(err) => {
                if let Ok(thread) = state.get_thread(thread_id).await {
                    if let Some(parent_thread_id) = owner_thread_id {
                        self.validate_loaded_v2_child(&thread, parent_thread_id)?;
                    }
                    self.state.clear_evicted_environments(thread_id);
                    drop(residency_slot);
                    self.touch_loaded_v2_residency(&state, thread_id).await;
                    return Ok(());
                }
                Err(err)
            }
        }
    }
}

fn build_agent_resume_config(turn: &TurnContext) -> CodexResult<Config> {
    let mut config = (*turn.config).clone();
    config.model = Some(turn.model_info().slug.clone());
    config.model_provider = turn.provider.info().clone();
    config.model_reasoning_effort = turn
        .reasoning_effort()
        .or(turn.model_info().default_reasoning_level.as_ref())
        .cloned();
    config.model_reasoning_summary = Some(turn.reasoning_summary());
    config.developer_instructions = turn.developer_instructions.clone();
    config
        .permissions
        .approval_policy
        .set(turn.approval_policy())
        .map_err(|err| CodexErr::InvalidRequest(format!("approval_policy is invalid: {err}")))?;
    #[allow(deprecated)]
    {
        config.cwd = turn.cwd.clone();
    }
    config
        .permissions
        .set_permission_profile_from_session_snapshot(
            turn.config
                .permissions
                .permission_profile_state()
                .snapshot(),
        )
        .map_err(|err| CodexErr::InvalidRequest(format!("permission_profile is invalid: {err}")))?;
    config.base_instructions = None;
    config.base_instructions_provenance = None;
    Ok(config)
}
