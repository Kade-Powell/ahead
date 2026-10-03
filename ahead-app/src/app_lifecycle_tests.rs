use super::{
    AgentWorkspacePanel, CenterPanel, CloseWindow, Quit, QuitInProgress, Shell,
    ShellShortcut, debug_terminal_command, request_close_window, request_quit,
};
use crate::code_panel::CodePanel;
use crate::proxy_client::ProxyClient;
use crate::workspace_panels::{
    ActivityBar, GitPanel, JustTasksPanel, LanguageServersPanel, ProblemsPanel,
    SearchPanel,
};
use ahead_rpc::ahead::AheadRequest;
use ahead_rpc::ahead::{
    DisplayPosition, DisplayRange, GitHubUser, SessionRole, WorkKind,
};
use ahead_rpc::core::{CoreRpc, CoreRpcHandler};
use ahead_rpc::file::EditorRecoverySnapshot;
use ahead_rpc::proxy::{
    ProxyHandler, ProxyNotification, ProxyRequest, ProxyResponse, ProxyRpc,
    ProxyRpcHandler,
};
use gpui_kit::component::dock::DockSkin;
use gpui_kit::{AppContext, Entity, Focusable, TestAppContext, VisualTestContext};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn start_mock_github_api() -> (String, Arc<AtomicBool>, std::thread::JoinHandle<()>)
{
    let api = TcpListener::bind("127.0.0.1:0").expect("mock GitHub API");
    api.set_nonblocking(true).expect("nonblocking API");
    let api_base = format!("http://{}", api.local_addr().expect("API address"));
    let api_stop = Arc::new(AtomicBool::new(false));
    let api_thread = {
        let api_stop = api_stop.clone();
        std::thread::spawn(move || {
            while !api_stop.load(Ordering::Relaxed) {
                match api.accept() {
                    Ok((mut socket, _)) => {
                        socket.set_nonblocking(false).expect("blocking API socket");
                        let mut headers = Vec::new();
                        while !headers.ends_with(b"\r\n\r\n") {
                            let mut byte = [0];
                            socket.read_exact(&mut byte).expect("API request");
                            headers.push(byte[0]);
                            assert!(headers.len() < 8192, "API headers too large");
                        }
                        assert!(
                            String::from_utf8_lossy(&headers)
                                .to_ascii_lowercase()
                                .contains("authorization: bearer guest-token")
                        );
                        let body = r#"{"login":"bob","id":42,"name":"Bob"}"#;
                        write!(
                            socket,
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .expect("API response");
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(error) => panic!("API accept: {error}"),
                }
            }
        })
    };
    (api_base, api_stop, api_thread)
}

fn start_mock_host_model() -> (
    String,
    std::sync::mpsc::Receiver<(String, serde_json::Value)>,
    std::thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock host model");
    listener.set_nonblocking(true).expect("nonblocking model");
    let endpoint = format!(
        "http://{}/v1",
        listener.local_addr().expect("model address")
    );
    let (sender, receiver) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "host model request missing"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("host model accept: {error}"),
            }
        };
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .expect("model read timeout");
        let mut request = Vec::new();
        let mut buffer = [0; 8192];
        let (headers, body) = loop {
            let read = socket.read(&mut buffer).expect("host model request");
            assert_ne!(read, 0, "host model request ended early");
            request.extend_from_slice(&buffer[..read]);
            let Some(header_end) = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4)
            else {
                continue;
            };
            let headers =
                String::from_utf8_lossy(&request[..header_end]).into_owned();
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .expect("host model content length");
            if request.len() >= header_end + length {
                let body = serde_json::from_slice(
                    &request[header_end..header_end + length],
                )
                .expect("host model request JSON");
                break (headers, body);
            }
        };
        let message = serde_json::json!({
            "id": "message-host-paid",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "HOST_PAID_RESPONSE"}],
        });
        let events = [
            (
                "response.output_item.added",
                serde_json::json!({
                    "type": "response.output_item.added",
                    "output_index": 0,
                    "item": {"id": "message-host-paid", "type": "message", "role": "assistant", "content": []},
                }),
            ),
            (
                "response.output_item.done",
                serde_json::json!({
                    "type": "response.output_item.done", "output_index": 0, "item": message,
                }),
            ),
            (
                "response.completed",
                serde_json::json!({
                    "type": "response.completed",
                    "response": {"id": "response-host-paid", "end_turn": true},
                }),
            ),
        ];
        let response = events
            .iter()
            .map(|(event, payload)| format!("event: {event}\ndata: {payload}\n\n"))
            .collect::<String>();
        write!(
            socket,
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}",
            response.len()
        )
        .expect("host model response");
        sender
            .send((headers, body))
            .expect("deliver host model request");
    });
    (endpoint, receiver, server)
}

fn authenticated_test_dispatcher(
    proxy: &Arc<ProxyClient>,
    workspace: &Path,
    token: &str,
    login: &str,
    github_id: u64,
    window_id: usize,
    api_base: Option<String>,
) -> (ahead_proxy::dispatch::Dispatcher, CoreRpcHandler) {
    use ahead_proxy::ahead::GitHubAuthManager;
    use ahead_proxy::dispatch::Dispatcher;

    let core = CoreRpcHandler::new();
    let mut dispatcher = Dispatcher::new(core.clone(), proxy.rpc_for_test());
    dispatcher.handle_notification(ProxyNotification::Initialize {
        workspace: Some(workspace.to_owned()),
        window_id,
        tab_id: window_id,
    });
    if let Some(api_base) = api_base {
        dispatcher.set_share_test_api_base(api_base);
    }
    let mut auth = GitHubAuthManager::with_custom_dir(workspace.join("auth"));
    auth.save_auth(
        token.into(),
        GitHubUser {
            login: login.into(),
            id: github_id,
            name: Some(login.into()),
            avatar_url: None,
            email: None,
            is_authenticated: true,
        },
    )
    .expect("test identity");
    dispatcher
        .ahead_host
        .as_ref()
        .expect("session store")
        .read()
        .set_auth_for_test(auth);
    (dispatcher, core)
}

