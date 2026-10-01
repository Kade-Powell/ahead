//! Durable streamed-session end-to-end test.
//!
//! Exercises the real `HarnessController`: a streamed turn persists durable
//! conversation messages, cancellation stops the turn, and the session can be
//! reopened with its message history intact.
//!
//! Opt-in (spawns the configured external ACP runtime, defaulting to the real
//! `codex-acp` adapter):
//! ```text
//! cargo test -p ahead-proxy --test harness_session_e2e -- --ignored --nocapture
//! ```

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ahead_agent::HarnessController;
use ahead_proxy::ahead::store::SessionStore;
use ahead_rpc::ahead::{
    AgentTurnRequestDto, AheadNotification, DisplayPosition, TurnEditorContext,
};

fn turn_dto(session_id: &str, message: &str) -> AgentTurnRequestDto {
    AgentTurnRequestDto {
        session_id: session_id.to_string(),
        thread_id: "thread-e2e".to_string(),
        harness: ahead_rpc::ahead::HarnessKind::ExternalAcp,
        external_agent_id: None,
        model: None,
        model_provider: None,
        user_message: message.to_string(),
        session_context: String::new(),
        context: TurnEditorContext {
            active_path: "src/lib.rs".to_string(),
            caret: DisplayPosition { line: 0, col: 0 },
            selection: None,
            file_content: String::new(),
            visible_end: None,
            attached_anchor_ids: Vec::new(),
            attached_files: Vec::new(),
            attached_memories: Vec::new(),
        },
        invariants: Vec::new(),
        cwd: Some(
            std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        ),
        expected_policy_sha256: String::new(),
        read_only: false,
        scope: None,
    }
}

fn all_messages(
    controller: &HarnessController,
    session_id: &str,
) -> anyhow::Result<Vec<ahead_rpc::ahead::ConversationMessage>> {
    let mut pages = Vec::new();
    let mut before = None;
    loop {
        let page = controller.messages_page(session_id, before, 100)?;
        let Some(first) = page.messages.first() else {
            break;
        };
        before = Some(ahead_rpc::ahead::ConversationMessageCursor {
            sequence: first.sequence,
            message_id: first.id.clone(),
        });
        let has_older = page.has_older;
        pages.push(page.messages);
        if !has_older {
            break;
        }
    }
    pages.reverse();
    Ok(pages.into_iter().flatten().collect())
}

