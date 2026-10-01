use super::*;
use crate::config::Constrained;
use crate::exec_policy::ExecPolicyManager;
use crate::sandboxing::SandboxPermissions;
use crate::session::step_context::StepContext;
use crate::session::tests::update_turn_settings_for_test;
use crate::session::turn_context::NewTurnContextOptions;
use crate::test_support::models_manager_with_provider;
use crate::tools::context::ToolCallSource;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::orchestrator::ToolOrchestrator;
use crate::tools::sandboxing::Approvable;
use crate::tools::sandboxing::ApprovalAction;
use crate::tools::sandboxing::ExecApprovalRequirement;
use crate::tools::sandboxing::SandboxAttempt;
use crate::tools::sandboxing::Sandboxable;
use crate::tools::sandboxing::ToolCtx;
use crate::tools::sandboxing::ToolError;
use crate::tools::sandboxing::ToolRuntime;
use crate::turn_diff_tracker::TurnDiffTracker;
use codex_exec_server::EnvironmentManager;
use codex_execpolicy::Decision;
use codex_execpolicy::Evaluation;
use codex_execpolicy::Policy;
use codex_execpolicy::RuleMatch;
use codex_features::Feature;
use codex_model_provider::create_model_provider;
use codex_network_proxy::NetworkDecision;
use codex_network_proxy::NetworkPolicyRequest;
use codex_network_proxy::NetworkProtocol;
use codex_protocol::error::SandboxErr;
use codex_protocol::models::AdditionalPermissionProfile as PermissionProfile;
use codex_protocol::models::ContentItem;
use codex_protocol::models::NetworkPermissions;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::request_permissions::PermissionGrantScope;
use codex_protocol::request_permissions::RequestPermissionProfile;
use codex_protocol::request_permissions::RequestPermissionsArgs;
use codex_protocol::request_permissions::RequestPermissionsResponse;
use core_test_support::codex_linux_sandbox_exe_or_skip;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::sse;
use core_test_support::responses::sse_response;
use core_test_support::responses::start_mock_server;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

fn expect_text_output<T>(output: &T) -> String
where
    T: ToolOutput + ?Sized,
{
    let response = output.to_response_item(
        "call-guardian",
        &ToolPayload::Function {
            arguments: "{}".to_string(),
        },
    );
    match response {
        ResponseInputItem::FunctionCallOutput { output, .. }
        | ResponseInputItem::CustomToolCallOutput { output, .. } => {
            output.body.to_text().unwrap_or_default()
        }
        other => panic!("expected function output, got {other:?}"),
    }
}

async fn activate_turn_with_new_approval_policy(session: &Arc<Session>) -> Arc<TurnContext> {
    let (current_turn, _) = session
        .new_turn_with_sub_id(
            "current-authority-turn".to_string(),
            SessionSettingsUpdate {
                step_settings: StepSettingsUpdate {
                    approval_policy: Some(AskForApproval::Never),
                    ..Default::default()
                },
                permission_profile: Some(codex_protocol::models::PermissionProfile::Disabled),
                ..Default::default()
            },
            NewTurnContextOptions::default(),
        )
        .await
        .expect("next turn should accept different approval authority");
    session
        .start_task(
            current_turn,
            Vec::new(),
            super::NeverEndingTask {
                kind: crate::state::TaskKind::Regular,
                listen_to_cancellation_token: true,
            },
        )
        .await;

    let (active_turn, _, _) = session
        .active_turn_context_and_strict_auto_review()
        .await
        .expect("next turn should have active review authority");
    assert_eq!(active_turn.approval_policy(), AskForApproval::Never);
    active_turn
}

fn captured_step_with_policy(
    turn: &mut Arc<TurnContext>,
    admitted_policy: AskForApproval,
    captured_policy: AskForApproval,
) -> Arc<StepContext> {
    let config = Arc::make_mut(
        &mut Arc::get_mut(turn)
            .expect("turn should not be shared")
            .config,
    );
    config
        .permissions
        .approval_policy
        .set(admitted_policy)
        .expect("set admitted turn approval policy");

    let mut step = StepContext::for_test(Arc::clone(turn));
    let captured = Arc::get_mut(&mut step).expect("step context should not be shared");
    update_selected_settings_for_test(Arc::make_mut(&mut captured.settings), |selected| {
        selected
            .approval_policy
            .set(captured_policy)
            .expect("set captured approval policy");
    });
    step
}

