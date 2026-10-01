use super::*;
use crate::session::tests::make_session_and_context_with_rx;
use codex_protocol::approvals::NetworkPolicyAmendment;
use pretty_assertions::assert_eq;

#[test]
fn approval_resolution_rejects_denied_network_policy_amendment() {
    let resolution = ApprovalResolution {
        decision: ReviewDecision::NetworkPolicyAmendment {
            network_policy_amendment: NetworkPolicyAmendment {
                host: "denied.example.com".to_string(),
                action: NetworkPolicyRuleAction::Deny,
            },
        },
        source: ApprovalResolutionSource::User,
    };

    assert!(matches!(
        resolution.into_tool_result(),
        Err(ToolError::Rejected(rejection)) if rejection == "rejected by user"
    ));
}

#[test]
fn approval_resolution_rejects_mcp_policy_amendment() {
    let resolution = ApprovalResolution {
        decision: ReviewDecision::ApprovedMcpPolicyAmendment,
        source: ApprovalResolutionSource::User,
    };

    assert!(matches!(
        resolution.into_tool_result(),
        Err(ToolError::Rejected(rejection)) if rejection == "Error while requesting approval"
    ));
}

#[test]
fn approval_resolution_aborts_turn_when_approval_is_aborted() {
    let resolution = ApprovalResolution {
        decision: ReviewDecision::Abort,
        source: ApprovalResolutionSource::User,
    };

    assert!(matches!(
        resolution.into_tool_result(),
        Err(ToolError::Codex(error))
            if matches!(
                error.details(),
                codex_protocol::error::CodexErrorDetails::TurnAborted
            )
    ));
}

#[test]
fn approval_resolution_uses_ahead_timeout_message() {
    let resolution = ApprovalResolution {
        decision: ReviewDecision::TimedOut,
        source: ApprovalResolutionSource::User,
    };

    assert!(matches!(
        resolution.into_tool_result(),
        Err(ToolError::Rejected(rejection))
            if rejection == super::super::APPROVAL_TIMEOUT_MESSAGE
    ));
}

#[test]
fn approval_cwd_preserves_drive_shaped_local_posix_path() {
    let native_cwd = AbsolutePathBuf::try_from(std::path::PathBuf::from("/C:/workspace"))
        .expect("drive-shaped POSIX path should be absolute");
    let cwd = PathUri::from_abs_path(&native_cwd);

    assert_eq!(
        approval_cwd(codex_exec_server::LOCAL_ENVIRONMENT_ID, cwd)
            .expect("local cwd should retain the host path convention"),
        native_cwd
    );
}

#[test]
fn approval_cwd_rejects_foreign_remote_path() {
    let cwd = PathUri::parse("file:///C:/workspace").expect("valid Windows path URI");

    assert!(approval_cwd(codex_exec_server::REMOTE_ENVIRONMENT_ID, cwd).is_err());
}
#[tokio::test]
async fn strict_mcp_review_routes_to_the_human() {
    let (session, turn, events) = make_session_and_context_with_rx().await;
    let action = ApprovalAction::McpToolCall {
        id: "mcp-override".to_string(),
        server: "example".to_string(),
        tool_name: "dangerous".to_string(),
        arguments: None,
        connector_id: None,
        connector_name: None,
        connector_description: None,
        connected_account_email: None,
        tool_title: None,
        tool_description: None,
        annotations: None,
        hook_tool_name: HookToolName::new("mcp__example__dangerous"),
        approval_policy: AskForApproval::OnRequest,
        approval_mode: AppToolApproval::Prompt,
        allow_session_remember: false,
    };
    let mut review_context = ApprovalReviewContext::from(&turn);
    review_context.approval_policy = AskForApproval::OnRequest;
    let context = ApprovalContext {
        review_context,
        call_id: "mcp-override".to_string(),
        tool_name: ToolName::plain("dangerous"),
        strict_auto_review: true,
        approval_reason: None,
        retry_reason: None,
        network_approval_context: None,
    };

    tokio::select! {
        resolution = session.request_approval(action, context) => {
            panic!("expected a user approval request, got {resolution:?}");
        }
        event = events.recv() => {
            let codex_protocol::protocol::EventMsg::ElicitationRequest(request) =
                event.expect("receive user approval request").msg
            else {
                panic!("expected an MCP user approval request");
            };
            assert_eq!(request.server_name, "example");
            assert_eq!(
                request.id,
                codex_protocol::mcp::RequestId::String(
                    "mcp_tool_call_approval_mcp-override".to_string()
                )
            );
        }
    }
}