#[gpui_kit::test]
fn two_authenticated_shells_join_a_direct_shared_session(cx: &mut TestAppContext) {
    use ahead_proxy::ahead::GitHubAuthManager;
    use ahead_proxy::dispatch::Dispatcher;

    let host_workspace = tempfile::tempdir().expect("host workspace");
    let guest_workspace = tempfile::tempdir().expect("guest workspace");
    std::fs::create_dir(host_workspace.path().join(".ahead"))
        .expect("host config directory");
    std::fs::write(
        host_workspace.path().join(".ahead/team.toml"),
        "[[members]]\ngithub = 'bob'\ndisplay_name = 'Bob'\nrole = 'editor'\n",
    )
    .expect("team allowlist");

    let (api_base, api_stop, api_thread) = start_mock_github_api();

    let mut guest_context = cx.clone();
    let (host_shell, host_cx, host_proxy) =
        open_shell(host_workspace.path(), &["src.rs"], cx);
    let (guest_shell, guest_cx, guest_proxy) =
        open_shell(guest_workspace.path(), &["local.rs"], &mut guest_context);

    let host_rpc = host_proxy.rpc_for_test();
    let host_core = CoreRpcHandler::new();
    let mut host_dispatcher = Dispatcher::new(host_core.clone(), host_rpc.clone());
    host_dispatcher.handle_notification(ProxyNotification::Initialize {
        workspace: Some(host_workspace.path().to_owned()),
        window_id: 1,
        tab_id: 1,
    });
    host_dispatcher.set_share_test_api_base(api_base);
    let host = host_dispatcher
        .ahead_host
        .as_ref()
        .expect("host session store")
        .clone();
    let mut host_auth =
        GitHubAuthManager::with_custom_dir(host_workspace.path().join("auth"));
    host_auth
        .save_auth(
            "host-token".into(),
            GitHubUser {
                login: "host".into(),
                id: 1,
                name: Some("Host".into()),
                avatar_url: None,
                email: None,
                is_authenticated: true,
            },
        )
        .expect("host identity");
    host.read().set_auth_for_test(host_auth);
    let view = host
        .read()
        .start_work(
            Some(WorkKind::ProductChange),
            "Shared work".into(),
            "Collaborate".into(),
            None,
        )
        .expect("active managed session");
    let verified_guest = GitHubUser {
        login: "bob".into(),
        id: 42,
        name: Some("Bob".into()),
        avatar_url: None,
        email: None,
        is_authenticated: true,
    };
    host.read()
        .add_session_participant_verified(
            &view.session.id,
            verified_guest.clone(),
            SessionRole::Editor,
        )
        .expect("verified guest");
    let host_thread = std::thread::spawn({
        let host_rpc = host_rpc.clone();
        move || host_rpc.mainloop(&mut host_dispatcher)
    });

    let guest_rpc = guest_proxy.rpc_for_test();
    let mut guest_dispatcher =
        Dispatcher::new(CoreRpcHandler::new(), guest_rpc.clone());
    guest_dispatcher.handle_notification(ProxyNotification::Initialize {
        workspace: Some(guest_workspace.path().to_owned()),
        window_id: 2,
        tab_id: 2,
    });
    let guest_host = guest_dispatcher
        .ahead_host
        .as_ref()
        .expect("guest session store")
        .clone();
    let mut guest_auth =
        GitHubAuthManager::with_custom_dir(guest_workspace.path().join("auth"));
    guest_auth
        .save_auth(
            "guest-token".into(),
            GitHubUser {
                login: "bob".into(),
                id: 42,
                name: Some("Bob".into()),
                avatar_url: None,
                email: None,
                is_authenticated: true,
            },
        )
        .expect("guest identity");
    guest_host.read().set_auth_for_test(guest_auth);
    let guest_thread = std::thread::spawn({
        let guest_rpc = guest_rpc.clone();
        move || guest_rpc.mainloop(&mut guest_dispatcher)
    });

    host_shell.update_in(host_cx, |shell, window, cx| {
        shell.code_tabs[0].update(cx, |code, cx| {
            code.watch_shared_buffer_changes(&host_proxy, window, cx);
            assert_eq!(code.editor.read(cx).value(), "saved\n");
        });
    });

    let host_session =
        host_shell.read_with(host_cx, |shell, _| shell.session.clone());
    host_session.update(host_cx, |panel, _| panel.set_shell(host_shell.clone()));
    host_session.update(host_cx, |panel, cx| {
        panel.attach_session(view.session.id.clone(), cx)
    });
    assert_eq!(
        host_session.read_with(host_cx, |panel, _| panel.session_id.clone()),
        Some(view.session.id.clone())
    );
    let offer = host_proxy
        .share_session(view.session.id.clone(), "127.0.0.1:0".into())
        .expect("direct TLS invite");
    host_session.update(host_cx, |panel, cx| panel.share_active_session(cx));
    let share_deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !host_session.read_with(host_cx, |panel, _| {
        panel.status.as_ref().starts_with("Sharing on ")
    }) && std::time::Instant::now() < share_deadline
    {
        host_cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(host_session.read_with(host_cx, |panel, _| {
        panel.status.as_ref().starts_with("Sharing on ")
    }));

    let guest_session =
        guest_shell.read_with(guest_cx, |shell, _| shell.session.clone());
    guest_session.update_in(guest_cx, |panel, window, cx| {
        panel.chat_input.update(cx, |input, cx| {
            input.set_value(
                serde_json::to_string(&offer).expect("invite JSON"),
                window,
                cx,
            )
        });
        panel.join_shared_session(window, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while guest_session.read_with(guest_cx, |panel, _| panel.session_id.clone())
        != Some(view.session.id.clone())
        && std::time::Instant::now() < deadline
    {
        guest_cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        guest_session.read_with(guest_cx, |panel, _| panel.session_id.clone()),
        Some(view.session.id.clone()),
        "{}",
        guest_session.read_with(guest_cx, |panel, _| panel.status.clone())
    );
    assert!(
        guest_session.read_with(guest_cx, |panel, _| panel.shared_guest_can_edit())
    );
    guest_session.update(guest_cx, |panel, _| panel.set_shell(guest_shell.clone()));

    let snapshot = guest_proxy
        .read_shared_buffer(view.session.id.clone(), "src.rs".into())
        .expect("host buffer over TLS");
    guest_shell.update_in(guest_cx, |shell, window, cx| {
        shell.open_shared_buffer(view.session.id.clone(), snapshot, None, window, cx)
    });
    guest_shell.read_with(guest_cx, |shell, cx| {
        let shared = shell.code_tabs.last().expect("shared editor").read(cx);
        assert_eq!(
            shared.shared_session_id.as_deref(),
            Some(view.session.id.as_str())
        );
        assert_eq!(shared.editor.read(cx).value(), "saved\n");
    });

    edit(&host_shell, 0, "host typed\n", host_cx);
    let host_edit_deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        host_cx.run_until_parked();
        if guest_proxy
            .read_shared_buffer(view.session.id.clone(), "src.rs".into())
            .expect("live host buffer")
            .content
            == "host typed\n"
        {
            break;
        }
        assert!(
            std::time::Instant::now() < host_edit_deadline,
            "host edit did not reach shared buffer"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let guest_update_deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        guest_cx
            .background_executor
            .advance_clock(std::time::Duration::from_millis(800));
        guest_cx.run_until_parked();
        if guest_shell.read_with(guest_cx, |shell, cx| {
            shell.code_tabs[1].read(cx).editor.read(cx).value() == "host typed\n"
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < guest_update_deadline,
            "host edit did not reach guest editor"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    edit(&guest_shell, 1, "edited by guest\n", guest_cx);
    guest_cx.run_until_parked();
    guest_cx
        .background_executor
        .advance_clock(std::time::Duration::from_millis(150));
    let edit_deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        guest_cx.run_until_parked();
        for message in host_core.rx().try_iter() {
            if let CoreRpc::Notification(notification) = message {
                host_proxy.route_core(*notification);
            }
        }
        host_cx.run_until_parked();
        if host_shell.read_with(host_cx, |shell, cx| {
            shell.code_tabs[0].read(cx).editor.read(cx).value()
                == "edited by guest\n"
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < edit_deadline,
            "guest edit did not reach host editor"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        guest_proxy
            .read_shared_buffer(view.session.id.clone(), "src.rs".into())
            .expect("committed host buffer")
            .content,
        "edited by guest\n"
    );

    host.read()
        .add_session_participant_verified(
            &view.session.id,
            verified_guest.clone(),
            SessionRole::Viewer,
        )
        .expect("downgrade guest to viewer");
    let viewer = guest_proxy
        .poll_shared_session(view.session.id.clone(), 0, None, None)
        .expect("viewer role update");
    guest_session
        .update(guest_cx, |panel, cx| panel.apply_shared_update(viewer, cx));
    assert!(
        !guest_session.read_with(guest_cx, |panel, _| panel.shared_guest_can_edit())
    );
    guest_shell.update(guest_cx, |shell, cx| shell.sync_shared_editor_roles(cx));
    guest_shell.read_with(guest_cx, |shell, cx| {
        let shared = shell.code_tabs[1].read(cx);
        assert_eq!(shared.editor.read(cx).value(), "edited by guest\n");
        assert!(shared.status.contains("read only"));
    });
    let snapshot = guest_proxy
        .read_shared_buffer(view.session.id.clone(), "src.rs".into())
        .expect("viewer can read shared buffer");
    assert!(
        guest_proxy
            .replace_shared_buffer(
                view.session.id.clone(),
                "src.rs".into(),
                snapshot.revision,
                "viewer edit must fail\n".into(),
            )
            .is_err(),
        "viewer must not edit host buffer"
    );
    host.read()
        .add_session_participant_verified(
            &view.session.id,
            verified_guest,
            SessionRole::Editor,
        )
        .expect("restore guest editor role");
    let editor = guest_proxy
        .poll_shared_session(view.session.id.clone(), 0, None, None)
        .expect("editor role update after reconnect");
    guest_session
        .update(guest_cx, |panel, cx| panel.apply_shared_update(editor, cx));
    assert!(
        guest_session.read_with(guest_cx, |panel, _| panel.shared_guest_can_edit())
    );
    guest_shell.update(guest_cx, |shell, cx| shell.sync_shared_editor_roles(cx));
    guest_shell.read_with(guest_cx, |shell, cx| {
        let shared = shell.code_tabs[1].read(cx);
        assert_eq!(shared.editor.read(cx).value(), "edited by guest\n");
        assert!(shared.status.contains("live editing"));
    });

    let message = guest_proxy
        .post_shared_human_message(
            view.session.id.clone(),
            "@host please review".into(),
        )
        .expect("guest chat over TLS");
    assert_eq!(message.human_recipient_ids(), Some(vec!["host"]));
    assert!(
        host_proxy
            .conversation_page(&view.session.id, None, 50)
            .expect("host conversation")
            .messages
            .iter()
            .any(|item| item.id == message.id)
    );

    let comment = guest_proxy
        .create_code_comment(
            &view.session.id,
            "src.rs".into(),
            DisplayRange {
                start: DisplayPosition { line: 0, col: 0 },
                end: DisplayPosition { line: 0, col: 6 },
            },
            "edited".into(),
            "0".repeat(64),
            "Please check this edit".into(),
        )
        .expect("guest comment over TLS");
    let comment_deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        host_session.update(host_cx, |panel, cx| panel.poll_stream(cx));
        host_cx.run_until_parked();
        if host_session.read_with(host_cx, |panel, _| {
            panel
                .code_comments_snapshot()
                .iter()
                .any(|item| item.id == comment.id)
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < comment_deadline,
            "guest comment did not reach host navigator"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    host_proxy
        .publish_shared_terminal(view.session.id.clone(), "host terminal\n".into())
        .expect("publish host terminal");
    host_proxy
        .publish_shared_presence(
            view.session.id.clone(),
            Some("src.rs".into()),
            Some(1),
        )
        .expect("publish host presence");
    let update = guest_proxy
        .poll_shared_session(view.session.id.clone(), message.sequence, None, None)
        .expect("guest poll");
    assert_eq!(update.terminal_output, "host terminal\n");
    assert!(update.presence.iter().any(|presence| {
        presence.actor_id == "host"
            && presence.path.as_deref() == Some("src.rs")
            && presence.line == Some(1)
    }));
    assert!(
        update
            .code_comments
            .iter()
            .any(|item| item.id == comment.id)
    );
    guest_shell.update(guest_cx, |shell, cx| {
        shell.set_active_code(shell.code_tabs[0].clone(), cx);
    });
    guest_session
        .update(guest_cx, |panel, cx| panel.apply_shared_update(update, cx));
    assert!(guest_session.read_with(guest_cx, |panel, _| {
        let (terminal, presence_count) = panel.shared_remote_state_for_test();
        terminal == "host terminal\n" && presence_count > 0
    }));
    assert!(guest_session.read_with(guest_cx, |panel, _| {
        panel
            .code_comments_snapshot()
            .iter()
            .any(|item| item.id == comment.id)
    }));
    guest_session.update_in(guest_cx, |panel, window, cx| {
        panel.follow_shared_actor("host".into(), window, cx)
    });
    guest_cx.run_until_parked();
    assert_eq!(
        guest_shell.read_with(guest_cx, |shell, cx| shell
            .code
            .read(cx)
            .file_path
            .clone()),
        "src.rs"
    );

    host_proxy
        .publish_shared_terminal(view.session.id.clone(), String::new())
        .expect("clear closed host terminal");
    let cleared = guest_proxy
        .poll_shared_session(view.session.id.clone(), message.sequence, None, None)
        .expect("guest poll after terminal close");
    guest_session
        .update(guest_cx, |panel, cx| panel.apply_shared_update(cleared, cx));
    assert!(guest_session.read_with(guest_cx, |panel, _| {
        let (terminal, presence_count) = panel.shared_remote_state_for_test();
        terminal.is_empty() && presence_count > 0
    }));
    host_proxy
        .publish_shared_terminal(
            view.session.id.clone(),
            "host terminal reopened\n".into(),
        )
        .expect("publish reopened host terminal");
    let reopened = guest_proxy
        .poll_shared_session(view.session.id.clone(), message.sequence, None, None)
        .expect("guest poll after terminal reopen");
    guest_session.update(guest_cx, |panel, cx| {
        panel.apply_shared_update(reopened, cx)
    });
    assert!(guest_session.read_with(guest_cx, |panel, _| {
        let (terminal, presence_count) = panel.shared_remote_state_for_test();
        terminal == "host terminal reopened\n" && presence_count > 0
    }));

    host_proxy
        .stop_sharing_session(view.session.id.clone())
        .expect("stop hosting shared session");
    assert!(
        guest_proxy
            .read_shared_buffer(view.session.id.clone(), "src.rs".into())
            .is_err(),
        "stopped share must remove host buffer access"
    );
    guest_session.update(guest_cx, |panel, cx| panel.poll_stream(cx));
    let stop_deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !guest_session.read_with(guest_cx, |panel, _| {
        panel
            .status
            .as_ref()
            .starts_with("Shared session disconnected:")
    }) && std::time::Instant::now() < stop_deadline
    {
        guest_cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(guest_session.read_with(guest_cx, |panel, _| {
        panel
            .status
            .as_ref()
            .starts_with("Shared session disconnected:")
    }));
    assert!(guest_session.read_with(guest_cx, |panel, _| {
        let (terminal, presence_count) = panel.shared_remote_state_for_test();
        terminal.is_empty() && presence_count == 0
    }));
    guest_shell.update(guest_cx, |shell, cx| shell.sync_shared_editor_roles(cx));
    guest_shell.read_with(guest_cx, |shell, cx| {
        let shared = shell.code_tabs[1].read(cx);
        assert_eq!(shared.editor.read(cx).value(), "edited by guest\n");
        assert!(shared.status.contains("read only"));
    });

    guest_rpc.shutdown();
    host_rpc.shutdown();
    guest_thread.join().expect("guest dispatcher");
    host_thread.join().expect("host dispatcher");
    api_stop.store(true, Ordering::Relaxed);
    api_thread.join().expect("mock API");
}

#[gpui_kit::test]
fn shared_guest_child_process(cx: &mut TestAppContext) {
    let Ok(offer_json) = std::env::var("AHEAD_COLLAB_PROCESS_OFFER") else {
        return;
    };
    let offer: ahead_rpc::ahead::SharedSessionOffer =
        serde_json::from_str(&offer_json).expect("shared offer");
    let guest_workspace = tempfile::tempdir().expect("guest workspace");
    let (guest_shell, guest_cx, guest_proxy) =
        open_shell(guest_workspace.path(), &["local.rs"], cx);
    let guest_rpc = guest_proxy.rpc_for_test();
    let (mut guest_dispatcher, _) = authenticated_test_dispatcher(
        &guest_proxy,
        guest_workspace.path(),
        "guest-token",
        "bob",
        42,
        2,
        None,
    );
    let guest_thread = std::thread::spawn({
        let guest_rpc = guest_rpc.clone();
        move || guest_rpc.mainloop(&mut guest_dispatcher)
    });
    let guest_session =
        guest_shell.read_with(guest_cx, |shell, _| shell.session.clone());
    guest_session.update_in(guest_cx, |panel, window, cx| {
        panel
            .chat_input
            .update(cx, |input, cx| input.set_value(offer_json, window, cx));
        panel.join_shared_session(window, cx);
    });
    let join_deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(5);
    while guest_session.read_with(guest_cx, |panel, _| panel.session_id.clone())
        != Some(offer.session_id.clone())
        && std::time::Instant::now() < join_deadline
    {
        guest_cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        guest_session.read_with(guest_cx, |panel, _| panel.session_id.clone()),
        Some(offer.session_id.clone()),
        "{}",
        guest_session.read_with(guest_cx, |panel, _| panel.status.clone())
    );
    assert!(
        guest_session.read_with(guest_cx, |panel, _| panel.shared_guest_can_edit())
    );
    guest_session.update(guest_cx, |panel, _| panel.set_shell(guest_shell.clone()));
    let update = guest_proxy
        .poll_shared_session(offer.session_id.clone(), 0, None, None)
        .expect("cross-process guest poll");
    assert_eq!(update.terminal_output, "host terminal across processes\n");
    assert!(update.presence.iter().any(|presence| {
        presence.actor_id == "host"
            && presence.path.as_deref() == Some("src.rs")
            && presence.line == Some(1)
    }));
    guest_session
        .update(guest_cx, |panel, cx| panel.apply_shared_update(update, cx));
    assert!(guest_session.read_with(guest_cx, |panel, _| {
        let (terminal, presence_count) = panel.shared_remote_state_for_test();
        terminal == "host terminal across processes\n" && presence_count > 0
    }));
    guest_session.update_in(guest_cx, |panel, window, cx| {
        panel.follow_shared_actor("host".into(), window, cx)
    });
    guest_cx.run_until_parked();
    assert_eq!(
        guest_shell.read_with(guest_cx, |shell, cx| shell
            .code
            .read(cx)
            .file_path
            .clone()),
        "src.rs"
    );
    let snapshot = guest_proxy
        .read_shared_buffer(offer.session_id.clone(), "src.rs".into())
        .expect("host buffer");
    guest_shell.update_in(guest_cx, |shell, window, cx| {
        shell.open_shared_buffer(
            offer.session_id.clone(),
            snapshot,
            None,
            window,
            cx,
        )
    });
    edit(&guest_shell, 1, "edited across processes\n", guest_cx);
    guest_cx.run_until_parked();
    guest_cx
        .background_executor
        .advance_clock(std::time::Duration::from_millis(150));
    let edit_deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        guest_cx.run_until_parked();
        let snapshot = guest_proxy
            .read_shared_buffer(offer.session_id.clone(), "src.rs".into())
            .expect("shared buffer after edit");
        if snapshot.content == "edited across processes\n" {
            break;
        }
        assert!(
            std::time::Instant::now() < edit_deadline,
            "guest edit did not reach host"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let message = guest_proxy
        .post_shared_human_message(
            offer.session_id.clone(),
            "@host the second AHEAD process edited src.rs".into(),
        )
        .expect("cross-process chat");
    assert_eq!(message.human_recipient_ids(), Some(vec!["host"]));
    guest_proxy
        .create_code_comment(
            &offer.session_id,
            "src.rs".into(),
            DisplayRange {
                start: DisplayPosition { line: 0, col: 0 },
                end: DisplayPosition { line: 0, col: 6 },
            },
            "edited".into(),
            "0".repeat(64),
            "Guest note from second process".into(),
        )
        .expect("cross-process guest comment");
    assert!(
        !guest_proxy
            .start_shared_agent_turn(
                offer.session_id.clone(),
                "Reply with HOST_PAID_RESPONSE".into(),
            )
            .expect("guest-started host agent turn")
            .is_empty()
    );
    assert!(
        guest_proxy
            .start_shared_agent_turn(
                offer.session_id.clone(),
                "@host this stays human-only".into(),
            )
            .is_err(),
        "addressed messages must not start the host agent"
    );
    let reconnected = guest_proxy
        .poll_shared_session(offer.session_id.clone(), message.sequence, None, None)
        .expect("guest reconnect after rejected turn");
    assert_eq!(reconnected.actor_id, "bob");
    guest_proxy
        .post_shared_human_message(
            offer.session_id.clone(),
            "@host guest reconnected to the shared session".into(),
        )
        .expect("chat after guest reconnect");
    let revoke_deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(70);
    while !guest_session.read_with(guest_cx, |panel, _| {
        panel
            .status
            .as_ref()
            .starts_with("Shared session disconnected:")
    }) && std::time::Instant::now() < revoke_deadline
    {
        guest_session.update(guest_cx, |panel, cx| panel.poll_stream(cx));
        guest_cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(guest_session.read_with(guest_cx, |panel, _| {
        panel
            .status
            .as_ref()
            .starts_with("Shared session disconnected:")
    }));
    assert!(guest_session.read_with(guest_cx, |panel, _| {
        let (terminal, presence_count) = panel.shared_remote_state_for_test();
        terminal.is_empty() && presence_count == 0
    }));
    assert!(
        guest_proxy
            .read_shared_buffer(offer.session_id, "src.rs".into())
            .is_err(),
        "revoked guest must lose host buffer access"
    );
    guest_shell.update(guest_cx, |shell, cx| shell.sync_shared_editor_roles(cx));
    guest_shell.read_with(guest_cx, |shell, cx| {
        let shared = shell.code_tabs[1].read(cx);
        assert_eq!(shared.editor.read(cx).value(), "edited across processes\n");
        assert!(shared.status.contains("read only"));
    });
    guest_rpc.shutdown();
    guest_thread.join().expect("guest dispatcher");
}

#[gpui_kit::test]
fn separate_ahead_processes_share_code_and_chat(cx: &mut TestAppContext) {
    use std::process::{Command, Stdio};

    let host_workspace = tempfile::tempdir().expect("host workspace");
    std::fs::create_dir(host_workspace.path().join(".ahead"))
        .expect("host config directory");
    std::fs::write(
        host_workspace.path().join(".ahead/team.toml"),
        "[[members]]\ngithub = 'bob'\ndisplay_name = 'Bob'\nrole = 'editor'\n",
    )
    .expect("team allowlist");
    let (model_endpoint, model_requests, model_server) = start_mock_host_model();
    std::fs::write(
        host_workspace.path().join(".ahead/settings.toml"),
        format!(
            "[ai]\nactive_connection = 'Host model'\n[[ai.connections]]\nname = 'Host model'\nprovider_id = 'mock'\nbase_url = '{model_endpoint}'\napi_key = 'host-model-key'\nmodel = 'gpt-5.6-sol'\n"
        ),
    )
    .expect("host-only model connection");
    let (api_base, api_stop, api_thread) = start_mock_github_api();
    let (host_shell, host_cx, host_proxy) =
        open_shell(host_workspace.path(), &["src.rs"], cx);
    let host_rpc = host_proxy.rpc_for_test();
    let (mut host_dispatcher, host_core) = authenticated_test_dispatcher(
        &host_proxy,
        host_workspace.path(),
        "host-token",
        "host",
        1,
        1,
        Some(api_base),
    );
    let host = host_dispatcher
        .ahead_host
        .as_ref()
        .expect("host session store")
        .clone();
    let view = host
        .read()
        .start_work(
            Some(WorkKind::ProductChange),
            "Shared process test".into(),
            "Collaborate".into(),
            None,
        )
        .expect("active managed session");
    host.read()
        .add_session_participant_verified(
            &view.session.id,
            GitHubUser {
                login: "bob".into(),
                id: 42,
                name: Some("Bob".into()),
                avatar_url: None,
                email: None,
                is_authenticated: true,
            },
            SessionRole::Editor,
        )
        .expect("verified guest");
    let host_thread = std::thread::spawn({
        let host_rpc = host_rpc.clone();
        move || host_rpc.mainloop(&mut host_dispatcher)
    });
    host_shell.update_in(host_cx, |shell, window, cx| {
        shell.code_tabs[0].update(cx, |code, cx| {
            code.watch_shared_buffer_changes(&host_proxy, window, cx)
        });
    });
    let host_session =
        host_shell.read_with(host_cx, |shell, _| shell.session.clone());
    host_session.update(host_cx, |panel, cx| {
        panel.attach_session(view.session.id.clone(), cx)
    });
    let offer = host_proxy
        .share_session(view.session.id.clone(), "127.0.0.1:0".into())
        .expect("direct TLS invite");
    host_session.update(host_cx, |panel, cx| panel.share_active_session(cx));
    let share_deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !host_session.read_with(host_cx, |panel, _| {
        panel.status.as_ref().starts_with("Sharing on ")
    }) && std::time::Instant::now() < share_deadline
    {
        host_cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(host_session.read_with(host_cx, |panel, _| {
        panel.status.as_ref().starts_with("Sharing on ")
    }));
    host_proxy
        .publish_shared_terminal(
            view.session.id.clone(),
            "host terminal across processes\n".into(),
        )
        .expect("cross-process host terminal");
    host_proxy
        .publish_shared_presence(
            view.session.id.clone(),
            Some("src.rs".into()),
            Some(1),
        )
        .expect("cross-process host presence");
    let child_test = "app::lifecycle_tests::shared_guest_child_process";
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", child_test, "--nocapture"])
        .env(
            "AHEAD_COLLAB_PROCESS_OFFER",
            serde_json::to_string(&offer).expect("offer JSON"),
        )
        .env_remove("ITERATIONS")
        .env("RUST_BACKTRACE", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("second AHEAD test process");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(75);
    let mut model_request = None;
    loop {
        for message in host_core.rx().try_iter() {
            if let CoreRpc::Notification(notification) = message {
                host_proxy.route_core(*notification);
            }
        }
        host_cx.run_until_parked();
        let host_changed = host_shell.read_with(host_cx, |shell, cx| {
            shell.code_tabs[0].read(cx).editor.read(cx).value()
                == "edited across processes\n"
        });
        if child.try_wait().expect("child status").is_some() {
            let output = child.wait_with_output().expect("child output");
            panic!(
                "guest process exited before revocation: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let conversation = host_proxy
            .conversation_page(&view.session.id, None, 50)
            .expect("host conversation");
        let posted = conversation.messages.iter().any(|message| {
            message.content == "@host the second AHEAD process edited src.rs"
                && message.human_recipient_ids() == Some(vec!["host"])
        });
        let reconnected = conversation.messages.iter().any(|message| {
            message.content == "@host guest reconnected to the shared session"
                && message.human_recipient_ids() == Some(vec!["host"])
        });
        if model_request.is_none() {
            match model_requests.try_recv() {
                Ok(request) => model_request = Some(request),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    panic!(
                        "host model server stopped before a request; session status: {}; messages: {:?}",
                        host_session
                            .read_with(host_cx, |panel, _| panel.status.clone()),
                        conversation
                            .messages
                            .iter()
                            .map(|message| (
                                &message.role,
                                &message.content,
                                &message.status
                            ))
                            .collect::<Vec<_>>()
                    )
                }
            }
        }
        let commented = host_proxy
            .code_comments(&view.session.id)
            .expect("host comments")
            .iter()
            .any(|comment| comment.body == "Guest note from second process");
        if host_changed
            && posted
            && reconnected
            && commented
            && model_request.is_some()
        {
            break;
        }
        if std::time::Instant::now() >= deadline {
            if let Err(error) = child.kill() {
                eprintln!("Could not stop stalled guest process: {error}");
            }
            let output = child.wait_with_output().expect("stalled child output");
            panic!(
                "separate-process edit did not converge: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    model_server.join().expect("host model server");
    let (headers, request) = model_request.expect("host model request");
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("authorization: bearer host-model-key"),
        "guest turn must use the host's model credential"
    );
    let request = request.to_string();
    assert!(request.contains("Reply with HOST_PAID_RESPONSE"));
    assert!(
        !request.contains("@host the second AHEAD process edited src.rs"),
        "addressed human-only chat reached the model"
    );
    host_proxy
        .revoke_session_participant(view.session.id.clone(), "bob".into())
        .expect("revoke guest across processes");
    while child.try_wait().expect("child status").is_none() {
        if std::time::Instant::now() >= deadline {
            if let Err(error) = child.kill() {
                eprintln!("Could not stop stalled guest process: {error}");
            }
            let output = child.wait_with_output().expect("stalled child output");
            panic!(
                "guest process did not stop after revocation: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = child.wait_with_output().expect("child output");
    assert!(
        output.status.success(),
        "guest process failed after revocation: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let answer_deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let messages = host_proxy
            .conversation_page(&view.session.id, None, 50)
            .expect("host conversation after guest turn")
            .messages;
        if messages.iter().any(|message| {
            message.role == "agent" && message.content.contains("HOST_PAID_RESPONSE")
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < answer_deadline,
            "host agent response missing: {:?}",
            messages
                .iter()
                .map(|message| (&message.role, &message.content))
                .collect::<Vec<_>>()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        host_proxy
            .conversation_page(&view.session.id, None, 50)
            .expect("host conversation")
            .messages
            .iter()
            .any(|message| {
                message.content == "@host the second AHEAD process edited src.rs"
                    && message.human_recipient_ids() == Some(vec!["host"])
            })
    );
    let comment_deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        host_session.update(host_cx, |panel, cx| panel.poll_stream(cx));
        host_cx.run_until_parked();
        if host_session.read_with(host_cx, |panel, _| {
            panel
                .code_comments_snapshot()
                .iter()
                .any(|comment| comment.body == "Guest note from second process")
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < comment_deadline,
            "guest comment did not reach host navigator across processes"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    host_rpc.shutdown();
    host_thread.join().expect("host dispatcher");
    api_stop.store(true, Ordering::Relaxed);
    api_thread.join().expect("mock API");
}

#[test]
fn debug_terminal_command_quotes_arguments_and_environment() {
    let arguments = ahead_rpc::dap_types::RunInTerminalArguments {
        kind: Some("integrated".into()),
        title: None,
        cwd: None,
        args: vec!["/bin/echo".into(), "hello'; touch /tmp/escaped".into()],
        env: Some(std::collections::HashMap::from([
            ("SAFE".into(), Some("a'b".into())),
            ("REMOVE".into(), None),
        ])),
    };
    assert_eq!(
        debug_terminal_command(&arguments).expect("command"),
        Some("unset REMOVE; export SAFE='a'\\''b'; '/bin/echo' 'hello'\\''; touch /tmp/escaped'".into())
    );
    let mut invalid = arguments;
    invalid.env = Some(std::collections::HashMap::from([("BAD;NAME".into(), None)]));
    assert!(debug_terminal_command(&invalid).is_err());
}

fn open_shell<'a>(
    workspace: &Path,
    files: &[&str],
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext, Arc<ProxyClient>) {
    cx.update(gpui_kit::component::init);
    let proxy = ProxyClient::new_for_test(workspace.to_owned());
    let root_path = workspace.to_string_lossy().into_owned();
    let paths: Vec<_> = files
        .iter()
        .map(|file| {
            let path = workspace.join(file);
            std::fs::create_dir_all(path.parent().expect("parent"))
                .expect("directories");
            std::fs::write(&path, "saved\n").expect("test source");
            path.to_string_lossy().into_owned()
        })
        .collect();
    let (root, cx) = cx.add_window_view(|window, cx| {
        let (area, _) = DockSkin::dock_area("lifecycle-test", None, window, cx);
        let code = cx.new(|cx| CodePanel::new(&paths[0], window, cx));
        let session = cx.new(|cx| {
            crate::session_panel::SessionPanel::new(workspace.to_owned(), window, cx)
        });
        let threads =
            cx.new(|cx| crate::threads_panel::ThreadsPanel::new(window, cx));
        let explorer = cx.new(|cx| {
            crate::explorer_panel::ExplorerPanel::new(&root_path, window, cx)
        });
        let settings = cx.new(|cx| {
            crate::settings_panel::SettingsPanel::new(
                workspace.to_owned(),
                window,
                cx,
            )
        });
        let extensions = cx.new(|cx| {
            crate::extensions_panel::ExtensionsPanel::new(proxy.clone(), window, cx)
        });
        let debug = cx.new(|cx| {
            crate::debug_bar::DebugBar::new(proxy.clone(), &paths[0], window, cx)
        });
        let git = cx.new(|cx| GitPanel::new(&root_path, window, cx));
        let tasks = cx.new(|cx| JustTasksPanel::new(&root_path, cx));
        let languages = cx.new(|cx| {
            LanguageServersPanel::new(workspace, proxy.clone(), cx)
        });
        let search = cx.new(|cx| {
            SearchPanel::new(
                &root_path,
                explorer.read(cx).mailbox_id,
                code.clone(),
                window,
                cx,
            )
        });
        let problems = cx.new(|cx| ProblemsPanel::new(code.clone(), cx));
        let help = cx.new(crate::help_panel::HelpPanel::new);
        let agent_workspace = cx.new(|cx| {
            AgentWorkspacePanel::new(session.clone(), threads.clone(), cx)
        });
        let activity = cx.new(|_| {
            ActivityBar::new(area.clone(), explorer.clone(), git, tasks, languages)
        });
        let shell = cx.new(|cx| {
            Shell::new(
                area,
                session.clone(),
                threads,
                code.clone(),
                explorer,
                debug,
                Vec::new(),
                problems,
                settings,
                extensions,
                help,
                search,
                agent_workspace,
                activity,
                cx,
            )
        });
        shell.update(cx, |shell, cx| {
            for path in paths.iter().skip(1) {
                shell
                    .code_tabs
                    .push(cx.new(|cx| CodePanel::new(path, window, cx)));
            }
            for code in &shell.code_tabs {
                code.update(cx, |code, _| {
                    code.proxy = Some(proxy.clone());
                    code.workspace = root_path.clone();
                });
            }
            shell
                .session
                .update(cx, |session, _| session.proxy = Some(proxy.clone()));
        });
        Shell::install_window_close_handler(&shell, window, cx);
        gpui_kit::component::Root::new(shell, window, cx)
    });
    let shell = root.update(cx, |root, _| {
        root.view().clone().downcast::<Shell>().ok().expect("shell")
    });
    (shell, cx, proxy)
}

fn edit(
    shell: &Entity<Shell>,
    index: usize,
    text: &str,
    cx: &mut VisualTestContext,
) {
    let editor = shell.update(cx, |shell, cx| {
        shell.code_tabs[index].read(cx).editor.clone()
    });
    editor.update_in(cx, |editor, window, cx| {
        editor.replace_all(text, window, cx)
    });
}

#[gpui_kit::test]
fn agent_command_focuses_the_active_chat_composer(cx: &mut TestAppContext) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, _) = open_shell(workspace.path(), &["main.rs"], cx);
    let session = shell.read_with(cx, |shell, _| shell.session.clone());

    shell.update_in(cx, |shell, window, cx| {
        shell.run_shell_command(ShellShortcut::Agent, window, cx);
        assert!(session.read(cx).focus.is_focused(window));
    });

    session.update(cx, |session, _| {
        session.session_id = Some("active-thread".into());
    });
    shell.update_in(cx, |shell, window, cx| {
        shell.run_shell_command(ShellShortcut::Agent, window, cx);
        assert!(
            session
                .read(cx)
                .chat_input
                .focus_handle(cx)
                .is_focused(window)
        );
    });
}

#[gpui_kit::test]
fn extensions_open_as_center_tab_and_close(cx: &mut TestAppContext) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, _) = open_shell(workspace.path(), &["main.rs"], cx);
    shell.update_in(cx, |shell, window, cx| {
        shell.open_extensions(window, cx);
    });
    assert!(shell.read_with(cx, |shell, cx| shell.extensions.read(cx).catalog_busy));
    cx.run_until_parked();
    assert!(shell.read_with(cx, |shell, _| {
        shell.open_center_panels.contains(&CenterPanel::Extensions)
    }));
    shell.update_in(cx, |shell, window, cx| {
        let focus = shell.extensions.read(cx).search_input.focus_handle(cx);
        assert!(focus.is_focused(window));
    });
    let panel_id = shell.read_with(cx, |shell, _| {
        gpui_kit::component::dock::PanelId::from(shell.extensions.entity_id())
    });
    shell.update_in(cx, |shell, window, cx| {
        shell.close_center_panel(panel_id, window, cx);
    });
    assert!(!shell.read_with(cx, |shell, _| {
        shell.open_center_panels.contains(&CenterPanel::Extensions)
    }));
}

#[gpui_kit::test]
fn center_utility_tabs_remain_open_together(cx: &mut TestAppContext) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, _) = open_shell(workspace.path(), &["main.rs"], cx);
    for panel in [
        CenterPanel::Search,
        CenterPanel::Help,
        CenterPanel::Settings,
        CenterPanel::Problems,
    ] {
        shell.update_in(cx, |shell, window, cx| {
            shell.open_center_panel(panel, window, cx);
        });
    }
    let center =
        shell.read_with(cx, |shell, cx| shell.area.read(cx).dump(cx).center);
    fn tabs(
        state: &gpui_kit::component::dock::PanelState,
    ) -> Option<&gpui_kit::component::dock::PanelState> {
        if matches!(
            state.info,
            gpui_kit::component::dock::PanelInfo::Tabs { .. }
        ) {
            Some(state)
        } else {
            state.children.iter().find_map(tabs)
        }
    }
    let center_tabs = tabs(&center).expect("center tab group");
    assert_eq!(center_tabs.children.len(), 5);
    assert_eq!(center_tabs.info.active_index(), Some(4));

    shell.update_in(cx, |shell, window, cx| shell.show_code(window, cx));
    let center =
        shell.read_with(cx, |shell, cx| shell.area.read(cx).dump(cx).center);
    let center_tabs = tabs(&center).expect("center tab group");
    assert_eq!(center_tabs.children.len(), 5);
    assert_eq!(center_tabs.info.active_index(), Some(0));

    shell.update_in(cx, |shell, window, cx| {
        shell.open_center_panel(CenterPanel::Search, window, cx);
    });
    let center =
        shell.read_with(cx, |shell, cx| shell.area.read(cx).dump(cx).center);
    let center_tabs = tabs(&center).expect("center tab group");
    assert_eq!(center_tabs.children.len(), 5);
    assert_eq!(center_tabs.info.active_index(), Some(1));

    let help_id = shell.read_with(cx, |shell, _| {
        gpui_kit::component::dock::PanelId::from(shell.help.entity_id())
    });
    shell.update_in(cx, |shell, window, cx| {
        shell.close_center_panel(help_id, window, cx)
    });
    let center =
        shell.read_with(cx, |shell, cx| shell.area.read(cx).dump(cx).center);
    assert_eq!(tabs(&center).expect("center tab group").children.len(), 4);
    assert!(shell.read_with(cx, |shell, _| {
        shell.open_center_panels.contains(&CenterPanel::Search)
    }));
}

#[cfg(unix)]
fn add_terminal(
    shell: &Entity<Shell>,
    workspace: &Path,
    cx: &mut VisualTestContext,
) -> Entity<crate::terminal_panel::TerminalPanel> {
    let terminal = cx.new(|cx| {
        crate::terminal_panel::TerminalPanel::new_with_shell(
            17,
            workspace.to_string_lossy().into_owned(),
            "/bin/sh",
            cx,
        )
    });
    shell.update(cx, |shell, _| shell.terminals.push(terminal.clone()));
    terminal
}

#[cfg(unix)]
#[gpui_kit::test]
fn terminal_close_stops_backend_while_panel_is_retained(cx: &mut TestAppContext) {
    let workspace = tempfile::tempdir().unwrap();
    let (shell, cx, _) = open_shell(workspace.path(), &["main.py"], cx);
    let terminal = add_terminal(&shell, workspace.path(), cx);
    assert!(!terminal.read_with(cx, |terminal, _| terminal.is_shutdown()));
    shell.update_in(cx, |shell, window, cx| shell.close_terminal(17, window, cx));
    assert!(shell.read_with(cx, |shell, _| shell.terminals.is_empty()));
    assert!(terminal.read_with(cx, |terminal, _| terminal.is_shutdown()));
}

fn take_request(
    rpc: &ProxyRpcHandler,
    predicate: impl Fn(&ProxyRequest) -> bool,
) -> ahead_rpc::RequestId {
    rpc.rx()
        .try_iter()
        .find_map(|message| match message {
            ProxyRpc::Request(id, request) if predicate(&request) => Some(id),
            _ => None,
        })
        .expect("expected RPC request")
}

#[gpui_kit::test(iterations = 5)]
async fn window_close_cancel_and_edits_during_prompts_preserve_all_tabs(
    cx: &mut TestAppContext,
) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, _) = open_shell(workspace.path(), &["main.ts", "main.py"], cx);
    edit(&shell, 0, "unsaved TS\n", cx);
    edit(&shell, 1, "unsaved Python\n", cx);
    let preparation = shell.update_in(cx, |shell, window, cx| {
        shell.prepare_window_close(window, cx)
    });
    cx.run_until_parked();
    cx.simulate_prompt_answer("Cancel");
    assert!(preparation.await.is_none());
    assert!(!shell.update(cx, |shell, _| shell.closing_code_tabs));
    assert_eq!(shell.update(cx, |shell, _| shell.code_tabs.len()), 2);

    let preparation = shell.update_in(cx, |shell, window, cx| {
        shell.prepare_window_close(window, cx)
    });
    cx.run_until_parked();
    cx.simulate_prompt_answer("Discard Changes");
    cx.run_until_parked();
    edit(&shell, 0, "newer edit after first approval\n", cx);
    cx.simulate_prompt_answer("Discard Changes");
    assert!(
        preparation.await.is_none(),
        "all earlier approvals must still be current"
    );
    assert_eq!(shell.update(cx, |shell, _| shell.code_tabs.len()), 2);
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("main.ts")).expect("disk"),
        "saved\n"
    );
}

#[gpui_kit::test(iterations = 5)]
async fn window_close_waits_for_save_and_preserves_failed_or_newer_buffers(
    cx: &mut TestAppContext,
) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, proxy) = open_shell(workspace.path(), &["main.py"], cx);
    let rpc = proxy.rpc_for_test();
    edit(&shell, 0, "unsaved\n", cx);
    let preparation = shell.update_in(cx, |shell, window, cx| {
        shell.prepare_window_close(window, cx)
    });
    cx.run_until_parked();
    cx.simulate_prompt_answer("Save");
    cx.run_until_parked();
    let failed = take_request(&rpc, |request| {
        matches!(request, ProxyRequest::SaveEditorBuffer { .. })
    });
    assert!(shell.update(cx, |shell, _| shell.closing_code_tabs));
    rpc.handle_response(
        failed,
        Err(ahead_rpc::RpcError {
            code: 1,
            message: "disk is full".into(),
        }),
    );
    assert!(preparation.await.is_none());
    assert!(shell.update(cx, |shell, cx| shell.code_tabs[0].read(cx).dirty));

    let preparation = shell.update_in(cx, |shell, window, cx| {
        shell.prepare_window_close(window, cx)
    });
    cx.run_until_parked();
    cx.simulate_prompt_answer("Save");
    cx.run_until_parked();
    let saved = take_request(&rpc, |request| {
        matches!(request, ProxyRequest::SaveEditorBuffer { .. })
    });
    edit(&shell, 0, "newer typing\n", cx);
    rpc.handle_response(saved, Ok(ProxyResponse::SaveResponse {}));
    assert!(preparation.await.is_none());
    assert!(shell.update(cx, |shell, cx| shell.code_tabs[0].read(cx).dirty));

    let preparation = shell.update_in(cx, |shell, window, cx| {
        shell.prepare_window_close(window, cx)
    });
    cx.run_until_parked();
    cx.simulate_prompt_answer("Save");
    cx.run_until_parked();
    let saved = take_request(&rpc, |request| {
        matches!(request, ProxyRequest::SaveEditorBuffer { .. })
    });
    rpc.handle_response(saved, Ok(ProxyResponse::SaveResponse {}));
    assert!(preparation.await.is_some());
    assert!(!shell.update(cx, |shell, cx| shell.code_tabs[0].read(cx).dirty));
}

#[gpui_kit::test(iterations = 5)]
fn trash_confirmation_cancel_stale_reply_and_rpc_failure_keep_buffers(
    cx: &mut TestAppContext,
) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, proxy) =
        open_shell(workspace.path(), &["src/main.ts", "outside.py"], cx);
    let rpc = proxy.rpc_for_test();
    let path = workspace.path().join("src");
    edit(&shell, 0, "unsaved\n", cx);
    shell.update_in(cx, |shell, window, cx| {
        shell.trash_workspace_path(path.clone(), window, cx)
    });
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(!rpc.rx().try_iter().any(|message| matches!(
        message,
        ProxyRpc::Request(_, ProxyRequest::TrashPath { .. })
    )));

    shell.update_in(cx, |shell, window, cx| {
        shell.trash_workspace_path(path.clone(), window, cx)
    });
    edit(&shell, 0, "newer unsaved\n", cx);
    cx.simulate_prompt_answer("Move to Trash");
    cx.run_until_parked();
    assert!(!rpc.rx().try_iter().any(|message| matches!(
        message,
        ProxyRpc::Request(_, ProxyRequest::TrashPath { .. })
    )));
    assert!(shell.update(cx, |shell, _| shell.status_message.contains("changed")));

    shell.update_in(cx, |shell, window, cx| {
        shell.trash_workspace_path(path.clone(), window, cx)
    });
    cx.simulate_prompt_answer("Move to Trash");
    cx.run_until_parked();
    let failed = take_request(
        &rpc,
        |request| matches!(request, ProxyRequest::TrashPath { path: target } if target == &path),
    );
    rpc.handle_response(
        failed,
        Err(ahead_rpc::RpcError {
            code: 1,
            message: "Trash unavailable".into(),
        }),
    );
    cx.run_until_parked();
    assert_eq!(shell.update(cx, |shell, _| shell.code_tabs.len()), 2);
    assert!(shell.update(cx, |shell, _| {
        shell.status_message.contains("Trash unavailable")
    }));

    shell.update_in(cx, |shell, window, cx| {
        shell.trash_workspace_path(path.clone(), window, cx)
    });
    cx.simulate_prompt_answer("Move to Trash");
    cx.run_until_parked();
    let pending = take_request(&rpc, |request| {
        matches!(request, ProxyRequest::TrashPath { .. })
    });
    edit(&shell, 0, "typing while Trash is pending\n", cx);
    rpc.handle_response(pending, Ok(ProxyResponse::Success {}));
    cx.run_until_parked();
    assert_eq!(shell.update(cx, |shell, _| shell.code_tabs.len()), 2);
    assert!(shell.update(cx, |shell, cx| shell.code_tabs[0].read(cx).dirty));

    shell.update_in(cx, |shell, window, cx| {
        shell.trash_workspace_path(path, window, cx)
    });
    cx.simulate_prompt_answer("Move to Trash");
    cx.run_until_parked();
    let confirmed = take_request(&rpc, |request| {
        matches!(request, ProxyRequest::TrashPath { .. })
    });
    rpc.handle_response(confirmed, Ok(ProxyResponse::Success {}));
    cx.run_until_parked();
    assert_eq!(shell.update(cx, |shell, _| shell.code_tabs.len()), 1);
    assert!(shell.update(cx, |shell, cx| {
        shell.code_tabs[0]
            .read(cx)
            .file_path
            .ends_with("outside.py")
    }));
    assert!(
        workspace.path().join("src/main.ts").exists(),
        "this UI test mocks Trash and never deletes a real file"
    );
}

#[gpui_kit::test(iterations = 5)]
fn quit_action_only_authorizes_after_all_buffer_decisions(cx: &mut TestAppContext) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, _) = open_shell(workspace.path(), &["main.ts", "main.py"], cx);
    edit(&shell, 0, "unsaved TS\n", cx);
    edit(&shell, 1, "unsaved Python\n", cx);
    cx.update(|_, cx| request_quit(&Quit, cx));
    cx.run_until_parked();
    cx.update(|_, cx| request_quit(&Quit, cx));
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(!shell.update(cx, |shell, _| shell.window_close_authorized));
    assert!(!cx.cx.read(|cx| cx.global::<QuitInProgress>().0));

    cx.update(|_, cx| request_quit(&Quit, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Discard Changes");
    cx.run_until_parked();
    cx.simulate_prompt_answer("Discard Changes");
    cx.run_until_parked();
    assert!(shell.update(cx, |shell, _| shell.window_close_authorized));
    assert!(shell.update(cx, |shell, cx| {
        shell
            .code_tabs
            .iter()
            .all(|code| code.read(cx).proxy.is_none())
    }));
}

#[gpui_kit::test(iterations = 5)]
async fn window_close_save_timeout_keeps_buffer_and_ignores_late_ack(
    cx: &mut TestAppContext,
) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, proxy) = open_shell(workspace.path(), &["main.py"], cx);
    edit(&shell, 0, "unsaved\n", cx);
    let preparation = shell.update_in(cx, |shell, window, cx| {
        shell.prepare_window_close(window, cx)
    });
    cx.run_until_parked();
    cx.simulate_prompt_answer("Save");
    cx.run_until_parked();
    let rpc = proxy.rpc_for_test();
    let save = take_request(&rpc, |request| {
        matches!(request, ProxyRequest::SaveEditorBuffer { .. })
    });
    cx.background_executor
        .advance_clock(std::time::Duration::from_secs(30));
    assert!(preparation.await.is_none());
    assert!(!shell.update(cx, |shell, _| shell.closing_code_tabs));
    assert!(shell.update(cx, |shell, cx| {
        let code = shell.code_tabs[0].read(cx);
        code.dirty && code.status.contains("timed out")
    }));
    rpc.handle_response(save, Ok(ProxyResponse::SaveResponse {}));
    cx.run_until_parked();
    assert!(shell.update(cx, |shell, cx| shell.code_tabs[0].read(cx).dirty));
}

#[gpui_kit::test(iterations = 5)]
fn close_window_action_cancels_then_releases_and_removes_window(
    cx: &mut TestAppContext,
) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, proxy) = open_shell(workspace.path(), &["main.ts"], cx);
    #[cfg(unix)]
    let terminal = add_terminal(&shell, workspace.path(), cx);
    let rpc = proxy.rpc_for_test();
    edit(&shell, 0, "unsaved\n", cx);
    cx.update(|window, cx| {
        window.activate_window();
        request_close_window(&CloseWindow, cx);
    });
    cx.run_until_parked();
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(cx.cx.read(|cx| cx.windows().len()), 1);
    assert!(!shell.update(cx, |shell, _| shell.window_close_authorized));

    assert!(
        !rpc.rx().try_iter().any(|message| matches!(
            message,
            ProxyRpc::Shutdown
                | ProxyRpc::Notification(ProxyNotification::Shutdown {})
        )),
        "cancelling window close must leave the proxy running"
    );
    #[cfg(unix)]
    assert!(!terminal.read_with(cx, |terminal, _| terminal.is_shutdown()));

    cx.update(|_, cx| request_close_window(&CloseWindow, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Discard Changes");
    cx.run_until_parked();
    assert!(cx.cx.read(|cx| cx.windows().is_empty()));
    #[cfg(unix)]
    assert!(terminal.read_with(cx, |terminal, _| terminal.is_shutdown()));
    cx.cx.read(|cx| {
        let shell = shell.read(cx);
        assert!(shell.window_close_authorized);
        assert!(shell.code_tabs[0].read(cx).proxy.is_none());
    });
    let queued: Vec<_> = rpc.rx().try_iter().collect();
    let closed = queued
        .iter()
        .position(|message| {
            matches!(
                message,
                ProxyRpc::Notification(ProxyNotification::CloseEditorBuffer { .. })
            )
        })
        .expect("release the buffer");
    let shutdown = queued
        .iter()
        .position(|message| {
            matches!(
                message,
                ProxyRpc::Notification(ProxyNotification::Shutdown {})
            )
        })
        .expect("stop this window's proxy");
    assert!(closed < shutdown);
    assert!(matches!(queued.last(), Some(ProxyRpc::Shutdown)));
    proxy.shutdown();
    assert!(
        rpc.rx().try_iter().next().is_none(),
        "shutdown is idempotent"
    );
}

#[gpui_kit::test(iterations = 5)]
fn quit_revalidates_earlier_windows_after_later_prompts(cx: &mut TestAppContext) {
    let first_workspace = tempfile::tempdir().expect("workspace");
    let second_workspace = tempfile::tempdir().expect("workspace");
    let mut second_context = cx.clone();
    let (first, first_cx, first_proxy) =
        open_shell(first_workspace.path(), &["main.ts"], cx);
    let (second, second_cx, second_proxy) =
        open_shell(second_workspace.path(), &["main.py"], &mut second_context);
    first_proxy.enable_editor_recovery();
    second_proxy.enable_editor_recovery();
    edit(&first, 0, "first unsaved\n", first_cx);
    edit(&second, 0, "second unsaved\n", second_cx);
    first_cx.update(|window, cx| {
        window.activate_window();
        request_quit(&Quit, cx);
    });
    first_cx.run_until_parked();
    first_cx.simulate_prompt_answer("Discard Changes");
    first_cx.run_until_parked();
    assert!(!first.update(first_cx, |shell, _| shell.closing_code_tabs));
    assert!(second.update(second_cx, |shell, _| shell.closing_code_tabs));
    edit(&first, 0, "typed after agreeing to discard\n", first_cx);
    second_cx.simulate_prompt_answer("Discard Changes");
    second_cx.run_until_parked();
    assert_eq!(second_cx.cx.read(|cx| cx.windows().len()), 2);
    for shell in [first, second] {
        assert!(shell.update(second_cx, |shell, cx| {
            !shell.window_close_authorized
                && shell.status_message.contains("changed")
                && shell.code_tabs[0].read(cx).dirty
                && shell.code_tabs[0].read(cx).proxy.is_some()
        }));
    }
    assert!(!second_cx.cx.read(|cx| cx.global::<QuitInProgress>().0));
    for proxy in [first_proxy, second_proxy] {
        let writes = recovery_writes(&proxy.rpc_for_test());
        assert_eq!(writes.len(), 1);
        assert!(
            writes[0].1.contents.is_some(),
            "do not clear any backup after a stale approval"
        );
    }
}

#[gpui_kit::test(iterations = 5)]
fn failed_settings_save_prevents_window_close_until_retry(cx: &mut TestAppContext) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, proxy) = open_shell(workspace.path(), &["main.py"], cx);
    let settings = shell.update(cx, |shell, _| shell.settings.clone());
    let path = workspace.path().join(".ahead/settings.toml");
    let original = std::fs::read_to_string(&path).expect("settings");
    std::fs::write(&path, "[invalid\n").expect("malformed settings");
    settings.update_in(cx, |settings, window, cx| {
        settings.add_connection(window, cx)
    });
    shell.update_in(cx, |shell, window, cx| {
        shell.request_window_close(window, cx)
    });
    cx.run_until_parked();
    cx.background_executor
        .advance_clock(std::time::Duration::from_millis(10));
    cx.run_until_parked();
    assert_eq!(cx.cx.read(|cx| cx.windows().len()), 1);
    assert!(shell.update(cx, |shell, cx| {
        !shell.window_close_authorized
            && !shell.closing_code_tabs
            && shell.status_message.contains("Settings were not saved")
            && shell.code_tabs[0].read(cx).proxy.is_some()
    }));
    assert!(
        !proxy
            .rpc_for_test()
            .rx()
            .try_iter()
            .any(|message| matches!(message, ProxyRpc::Shutdown))
    );

    std::fs::write(&path, original).expect("repair settings");
    settings.update_in(cx, |settings, window, cx| settings.save_config(window, cx));
    shell.update_in(cx, |shell, window, cx| {
        shell.request_window_close(window, cx)
    });
    cx.run_until_parked();
    cx.background_executor
        .advance_clock(std::time::Duration::from_millis(10));
    cx.run_until_parked();
    assert!(cx.cx.read(|cx| cx.windows().is_empty()));
    assert!(
        std::fs::read_to_string(path)
            .expect("saved settings")
            .contains("Server 2")
    );
}

#[gpui_kit::test(iterations = 5)]
fn quit_revalidates_settings_after_preparing_an_earlier_window(
    cx: &mut TestAppContext,
) {
    let first_workspace = tempfile::tempdir().expect("workspace");
    let second_workspace = tempfile::tempdir().expect("workspace");
    let mut second_context = cx.clone();
    let (first, first_cx, _) = open_shell(first_workspace.path(), &["main.ts"], cx);
    let (second, second_cx, _) =
        open_shell(second_workspace.path(), &["main.py"], &mut second_context);
    edit(&second, 0, "unsaved\n", second_cx);
    first_cx.update(|window, cx| {
        window.activate_window();
        request_quit(&Quit, cx);
    });
    first_cx.run_until_parked();
    assert!(!first.update(first_cx, |shell, _| shell.closing_code_tabs));
    assert!(second.update(second_cx, |shell, _| shell.closing_code_tabs));
    let settings = first.update(first_cx, |shell, _| shell.settings.clone());
    settings.update_in(first_cx, |settings, window, cx| {
        settings.add_connection(window, cx)
    });
    second_cx.simulate_prompt_answer("Discard Changes");
    second_cx.run_until_parked();
    assert_eq!(second_cx.cx.read(|cx| cx.windows().len()), 2);
    for shell in [first, second] {
        assert!(shell.update(second_cx, |shell, cx| {
            !shell.window_close_authorized
                && shell.status_message.contains("settings changed")
                && shell.code_tabs[0].read(cx).proxy.is_some()
        }));
    }
    assert!(!second_cx.cx.read(|cx| cx.global::<QuitInProgress>().0));
}

fn recovery_writes(
    rpc: &ProxyRpcHandler,
) -> Vec<(ahead_rpc::RequestId, EditorRecoverySnapshot)> {
    rpc.rx()
        .try_iter()
        .filter_map(|message| match message {
            ProxyRpc::Request(
                id,
                ProxyRequest::AheadRequest {
                    request: AheadRequest::WriteEditorRecovery { snapshot },
                },
            ) => Some((id, snapshot)),
            _ => None,
        })
        .collect()
}

fn recovery_response(
    rpc: &ProxyRpcHandler,
    id: ahead_rpc::RequestId,
    value: serde_json::Value,
) {
    rpc.handle_response(id, Ok(ProxyResponse::AheadResponse { response: value }));
}

fn offer_recovery(
    shell: &Entity<Shell>,
    rpc: &ProxyRpcHandler,
    snapshot: &EditorRecoverySnapshot,
    cx: &mut VisualTestContext,
) -> ahead_rpc::RequestId {
    shell.update_in(cx, |shell, window, cx| {
        shell.restore_unsaved_buffers(window, cx)
    });
    cx.run_until_parked();
    let list = take_request(rpc, |request| {
        matches!(
            request,
            ProxyRequest::AheadRequest {
                request: AheadRequest::ListEditorRecoveries
            }
        )
    });
    recovery_response(
        rpc,
        list,
        serde_json::json!([{ "buffer_id": snapshot.buffer_id, "path": snapshot.path, "revision": snapshot.revision }]),
    );
    cx.run_until_parked();
    take_request(rpc, |request| {
        matches!(
            request,
            ProxyRequest::AheadRequest {
                request: AheadRequest::ReadEditorRecovery { .. }
            }
        )
    })
}

#[gpui_kit::test(iterations = 5)]
async fn recovery_late_restore_preserves_typing_and_can_reopen_after_last_tab_closes(
    cx: &mut TestAppContext,
) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, proxy) = open_shell(workspace.path(), &["main.py"], cx);
    let rpc = proxy.rpc_for_test();
    let snapshot = EditorRecoverySnapshot {
        buffer_id: uuid::Uuid::new_v4().to_string(),
        revision: 8,
        path: "main.py".into(),
        contents: Some("recovered Python\n".into()),
        saved_sha256: None,
    };
    let read = offer_recovery(&shell, &rpc, &snapshot, cx);
    edit(&shell, 0, "typing while recovery loads\n", cx);
    recovery_response(&rpc, read, serde_json::json!(snapshot));
    cx.run_until_parked();
    assert!(shell.update(cx, |shell, cx| {
        shell.code.read(cx).editor.read(cx).text().to_string()
            == "typing while recovery loads\n"
            && shell.status_message.contains("Additional recovery")
    }));
    let close = shell.update_in(cx, |shell, window, cx| {
        shell.close_confirmed_code_tab(
            shell.code.entity_id(),
            shell.code.read(cx).request_generation,
            window,
            cx,
        )
    });
    let clear = recovery_writes(&rpc)
        .pop()
        .expect("clear current buffer only");
    assert_ne!(clear.1.buffer_id, snapshot.buffer_id);
    recovery_response(&rpc, clear.0, serde_json::json!(true));
    close.await;
    assert!(shell.update(cx, |shell, _| shell.code_tabs.is_empty()));
    let read = offer_recovery(&shell, &rpc, &snapshot, cx);
    recovery_response(&rpc, read, serde_json::json!(snapshot));
    cx.run_until_parked();
    assert!(shell.update(cx, |shell, cx| shell.code_tabs.len() == 1
        && shell.code.read(cx).dirty
        && shell.code.read(cx).editor.read(cx).text().to_string()
            == "recovered Python\n"));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("main.py")).expect("disk"),
        "saved\n"
    );
}

#[gpui_kit::test(iterations = 5)]
fn recovery_restores_missing_files_without_recreating_them(cx: &mut TestAppContext) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, proxy) = open_shell(workspace.path(), &["main.ts"], cx);
    let rpc = proxy.rpc_for_test();
    let snapshot = EditorRecoverySnapshot {
        buffer_id: uuid::Uuid::new_v4().to_string(),
        revision: 2,
        path: "missing.py".into(),
        contents: Some("unsaved missing file\n".into()),
        saved_sha256: None,
    };
    let read = offer_recovery(&shell, &rpc, &snapshot, cx);
    recovery_response(&rpc, read, serde_json::json!(snapshot));
    cx.run_until_parked();
    assert!(shell.update(cx, |shell, cx| shell.code_tabs.len() == 2
        && shell.code.read(cx).dirty
        && shell.code.read(cx).editor.read(cx).text().to_string()
            == "unsaved missing file\n"));
    assert!(!workspace.path().join("missing.py").exists());
}