async fn next_exec_approval(
    events: &async_channel::Receiver<Event>,
) -> codex_protocol::protocol::ExecApprovalRequestEvent {
    timeout(Duration::from_secs(5), async {
        loop {
            if let EventMsg::ExecApprovalRequest(approval) =
                events.recv().await.expect("receive approval event").msg
            {
                break approval;
            }
        }
    })
    .await
    .expect("captured action should request user approval")
}

#[tokio::test]
async fn network_approval_uses_published_task_authority_within_same_turn() {
    for admitted_policy in [AskForApproval::Never, AskForApproval::OnRequest] {
        let (session, turn, events) = make_session_and_context_with_auth_and_config_and_rx(
            CodexAuth::from_api_key("Test API Key"),
            Vec::new(),
            move |config| {
                config.permissions.approval_policy = Constrained::allow_any(admitted_policy);
                config
                    .permissions
                    .set_permission_profile(
                        codex_protocol::models::PermissionProfile::workspace_write(),
                    )
                    .expect("set managed permissions");
            },
        )
        .await;
        session
            .start_task(
                Arc::clone(&turn),
                Vec::new(),
                super::NeverEndingTask {
                    kind: crate::state::TaskKind::Regular,
                    listen_to_cancellation_token: true,
                },
            )
            .await;
        // Inject later-step authority directly while live policy changes remain gated.
        {
            let active = session.active_turn.lock().await;
            let task = active
                .as_ref()
                .expect("active turn")
                .task
                .as_ref()
                .expect("active task");
            let mut settings = task.turn_context.current_settings.load_full();
            update_selected_settings_for_test(Arc::make_mut(&mut settings), |selected| {
                selected
                    .approval_policy
                    .set(AskForApproval::OnRequest)
                    .expect("update policy");
            });
            task.turn_context.current_settings.store(settings);
        }
        let decision = session
            .services
            .network_approval
            .handle_inline_policy_request(
                Arc::clone(&session),
                NetworkPolicyRequest {
                    protocol: NetworkProtocol::Http,
                    host: "example.com".to_string(),
                    port: 80,
                    environment_id: None,
                    client_addr: None,
                    method: None,
                    command: None,
                    exec_policy_hint: None,
                    execution_id: None,
                    disconnect: None,
                },
            );
        tokio::pin!(decision);
        let approval = timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = &mut decision => panic!("expected user network approval, got {result:?}"),
                event = events.recv() => {
                    match event.expect("approval event").msg {
                        EventMsg::ExecApprovalRequest(approval) => break approval,
                        _ => {}
                    }
                }
            }
        }
    })
    .await
    .expect("network approval requested");
        assert_eq!(approval.turn_id, turn.sub_id);
        session
            .notify_approval(&approval.call_id, ReviewDecision::Approved)
            .await;
        assert_eq!(
            timeout(Duration::from_secs(5), decision)
                .await
                .expect("network decision"),
            NetworkDecision::Allow
        );
        session.abort_all_tasks(TurnAbortReason::Interrupted).await;
    }
}

#[tokio::test]
async fn delayed_exec_command_uses_its_captured_authority_after_next_turn_starts() {
    let (mut session, mut action_turn, events) = make_session_and_context_with_rx().await;
    // Windows can allow safe echo commands without prompting when its sandbox is disabled.
    let mut exec_policy = Policy::empty();
    exec_policy
        .add_prefix_rule(
            &["echo".to_string(), "captured-action-authority".to_string()],
            Decision::Prompt,
        )
        .expect("test command should require approval");
    Arc::get_mut(&mut session)
        .expect("session should not be shared")
        .services
        .exec_policy = Arc::new(ExecPolicyManager::new(Arc::new(exec_policy)));
    let step_context = captured_step_with_policy(
        &mut action_turn,
        AskForApproval::Never,
        AskForApproval::OnRequest,
    );
    let current_turn = activate_turn_with_new_approval_policy(&session).await;
    assert_ne!(action_turn.sub_id, current_turn.sub_id);

    let call_id = "delayed-captured-authority-shell-command";
    let command = "echo captured-action-authority";
    let handler = crate::tools::handlers::ExecCommandHandler::default();
    let invocation = handler.handle(ToolInvocation {
        session: Arc::clone(&session),
        turn: Arc::clone(&action_turn),
        step_context,
        cancellation_token: CancellationToken::new(),
        tracker: Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new())),
        call_id: call_id.to_string(),
        tool_name: codex_tools::ToolName::plain("exec_command"),
        source: ToolCallSource::Direct,
        payload: ToolPayload::Function {
            arguments: serde_json::json!({
                "cmd": command,
                "login": false,
                "sandbox_permissions": SandboxPermissions::RequireEscalated,
                "justification": "verify captured action authority",
            })
            .to_string(),
        },
    });
    let approve = async {
        let approval = next_exec_approval(&events).await;
        assert_eq!(approval.call_id, call_id);
        assert_eq!(approval.turn_id, action_turn.sub_id);
        assert!(approval.command.join(" ").contains(command));
        session
            .notify_approval(call_id, ReviewDecision::Approved)
            .await;
    };

    let (output, ()) = tokio::join!(invocation, approve);
    let output = output.expect("approved shell command should succeed");
    assert!(expect_text_output(output.as_ref()).contains("captured-action-authority"));
}

