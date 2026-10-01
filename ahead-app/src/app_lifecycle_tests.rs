use super::{
    AgentWorkspacePanel, CloseWindow, Quit, QuitInProgress, Shell,
    debug_terminal_command, request_close_window, request_quit,
};
use crate::code_panel::CodePanel;
use crate::proxy_client::ProxyClient;
use crate::workspace_panels::{
    ActivityBar, GitPanel, JustTasksPanel, LanguageServersPanel, ProblemsPanel,
    SearchPanel,
};
use ahead_rpc::ahead::AheadRequest;
use ahead_rpc::file::EditorRecoverySnapshot;
use ahead_rpc::proxy::{
    ProxyNotification, ProxyRequest, ProxyResponse, ProxyRpc, ProxyRpcHandler,
};
use gpui_kit::component::dock::DockSkin;
use gpui_kit::{AppContext, Entity, TestAppContext, VisualTestContext};
use std::path::Path;
use std::sync::Arc;

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
        let debug = cx.new(|cx| {
            crate::debug_bar::DebugBar::new(proxy.clone(), &paths[0], window, cx)
        });
        let git = cx.new(|cx| GitPanel::new(&root_path, window, cx));
        let tasks = cx.new(|cx| JustTasksPanel::new(&root_path, cx));
        let languages =
            cx.new(|cx| LanguageServersPanel::new(&root_path, proxy.clone(), cx));
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