#[gpui_kit::test(iterations = 5)]
fn recovery_close_timeout_keeps_window_and_fences_late_clear(
    cx: &mut TestAppContext,
) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, proxy) = open_shell(workspace.path(), &["main.py"], cx);
    proxy.enable_editor_recovery();
    let rpc = proxy.rpc_for_test();
    edit(&shell, 0, "keep after timeout\n", cx);
    shell.update_in(cx, |shell, window, cx| {
        shell.request_window_close(window, cx)
    });
    cx.run_until_parked();
    cx.simulate_prompt_answer("Discard Changes");
    cx.run_until_parked();
    let clear = recovery_writes(&rpc).pop().expect("pending clear");
    cx.background_executor
        .advance_clock(std::time::Duration::from_secs(30));
    cx.run_until_parked();
    assert_eq!(cx.cx.read(|cx| cx.windows().len()), 1);
    assert!(shell.update(cx, |shell, cx| shell.code.read(cx).dirty
        && shell.status_message.contains("not confirmed")));
    let backup = recovery_writes(&rpc).pop().expect("restore retained text");
    assert!(backup.1.revision > clear.1.revision);
    assert_eq!(backup.1.contents.as_deref(), Some("keep after timeout\n"));
    recovery_response(&rpc, backup.0, serde_json::json!(true));
    recovery_response(&rpc, clear.0, serde_json::json!(true));
    cx.run_until_parked();
    assert!(!shell.update(cx, |shell, _| shell.window_close_authorized));
    shell.update_in(cx, |shell, window, cx| {
        shell.request_window_close(window, cx)
    });
    cx.run_until_parked();
    cx.simulate_prompt_answer("Discard Changes");
    cx.run_until_parked();
    let clear = recovery_writes(&rpc).pop().expect("current clear");
    recovery_response(&rpc, clear.0, serde_json::json!(true));
    cx.run_until_parked();
    assert!(cx.cx.read(|cx| cx.windows().is_empty()));
}

