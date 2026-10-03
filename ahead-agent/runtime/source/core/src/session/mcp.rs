use super::mcp_refresh::McpRefreshInvalidationGuard;
use super::*;
use crate::environment_selection::combine_selected_capability_roots;
use codex_exec_server::ExecutorCapabilityDiscoveryCache;
use codex_exec_server::ExecutorCapabilityDiscoverySnapshot;
use codex_exec_server::MAX_SELECTED_CAPABILITY_ROOTS;
use codex_exec_server::ResolvedSelectedCapabilityRoot;
use codex_protocol::capabilities::CapabilityRootLocation;
use codex_protocol::capabilities::SelectedCapabilityRoot;

pub(crate) struct McpServerElicitationOutcome {
    pub(crate) response: Option<ElicitationResponse>,
}

impl Session {
    pub(crate) async fn runtime_mcp_config(&self, config: &Config) -> McpConfig {
        self.runtime_mcp_config_and_context(config).await.0
    }

    pub(crate) async fn runtime_mcp_config_and_context(
        &self,
        config: &Config,
    ) -> (McpConfig, McpRuntimeContext) {
        let host_fallback_cwd = self.state.lock().await.session_configuration.cwd().clone();
        let environments = self.services.turn_environments.snapshot().await;
        let mcp_projection = self
            .services
            .mcp_manager
            .runtime_config_for_step(
                config,
                McpEnvironmentScope::Live(&self.services.turn_environments),
            )
            .await;
        let mcp_config = mcp_projection.config;
        let local_process_cwd = environments
            .local_environment_cwd()
            .map(|cwd| cwd.to_path_buf())
            .unwrap_or_else(|| host_fallback_cwd.to_path_buf());
        let runtime_context = McpRuntimeContext::new(
            self.services.turn_environments.environment_manager(),
            local_process_cwd,
        )
        .with_selected_environments(
            environments
                .turn_environments()
                .map(|environment| {
                    (
                        environment.selection.environment_id.clone(),
                        Arc::clone(&environment.environment),
                    )
                })
                .collect(),
        );
        (mcp_config, runtime_context)
    }

    /// Publishes changed MCP state, waiting for any refresh already in progress.
    #[tracing::instrument(name = "mcp.runtime.refresh_if_dirty", skip_all)]
    pub(crate) async fn refresh_mcp_if_dirty(self: &Arc<Self>) {
        let Ok(_refresh) = self.mcp_refresh.acquire().await else {
            error!("MCP runtime refresh semaphore closed");
            return;
        };
        loop {
            let environments = self.services.turn_environments.snapshot().await;
            let ready_environments = environments
                .turn_environments()
                .map(|environment| {
                    (
                        environment.selection.environment_id.clone(),
                        Arc::clone(&environment.environment),
                    )
                })
                .collect();
            // Attachment resolution can finish after the owner's configuration callback.
            if !self
                .services
                .mcp_runtime
                .current_environments_match(&ready_environments)
            {
                self.mark_mcp_runtime_dirty();
            }
            if !self.mcp_refresh.claim() {
                return;
            }
            let mut refresh_invalidation = McpRefreshInvalidationGuard {
                refresh: &self.mcp_refresh,
                published: false,
            };
            let desired = self.latest_mcp_desired_state().await;
            let selected_capability_roots = self
                .resolve_selected_capability_roots_for_step(&desired.environments)
                .await;
            let ready_selected_capability_roots =
                Self::ready_selected_capability_roots(&selected_capability_roots);
            let mcp_projection = self
                .services
                .mcp_manager
                .runtime_config_for_step(
                    &desired.config,
                    McpEnvironmentScope::Live(&self.services.turn_environments),
                )
                .await;
            self.publish_mcp_runtime(&desired, mcp_projection, &ready_selected_capability_roots)
                .await;
            refresh_invalidation.published = true;
        }
    }

    pub(super) fn mark_mcp_runtime_dirty(&self) {
        self.mcp_refresh.invalidate();
    }

    #[tracing::instrument(name = "mcp.runtime.resolve_for_step", skip_all)]
    pub(crate) async fn mcp_runtime_for_step(
        self: &Arc<Self>,
        turn_context: &TurnContext,
        selected_capability_roots: &[ResolvedSelectedCapabilityRoot],
        required_servers: &[String],
    ) -> Arc<codex_mcp::McpBinding> {
        let ready_selected_capability_roots =
            Self::ready_selected_capability_roots(selected_capability_roots);
        if self
            .services
            .mcp_runtime
            .current_ready_selected_capability_roots()
            != ready_selected_capability_roots
        {
            self.mark_mcp_runtime_dirty();
        }

        let recovered_oauth_servers = self
            .services
            .mcp_runtime
            .updated_oauth_credentials_after_auth_failure()
            .await;
        if !recovered_oauth_servers.is_empty()
            && let Ok(_refresh) = self.mcp_refresh.acquire().await
            && self
                .services
                .mcp_runtime
                .has_authentication_failed_servers(&recovered_oauth_servers)
                .await
        {
            self.mark_mcp_runtime_dirty();
        }
        self.refresh_mcp_if_dirty().await;
        let required_servers = required_servers
            .iter()
            .chain(&recovered_oauth_servers)
            .cloned()
            .collect::<Vec<_>>();
        if let Some(binding) = self
            .services
            .mcp_runtime
            .current_binding_with_required_servers(&required_servers)
            .await
        {
            return binding;
        }
        let config = Arc::new(self.runtime_mcp_config(&turn_context.config).await);
        Arc::new(codex_mcp::McpBinding::empty(config))
    }