#[tokio::test]
async fn sandbox_denied_retry_uses_the_action_policy_and_reviewer() {
    #[derive(Default)]
    struct DeniedOnceRuntime {
        attempts: usize,
    }

    impl Approvable<TurnEnvironment> for DeniedOnceRuntime {
        fn exec_approval_requirement(
            &self,
            _request: &TurnEnvironment,
        ) -> Option<ExecApprovalRequirement> {
            Some(ExecApprovalRequirement::Skip {
                bypass_sandbox: false,
                proposed_execpolicy_amendment: None,
            })
        }

        fn approval_action(
            &self,
            request: &TurnEnvironment,
            call_id: &str,
        ) -> std::io::Result<ApprovalAction> {
            Ok(ApprovalAction::ExecCommand {
                id: call_id.to_string(),
                environment_id: codex_exec_server::LOCAL_ENVIRONMENT_ID.to_string(),
                command: vec!["echo".to_string(), "sandbox-retry".to_string()],
                hook_command: "echo sandbox-retry".to_string(),
                cwd: request.cwd().clone(),
                sandbox_permissions: SandboxPermissions::UseDefault,
                additional_permissions: None,
                justification: None,
                tty: false,
                proposed_execpolicy_amendment: None,
            })
        }
    }

    impl Sandboxable for DeniedOnceRuntime {
        fn sandbox_preference(&self) -> codex_sandboxing::SandboxablePreference {
            codex_sandboxing::SandboxablePreference::Auto
        }
    }

    impl ToolRuntime<TurnEnvironment, String> for DeniedOnceRuntime {
        fn turn_environment<'a>(&self, request: &'a TurnEnvironment) -> &'a TurnEnvironment {
            request
        }

        async fn run(
            &mut self,
            _request: &TurnEnvironment,
            _attempt: &SandboxAttempt<'_>,
            _context: &ToolCtx,
        ) -> Result<String, ToolError> {
            self.attempts += 1;
            if self.attempts == 1 {
                return Err(ToolError::Codex(CodexErr::Sandbox(SandboxErr::Denied {
                    output: Box::new(ExecToolCallOutput {
                        exit_code: 1,
                        ..Default::default()
                    }),
                    network_policy_decision: None,
                })));
            }
            Ok("sandbox-retry-succeeded".to_string())
        }
    }

    let (session, mut action_turn, events) = make_session_and_context_with_rx().await;
    let step_context = captured_step_with_policy(
        &mut action_turn,
        AskForApproval::OnRequest,
        AskForApproval::UnlessTrusted,
    );

    let current_turn = activate_turn_with_new_approval_policy(&session).await;
    assert_ne!(action_turn.sub_id, current_turn.sub_id);

    let call_id = "captured-action-sandbox-retry";
    let context = ToolCtx {
        session: Arc::clone(&session),
        step_context,
        cancellation_token: CancellationToken::new(),
        call_id: call_id.to_string(),
        tool_name: codex_tools::ToolName::plain("exec_command"),
    };
    let environment = context
        .step_context
        .environments
        .primary()
        .expect("primary environment");
    let mut orchestrator = ToolOrchestrator::new();
    let mut runtime = DeniedOnceRuntime::default();
    let approve = async {
        let approval = next_exec_approval(&events).await;
        assert_eq!(approval.call_id, call_id);
        assert_eq!(approval.turn_id, action_turn.sub_id);
        assert_eq!(
            approval.reason.as_deref(),
            Some("command failed; retry without sandbox?")
        );
        session
            .notify_approval(call_id, ReviewDecision::Approved)
            .await;
    };

    let (output, ()) = tokio::join!(
        orchestrator.run(&mut runtime, environment, &context),
        approve
    );
    assert_eq!(
        output
            .expect("approved sandbox retry should succeed")
            .output,
        "sandbox-retry-succeeded"
    );
    assert_eq!(runtime.attempts, 2);
}