#[gpui_kit::test(iterations = 5)]
fn recovery_coalesces_edits_and_surfaces_rejected_writes(cx: &mut TestAppContext) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, proxy) = open_shell(workspace.path(), &["main.ts"], cx);
    proxy.enable_editor_recovery();
    let rpc = proxy.rpc_for_test();
    let code = shell.update(cx, |shell, _| shell.code.clone());
    edit(&shell, 0, "first edit\n", cx);
    code.update(cx, |code, cx| code.poll_recovery(cx));
    let first = recovery_writes(&rpc).pop().expect("first backup");
    assert_eq!(first.1.path, Path::new("main.ts"));
    assert_eq!(first.1.contents.as_deref(), Some("first edit\n"));
    edit(&shell, 0, "latest edit\n", cx);
    code.update(cx, |code, cx| code.poll_recovery(cx));
    assert!(
        recovery_writes(&rpc).is_empty(),
        "one periodic write in flight"
    );
    recovery_response(&rpc, first.0, serde_json::json!(true));
    code.update(cx, |code, cx| code.poll_recovery(cx));
    let latest = recovery_writes(&rpc).pop().expect("coalesced backup");
    assert_eq!(latest.1.contents.as_deref(), Some("latest edit\n"));
    assert!(latest.1.revision > first.1.revision);
    recovery_response(&rpc, latest.0, serde_json::json!(false));
    code.update(cx, |code, cx| code.poll_recovery(cx));
    assert!(code.update(cx, |code, _| code.status.contains("rejected")));
    assert!(
        recovery_writes(&rpc).is_empty(),
        "no failed-write busy loop"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("main.ts")).expect("disk"),
        "saved\n"
    );
}