    #[tracing::instrument(
        name = "capability_roots.snapshot_for_step",
        skip_all,
        fields(root_count = ready_selected_capability_roots.len())
    )]
    pub(crate) async fn executor_capability_discovery_for_step(
        &self,
        config: &Config,
        ready_selected_capability_roots: &[SelectedCapabilityRoot],
        environments: &TurnEnvironmentSnapshot,
    ) -> Option<Arc<ExecutorCapabilityDiscoverySnapshot>> {
        // Capability roots can currently be selected independently of turn environments, so a
        // root may be ready when there is no primary `TurnEnvironment`. Keep using the thread
        // policy in that case so restricted discovery fails closed below. Once every selected
        // root belongs to a thread/environment attachment whose `EnvironmentConfig` is installed
        // before the root becomes ready, discovery can use the root owner's policy and this
        // fallback can be removed.
        let restricted_file_system = environments.primary().map_or_else(
            || {
                !config
                    .permissions
                    .file_system_sandbox_policy()
                    .has_full_disk_read_access()
            },
            |_| {
                environments.turn_environments().any(|environment| {
                    !environment
                        .permission_profile()
                        .file_system_sandbox_policy()
                        .has_full_disk_read_access()
                })
            },
        );
        if !restricted_file_system
            && !config
                .features
                .enabled(Feature::ExecutorCapabilityDiscovery)
        {
            return None;
        }
        let sandbox_contexts = if restricted_file_system {
            environments
                .turn_environments()
                .map(|environment| {
                    (
                        environment.selection.environment_id.clone(),
                        environment.sandbox_context(/*additional_permissions*/ None),
                    )
                })
                .collect::<HashMap<_, _>>()
        } else {
            HashMap::new()
        };
        let environment_manager = self.services.turn_environments.environment_manager();
        let cache = self
            .services
            .thread_extension_data
            .get_or_init(|| ExecutorCapabilityDiscoveryCache::new(environment_manager));
        let selected_capability_roots = ready_selected_capability_roots
            .iter()
            .filter(|selected_root| {
                if !restricted_file_system {
                    return true;
                }
                let CapabilityRootLocation::Environment { environment_id, .. } =
                    &selected_root.location;
                if sandbox_contexts.contains_key(environment_id) {
                    return true;
                }
                warn!(
                    selected_root = selected_root.id,
                    environment_id, "skipping capability root without a filesystem sandbox context"
                );
                false
            })
            .cloned()
            .collect::<Vec<_>>();
        let discovery = cache
            .snapshot(&selected_capability_roots, &sandbox_contexts)
            .await;
        if cache.take_recovered_discovery() {
            // Root selection is unchanged, but recovered manifests can change MCP servers.
            self.mark_mcp_runtime_dirty();
        }
        Some(Arc::new(discovery))
    }

    pub(crate) async fn resolve_selected_capability_roots_for_step(
        &self,
        environments: &TurnEnvironmentSnapshot,
    ) -> Vec<ResolvedSelectedCapabilityRoot> {
        let thread_root_count = self.services.selected_capability_roots.len();
        let mut root_locations_by_id = HashMap::new();
        let mut selected_capability_roots = Vec::new();
        let mut ready_environment_root_count = 0;
        let combined_roots = combine_selected_capability_roots(
            &self.services.selected_capability_roots,
            environments.turn_environments().map(|environment| {
                (
                    environment.config_origin,
                    environment
                        .config_origin
                        .selected_capability_roots(&environment.environment, environment.config()),
                )
            }),
        );
        for (index, root) in combined_roots.into_iter().enumerate() {
            if let Some(kept_location) = root_locations_by_id.get(&root.id) {
                if kept_location != &root.location {
                    tracing::warn!(
                        root_id = root.id,
                        ?kept_location,
                        ignored_location = ?root.location,
                        "ignoring selected capability root with conflicting location"
                    );
                }
                continue;
            }
            if index >= thread_root_count {
                if ready_environment_root_count == MAX_SELECTED_CAPABILITY_ROOTS {
                    tracing::warn!(
                        max_root_count = MAX_SELECTED_CAPABILITY_ROOTS,
                        "ignoring excess selected capability roots from ready environments"
                    );
                    break;
                }
                ready_environment_root_count += 1;
            }
            root_locations_by_id.insert(root.id.clone(), root.location.clone());
            selected_capability_roots.push(root);
        }
        self.services
            .turn_environments
            .environment_manager()
            .resolve_selected_capability_roots(
                &selected_capability_roots,
                &environments.captured_environments(),
            )
            .await
    }

    pub(crate) fn mcp_elicitation_lifecycle(&self) -> codex_mcp::ElicitationLifecycle {
        self.mcp_elicitation_lifecycle_handle
            .get_or_init(|| {
                let elicitations = self.services.elicitations.clone();
                codex_mcp::ElicitationLifecycle::new(move || elicitations.register())
            })
            .clone()
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "active turn checks and turn state updates must remain atomic"
    )]
    pub async fn request_mcp_server_elicitation(
        &self,
        turn_context: &TurnContext,
        server_name: String,
        request_id: RequestId,
        request: ElicitationRequest,
    ) -> McpServerElicitationOutcome {
        if self.services.mcp_runtime.elicitations_auto_deny() {
            return McpServerElicitationOutcome {
                response: Some(ElicitationResponse {
                    action: codex_rmcp_client::ElicitationAction::Accept,
                    content: Some(serde_json::json!({})),
                    meta: None,
                }),
            };
        }

        let _elicitation = self.services.elicitations.register();
        let (tx_response, rx_response) = oneshot::channel();
        let prev_entry = {
            let mut active = self.active_turn.lock().await;
            match active.as_mut() {
                Some(at) => {
                    let mut ts = at.turn_state.lock().await;
                    ts.insert_pending_elicitation(
                        server_name.clone(),
                        request_id.clone(),
                        tx_response,
                    )
                }
                None => None,
            }
        };
        if prev_entry.is_some() {
            warn!(
                "Overwriting existing pending elicitation for server_name: {server_name}, request_id: {request_id}"
            );
        }
        let id = match request_id {
            rmcp::model::NumberOrString::String(value) => {
                codex_protocol::mcp::RequestId::String(value.to_string())
            }
            rmcp::model::NumberOrString::Number(value) => {
                codex_protocol::mcp::RequestId::Integer(value)
            }
        };
        let event = EventMsg::ElicitationRequest(ElicitationRequestEvent {
            turn_id: Some(turn_context.sub_id.clone()),
            server_name,
            id,
            request,
        });
        turn_context
            .turn_metadata_state
            .mark_user_input_requested_during_turn();
        self.send_event(turn_context, event).await;
        McpServerElicitationOutcome {
            response: rx_response.await.ok(),
        }
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "active turn checks and manager fallback must stay serialized"
    )]
    pub async fn resolve_elicitation(
        &self,
        server_name: String,
        id: RequestId,
        response: ElicitationResponse,
    ) -> anyhow::Result<()> {
        let entry = {
            let mut active = self.active_turn.lock().await;
            match active.as_mut() {
                Some(at) => {
                    let mut ts = at.turn_state.lock().await;
                    ts.remove_pending_elicitation(&server_name, &id)
                }
                None => None,
            }
        };
        if let Some(tx_response) = entry {
            tx_response
                .send(response)
                .map_err(|e| anyhow::anyhow!("failed to send elicitation response: {e:?}"))?;
            return Ok(());
        }

        self.services
            .mcp_runtime
            .resolve_elicitation(server_name, id, response)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn refresh_mcp_servers_now(&self, refresh_config: &Config) {
        let Ok(_refresh) = self.mcp_refresh.acquire().await else {
            error!("MCP runtime refresh semaphore closed");
            return;
        };
        {
            let mut state = self.state.lock().await;
            let mut config = (*state.session_configuration.original_config_do_not_use).clone();
            config.mcp_servers = refresh_config.mcp_servers.clone();
            state.session_configuration.original_config_do_not_use = Arc::new(config);
        }
        let ready_selected_capability_roots = self
            .services
            .mcp_runtime
            .current_ready_selected_capability_roots();
        let mcp_projection = self
            .services
            .mcp_manager
            .runtime_config_for_step(
                refresh_config,
                McpEnvironmentScope::Live(&self.services.turn_environments),
            )
            .await;
        let mut desired = self.latest_mcp_desired_state().await;
        desired.config = Arc::new(refresh_config.clone());
        self.publish_mcp_runtime(&desired, mcp_projection, &ready_selected_capability_roots)
            .await;
    }

    pub(crate) fn ready_selected_capability_roots(
        selected_capability_roots: &[ResolvedSelectedCapabilityRoot],
    ) -> Vec<SelectedCapabilityRoot> {
        selected_capability_roots
            .iter()
            .map(|root| root.selected_root().clone())
            .collect()
    }

    pub(crate) fn cancel_mcp_startup(&self) {
        self.services.mcp_runtime.cancel_startup();
    }
}
