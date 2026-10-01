use super::*;
use crate::mcp::tests::test_elicitation_config;
use async_channel::Receiver;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::GranularApprovalConfig;
use pretty_assertions::assert_eq;
use rmcp::model::ElicitRequestParams;
use rmcp::model::ElicitationSchema;
use rmcp::model::RequestMetaObject;
use serde_json::Map;
use serde_json::json;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::Relaxed;

struct LifecycleRegistration(Arc<AtomicUsize>);

impl Drop for LifecycleRegistration {
    fn drop(&mut self) {
        self.0.fetch_sub(/*val*/ 1, Relaxed);
    }
}

fn elicitation_fixture(
    approval_policy: AskForApproval,
    permission_profile: PermissionProfile,
) -> (ElicitationRequestManager, Receiver<Event>, SendElicitation) {
    let mut config = test_elicitation_config(
        "independent-mcp",
        approval_policy,
        permission_profile.clone(),
    );
    Arc::make_mut(&mut config)
        .server_permission_profiles
        .insert("another-independent-mcp".to_string(), permission_profile);
    let manager = ElicitationRequestManager::new(
        config,
        /*lifecycle*/ None,
        ElicitationRequestRouter::default(),
    );
    let (tx_event, events) = async_channel::bounded(1);
    let sender = manager.make_sender("independent-mcp".to_string(), Some(tx_event));
    (manager, events, sender)
}

async fn send_elicitation(sender: &SendElicitation, marker: Option<Value>) -> ElicitationResponse {
    let elicitation = Elicitation::Mcp(ElicitRequestParams::FormElicitationParams {
        meta: marker.map(|value| {
            RequestMetaObject::from(Map::from_iter([(STRICT_AUTO_REVIEW_KEY.into(), value)]))
        }),
        message: "Review this request".to_string(),
        requested_schema: ElicitationSchema::builder().build().unwrap(),
    });
    sender(RequestId::Number(7), elicitation)
        .await
        .expect("elicitation must receive a terminal response")
}

#[test]
fn closed_event_channel_immediately_cleans_up_pending_elicitation() {
    let active_elicitations = Arc::new(AtomicUsize::new(0));
    let registrations = active_elicitations.clone();
    let lifecycle = ElicitationLifecycle::new(move || {
        registrations.fetch_add(/*val*/ 1, Relaxed);
        LifecycleRegistration(registrations.clone())
    });
    let (manager, events, sender) =
        elicitation_fixture(AskForApproval::OnRequest, PermissionProfile::Disabled);
    assert!(manager.update(
        test_elicitation_config(
            "independent-mcp",
            AskForApproval::OnRequest,
            PermissionProfile::Disabled
        ),
        Some(lifecycle),
    ));
    drop(events);

    let elicitation = Elicitation::Mcp(ElicitRequestParams::FormElicitationParams {
        meta: None,
        message: "Review this request".to_string(),
        requested_schema: ElicitationSchema::builder().build().unwrap(),
    });
    let error = sender(RequestId::Number(7), elicitation)
        .now_or_never()
        .expect("closed event channel must not leave an elicitation pending")
        .expect_err("closed event channel must fail the elicitation");

    assert_eq!(
        error.to_string(),
        "failed to deliver MCP elicitation request"
    );
    assert!(
        manager
            .router
            .requests
            .lock()
            .expect("pending request router should be available")
            .is_empty()
    );
    assert_eq!(active_elicitations.load(Relaxed), 0);
}

#[tokio::test]
async fn strict_auto_review_requests_fail_closed_without_a_reviewer() {
    for policy in [
        AskForApproval::OnRequest,
        AskForApproval::UnlessTrusted,
        AskForApproval::Never,
        AskForApproval::Granular(GranularApprovalConfig {
            sandbox_approval: true,
            rules: true,
            skill_approval: true,
            request_permissions: true,
            mcp_elicitations: false,
        }),
    ] {
        let (_, events, sender) = elicitation_fixture(policy, PermissionProfile::Disabled);
        assert_eq!(
            send_elicitation(&sender, Some(json!(true))).await,
            strict_auto_review_decline()
        );
        assert!(events.is_empty(), "strict review must not emit an event");
    }
}