#[gpui_kit::test(iterations = 5)]
fn recovery_quit_waits_for_clear_and_preserves_new_edits_or_failed_clear(
    cx: &mut TestAppContext,
) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, proxy) = open_shell(workspace.path(), &["main.py"], cx);
    #[cfg(unix)]
    let terminal = add_terminal(&shell, workspace.path(), cx);
    proxy.enable_editor_recovery();
    let rpc = proxy.rpc_for_test();
    edit(&shell, 0, "approved for discard\n", cx);
    cx.update(|_, cx| request_quit(&Quit, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Discard Changes");
    cx.run_until_parked();
    let clear = recovery_writes(&rpc).pop().expect("clear backup");
    assert!(clear.1.contents.is_none());
    assert!(!shell.update(cx, |shell, _| shell.window_close_authorized));
    edit(&shell, 0, "new typing during clear\n", cx);
    recovery_response(&rpc, clear.0, serde_json::json!(true));
    cx.run_until_parked();
    assert!(!shell.update(cx, |shell, _| shell.window_close_authorized));
    let newer = recovery_writes(&rpc).pop().expect("restore newer backup");
    assert!(newer.1.revision > clear.1.revision);
    assert_eq!(
        newer.1.contents.as_deref(),
        Some("new typing during clear\n")
    );
    recovery_response(&rpc, newer.0, serde_json::json!(true));

    cx.update(|_, cx| request_quit(&Quit, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Discard Changes");
    cx.run_until_parked();
    let clear = recovery_writes(&rpc).pop().expect("retry clear");
    rpc.handle_response(
        clear.0,
        Err(ahead_rpc::RpcError {
            code: 1,
            message: "recovery storage unavailable".into(),
        }),
    );
    cx.run_until_parked();
    assert!(shell.update(cx, |shell, cx| !shell.window_close_authorized
        && shell.code.read(cx).dirty
        && shell.status_message.contains("storage unavailable")));
    #[cfg(unix)]
    assert!(!terminal.read_with(cx, |terminal, _| terminal.is_shutdown()));
    let retained = recovery_writes(&rpc).pop().expect("retain backup");
    recovery_response(&rpc, retained.0, serde_json::json!(true));

    cx.update(|_, cx| request_quit(&Quit, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Discard Changes");
    cx.run_until_parked();
    let clear = recovery_writes(&rpc).pop().expect("confirmed clear");
    recovery_response(&rpc, clear.0, serde_json::json!(true));
    cx.run_until_parked();
    assert!(shell.update(cx, |shell, _| shell.window_close_authorized));
    #[cfg(unix)]
    assert!(terminal.read_with(cx, |terminal, _| terminal.is_shutdown()));
    assert!(
        rpc.rx().try_iter().any(|message| matches!(
            message,
            ProxyRpc::Notification(ProxyNotification::Shutdown {})
        )),
        "quit stops the proxy only after recovery is confirmed"
    );
}

#[gpui_kit::test(iterations = 5)]
fn recovery_restore_never_writes_disk_and_reviews_changed_baseline(
    cx: &mut TestAppContext,
) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, proxy) = open_shell(workspace.path(), &["main.ts"], cx);
    proxy.enable_editor_recovery();
    let rpc = proxy.rpc_for_test();
    let code = shell.update(cx, |shell, _| shell.code.clone());
    let snapshot = EditorRecoverySnapshot {
        buffer_id: uuid::Uuid::new_v4().to_string(),
        revision: 7,
        path: "main.ts".into(),
        contents: Some("recovered TS\n".into()),
        saved_sha256: None,
    };
    code.update_in(cx, |code, window, cx| {
        code.restore_recovery(snapshot, window, cx)
    })
    .expect("restore");
    assert!(code.update(cx, |code, cx| code.dirty
        && code.editor.read(cx).text().to_string() == "recovered TS\n"));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("main.ts")).expect("disk"),
        "saved\n"
    );
    code.update_in(cx, |code, window, cx| code.save(window, cx));
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(!rpc.rx().try_iter().any(|message| matches!(
        message,
        ProxyRpc::Request(_, ProxyRequest::SaveEditorBuffer { .. })
    )));
    code.update_in(cx, |code, window, cx| code.save(window, cx));
    edit(&shell, 0, "edited during overwrite prompt\n", cx);
    cx.simulate_prompt_answer("Overwrite");
    cx.run_until_parked();
    assert!(!rpc.rx().try_iter().any(|message| matches!(
        message,
        ProxyRpc::Request(_, ProxyRequest::SaveEditorBuffer { .. })
    )));
    code.update_in(cx, |code, window, cx| code.save(window, cx));
    cx.simulate_prompt_answer("Overwrite");
    cx.run_until_parked();
    let save = take_request(&rpc, |request| {
        matches!(request, ProxyRequest::SaveEditorBuffer { .. })
    });
    assert!(code.update(cx, |code, _| code.dirty));
    rpc.handle_response(save, Ok(ProxyResponse::SaveResponse {}));
    cx.run_until_parked();
    assert!(!code.update(cx, |code, _| code.dirty));
    code.update(cx, |code, cx| code.poll_recovery(cx));
    let clear = recovery_writes(&rpc).pop().expect("saved backup clear");
    assert!(clear.1.contents.is_none());
    assert!(clear.1.revision > 7);
    recovery_response(&rpc, clear.0, serde_json::json!(true));
}

#[gpui_kit::test(iterations = 5)]
fn trash_timeout_retains_buffers_and_does_not_apply_late_reply(
    cx: &mut TestAppContext,
) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, proxy) = open_shell(workspace.path(), &["main.py"], cx);
    edit(&shell, 0, "unsaved\n", cx);
    shell.update_in(cx, |shell, window, cx| {
        shell.trash_workspace_path(workspace.path().join("main.py"), window, cx)
    });
    cx.simulate_prompt_answer("Move to Trash");
    cx.run_until_parked();
    let rpc = proxy.rpc_for_test();
    let request = take_request(&rpc, |request| {
        matches!(request, ProxyRequest::TrashPath { .. })
    });
    cx.background_executor
        .advance_clock(std::time::Duration::from_secs(30));
    cx.run_until_parked();
    assert!(shell.update(cx, |shell, cx| {
        !shell.closing_code_tabs
            && shell.status_message.contains("timed out")
            && shell.code_tabs[0].read(cx).dirty
    }));
    rpc.handle_response(request, Ok(ProxyResponse::Success {}));
    cx.run_until_parked();
    assert_eq!(shell.update(cx, |shell, _| shell.code_tabs.len()), 1);
    assert!(shell.update(cx, |shell, cx| shell.code_tabs[0].read(cx).dirty));
}

#[gpui_kit::test(iterations = 5)]
async fn opening_a_tab_during_window_close_cancels_the_original_snapshot(
    cx: &mut TestAppContext,
) {
    let workspace = tempfile::tempdir().expect("workspace");
    let (shell, cx, _) = open_shell(workspace.path(), &["main.ts"], cx);
    edit(&shell, 0, "unsaved\n", cx);
    let preparation = shell.update_in(cx, |shell, window, cx| {
        shell.prepare_window_close(window, cx)
    });
    cx.run_until_parked();
    let path = workspace.path().join("opened.py");
    std::fs::write(&path, "saved\n").expect("test source");
    shell.update_in(cx, |shell, window, cx| {
        shell
            .code_tabs
            .push(cx.new(|cx| CodePanel::new(&path.to_string_lossy(), window, cx)));
    });
    edit(&shell, 1, "new tab edits\n", cx);
    cx.simulate_prompt_answer("Discard Changes");
    assert!(preparation.await.is_none());
    assert!(shell.update(cx, |shell, cx| {
        shell.code_tabs.len() == 2
            && shell.code_tabs.iter().all(|code| code.read(cx).dirty)
            && !shell.closing_code_tabs
    }));
}