#[tokio::test]
async fn unified_exec_rejects_missing_additional_permissions() {
    let (mut session, mut turn_context_raw) = make_session_and_context().await;
    Arc::make_mut(&mut turn_context_raw.config)
        .permissions
        .approval_policy
        .set(AskForApproval::OnRequest)
        .expect("test setup should allow updating approval policy");
    session
        .features
        .enable(Feature::ExecPermissionApprovals)
        .expect("test setup should allow enabling request permissions");
    let session = Arc::new(session);
    let turn_context = Arc::new(turn_context_raw);
    let tracker = Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new()));
    let step_context = StepContext::for_test(Arc::clone(&turn_context));

    let handler = ExecCommandHandler::default();
    let resp = handler
        .handle(ToolInvocation {
            session: Arc::clone(&session),
            turn: Arc::clone(&turn_context),
            step_context,
            cancellation_token: CancellationToken::new(),
            tracker: Arc::clone(&tracker),
            call_id: "exec-call".to_string(),
            tool_name: codex_tools::ToolName::plain("exec_command"),
            source: crate::tools::context::ToolCallSource::Direct,
            payload: ToolPayload::Function {
                arguments: serde_json::json!({
                    "cmd": "echo hi",
                    "sandbox_permissions": SandboxPermissions::WithAdditionalPermissions,
                    "justification": "need additional sandbox permissions",
                })
                .to_string(),
            },
        })
        .await;

    let Err(FunctionCallError::RespondToModel(output)) = resp else {
        panic!("expected validation error result");
    };

    assert_eq!(
        output,
        "missing `additional_permissions`; provide at least one of `network` or `file_system` when using `with_additional_permissions`"
    );
}

#[tokio::test]
#[cfg(unix)]
#[expect(
    clippy::await_holding_invalid_type,
    reason = "test mutates active turn state directly to seed granted permissions"
)]
async fn exec_command_allows_sticky_turn_permissions_without_inline_request_permissions_feature() {
    let (mut session, turn_context_raw) = make_session_and_context().await;
    session
        .features
        .enable(Feature::RequestPermissionsTool)
        .expect("test setup should allow enabling request permissions tool");
    *session.active_turn.lock().await = Some(ActiveTurn::default());
    {
        let mut active_turn = session.active_turn.lock().await;
        let active_turn = active_turn.as_mut().expect("active turn");
        let mut turn_state = active_turn.turn_state.lock().await;
        turn_state.record_granted_permissions(
            codex_exec_server::LOCAL_ENVIRONMENT_ID,
            PermissionProfile {
                network: Some(NetworkPermissions {
                    enabled: Some(true),
                }),
                ..Default::default()
            },
        );
    }

    let session = Arc::new(session);
    let turn_context = Arc::new(turn_context_raw);

    let handler = crate::tools::handlers::ExecCommandHandler::default();
    #[allow(deprecated)]
    let workdir = Some(turn_context.cwd.to_string_lossy().to_string());
    let step_context = StepContext::for_test(Arc::clone(&turn_context));
    let resp = handler
        .handle(ToolInvocation {
            session: Arc::clone(&session),
            turn: Arc::clone(&turn_context),
            step_context,
            cancellation_token: CancellationToken::new(),
            tracker: Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new())),
            call_id: "sticky-turn-grant".to_string(),
            tool_name: codex_tools::ToolName::plain("exec_command"),
            source: crate::tools::context::ToolCallSource::Direct,
            payload: ToolPayload::Function {
                arguments: serde_json::json!({
                    "cmd": "echo hi",
                    "login": false,
                    "yield_time_ms": 10_000_u64,
                    "workdir": workdir,
                })
                .to_string(),
            },
        })
        .await;

    match resp {
        Ok(output) => {
            let output = expect_text_output(&output);
            assert!(output.contains("hi"));
        }
        Err(FunctionCallError::RespondToModel(output)) => {
            assert!(
                !output.contains("additional permissions are disabled"),
                "sticky turn permissions should bypass inline validation: {output}"
            );
        }
        Err(err) => panic!("unexpected error: {err:?}"),
    }
}