#[tokio::test]
async fn malformed_strict_review_metadata_fails_closed() {
    for marker in ["null", "\"true\"", "1", "{}", "[true]"] {
        let marker = serde_json::from_str(marker).expect("valid malformed marker");
        let (_, events, sender) =
            elicitation_fixture(AskForApproval::OnRequest, PermissionProfile::Disabled);
        assert_eq!(
            send_elicitation(&sender, Some(marker)).await,
            strict_auto_review_decline()
        );
        assert!(events.is_empty());
    }
}

#[tokio::test]
async fn reused_elicitation_senders_follow_each_servers_latest_permission_authority() {
    let mut config = crate::mcp::tests::test_mcp_config();
    config.approval_policy = codex_config::Constrained::allow_any(AskForApproval::Never);
    config.permission_profile = PermissionProfile::Disabled;

    let hosted_server = crate::mcp::tests::test_http_server_config("https://example.com/mcp");
    let mut attached_server = hosted_server.clone();
    attached_server.environment_id = "attached".to_string();
    let mut catalog = crate::ResolvedMcpCatalog::builder();
    catalog.register(crate::McpServerRegistration::from_config(
        "attached".to_string(),
        attached_server,
    ));
    catalog.register(crate::McpServerRegistration::from_hosted_apps(
        "host",
        /*contribution_order*/ 0,
        hosted_server,
    ));
    config.mcp_server_catalog = catalog.build();
    let servers = crate::effective_mcp_servers(&config);
    config.set_server_permission_profiles(
        &servers,
        [("attached".to_string(), PermissionProfile::read_only())],
    );

    let manager = ElicitationRequestManager::new(
        Arc::new(config.clone()),
        /*lifecycle*/ None,
        ElicitationRequestRouter::default(),
    );
    let attached = manager.make_sender("attached".to_string(), /*tx_event*/ None);
    let hosted = manager.make_sender(
        crate::CODEX_APPS_MCP_SERVER_NAME.to_string(),
        /*tx_event*/ None,
    );

    assert_eq!(
        send_elicitation(&attached, /*marker*/ None).await.action,
        ElicitationAction::Decline
    );
    assert_eq!(
        send_elicitation(&hosted, /*marker*/ None).await.action,
        ElicitationAction::Accept
    );

    config.set_server_permission_profiles(
        &servers,
        [("attached".to_string(), PermissionProfile::Disabled)],
    );
    assert!(manager.update(Arc::new(config.clone()), /*lifecycle*/ None,));
    assert_eq!(
        send_elicitation(&attached, /*marker*/ None).await.action,
        ElicitationAction::Accept
    );

    let mut configured_servers = config.mcp_server_catalog.configured_servers();
    configured_servers
        .get_mut("attached")
        .expect("attached server should be registered")
        .enabled = false;
    config.mcp_server_catalog = config
        .mcp_server_catalog
        .with_materialized_servers(configured_servers);
    let servers = crate::effective_mcp_servers(&config);
    config.set_server_permission_profiles(
        &servers,
        [("attached".to_string(), PermissionProfile::Disabled)],
    );
    assert!(manager.update(Arc::new(config.clone()), /*lifecycle*/ None,));
    assert_eq!(
        send_elicitation(&attached, /*marker*/ None).await.action,
        ElicitationAction::Decline
    );

    config.mcp_server_catalog = crate::ResolvedMcpCatalog::default();
    let servers = crate::effective_mcp_servers(&config);
    config.set_server_permission_profiles(&servers, std::iter::empty());
    assert!(manager.update(Arc::new(config.clone()), /*lifecycle*/ None,));
    assert_eq!(
        send_elicitation(&hosted, /*marker*/ None).await.action,
        ElicitationAction::Decline
    );
}
