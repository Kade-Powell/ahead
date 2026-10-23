use super::*;
use crate::mcp::tests::test_elicitation_config;
use async_channel::Receiver;
use codex_protocol::models::PermissionProfile;
use pretty_assertions::assert_eq;
use rmcp::model::ElicitRequestParams;
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

async fn send_test_elicitation(sender: &SendElicitation, request_id: i64) -> ElicitationAction {
    sender(
        RequestId::Number(request_id),
        Elicitation::Mcp(ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "Confirm?".to_string(),
            requested_schema: rmcp::model::ElicitationSchema::builder()
                .build()
                .expect("form schema should build"),
        }),
    )
    .await
    .expect("test elicitation should resolve")
    .action
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
        requested_schema: rmcp::model::ElicitationSchema::builder()
            .build()
            .expect("form schema should build"),
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
async fn reused_elicitation_sender_follows_latest_local_permission_authority() {
    let mut config = crate::mcp::tests::test_mcp_config();
    config.approval_policy = codex_config::Constrained::allow_any(AskForApproval::Never);
    config.permission_profile = PermissionProfile::Disabled;

    let local_server = crate::mcp::tests::test_stdio_server_config("docs-mcp");
    let mut catalog = crate::ResolvedMcpCatalog::builder();
    catalog.register(crate::McpServerRegistration::from_config(
        "local".to_string(),
        local_server,
    ));
    config.mcp_server_catalog = catalog.build();
    let servers = crate::effective_mcp_servers(&config);
    config.set_server_permission_profiles(&servers, std::iter::empty());

    let manager = ElicitationRequestManager::new(
        Arc::new(config.clone()),
        /*lifecycle*/ None,
        ElicitationRequestRouter::default(),
    );
    let local = manager.make_sender("local".to_string(), /*tx_event*/ None);

    assert_eq!(
        send_test_elicitation(&local, 1).await,
        ElicitationAction::Accept
    );

    config.permission_profile = PermissionProfile::read_only();
    config.set_server_permission_profiles(&servers, std::iter::empty());
    assert!(manager.update(Arc::new(config.clone()), /*lifecycle*/ None,));
    assert_eq!(
        send_test_elicitation(&local, 2).await,
        ElicitationAction::Decline
    );

    let mut configured_servers = config.mcp_server_catalog.configured_servers();
    configured_servers
        .get_mut("local")
        .expect("local server should be registered")
        .enabled = false;
    let mut catalog = crate::ResolvedMcpCatalog::builder();
    for (name, server) in configured_servers {
        catalog.register(crate::McpServerRegistration::from_config(name, server));
    }
    config.mcp_server_catalog = catalog.build();
    let servers = crate::effective_mcp_servers(&config);
    config.set_server_permission_profiles(&servers, std::iter::empty());
    assert!(manager.update(Arc::new(config.clone()), /*lifecycle*/ None,));
    assert_eq!(
        send_test_elicitation(&local, 3).await,
        ElicitationAction::Decline
    );

    config.mcp_server_catalog = crate::ResolvedMcpCatalog::default();
    let servers = crate::effective_mcp_servers(&config);
    config.set_server_permission_profiles(&servers, std::iter::empty());
    assert!(manager.update(Arc::new(config.clone()), /*lifecycle*/ None,));
    assert_eq!(
        send_test_elicitation(&local, 4).await,
        ElicitationAction::Decline
    );
}
