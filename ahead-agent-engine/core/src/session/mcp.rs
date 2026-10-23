use super::mcp_refresh::McpRefreshInvalidationGuard;
use super::*;

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
            let mcp_projection = self
                .services
                .mcp_manager
                .runtime_config_for_step(
                    &desired.config,
                    McpEnvironmentScope::Live(&self.services.turn_environments),
                )
                .await;
            self.publish_mcp_runtime(&desired, mcp_projection).await;
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
        required_servers: &[String],
    ) -> Arc<codex_mcp::McpBinding> {
        self.refresh_mcp_if_dirty().await;
        if let Some(binding) = self
            .services
            .mcp_runtime
            .current_binding_with_required_servers(required_servers)
            .await
        {
            return binding;
        }
        let config = Arc::new(self.runtime_mcp_config(&turn_context.config).await);
        Arc::new(codex_mcp::McpBinding::empty(config))
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
        self.publish_mcp_runtime(&desired, mcp_projection).await;
    }

    pub(crate) fn cancel_mcp_startup(&self) {
        self.services.mcp_runtime.cancel_startup();
    }
}