#[test]
#[ignore = "requires a configured external ACP agent and model credentials"]
fn durable_streamed_turn_persists_cancels_and_reopens() {
    // Start a durable AHEAD work session.
    let host = ahead_proxy::ahead::host::AheadSessionHost::new(
        SessionStore::in_memory().unwrap(),
    );
    let view = host
        .start_work(
            Some(ahead_rpc::ahead::WorkKind::ProductChange),
            "Harness e2e".to_string(),
            "starting point".to_string(),
            None,
        )
        .expect("start work");
    let session_id = view.session.id.clone();

    // The controller reads sessions from its own store, so insert the view
    // before wrapping the shared store in the `HarnessStore` adapter.
    let database_dir = tempfile::tempdir().expect("temporary database directory");
    let database_path = database_dir.path().join("sessions.db");
    let mut store_impl = SessionStore::open(&database_path).expect("file store");
    store_impl.insert_session(&view).expect("insert session");
    let shared = Arc::new(parking_lot::RwLock::new(store_impl));
    let controller = HarnessController::new(Arc::new(
        ahead_proxy::ahead::store::SharedSessionStore(shared.clone()),
    ));
    controller.set_workspace(std::env::current_dir().unwrap());

    let events: Arc<Mutex<Vec<AheadNotification>>> =
        Arc::new(Mutex::new(Vec::new()));
    let events_cb = events.clone();
    controller.set_notification_sink(Arc::new(move |n| {
        events_cb.lock().unwrap().push(n);
    }));

    // 1. Real streamed turn.
    controller
        .start_turn(turn_dto(&session_id, "Reply with exactly: PONG"))
        .expect("start turn");
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if !controller.has_active_turn(&session_id) {
            break;
        }
        if Instant::now() > deadline {
            panic!("turn did not complete in time");
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    let messages = all_messages(&controller, &session_id).expect("messages");
    eprintln!(
        "messages: {:?}",
        messages
            .iter()
            .map(|m| (m.role.clone(), m.status.clone(), m.content.clone()))
            .collect::<Vec<_>>()
    );
    assert!(messages.iter().any(|m| m.role == "human"));
    let agent = messages
        .iter()
        .find(|m| m.role == "agent")
        .expect("agent message");
    assert_eq!(agent.status, "complete");
    assert!(
        agent.content.contains("PONG"),
        "expected PONG, got {}",
        agent.content
    );

    // 2. Cancel a long turn.
    controller
        .start_turn(turn_dto(
            &session_id,
            "Count from 1 to 200 slowly, one number per line.",
        ))
        .expect("start long turn");
    std::thread::sleep(Duration::from_millis(2500));
    let cancelled = controller.cancel_turn(&session_id).expect("cancel");
    assert!(cancelled, "an active turn should be cancellable");

    let deadline = Instant::now() + Duration::from_secs(120);
    while controller.has_active_turn(&session_id) {
        if Instant::now() > deadline {
            panic!("cancelled turn did not settle");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let messages = all_messages(&controller, &session_id).expect("messages");
    let last_agent = messages
        .iter()
        .rev()
        .find(|m| m.role == "agent")
        .expect("agent");
    assert_eq!(
        last_agent.status, "cancelled",
        "cancel must mark the message"
    );

    // 3. Durable reopen: release every handle, reopen the database file, and
    //    continue the bound harness conversation in a fresh controller.
    controller.shutdown_harness();
    drop(controller);
    drop(shared);
    let reopened_store = SessionStore::open(&database_path).expect("reopen store");
    let reopened_shared = Arc::new(parking_lot::RwLock::new(reopened_store));
    let reopened = HarnessController::new(Arc::new(
        ahead_proxy::ahead::store::SharedSessionStore(reopened_shared),
    ));
    reopened.set_workspace(std::env::current_dir().unwrap());
    let history = all_messages(&reopened, &session_id).expect("reopened messages");
    assert_eq!(history.len(), messages.len());
    reopened
        .start_turn(turn_dto(&session_id, "Reply with exactly: AGAIN"))
        .expect("continue after reopen");
    let deadline = Instant::now() + Duration::from_secs(180);
    while reopened.has_active_turn(&session_id) {
        if Instant::now() > deadline {
            panic!("reopened turn did not complete");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let final_messages =
        all_messages(&reopened, &session_id).expect("final messages");
    let last = final_messages
        .iter()
        .rev()
        .find(|m| m.role == "agent")
        .expect("agent");
    assert!(last.content.contains("AGAIN"), "got {}", last.content);

    reopened.shutdown_harness();
}

#[test]
#[ignore = "requires managed model credentials and native sandbox support"]
fn managed_file_change_records_ahead_anchor() {
    let root = std::env::temp_dir().join(format!(
        "ahead-managed-anchor-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();

    let host = ahead_proxy::ahead::host::AheadSessionHost::new(
        SessionStore::in_memory().unwrap(),
    );
    let view = host
        .start_work(
            Some(ahead_rpc::ahead::WorkKind::ProductChange),
            "Managed anchor e2e".to_string(),
            "starting point".to_string(),
            None,
        )
        .expect("start work");
    let session_id = view.session.id.clone();

    let mut store = SessionStore::in_memory().unwrap();
    store.insert_session(&view).unwrap();
    let shared = Arc::new(parking_lot::RwLock::new(store));
    let controller = HarnessController::new(Arc::new(
        ahead_proxy::ahead::store::SharedSessionStore(shared.clone()),
    ));
    controller.set_workspace(root.clone());
    controller
        .start_turn(AgentTurnRequestDto {
            session_id: session_id.clone(),
            thread_id: "managed-anchor-e2e".to_string(),
            harness: ahead_rpc::ahead::HarnessKind::Ahead,
            external_agent_id: None,
            model: None,
            model_provider: Some("openai".to_string()),
            user_message: "Use the native file editing tool, not shell, to create proof.txt containing exactly `AHEAD managed proof`. Do not edit any other path.".to_string(),
            session_context: String::new(),
            context: TurnEditorContext {
                active_path: "proof.txt".to_string(),
                caret: DisplayPosition { line: 0, col: 0 },
                selection: None,
                file_content: String::new(),
                visible_end: None,
                attached_anchor_ids: Vec::new(),
                attached_files: Vec::new(),
                attached_memories: Vec::new(),
            },
            invariants: Vec::new(),
            cwd: Some(root.to_string_lossy().to_string()),
            expected_policy_sha256: String::new(),
            read_only: false,
            scope: None,
        })
        .expect("start managed turn");

    let deadline = Instant::now() + Duration::from_secs(180);
    while controller.has_active_turn(&session_id) {
        if Instant::now() > deadline {
            panic!("managed file-change turn did not complete");
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    let content = std::fs::read_to_string(root.join("proof.txt")).unwrap();
    assert!(content.contains("AHEAD managed proof"));
    let anchors = shared.read().list_anchors(&session_id).unwrap();
    assert!(anchors.iter().any(|anchor| {
        anchor.actor_id == ahead_rpc::ahead::AHEAD_ACTOR_ID
            && anchor.path == "proof.txt"
            && anchor
                .surrounding_context
                .as_deref()
                .is_some_and(|quote| quote.contains("AHEAD managed proof"))
    }));

    controller.shutdown_harness();
    std::fs::remove_dir_all(root).unwrap();
}
