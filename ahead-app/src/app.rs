//! AHEAD Application Shell - GPUI Application Lifecycle
//!
//! 4-column Zed-style layout:
//! - Left dock: File explorer (toggleable via ⌘B or bottom icon)
//! - Center: Code editor and settings tabs
//! - Bottom dock: interactive PTY terminal and Zed-style debugger tabs (toggleable via ⌃` / ⌘J)
//! - Right dock: AHEAD Agent conversation and Threads rail together (toggleable via ⌥⌘B or bottom icon)
//! - Top: TitleBar chrome with brand and native traffic lights
//! - Bottom: StatusBar with shortcut tooltips, branch, dock toggles, and metadata

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui_kit::component::TitleBar;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, DockArea, DockLayout, DockPlacement, DockSkin, Panel, PanelControl,
    PanelEvent, PanelStyle, panel_handle,
};
use gpui_kit::component::input::RopeExt;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, WindowExt, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit_assets::IconName;
use sha2::Digest;

use crate::proxy_client::{DebugTerminalEvent, EditorPresentationRequest};
use crate::workspace_panels::{
    ActivityBar, GitPanel, JustTasksPanel, LanguageServersPanel, ProblemsPanel,
    SearchPanel, SearchReplaceRequest, WorkspaceView,
};

gpui_kit::actions!(
    ahead,
    [
        Quit,
        CloseWindow,
        NewWindow,
        OpenFile,
        OpenFolder,
        RecoverUnsavedChanges,
        OpenHelp,
        SaveFile,
        GoToFile,
        GoToLine,
        GoToDefinition,
        FindReferences,
        GoToImplementation,
        GoBack,
        GoForward,
        RenameSymbol,
        FormatDocument,
        ShowCompletions,
        ShowSignatureHelp,
        ShowCodeActions,
        NextSnippetTabstop,
        PreviousSnippetTabstop,
        StartDebugging,
        StopDebugging,
        ToggleBreakpoint,
        StepOver,
        StepInto,
        StepOut,
        MinimizeWindow,
        ZoomWindow,
    ]
);

#[derive(Default)]
struct QuitInProgress(bool);
impl Global for QuitInProgress {}

#[derive(Default)]
struct PendingTerminalShutdowns(Vec<crossbeam_channel::Receiver<()>>);
impl Global for PendingTerminalShutdowns {}

fn shutdown_terminal(
    terminal: &Entity<crate::terminal_panel::TerminalPanel>,
    cx: &mut App,
) {
    if let Some(complete) = terminal.update(cx, |terminal, cx| terminal.shutdown(cx))
    {
        let pending = cx.default_global::<PendingTerminalShutdowns>();
        pending.0.retain(|reply| {
            matches!(
                reply.try_recv(),
                Err(crossbeam_channel::TryRecvError::Empty)
            )
        });
        pending.0.push(complete);
    }
}

async fn wait_for_terminal_shutdowns(
    mut replies: Vec<crossbeam_channel::Receiver<()>>,
    executor: &BackgroundExecutor,
) {
    for poll in 0..=30 {
        replies.retain(|reply| {
            matches!(
                reply.try_recv(),
                Err(crossbeam_channel::TryRecvError::Empty)
            )
        });
        if replies.is_empty() {
            return;
        }
        if poll < 30 {
            executor.timer(std::time::Duration::from_millis(5)).await;
        }
    }
    eprintln!(
        "{} terminals did not finish cleanup before the quit deadline",
        replies.len()
    );
}

#[derive(PartialEq, Eq)]
struct WindowCloseSnapshot {
    buffers: Vec<(EntityId, u64)>,
    settings_generation: u64,
}
type RecoveryReply = async_channel::Receiver<Result<bool, String>>;

#[derive(Debug)]
struct PreparedWorkspaceDocument {
    path: std::path::PathBuf,
    text: String,
    edits: Vec<lsp_types::TextEdit>,
}

fn canonical_workspace_file(
    workspace: &std::path::Path,
    path: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let path = path
        .canonicalize()
        .map_err(|error| format!("Could not resolve {}: {error}", path.display()))?;
    if !path.starts_with(workspace) {
        return Err(format!(
            "The language server returned a file outside the workspace: {}",
            path.display()
        ));
    }
    if !path.is_file() {
        return Err(format!(
            "Workspace edit target is not a file: {}",
            path.display()
        ));
    }
    Ok(path)
}

fn same_file_path(left: &std::path::Path, right: &std::path::Path) -> bool {
    left == right
        || left
            .canonicalize()
            .ok()
            .zip(right.canonicalize().ok())
            .is_some_and(|(left, right)| left == right)
}

fn prepare_workspace_documents(
    workspace: &std::path::Path,
    documents: Vec<crate::code_panel::WorkspaceTextEdits>,
    open_documents: &[lsp_types::TextDocumentItem],
    buffers: &[crate::code_panel::WorkspaceBufferSnapshot],
) -> Result<Vec<PreparedWorkspaceDocument>, String> {
    let workspace = workspace
        .canonicalize()
        .map_err(|error| format!("Could not resolve workspace root: {error}"))?;
    let mut open_documents_by_path = std::collections::HashMap::new();
    for document in open_documents {
        let Ok(path) = document.uri.to_file_path() else {
            continue;
        };
        if let Ok(path) = canonical_workspace_file(&workspace, &path) {
            open_documents_by_path.insert(path, document);
        }
    }
    let mut buffers_by_path = std::collections::HashMap::new();
    for buffer in buffers {
        if let Ok(path) = canonical_workspace_file(&workspace, &buffer.path) {
            buffers_by_path.insert(path, buffer);
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut prepared = Vec::with_capacity(documents.len());
    for document in documents {
        let path = canonical_workspace_file(&workspace, &document.path)?;
        if !seen.insert(path.clone()) {
            return Err(format!(
                "The workspace edit refers to the same canonical file more than once: {}",
                path.display()
            ));
        }
        let open_document = open_documents_by_path.get(&path).copied();
        if let Some(expected_version) = document.version {
            let Some(open_document) = open_document else {
                return Err(format!(
                    "The language server's versioned document is no longer open: {}",
                    path.display()
                ));
            };
            if open_document.version != expected_version {
                return Err(format!(
                    "The file changed in the language server before the workspace edit could be applied: {}",
                    path.display()
                ));
            }
        }

        let buffer = buffers_by_path.get(&path).copied();
        let text = if let Some(buffer) = buffer {
            if !buffer.editable {
                return Err(format!(
                    "Workspace edit targets a read-only shared file: {}",
                    path.display()
                ));
            }
            if open_document.is_some_and(|document| document.text != buffer.text) {
                return Err(format!(
                    "The editor and language server have different buffer contents: {}",
                    path.display()
                ));
            }
            buffer.text.clone()
        } else {
            if open_document.is_some() {
                return Err(format!(
                    "The language server has an open buffer that AHEAD cannot safely edit: {}",
                    path.display()
                ));
            }
            std::fs::read_to_string(&path).map_err(|error| {
                format!("Could not read {}: {error}", path.display())
            })?
        };
        let selection = buffer
            .map(|buffer| buffer.selection.clone())
            .unwrap_or(0..0);
        if crate::code_panel::formatting_edit(&text, &document.edits, selection)?
            .is_some()
        {
            prepared.push(PreparedWorkspaceDocument {
                path,
                text,
                edits: document.edits,
            });
        }
    }
    Ok(prepared)
}

#[cfg(test)]
mod workspace_edit_tests {
    use super::prepare_workspace_documents;
    use crate::code_panel::{WorkspaceBufferSnapshot, WorkspaceTextEdits};

    #[test]
    fn workspace_edit_preflight_checks_versions_and_read_only_buffers() {
        use lsp_types::{Position, Range, TextDocumentItem, TextEdit, Url};

        let workspace = tempfile::tempdir().expect("workspace");
        let path = workspace.path().join("main.rs");
        let text = "let item = 1;\n";
        std::fs::write(&path, text).expect("source file");
        let uri = Url::from_file_path(&path).expect("file URI");
        let edits = vec![WorkspaceTextEdits {
            path: path.clone(),
            version: Some(5),
            edits: vec![TextEdit {
                range: Range::new(Position::new(0, 4), Position::new(0, 8)),
                new_text: "value".into(),
            }],
        }];
        let open_documents = vec![TextDocumentItem {
            uri: uri.clone(),
            language_id: "rust".into(),
            version: 5,
            text: text.into(),
        }];
        let buffers = vec![WorkspaceBufferSnapshot {
            path: path.clone(),
            text: text.into(),
            selection: 0..0,
            editable: true,
        }];
        let prepared = prepare_workspace_documents(
            workspace.path(),
            edits.clone(),
            &open_documents,
            &buffers,
        )
        .expect("fresh versioned edit");
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].text, text);

        let stale_documents = vec![TextDocumentItem {
            uri,
            language_id: "rust".into(),
            version: 4,
            text: text.into(),
        }];
        assert!(
            prepare_workspace_documents(
                workspace.path(),
                edits.clone(),
                &stale_documents,
                &buffers,
            )
            .expect_err("stale document version")
            .contains("changed in the language server")
        );

        let read_only_buffers = vec![WorkspaceBufferSnapshot {
            editable: false,
            ..buffers[0].clone()
        }];
        assert!(
            prepare_workspace_documents(
                workspace.path(),
                edits,
                &open_documents,
                &read_only_buffers,
            )
            .expect_err("read-only editor")
            .contains("read-only shared file")
        );
    }
}

async fn confirm_recovery_writes(
    replies: Vec<RecoveryReply>,
    executor: &BackgroundExecutor,
) -> Result<(), String> {
    for reply in replies {
        if !crate::proxy_client::await_editor_recovery(reply, executor).await? {
            return Err(
                "The recovery snapshot changed before it could be cleared".into()
            );
        }
    }
    Ok(())
}

fn request_recovery(_: &RecoverUnsavedChanges, cx: &mut App) {
    let active = cx.active_window();
    cx.defer(move |cx| {
        if let Some((window, shell)) = editor_windows(cx)
            .into_iter()
            .find(|(window, _)| Some(*window) == active)
        {
            if let Err(error) = window.update(cx, |_, window, cx| {
                shell.update(cx, |shell, cx| {
                    shell.restore_unsaved_buffers(window, cx)
                })
            }) {
                eprintln!("Opening recovered buffers: {error}");
            }
        }
    });
}

fn editor_windows(cx: &App) -> Vec<(AnyWindowHandle, Entity<Shell>)> {
    cx.windows()
        .into_iter()
        .filter_map(|window| {
            let root = window
                .downcast::<gpui_kit::component::Root>()?
                .read(cx)
                .ok()?;
            let shell = root.view().clone().downcast::<Shell>().ok()?;
            Some((window, shell))
        })
        .collect()
}

fn request_quit(_: &Quit, cx: &mut App) {
    if cx
        .try_global::<QuitInProgress>()
        .is_some_and(|state| state.0)
    {
        return;
    }
    cx.set_global(QuitInProgress(true));
    cx.spawn(async move |cx| {
        // Action dispatch can temporarily take the active window out of App.
        let windows = cx.update(|cx| {
            let mut windows = editor_windows(cx);
            let active = cx.active_window();
            windows.sort_by_key(|(window, _)| Some(*window) != active);
            windows
        });
        let mut prepared = Vec::new();
        for (window, shell) in windows {
            let task = window.update(cx, |_, window, cx| shell.update(cx, |shell, cx| shell.prepare_window_close(window, cx)));
            let snapshot = match task {
                Ok(task) => task.await,
                Err(error) => { eprintln!("Preparing window to quit: {error}"); None }
            };
            let Some(snapshot) = snapshot else {
                cx.update(|cx| cx.set_global(QuitInProgress(false)));
                return;
            };
            prepared.push((window, shell, snapshot));
        }
        let replies = cx.update(|cx| {
            let current = editor_windows(cx);
            let unchanged = current.len() == prepared.len() && prepared.iter().all(|(window, shell, snapshot)| {
                current.iter().any(|(current, _)| current == window) && shell.read(cx).window_close_snapshot(cx) == *snapshot
            });
            unchanged.then(|| prepared.iter().flat_map(|(_, shell, _)| {
                shell.update(cx, |shell, cx| shell.queue_recoveries(true, cx))
            }).collect())
        });
        let recovery_result = match replies {
            Some(replies) => confirm_recovery_writes(replies, cx.background_executor()).await,
            None => Err("Files or settings changed while preparing to quit. Review them and try again.".into()),
        };
        cx.update(|cx| {
            let current = editor_windows(cx);
            let unchanged = current.len() == prepared.len() && prepared.iter().all(|(window, shell, snapshot)| {
                current.iter().any(|(current, _)| current == window) && shell.read(cx).window_close_snapshot(cx) == *snapshot
            });
            if unchanged && recovery_result.is_ok() {
                for (_, shell, _) in prepared {
                    shell.update(cx, |shell, cx| {
                        shell.window_close_authorized = true;
                        if cfg!(debug_assertions) {
                            eprintln!("AHEAD application quit: shutting down workspace proxy");
                        }
                        shell.shutdown_workspace(cx);
                    });
                }
                cx.quit();
            } else {
                for (_, shell) in current {
                    shell.update(cx, |shell, cx| {
                        for code in &shell.code_tabs { code.update(cx, |code, cx| code.refresh_recovery(cx)); }
                        shell.status_message = recovery_result.as_ref().err().cloned().unwrap_or_else(|| "Files or settings changed while preparing to quit. Review them and try again.".into());
                        cx.notify();
                    });
                }
            }
            cx.set_global(QuitInProgress(false));
        });
    }).detach();
}

fn request_close_window(_: &CloseWindow, cx: &mut App) {
    let active = cx.active_window();
    cx.defer(move |cx| {
        if let Some((window, shell)) = editor_windows(cx)
            .into_iter()
            .find(|(window, _)| Some(*window) == active)
        {
            if let Err(error) = window.update(cx, |_, window, cx| {
                shell.update(cx, |shell, cx| shell.request_window_close(window, cx))
            }) {
                eprintln!("Closing window: {error}");
            }
        }
    });
}

fn launch_ahead_with_path(path: std::path::PathBuf) -> std::io::Result<()> {
    // ponytail: spawn per workspace until the project window factory is reusable in-process.
    std::process::Command::new(std::env::current_exe()?)
        .arg(path)
        .spawn()
        .map(|_| ())
}

fn request_open_path(options: PathPromptOptions, cx: &mut App) {
    let selection = cx.prompt_for_paths(options);
    cx.spawn(async move |cx| {
        let path = match selection.await {
            Ok(Ok(Some(paths))) => paths.into_iter().next(),
            Ok(Ok(None)) => None,
            Ok(Err(error)) => {
                eprintln!("AHEAD path picker failed: {error}");
                None
            }
            Err(_) => {
                eprintln!("AHEAD path picker was interrupted");
                None
            }
        };
        if let Some(path) = path {
            let result = cx
                .background_spawn(async move { launch_ahead_with_path(path) })
                .await;
            if let Err(error) = result {
                eprintln!("AHEAD could not open the selected path: {error}");
            }
        }
    })
    .detach();
}

fn request_new_window(_: &NewWindow, cx: &mut App) {
    let active = cx.active_window();
    let workspace = editor_windows(cx)
        .into_iter()
        .find(|(window, _)| Some(*window) == active)
        .map(|(_, shell)| shell.read(cx).explorer.read(cx).root.clone());
    if let Some(workspace) = workspace
        && let Err(error) = launch_ahead_with_path(workspace.into())
    {
        eprintln!("AHEAD could not open a new window: {error}");
    }
}

fn request_shell_command(command: ShellShortcut, cx: &mut App) {
    let active = cx.active_window();
    cx.defer(move |cx| {
        if let Some((window, shell)) = editor_windows(cx)
            .into_iter()
            .find(|(window, _)| Some(*window) == active)
        {
            if let Err(error) = window.update(cx, |_, window, cx| {
                shell.update(cx, |shell, cx| {
                    shell.run_shell_command(command, window, cx);
                })
            }) {
                eprintln!("AHEAD could not run menu command: {error}");
            }
        }
    });
}

fn minimize_window(_: &MinimizeWindow, cx: &mut App) {
    let active = cx.active_window();
    cx.defer(move |cx| {
        if let Some(window) = active {
            if let Err(error) = window.update(cx, |_, window, _| {
                window.minimize_window();
            }) {
                eprintln!("AHEAD could not minimize the window: {error}");
            }
        }
    });
}

fn zoom_window(_: &ZoomWindow, cx: &mut App) {
    let active = cx.active_window();
    cx.defer(move |cx| {
        if let Some(window) = active {
            if let Err(error) = window.update(cx, |_, window, _| {
                window.zoom_window();
            }) {
                eprintln!("AHEAD could not zoom the window: {error}");
            }
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShellShortcut {
    ToggleLeft,
    ToggleBottom,
    ToggleRight,
    Settings,
    Extensions,
    Help,
    Explorer,
    Search,
    SourceControl,
    StageAllChanges,
    CommitStagedChanges,
    Tasks,
    LanguageServers,
    Problems,
    QuickOpen,
    GoToLine,
    OpenFileDialog,
    OpenFolderDialog,
    SaveActiveFile,
    CloseActiveItem,
    CloseEditor,
    CloseOtherEditors,
    CloseEditorsToLeft,
    CloseEditorsToRight,
    CloseAllEditors,
    MoveEditorToOtherGroup,
    Editor(crate::code_panel::EditorCommand),
    NavigateBack,
    NavigateForward,
    CommandPalette,
    NewTerminal,
    Debug,
    DebugKey(&'static str, bool),
    Agent,
    NewAheadSession,
    NewExternalAgent,
    ChatZoom,
    Threads,
}

fn shell_shortcut(
    key: &str,
    platform: bool,
    control: bool,
    alt: bool,
    shift: bool,
) -> Option<ShellShortcut> {
    use ShellShortcut::*;

    if !platform && !control && !alt {
        match (key, shift) {
            ("f1", false) => return Some(CommandPalette),
            ("f5", false) => return Some(DebugKey("f5", false)),
            ("f9", false) => return Some(DebugKey("f9", false)),
            ("f10", false) => return Some(DebugKey("f10", false)),
            ("f11", false) => return Some(DebugKey("f11", false)),
            ("f5", true) => return Some(DebugKey("f5", true)),
            ("f11", true) => return Some(DebugKey("f11", true)),
            _ => {}
        }
    }

    let primary = platform && (!cfg!(target_os = "macos") || !control);
    if primary && !alt {
        let action = match (key, shift) {
            ("b", false) => ToggleLeft,
            ("j", false) => ToggleBottom,
            (",", false) => Settings,
            ("e", true) => Explorer,
            ("f", true) => Search,
            ("m", true) => Problems,
            ("p", false) => QuickOpen,
            ("p", true) => CommandPalette,
            ("w", false) => CloseActiveItem,
            ("x", true) => Extensions,
            ("d", true) => Debug,
            _ => return control_shortcut(key, platform, control, alt, shift),
        };
        return Some(action);
    }
    if primary && alt && !shift && key == "b" {
        return Some(ToggleRight);
    }
    control_shortcut(key, platform, control, alt, shift)
}

fn control_shortcut(
    key: &str,
    platform: bool,
    control: bool,
    alt: bool,
    shift: bool,
) -> Option<ShellShortcut> {
    use ShellShortcut::*;

    let plain_control = control && (!cfg!(target_os = "macos") || !platform);
    if plain_control && !alt {
        match (key, shift) {
            ("`" | "~", false) => return Some(ToggleBottom),
            ("f4", false) if !cfg!(target_os = "macos") => {
                return Some(CloseActiveItem);
            }
            ("g", true) => return Some(SourceControl),
            _ => {}
        }
    }
    if key == "i"
        && !shift
        && if cfg!(target_os = "macos") {
            control && platform && !alt
        } else {
            control && alt
        }
    {
        return Some(Agent);
    }
    if plain_control && alt && shift {
        return match key {
            "k" => Some(Tasks),
            "l" => Some(LanguageServers),
            "m" => Some(ChatZoom),
            "t" => Some(Threads),
            _ => None,
        };
    }
    None
}

pub(crate) fn shortcut_hint(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        mac
    } else {
        other
    }
}

pub(crate) fn ahead_icon(size: Pixels, cx: &App) -> Icon {
    Icon::default()
        .data(include_bytes!("../../icons/ahead_logo.svg"))
        .size(size)
        .text_color(cx.theme().primary)
}

#[cfg(target_os = "macos")]
fn set_application_icon() -> std::result::Result<(), &'static str> {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let main_thread = MainThreadMarker::new()
        .ok_or("the application icon must be set on the main thread")?;
    let data = NSData::with_bytes(include_bytes!(
        "../../extra/macos/Ahead.app/Contents/Resources/ahead.icns"
    ));
    let icon = NSImage::initWithData(NSImage::alloc(), &data)
        .ok_or("the bundled AHEAD icon could not be decoded")?;
    // SAFETY: AppKit requires a non-null image; the decoded icon is always Some.
    unsafe {
        NSApplication::sharedApplication(main_thread)
            .setApplicationIconImage(Some(&icon));
    }
    Ok(())
}

pub struct AgentWorkspacePanel {
    focus: FocusHandle,
    session: Entity<crate::session_panel::SessionPanel>,
    threads: Entity<crate::threads_panel::ThreadsPanel>,
    _threads_observation: Subscription,
    threads_visible: bool,
    chat_zoomed: bool,
    threads_width: Pixels,
    resize_last_x: Option<Pixels>,
}

#[derive(Clone)]
struct DraggedAgentThreads;

impl Render for DraggedAgentThreads {
    fn render(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement {
        gpui_kit::Empty
    }
}

impl AgentWorkspacePanel {
    pub fn new(
        session: Entity<crate::session_panel::SessionPanel>,
        threads: Entity<crate::threads_panel::ThreadsPanel>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _threads_observation = cx.observe(&threads, |this, threads, cx| {
            let setup_open = threads.read(cx).is_new_session_open();
            if setup_open && !this.threads_visible {
                this.threads_visible = true;
                cx.notify();
            }
        });
        Self {
            focus: cx.focus_handle(),
            session,
            threads,
            _threads_observation,
            threads_visible: true,
            chat_zoomed: false,
            threads_width: px(300.),
            resize_last_x: None,
        }
    }

    fn set_threads_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.threads_visible = visible;
        cx.notify();
    }

    fn set_chat_zoomed(&mut self, zoomed: bool, cx: &mut Context<Self>) {
        self.chat_zoomed = zoomed;
        cx.notify();
    }
}

impl BasePanel for AgentWorkspacePanel {
    fn panel_name(&self) -> &'static str {
        "ahead_agent_workspace"
    }
}

impl Panel for AgentWorkspacePanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        "AHEAD Agent & Threads"
    }

    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}

impl EventEmitter<PanelEvent> for AgentWorkspacePanel {}

impl Focusable for AgentWorkspacePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AgentWorkspacePanel {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let border = cx.theme().border;
        let threads = self.threads.clone();
        let show_threads = self.threads_visible;
        let threads_width = self.threads_width;
        h_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .on_drag_move::<DraggedAgentThreads>(cx.listener(
                |this, event: &DragMoveEvent<DraggedAgentThreads>, _, cx| {
                    if let Some(previous_x) = this.resize_last_x {
                        this.threads_width = (this.threads_width + previous_x
                            - event.event.position.x)
                            .max(px(160.))
                            .min(px(520.));
                        this.resize_last_x = Some(event.event.position.x);
                        cx.notify();
                    }
                },
            ))
            .when(!self.chat_zoomed, |this| {
                this.child(
                    div()
                        .flex_1()
                        .h_full()
                        .min_w_0()
                        .child(self.session.clone()),
                )
            })
            .when(show_threads, |this| {
                this.when(!self.chat_zoomed, |this| {
                    this.child(
                        div()
                            .h_full()
                            .w(px(4.))
                            .cursor(CursorStyle::ResizeLeftRight)
                            .border_l_1()
                            .border_color(border)
                            .id("agent-threads-splitter")
                            .on_drag(DraggedAgentThreads, |dragged, _, _, cx| {
                                cx.stop_propagation();
                                cx.new(|_| dragged.clone())
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(
                                    |this, event: &MouseDownEvent, _, cx| {
                                        this.resize_last_x = Some(event.position.x);
                                        cx.notify();
                                    },
                                ),
                            ),
                    )
                })
                .child(div().h_full().w(threads_width).child(threads))
            })
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn just_task_terminal_command(justfile: &str, recipe: &str, watch: bool) -> String {
    let justfile = std::path::Path::new(justfile)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| justfile.to_string());
    let command = format!(
        "just --justfile {} {}",
        shell_quote(&justfile),
        shell_quote(recipe)
    );
    if watch {
        format!("watchexec --restart --clear --shell=none -- {command}")
    } else {
        command
    }
}

fn debug_terminal_command(
    arguments: &ahead_rpc::dap_types::RunInTerminalArguments,
) -> Result<Option<String>, String> {
    let mut parts = Vec::new();
    if let Some(env) = &arguments.env {
        let mut entries = env.iter().collect::<Vec<_>>();
        entries.sort_by_key(|(name, _)| *name);
        for (name, value) in entries {
            if name.is_empty()
                || !name.bytes().enumerate().all(|(index, byte)| {
                    if index == 0 {
                        byte.is_ascii_alphabetic() || byte == b'_'
                    } else {
                        byte.is_ascii_alphanumeric() || byte == b'_'
                    }
                })
            {
                return Err(format!(
                    "Invalid debug terminal environment name: {name}"
                ));
            }
            match value {
                Some(value) => {
                    parts.push(format!("export {name}={}", shell_quote(value)))
                }
                None => parts.push(format!("unset {name}")),
            }
        }
    }
    if !arguments.args.is_empty() {
        parts.push(
            arguments
                .args
                .iter()
                .map(|arg| shell_quote(arg))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    Ok((!parts.is_empty()).then(|| parts.join("; ")))
}

type SystemSpeechProcess = Arc<std::sync::Mutex<Option<std::process::Child>>>;

fn stop_system_speech(process: &SystemSpeechProcess) -> Result<bool, String> {
    let mut slot = process.lock().unwrap_or_else(|error| error.into_inner());
    let Some(mut child) = slot.take() else {
        return Ok(false);
    };
    match child.try_wait() {
        Ok(Some(_)) => return Ok(true),
        Ok(None) => {}
        Err(error) => {
            *slot = Some(child);
            return Err(format!(
                "Could not inspect the system speech process: {error}"
            ));
        }
    }
    if let Err(error) = child.kill() {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(true),
            Ok(None) => {
                *slot = Some(child);
                return Err(format!("Could not stop system speech: {error}"));
            }
            Err(wait_error) => {
                *slot = Some(child);
                return Err(format!(
                    "Could not stop or inspect system speech: {error}; {wait_error}"
                ));
            }
        }
    }
    match child.wait() {
        Ok(_) => Ok(true),
        Err(error) => {
            *slot = Some(child);
            Err(format!(
                "Could not reap the stopped speech process: {error}"
            ))
        }
    }
}

fn interrupt_system_speech(process: &SystemSpeechProcess) -> bool {
    let mut slot = process.lock().unwrap_or_else(|error| error.into_inner());
    let Some(child) = slot.as_mut() else {
        return true;
    };
    match child.kill() {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => true,
        Err(error) => {
            eprintln!("AHEAD could not interrupt system speech: {error}");
            false
        }
    }
}

fn start_system_speech(
    process: &SystemSpeechProcess,
    text: &str,
) -> Result<(), String> {
    stop_system_speech(process)?;
    #[cfg(target_os = "macos")]
    let child = std::process::Command::new("/usr/bin/say")
        .arg(text)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not start system speech: {error}"))?;
    #[cfg(not(target_os = "macos"))]
    return Err("System speech is currently supported on macOS only.".to_string());

    #[cfg(target_os = "macos")]
    {
        *process.lock().unwrap_or_else(|error| error.into_inner()) = Some(child);
        Ok(())
    }
}

fn reap_system_speech(process: &SystemSpeechProcess) -> bool {
    let mut slot = process.lock().unwrap_or_else(|error| error.into_inner());
    let Some(child) = slot.as_mut() else {
        return false;
    };
    match child.try_wait() {
        Ok(Some(_)) => {
            slot.take();
            true
        }
        Ok(None) => false,
        Err(error) => {
            eprintln!("AHEAD could not inspect system speech: {error}");
            false
        }
    }
}

fn presentation_request_matches_active_turn(
    request: &EditorPresentationRequest,
    active_session_id: Option<&str>,
    active_turn_id: Option<&str>,
) -> bool {
    active_session_id == Some(request.session_id.as_str())
        && active_turn_id == Some(request.turn_id.as_str())
}

const MAX_RECENT_FILES: usize = 20;
const MAX_EDITOR_NAVIGATION_HISTORY: usize = 100;

#[derive(Default)]
struct EditorNavigationHistory {
    backward: Vec<crate::ross::OpenRequest>,
    forward: Vec<crate::ross::OpenRequest>,
}

impl EditorNavigationHistory {
    fn record_jump(
        &mut self,
        current: Option<crate::ross::OpenRequest>,
        target: &crate::ross::OpenRequest,
    ) {
        let Some(current) = current else {
            return;
        };
        if target.location.is_none() || same_editor_location(&current, target) {
            return;
        }
        push_editor_location(&mut self.backward, current);
        self.forward.clear();
    }

    fn go_back(
        &mut self,
        current: Option<crate::ross::OpenRequest>,
    ) -> Option<crate::ross::OpenRequest> {
        traverse_editor_history(&mut self.backward, &mut self.forward, current)
    }

    fn go_forward(
        &mut self,
        current: Option<crate::ross::OpenRequest>,
    ) -> Option<crate::ross::OpenRequest> {
        traverse_editor_history(&mut self.forward, &mut self.backward, current)
    }
}

fn push_editor_location(
    history: &mut Vec<crate::ross::OpenRequest>,
    location: crate::ross::OpenRequest,
) {
    if history.len() == MAX_EDITOR_NAVIGATION_HISTORY {
        history.remove(0);
    }
    history.push(location);
}

fn traverse_editor_history(
    source: &mut Vec<crate::ross::OpenRequest>,
    destination: &mut Vec<crate::ross::OpenRequest>,
    current: Option<crate::ross::OpenRequest>,
) -> Option<crate::ross::OpenRequest> {
    let target = source.pop()?;
    if let Some(current) = current
        && !same_editor_location(&target, &current)
    {
        push_editor_location(destination, current);
    }
    Some(target)
}

fn same_editor_location(
    left: &crate::ross::OpenRequest,
    right: &crate::ross::OpenRequest,
) -> bool {
    left.path == right.path && left.location == right.location
}

fn closes_code_tab(
    action: crate::code_panel::CodeTabAction,
    target: usize,
    candidate: usize,
) -> bool {
    use crate::code_panel::CodeTabAction;
    match action {
        CodeTabAction::Close => candidate == target,
        CodeTabAction::CloseOthers => candidate != target,
        CodeTabAction::CloseLeft => candidate < target,
        CodeTabAction::CloseRight => candidate > target,
        CodeTabAction::CloseAll => true,
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CenterPanel {
    Settings,
    Extensions,
    Search,
    Problems,
    Help,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditorGroupSide {
    Left,
    Right,
}

fn toggle_editor_group(
    right_group: &mut std::collections::HashSet<u64>,
    panel_id: u64,
) -> EditorGroupSide {
    if right_group.remove(&panel_id) {
        EditorGroupSide::Left
    } else {
        right_group.insert(panel_id);
        EditorGroupSide::Right
    }
}

fn assign_editor_group(
    right_group: &mut std::collections::HashSet<u64>,
    left_group_id: &mut Option<u64>,
    right_group_id: &mut Option<u64>,
    panel_id: u64,
    group_id: u64,
) -> EditorGroupSide {
    if *right_group_id == Some(group_id) {
        right_group.insert(panel_id);
        EditorGroupSide::Right
    } else if *left_group_id == Some(group_id) {
        right_group.remove(&panel_id);
        EditorGroupSide::Left
    } else if right_group.contains(&panel_id) {
        *right_group_id = Some(group_id);
        EditorGroupSide::Right
    } else {
        *left_group_id = Some(group_id);
        EditorGroupSide::Left
    }
}

pub struct Shell {
    pub area: Entity<DockArea>,
    pub focus: FocusHandle,
    pub session: Entity<crate::session_panel::SessionPanel>,
    pub threads: Entity<crate::threads_panel::ThreadsPanel>,
    pub code: Entity<crate::code_panel::CodePanel>,
    git: Entity<GitPanel>,
    pub code_tabs: Vec<Entity<crate::code_panel::CodePanel>>,
    right_editor_tabs: std::collections::HashSet<u64>,
    left_editor_group_id: Option<u64>,
    right_editor_group_id: Option<u64>,
    active_editor_group: EditorGroupSide,
    navigation_history: EditorNavigationHistory,
    closing_code_tabs: bool,
    restoring_buffers: bool,
    window_close_authorized: bool,
    recent_files: Vec<std::path::PathBuf>,
    pub explorer: Entity<crate::explorer_panel::ExplorerPanel>,
    pub debug_bar: Entity<crate::debug_bar::DebugBar>,
    pub terminals: Vec<Entity<crate::terminal_panel::TerminalPanel>>,
    debug_terminals:
        std::collections::HashMap<ahead_rpc::dap_types::DapId, Vec<usize>>,
    pub problems: Entity<ProblemsPanel>,
    pub settings: Entity<crate::settings_panel::SettingsPanel>,
    extensions: Entity<crate::extensions_panel::ExtensionsPanel>,
    help: Entity<crate::help_panel::HelpPanel>,
    pub search: Entity<SearchPanel>,
    pub agent_workspace: Entity<AgentWorkspacePanel>,
    pub activity: Entity<ActivityBar>,
    workspace_name: String,
    worktree_name: String,
    pub threads_visible: bool,
    pub chat_zoomed: bool,
    left_dock_was_open: bool,
    right_dock_was_open: bool,
    bottom_dock_was_open: bool,
    bottom_debug_active: bool,
    debug_visible: bool,
    active_terminal: usize,
    next_terminal_id: usize,
    open_center_panels: Vec<CenterPanel>,
    active_center_panel: Option<CenterPanel>,
    system_speech_process: SystemSpeechProcess,
    system_speech_active: Arc<AtomicBool>,
    _shortcut_interceptor: Option<Subscription>,
    status_message: String,
    pending_comment: Option<(String, u32)>,
}

impl Shell {
    pub(crate) fn active_terminal_context(
        &self,
        cx: &App,
    ) -> Option<(String, String, bool)> {
        let terminal = self.terminals.get(self.active_terminal)?;
        terminal.read_with(cx, |terminal, _| terminal.agent_context())
    }

    pub(crate) fn shared_terminal_text(&self, cx: &App) -> String {
        self.terminals
            .iter()
            .map(|terminal| terminal.read(cx).shared_snapshot_text())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn install_window_close_handler(
        shell: &Entity<Self>,
        window: &Window,
        cx: &App,
    ) {
        let shell = shell.downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            match shell.update(cx, |shell, cx| {
                if shell.window_close_authorized {
                    return true;
                }
                shell.request_window_close(window, cx);
                false
            }) {
                Ok(allow) => allow,
                Err(error) => {
                    eprintln!("Window already released during close: {error}");
                    true
                }
            }
        });
    }

    pub fn new(
        area: Entity<DockArea>,
        session: Entity<crate::session_panel::SessionPanel>,
        threads: Entity<crate::threads_panel::ThreadsPanel>,
        code: Entity<crate::code_panel::CodePanel>,
        explorer: Entity<crate::explorer_panel::ExplorerPanel>,
        git: Entity<GitPanel>,
        debug_bar: Entity<crate::debug_bar::DebugBar>,
        terminals: Vec<Entity<crate::terminal_panel::TerminalPanel>>,
        problems: Entity<ProblemsPanel>,
        settings: Entity<crate::settings_panel::SettingsPanel>,
        extensions: Entity<crate::extensions_panel::ExtensionsPanel>,
        help: Entity<crate::help_panel::HelpPanel>,
        search: Entity<SearchPanel>,
        agent_workspace: Entity<AgentWorkspacePanel>,
        activity: Entity<ActivityBar>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (workspace_name, worktree_name) =
            workspace_git_names(&explorer.read(cx).root);
        let initial_file_path =
            std::path::PathBuf::from(code.read(cx).file_path.clone());
        let recent_files = if initial_file_path.as_os_str().is_empty() {
            Vec::new()
        } else {
            vec![initial_file_path.clone()]
        };
        if !initial_file_path.as_os_str().is_empty()
            && let Some(proxy) = code.read(cx).proxy.clone()
        {
            Self::persist_recent_file(
                proxy,
                initial_file_path.clone(),
                chrono::Utc::now().timestamp_micros(),
                cx,
            );
        }
        let system_speech_process = Arc::new(std::sync::Mutex::new(None));
        let system_speech_active = Arc::new(AtomicBool::new(false));
        let voice_speech_process = system_speech_process.clone();
        let voice_speech_active = system_speech_active.clone();
        session.update(cx, |session, _| {
            session.set_voice_interruption_handler(
                Arc::new(move || {
                    if interrupt_system_speech(&voice_speech_process) {
                        voice_speech_active.store(false, Ordering::SeqCst);
                    }
                }),
                system_speech_active.clone(),
            );
        });
        let debug_speech_process = system_speech_process.clone();
        let debug_speech_active = system_speech_active.clone();
        debug_bar.update(cx, |debug_bar, _| {
            debug_bar.set_speech_interruption_handler(move || {
                if interrupt_system_speech(&debug_speech_process) {
                    debug_speech_active.store(false, Ordering::SeqCst);
                }
            });
        });
        if let Some(proxy) = session.read(cx).proxy.as_ref() {
            let updates = proxy.subscribe_diagnostics();
            let diagnostic_proxy = proxy.clone();
            let activity = activity.clone();
            let mut git_generation = diagnostic_proxy.git_generation();
            cx.spawn(async move |shell, cx| {
                while updates.recv().await.is_ok() {
                    let current_generation = diagnostic_proxy.git_generation();
                    if current_generation != git_generation {
                        git_generation = current_generation;
                        activity.update(cx, |activity, cx| {
                            activity.refresh_active_git_status(cx)
                        });
                    }
                    if shell.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
            .detach();
            if let Some(requests) = proxy.subscribe_core_requests() {
                let request_proxy = proxy.clone();
                cx.spawn(async move |shell, cx| {
                    while let Ok(event) = requests.recv().await {
                        let request_id = event.request_id;
                        let failure_proxy = request_proxy.clone();
                        let proxy = request_proxy.clone();
                        let request = event.request.clone();
                        if shell
                            .update_in(cx, |shell, window, cx| match event.request {
                                ahead_rpc::core::CoreRequest::ApplyWorkspaceEdit(
                                    params,
                                ) => shell.apply_server_workspace_edit(
                                    event.request_id,
                                    params,
                                    proxy,
                                    window,
                                    cx,
                                ),
                                ahead_rpc::core::CoreRequest::GitCredentialPrompt {
                                    prompt,
                                    secret,
                                } => {
                                    shell.git.update(
                                        cx,
                                        |git, cx| {
                                            git.request_git_credential(
                                                event.request_id,
                                                prompt,
                                                secret,
                                                window,
                                                cx,
                                            );
                                        },
                                    );
                                }
                            })
                            .is_err()
                        {
                            failure_proxy.respond_core_request(
                                request_id,
                                crate::proxy_client::unavailable_core_request_response(
                                    request,
                                    "The editor closed before handling the request",
                                ),
                            );
                            break;
                        }
                    }
                })
                .detach();
            }
        }
        Self {
            area,
            focus: cx.focus_handle(),
            session,
            threads,
            code_tabs: if code.read(cx).file_path.is_empty() {
                Vec::new()
            } else {
                vec![code.clone()]
            },
            right_editor_tabs: std::collections::HashSet::new(),
            left_editor_group_id: None,
            right_editor_group_id: None,
            active_editor_group: EditorGroupSide::Left,
            navigation_history: EditorNavigationHistory::default(),
            recent_files,
            closing_code_tabs: false,
            restoring_buffers: false,
            window_close_authorized: false,
            code,
            explorer,
            git,
            debug_bar,
            terminals,
            debug_terminals: std::collections::HashMap::new(),
            problems,
            settings,
            extensions,
            help,
            search,
            agent_workspace,
            activity,
            workspace_name,
            worktree_name,
            threads_visible: true,
            chat_zoomed: false,
            left_dock_was_open: true,
            right_dock_was_open: true,
            bottom_dock_was_open: true,
            bottom_debug_active: false,
            debug_visible: false,
            active_terminal: 0,
            next_terminal_id: 2,
            open_center_panels: Vec::new(),
            active_center_panel: None,
            system_speech_process,
            system_speech_active,
            _shortcut_interceptor: None,
            status_message: String::new(),
            pending_comment: None,
        }
    }

    fn persist_recent_file(
        proxy: Arc<crate::proxy_client::ProxyClient>,
        path: std::path::PathBuf,
        opened_at: i64,
        cx: &mut Context<Self>,
    ) {
        cx.background_spawn(async move {
            if let Err(error) = proxy.record_recent_workspace_file(&path, opened_at)
            {
                eprintln!(
                    "AHEAD could not persist a recent workspace file: {}",
                    error.message
                );
            }
        })
        .detach();
    }

    fn set_active_code(
        &mut self,
        code: Entity<crate::code_panel::CodePanel>,
        cx: &mut Context<Self>,
    ) {
        self.active_editor_group =
            if self.right_editor_tabs.contains(&code.entity_id().as_u64()) {
                EditorGroupSide::Right
            } else {
                EditorGroupSide::Left
            };
        let (path, proxy) = code.read_with(cx, |code, _| {
            (
                std::path::PathBuf::from(code.file_path.clone()),
                code.proxy.clone(),
            )
        });
        if !path.as_os_str().is_empty() {
            let should_persist = self.recent_files.first() != Some(&path);
            self.recent_files.retain(|recent| recent != &path);
            self.recent_files.insert(0, path.clone());
            self.recent_files.truncate(MAX_RECENT_FILES);
            if should_persist && let Some(proxy) = proxy {
                Self::persist_recent_file(
                    proxy,
                    path,
                    chrono::Utc::now().timestamp_micros(),
                    cx,
                );
            }
        }
        self.code = code.clone();
        self.session.update(cx, |session, cx| {
            session.code = Some(code.clone());
            let (comment_session_id, comments) = session.code_comment_context();
            code.update(cx, |code, cx| {
                code.set_code_comments(comment_session_id, comments, cx);
            });
        });
        if let Some((comment_path, line)) = self.pending_comment.as_ref()
            && code.read(cx).file_path == *comment_path
        {
            code.update(cx, |code, cx| code.show_code_comment_line(*line, cx));
            self.pending_comment = None;
        }
        self.problems.update(cx, |problems, _| {
            problems.code = code;
        });
    }

    fn code_panel_added_to_group(
        &mut self,
        panel_id: u64,
        group_id: u64,
        cx: &mut Context<Self>,
    ) {
        let mut side = assign_editor_group(
            &mut self.right_editor_tabs,
            &mut self.left_editor_group_id,
            &mut self.right_editor_group_id,
            panel_id,
            group_id,
        );
        let right_count = self
            .code_tabs
            .iter()
            .filter(|code| {
                self.right_editor_tabs.contains(&code.entity_id().as_u64())
            })
            .count();
        if right_count == 0 {
            self.right_editor_group_id = None;
        }
        if right_count == self.code_tabs.len() && self.open_center_panels.is_empty()
        {
            self.right_editor_tabs.clear();
            self.left_editor_group_id = Some(group_id);
            self.right_editor_group_id = None;
            self.active_editor_group = EditorGroupSide::Left;
            side = EditorGroupSide::Left;
        }
        if self.code.entity_id().as_u64() == panel_id {
            self.active_editor_group = side;
        }
        cx.notify();
    }

    fn configure_code(
        &mut self,
        code: Entity<crate::code_panel::CodePanel>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(
            &code,
            window,
            |_shell,
             code,
             event: &crate::code_panel::CodePanelGroupChanged,
             window,
             cx| {
                let panel_id = code.entity_id().as_u64();
                let group_id = event.group_id;
                cx.defer_in(window, move |shell, _, cx| {
                    shell.code_panel_added_to_group(panel_id, group_id, cx);
                });
            },
        )
        .detach();
        cx.subscribe_in(
            &code,
            window,
            |_shell, code, _: &crate::code_panel::CodePanelActivated, window, cx| {
                let code = code.clone();
                cx.defer_in(window, move |shell, _, cx| {
                    shell.set_active_code(code, cx);
                });
            },
        )
        .detach();
        cx.subscribe_in(
            &code,
            window,
            |shell, _, request: &crate::ross::OpenRequest, window, cx| {
                shell.navigate_to_editor_location(request.clone(), window, cx);
            },
        )
        .detach();
        let applying_code = code.clone();
        cx.subscribe_in(
            &code,
            window,
            move |shell,
                  _,
                  request: &crate::code_panel::WorkspaceEditRequest,
                  window,
                  cx| {
                shell.apply_workspace_edit(
                    applying_code.clone(),
                    request.clone(),
                    window,
                    cx,
                );
            },
        )
        .detach();
        let shell = cx.entity().downgrade();
        let shell_for_close = shell.clone();
        let shell_for_tabs = shell;
        let speech_process = self.system_speech_process.clone();
        let speech_active = self.system_speech_active.clone();
        let voice_output_interrupt_slot =
            self.session.read(cx).voice_output_interrupt_slot();
        let session_panel = self.session.downgrade();
        let (session_id, comments) = self
            .session
            .read_with(cx, |session, _| session.code_comment_context());
        code.update(cx, |code, cx| {
            code.set_session_panel(session_panel);
            code.set_code_comments(session_id, comments, cx);
            code.set_close_handler(move |panel, window, cx| {
                _ = shell_for_close.update(cx, |shell, cx| {
                    shell.close_code_tab(panel, window, cx);
                });
            });
            code.set_tab_handler(move |panel, action, window, cx| {
                _ = shell_for_tabs.update(cx, |shell, cx| {
                    shell.handle_code_tab_action(panel, action, window, cx);
                });
            });
            let voice_output_interrupt_slot = voice_output_interrupt_slot.clone();
            code.set_speech_interruption_handler(move || {
                match stop_system_speech(&speech_process) {
                    Ok(_) => speech_active.store(false, Ordering::SeqCst),
                    Err(error) => {
                        eprintln!(
                            "AHEAD could not interrupt system speech: {error}"
                        );
                    }
                }
                let interrupt_output = voice_output_interrupt_slot
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                if let Some(interrupt_output) = interrupt_output {
                    interrupt_output();
                }
            });
        });
    }

    fn apply_server_workspace_edit(
        &mut self,
        request_id: ahead_rpc::RequestId,
        params: lsp_types::ApplyWorkspaceEditParams,
        proxy: Arc<crate::proxy_client::ProxyClient>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (source_path, generation) = self.code.read_with(cx, |code, _| {
            (
                std::path::PathBuf::from(&code.file_path),
                code.buffer_generation(),
            )
        });
        let (completion, result) = async_channel::bounded(1);
        let request = crate::code_panel::WorkspaceEditRequest {
            edit: params.edit,
            source_path,
            purpose: crate::code_panel::WorkspaceEditPurpose::ServerRequest {
                label: params.label,
            },
            generation,
            completion: Some(completion),
        };
        self.apply_workspace_edit(self.code.clone(), request, window, cx);
        cx.background_spawn(async move {
            let response = match result.recv().await {
                Ok(Ok(_)) => lsp_types::ApplyWorkspaceEditResponse {
                    applied: true,
                    failure_reason: None,
                    failed_change: None,
                },
                Ok(Err(reason)) => lsp_types::ApplyWorkspaceEditResponse {
                    applied: false,
                    failure_reason: Some(reason),
                    failed_change: None,
                },
                Err(_) => lsp_types::ApplyWorkspaceEditResponse {
                    applied: false,
                    failure_reason: Some(
                        "The editor stopped before applying the workspace edit"
                            .into(),
                    ),
                    failed_change: None,
                },
            };
            proxy.respond_core_request(
                request_id,
                ahead_rpc::core::CoreResponse::ApplyWorkspaceEdit(response),
            );
        })
        .detach();
    }

    fn apply_workspace_edit(
        &mut self,
        source: Entity<crate::code_panel::CodePanel>,
        request: crate::code_panel::WorkspaceEditRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (source_path, source_generation) = source.read_with(cx, |code, _| {
            (
                std::path::PathBuf::from(&code.file_path),
                code.buffer_generation(),
            )
        });
        if !matches!(
            &request.purpose,
            crate::code_panel::WorkspaceEditPurpose::ServerRequest { .. }
        ) && (source_path != request.source_path
            || source_generation != request.generation)
        {
            let message = match &request.purpose {
                crate::code_panel::WorkspaceEditPurpose::Rename { .. } => {
                    "Rename canceled because the source buffer changed".to_string()
                }
                crate::code_panel::WorkspaceEditPurpose::CodeAction { .. } => {
                    "Code action canceled because the source buffer changed"
                        .to_string()
                }
                crate::code_panel::WorkspaceEditPurpose::ServerRequest {
                    ..
                } => "Language server workspace edit could not be applied".into(),
            };
            request.complete(Err(message.clone()));
            self.status_message = message.into();
            cx.notify();
            return;
        }
        let documents =
            match crate::code_panel::workspace_edit_text_edits(&request.edit) {
                Ok(documents) => documents,
                Err(error) => {
                    let message = match &request.purpose {
                        crate::code_panel::WorkspaceEditPurpose::Rename {
                            ..
                        } => {
                            format!("Workspace rename failed: {error}")
                        }
                        crate::code_panel::WorkspaceEditPurpose::CodeAction {
                            title,
                        } => {
                            format!("Could not apply code action '{title}': {error}")
                        }
                        crate::code_panel::WorkspaceEditPurpose::ServerRequest {
                            ..
                        } => {
                            format!("Language server workspace edit failed: {error}")
                        }
                    };
                    request.complete(Err(error));
                    self.status_message = message.into();
                    cx.notify();
                    return;
                }
            };
        let Some(proxy) = self.session.read(cx).proxy.clone() else {
            let message = match &request.purpose {
                crate::code_panel::WorkspaceEditPurpose::Rename { .. } => {
                    "Workspace rename failed: language services are unavailable"
                        .to_string()
                }
                crate::code_panel::WorkspaceEditPurpose::CodeAction { title } => {
                    format!(
                        "Could not apply code action '{title}': language services are unavailable"
                    )
                }
                crate::code_panel::WorkspaceEditPurpose::ServerRequest { .. } => {
                    "Language server workspace edit failed: language services are unavailable".into()
                }
            };
            request.complete(Err("Language services are unavailable".into()));
            self.status_message = message.into();
            cx.notify();
            return;
        };
        let workspace =
            std::path::PathBuf::from(self.explorer.read(cx).root.clone());
        let receiver = proxy.request_open_files_content();
        self.status_message = match &request.purpose {
            crate::code_panel::WorkspaceEditPurpose::Rename { .. } => {
                "Validating workspace rename…".into()
            }
            crate::code_panel::WorkspaceEditPurpose::CodeAction { .. } => {
                "Validating workspace code action…".into()
            }
            crate::code_panel::WorkspaceEditPurpose::ServerRequest { label } => {
                label.as_ref().map_or_else(
                    || "Validating language server workspace edit…".into(),
                    |label| {
                        format!("Applying language server edit '{label}'…").into()
                    },
                )
            }
        };
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let open_documents = cx
                .background_spawn(async move {
                    receiver
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .map_err(|error| {
                            format!("Waiting for open buffers: {error}")
                        })?
                })
                .await;
            if let Err(error) = this.update_in(cx, |this, window, cx| {
                this.finish_workspace_edit(
                    source,
                    request,
                    documents,
                    workspace,
                    open_documents,
                    window,
                    cx,
                );
            }) {
                eprintln!("Applying workspace edit: {error}");
            }
        })
        .detach();
    }

    fn finish_workspace_edit(
        &mut self,
        source: Entity<crate::code_panel::CodePanel>,
        request: crate::code_panel::WorkspaceEditRequest,
        documents: Vec<crate::code_panel::WorkspaceTextEdits>,
        workspace: std::path::PathBuf,
        open_documents: Result<Vec<lsp_types::TextDocumentItem>, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous_code = self.code.clone();
        let previous_center_panel = self.active_center_panel;
        let result = (|| -> Result<usize, String> {
            let open_documents = open_documents?;
            let (source_path, source_generation) =
                source.read_with(cx, |code, _| {
                    (
                        std::path::PathBuf::from(&code.file_path),
                        code.buffer_generation(),
                    )
                });
            if !matches!(
                &request.purpose,
                crate::code_panel::WorkspaceEditPurpose::ServerRequest { .. }
            ) && (source_path != request.source_path
                || source_generation != request.generation)
            {
                return Err(match &request.purpose {
                    crate::code_panel::WorkspaceEditPurpose::Rename { .. } => {
                        "Rename canceled because the source buffer changed".into()
                    }
                    crate::code_panel::WorkspaceEditPurpose::CodeAction {
                        ..
                    } => "Code action canceled because the source buffer changed"
                        .into(),
                    crate::code_panel::WorkspaceEditPurpose::ServerRequest {
                        ..
                    } => {
                        "Language server workspace edit could not be applied".into()
                    }
                });
            }
            let buffers = self
                .code_tabs
                .iter()
                .filter_map(|panel| {
                    panel
                        .read_with(cx, |code, cx| code.workspace_edit_snapshot(cx))
                        .ok()
                })
                .collect::<Vec<_>>();
            let prepared = prepare_workspace_documents(
                &workspace,
                documents,
                &open_documents,
                &buffers,
            )?;
            if prepared.is_empty() {
                return Err(
                    "The language server returned no effective workspace changes"
                        .into(),
                );
            }

            for document in &prepared {
                let already_open = self.code_tabs.iter().any(|panel| {
                    let path = std::path::PathBuf::from(&panel.read(cx).file_path);
                    same_file_path(&path, &document.path)
                });
                if !already_open {
                    self.open_file_request(
                        crate::ross::OpenRequest {
                            path: document.path.to_string_lossy().into_owned(),
                            permanent: true,
                            location: None,
                        },
                        window,
                        cx,
                    );
                }
            }

            let mut targets = Vec::with_capacity(prepared.len());
            for document in prepared {
                let panel = self
                    .code_tabs
                    .iter()
                    .find(|panel| {
                        let path =
                            std::path::PathBuf::from(&panel.read(cx).file_path);
                        same_file_path(&path, &document.path)
                    })
                    .cloned()
                    .ok_or_else(|| {
                        format!(
                            "Could not open workspace edit target {}",
                            document.path.display()
                        )
                    })?;
                let snapshot = panel
                    .read_with(cx, |code, cx| code.workspace_edit_snapshot(cx))
                    .map_err(|error| error.to_string())?;
                if snapshot.text != document.text {
                    return Err(format!(
                        "A file changed while the workspace edit was being prepared: {}",
                        document.path.display()
                    ));
                }
                if !snapshot.editable {
                    return Err(format!(
                        "Workspace edit targets a read-only shared file: {}",
                        document.path.display()
                    ));
                }
                crate::code_panel::formatting_edit(
                    &snapshot.text,
                    &document.edits,
                    snapshot.selection,
                )?;
                let preserve_code_action_request =
                    matches!(
                        &request.purpose,
                        crate::code_panel::WorkspaceEditPurpose::CodeAction { .. }
                    ) && panel.read_with(cx, |code, _| {
                        std::path::PathBuf::from(&code.file_path)
                            == request.source_path
                    });
                targets.push((
                    panel,
                    document.text,
                    document.edits,
                    preserve_code_action_request,
                ));
            }

            for (panel, text, edits, preserve_code_action_request) in &targets {
                panel
                    .update(cx, |code, cx| {
                        code.apply_workspace_text_edits(
                            text,
                            edits,
                            *preserve_code_action_request,
                            window,
                            cx,
                        )
                    })
                    .map_err(|error| error.to_string())?;
            }
            Ok(targets.len())
        })();

        self.set_active_code(previous_code, cx);
        self.active_center_panel = previous_center_panel;
        self.refresh_center_layout(window, cx);
        let completion_result = result
            .as_ref()
            .map(|_| source.read_with(cx, |code, _| code.buffer_generation()))
            .map_err(Clone::clone);
        request.complete(completion_result);
        self.status_message = match (&request.purpose, &result) {
            (
                crate::code_panel::WorkspaceEditPurpose::Rename {
                    old_name,
                    new_name,
                },
                Ok(count),
            ) => format!(
                "Renamed {old_name} to {new_name} in {count} file{}; review and save the changed tabs",
                if *count == 1 { "" } else { "s" }
            ),
            (
                crate::code_panel::WorkspaceEditPurpose::CodeAction { title },
                Ok(count),
            ) => format!(
                "Applied '{title}' in {count} file{}; review and save the changed tabs",
                if *count == 1 { "" } else { "s" }
            ),
            (
                crate::code_panel::WorkspaceEditPurpose::ServerRequest { label },
                Ok(count),
            ) => format!(
                "Applied language server edit{} in {count} file{}; review and save the changed tabs",
                label
                    .as_ref()
                    .map_or_else(String::new, |label| format!(" '{label}'")),
                if *count == 1 { "" } else { "s" }
            ),
            (crate::code_panel::WorkspaceEditPurpose::Rename { .. }, Err(error)) => {
                format!("Workspace rename failed: {error}")
            }
            (
                crate::code_panel::WorkspaceEditPurpose::CodeAction { title },
                Err(error),
            ) => format!("Could not apply code action '{title}': {error}"),
            (
                crate::code_panel::WorkspaceEditPurpose::ServerRequest { .. },
                Err(error),
            ) => format!("Language server workspace edit failed: {error}"),
        };
        cx.notify();
    }

    fn apply_search_replacements(
        &mut self,
        request: SearchReplaceRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let previous_code = self.code.clone();
        let previous_center_panel = self.active_center_panel;
        let result = (|| -> Result<(usize, usize), String> {
            if request.files.is_empty() || request.match_count == 0 {
                return Err("There are no search matches to replace".into());
            }
            let workspace =
                std::path::PathBuf::from(self.explorer.read(cx).root.clone())
                    .canonicalize()
                    .map_err(|error| {
                        format!("Could not resolve workspace root: {error}")
                    })?;
            let mut seen = std::collections::HashSet::new();
            let mut match_count = 0usize;
            let mut documents = Vec::with_capacity(request.files.len());
            for file in request.files {
                let path = canonical_workspace_file(&workspace, &file.path)?;
                if !seen.insert(path.clone()) {
                    return Err(format!(
                        "Search replacement returned the same file more than once: {}",
                        path.display()
                    ));
                }
                if file.match_count == 0
                    || file.original_text == file.replacement_text
                {
                    return Err(format!(
                        "Search replacement returned no effective changes for {}",
                        path.display()
                    ));
                }
                match_count = match_count
                    .checked_add(file.match_count)
                    .ok_or("Search replacement match count overflowed")?;
                let line = u32::try_from(
                    file.original_text
                        .bytes()
                        .filter(|byte| *byte == b'\n')
                        .count(),
                )
                .map_err(|_| "Search replacement target has too many lines")?;
                let raw_last_line =
                    file.original_text.rsplit('\n').next().unwrap_or_default();
                let last_line =
                    raw_last_line.strip_suffix('\r').unwrap_or(raw_last_line);
                let character = u32::try_from(last_line.encode_utf16().count())
                    .map_err(|_| "Search replacement target line is too long")?;
                let edit = lsp_types::TextEdit {
                    range: lsp_types::Range::new(
                        lsp_types::Position::new(0, 0),
                        lsp_types::Position::new(line, character),
                    ),
                    new_text: file.replacement_text,
                };
                documents.push((path, file.original_text, edit));
            }
            if match_count != request.match_count {
                return Err("Search replacement match count did not validate".into());
            }

            for (path, _, _) in &documents {
                let already_open = self.code_tabs.iter().any(|panel| {
                    let open_path =
                        std::path::PathBuf::from(&panel.read(cx).file_path);
                    same_file_path(&open_path, path)
                });
                if !already_open {
                    self.open_file_request(
                        crate::ross::OpenRequest {
                            path: path.to_string_lossy().into_owned(),
                            permanent: true,
                            location: None,
                        },
                        window,
                        cx,
                    );
                }
            }

            let mut targets = Vec::with_capacity(documents.len());
            for (path, original_text, edit) in documents {
                let panel = self
                    .code_tabs
                    .iter()
                    .find(|panel| {
                        let open_path =
                            std::path::PathBuf::from(&panel.read(cx).file_path);
                        same_file_path(&open_path, &path)
                    })
                    .cloned()
                    .ok_or_else(|| format!("Could not open {}", path.display()))?;
                let snapshot = panel
                    .read_with(cx, |code, cx| code.workspace_edit_snapshot(cx))
                    .map_err(|error| error.to_string())?;
                if snapshot.text != original_text {
                    return Err(format!(
                        "A file changed after search replacement was prepared: {}",
                        path.display()
                    ));
                }
                if !snapshot.editable {
                    return Err(format!(
                        "Search replacement targets a read-only shared file: {}",
                        path.display()
                    ));
                }
                crate::code_panel::formatting_edit(
                    &snapshot.text,
                    std::slice::from_ref(&edit),
                    snapshot.selection,
                )?;
                targets.push((panel, snapshot.text, edit));
            }

            for (panel, original_text, edit) in &targets {
                panel
                    .update(cx, |code, cx| {
                        code.apply_workspace_text_edits(
                            original_text,
                            std::slice::from_ref(edit),
                            false,
                            window,
                            cx,
                        )
                    })
                    .map_err(|error| error.to_string())?;
            }
            Ok((targets.len(), match_count))
        })();

        self.set_active_code(previous_code, cx);
        self.active_center_panel = previous_center_panel;
        self.refresh_center_layout(window, cx);
        match result {
            Ok((file_count, match_count)) => {
                self.status_message = format!(
                    "Replaced {match_count} occurrence{} in {file_count} file{}; review and save the changed tabs",
                    if match_count == 1 { "" } else { "s" },
                    if file_count == 1 { "" } else { "s" }
                );
                cx.notify();
                Ok(())
            }
            Err(error) => {
                self.status_message = format!("Replace failed: {error}");
                cx.notify();
                Err(error)
            }
        }
    }

    fn open_center_panel(
        &mut self,
        panel: CenterPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.open_center_panels.contains(&panel) {
            self.open_center_panels.push(panel);
        }
        self.active_center_panel = Some(panel);
        self.refresh_center_layout(window, cx);
    }

    fn open_extensions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_center_panel(CenterPanel::Extensions, window, cx);
        self.extensions
            .update(cx, |extensions, cx| extensions.load_catalog_if_needed(cx));
        let search = self.extensions.read(cx).search_input.clone();
        search.focus_handle(cx).focus(window, cx);
    }

    fn show_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active_center_panel = None;
        self.refresh_center_layout(window, cx);
    }

    fn refresh_center_layout(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open_center_panels = self.open_center_panels.clone();
        let active_center_panel = self.active_center_panel;
        let settings = self.settings.clone();
        let extensions = self.extensions.clone();
        let help = self.help.clone();
        let search = self.search.clone();
        let problems = self.problems.clone();
        let left_code_tabs: Vec<_> = self
            .code_tabs
            .iter()
            .filter(|code| {
                !self.right_editor_tabs.contains(&code.entity_id().as_u64())
            })
            .cloned()
            .collect();
        let right_code_tabs: Vec<_> = self
            .code_tabs
            .iter()
            .filter(|code| {
                self.right_editor_tabs.contains(&code.entity_id().as_u64())
            })
            .cloned()
            .collect();
        let left_code_count = left_code_tabs.len();
        let active_code_id = self.code.entity_id();
        let active_is_right = right_code_tabs
            .iter()
            .any(|code| code.entity_id() == active_code_id);
        let active_code_index = if active_is_right {
            right_code_tabs
                .iter()
                .position(|code| code.entity_id() == active_code_id)
                .unwrap_or(0)
        } else {
            left_code_tabs
                .iter()
                .position(|code| code.entity_id() == active_code_id)
                .unwrap_or(0)
        };
        self.area.update(cx, |area, cx| {
            let mut left_tabs = DockLayout::tabs();
            for code in left_code_tabs {
                left_tabs = left_tabs.panel_view(panel_handle(code), cx);
            }
            for panel in &open_center_panels {
                left_tabs = match panel {
                    CenterPanel::Settings => {
                        left_tabs.panel_view(panel_handle(settings.clone()), cx)
                    }
                    CenterPanel::Extensions => {
                        left_tabs.panel_view(panel_handle(extensions.clone()), cx)
                    }
                    CenterPanel::Search => {
                        left_tabs.panel_view(panel_handle(search.clone()), cx)
                    }
                    CenterPanel::Problems => {
                        left_tabs.panel_view(panel_handle(problems.clone()), cx)
                    }
                    CenterPanel::Help => {
                        left_tabs.panel_view(panel_handle(help.clone()), cx)
                    }
                };
            }
            let active_index = active_center_panel
                .and_then(|panel| {
                    open_center_panels.iter().position(|open| *open == panel)
                })
                .map(|index| left_code_count + index)
                .unwrap_or_else(|| {
                    if active_is_right {
                        0
                    } else {
                        active_code_index
                    }
                });
            left_tabs = left_tabs.active_index(active_index);
            if right_code_tabs.is_empty() {
                area.set_center(left_tabs, window, cx);
            } else {
                let mut right_tabs = DockLayout::tabs();
                for code in right_code_tabs {
                    right_tabs = right_tabs.panel_view(panel_handle(code), cx);
                }
                right_tabs = right_tabs.active_index(active_code_index);
                area.set_center(
                    DockLayout::h_split()
                        .child(left_tabs, None)
                        .child(right_tabs, None),
                    window,
                    cx,
                );
            }
        });
        cx.notify();
    }

    fn close_center_panel(
        &mut self,
        panel: gpui_kit::component::dock::PanelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let search_panel =
            gpui_kit::component::dock::PanelId::from(self.search.entity_id());
        let settings_panel =
            gpui_kit::component::dock::PanelId::from(self.settings.entity_id());
        let extensions_panel =
            gpui_kit::component::dock::PanelId::from(self.extensions.entity_id());
        let problems_panel =
            gpui_kit::component::dock::PanelId::from(self.problems.entity_id());
        let help_panel =
            gpui_kit::component::dock::PanelId::from(self.help.entity_id());
        let closed = if panel == search_panel {
            CenterPanel::Search
        } else if panel == settings_panel {
            CenterPanel::Settings
        } else if panel == extensions_panel {
            CenterPanel::Extensions
        } else if panel == problems_panel {
            CenterPanel::Problems
        } else if panel == help_panel {
            CenterPanel::Help
        } else {
            return;
        };
        self.open_center_panels.retain(|open| *open != closed);
        if self.active_center_panel == Some(closed) {
            self.active_center_panel = self.open_center_panels.last().copied();
        }
        self.refresh_center_layout(window, cx);
    }

    fn poll_explorer_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let explorer_id = self.explorer.read(cx).mailbox_id;
        let Some(request) = crate::ross::take_open(explorer_id) else {
            return;
        };
        let shell = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            _ = shell.update(cx, |shell, cx| {
                shell.open_file_request(request, window, cx);
            });
        });
    }

    fn poll_editor_presentations(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let proxy = self.code.read_with(cx, |code, _| code.proxy.clone());
        let Some(proxy) = proxy else {
            return;
        };
        let requests = proxy.take_editor_presentation_requests();
        if requests.is_empty() {
            return;
        }
        let shell = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            for request in requests {
                let request_for_frame = request.clone();
                let proxy_for_frame = proxy.clone();
                let result = shell.update(cx, |shell, cx| {
                    let (applied, message) =
                        shell.apply_editor_presentation(&request, window, cx);
                    cx.notify();
                    cx.on_next_frame(window, move |_, _, cx| {
                        cx.background_spawn(async move {
                            if let Err(error) = proxy_for_frame
                                .answer_editor_presentation_request(
                                    request_for_frame,
                                    applied,
                                    message,
                                )
                            {
                                eprintln!("AHEAD could not acknowledge editor presentation: {error:?}");
                            }
                        })
                        .detach();
                    });
                });
                if result.is_err() {
                    let proxy = proxy.clone();
                    cx.background_spawn(async move {
                        if let Err(error) = proxy
                            .answer_editor_presentation_request(
                                request,
                                false,
                                "The editor closed before the presentation was applied.".to_string(),
                            )
                        {
                            eprintln!("AHEAD could not reject editor presentation: {error:?}");
                        }
                    })
                    .detach();
                }
            }
        });
    }

    fn apply_editor_presentation(
        &mut self,
        request: &EditorPresentationRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (bool, String) {
        let active_session_id = self
            .session
            .read(cx)
            .session_id
            .as_deref()
            .map(str::to_string);
        let Some(proxy) = self.code.read_with(cx, |code, _| code.proxy.clone())
        else {
            return (false, "The editor connection is unavailable.".to_string());
        };
        let active_turn_id = proxy.active_turn_id(&request.session_id);
        if !presentation_request_matches_active_turn(
            request,
            active_session_id.as_deref(),
            active_turn_id.as_deref(),
        ) {
            return (
                false,
                "This editor action belongs to a session or turn that is no longer active. Select that session and try again."
                    .to_string(),
            );
        }
        match request.action.clone() {
            ahead_rpc::ahead::AgentPresentationAction::Clear { cue_id } => {
                let mut cleared = false;
                for code in &self.code_tabs {
                    cleared |= code.update(cx, |code, cx| {
                        code.clear_presentation_for_session(
                            &request.session_id,
                            cue_id.as_deref(),
                            cx,
                        )
                    });
                }
                if cleared {
                    (true, "Code presentation cleared.".to_string())
                } else {
                    (
                        true,
                        "There was no matching code presentation to clear."
                            .to_string(),
                    )
                }
            }
            ahead_rpc::ahead::AgentPresentationAction::Present {
                cue_id,
                path,
                quote,
                label,
                note,
            } => {
                let workspace =
                    self.code.read_with(cx, |code, _| code.workspace.clone());
                if workspace.is_empty() {
                    return (
                        false,
                        "The editor has no active worktree.".to_string(),
                    );
                }
                let relative = std::path::Path::new(&path);
                if relative.as_os_str().is_empty()
                    || !relative.components().all(|component| {
                        matches!(component, std::path::Component::Normal(_))
                    })
                    || ahead_core::search::is_private_file(relative)
                {
                    return (
                        false,
                        "The requested path is not an allowed worktree file."
                            .to_string(),
                    );
                }
                let Some(target_path) = ahead_core::search::resolve_open_buffer_path(
                    std::path::Path::new(&workspace),
                    relative,
                ) else {
                    return (
                        false,
                        "The requested file is outside the active worktree."
                            .to_string(),
                    );
                };
                let target_path = target_path.canonicalize().unwrap_or(target_path);
                let target = self
                    .code_tabs
                    .iter()
                    .find(|code| {
                        let file_path =
                            std::path::PathBuf::from(&code.read(cx).file_path);
                        file_path == target_path
                            || file_path
                                .canonicalize()
                                .is_ok_and(|path| path == target_path)
                    })
                    .cloned();
                let is_new_tab = target.is_none();
                let code = target.unwrap_or_else(|| {
                    cx.new(|cx| {
                        crate::code_panel::CodePanel::new(
                            target_path.to_string_lossy().as_ref(),
                            window,
                            cx,
                        )
                        .with_proxy(
                            proxy.clone(),
                            &workspace,
                            window,
                            cx,
                        )
                    })
                });
                let presentation = code.update(cx, |code, cx| {
                    code.present_quote(
                        request.session_id.clone(),
                        cue_id.clone(),
                        path.clone(),
                        &quote,
                        label,
                        note,
                        cx,
                    )
                });
                if let Err(message) = presentation {
                    return (false, message);
                }
                for existing in &self.code_tabs {
                    if existing.entity_id() != code.entity_id() {
                        existing.update(cx, |code, cx| {
                            code.clear_presentation(None, cx);
                        });
                    }
                }
                if is_new_tab {
                    code.update(cx, |code, cx| {
                        code.is_preview = true;
                        cx.notify();
                    });
                    self.configure_code(code.clone(), window, cx);
                    self.code_tabs.push(code.clone());
                    if self.active_editor_group == EditorGroupSide::Right {
                        self.right_editor_tabs.insert(code.entity_id().as_u64());
                    }
                    let buffers = self.code_tabs.clone();
                    self.session.update(cx, |session, cx| {
                        session.set_buffers(buffers.clone(), cx);
                    });
                    self.search.update(cx, |search, cx| {
                        search.set_buffers(buffers, cx);
                    });
                }
                self.set_active_code(code, cx);
                self.show_code(window, cx);
                (
                    true,
                    format!(
                        "Displayed cue `{cue_id}` with the quoted code, label, and note; the human caret was preserved."
                    ),
                )
            }
            ahead_rpc::ahead::AgentPresentationAction::MovePointer {
                cue_id,
                quote,
            } => {
                let Some(code) = self
                    .code_tabs
                    .iter()
                    .find(|code| {
                        code.read(cx).has_presentation_for_session(
                            &request.session_id,
                            &cue_id,
                        )
                    })
                    .cloned()
                else {
                    return (
                        false,
                        "The requested presentation cue is no longer active."
                            .to_string(),
                    );
                };
                let result = code.update(cx, |code, cx| {
                    code.move_pointer(&request.session_id, &cue_id, &quote, cx)
                });
                if let Err(message) = result {
                    return (false, message);
                }
                self.set_active_code(code, cx);
                self.show_code(window, cx);
                (
                    true,
                    format!(
                        "Moved the agent pointer for cue `{cue_id}`; the highlighted range, note and human caret were preserved."
                    ),
                )
            }
            ahead_rpc::ahead::AgentPresentationAction::Speak { cue_id, text } => {
                if cue_id.as_deref().is_some_and(|cue_id| {
                    !self.code_tabs.iter().any(|code| {
                        code.read(cx).has_presentation_for_session(
                            &request.session_id,
                            cue_id,
                        )
                    })
                }) {
                    return (
                        false,
                        "The requested presentation cue is no longer active."
                            .to_string(),
                    );
                }
                match start_system_speech(&self.system_speech_process, &text) {
                    Ok(()) => {
                        self.system_speech_active.store(true, Ordering::SeqCst);
                        (true, "Started speaking the explanation.".to_string())
                    }
                    Err(error) => {
                        self.system_speech_active.store(false, Ordering::SeqCst);
                        (false, error)
                    }
                }
            }
            ahead_rpc::ahead::AgentPresentationAction::StopSpeaking => {
                let result = match stop_system_speech(&self.system_speech_process) {
                    Ok(true) => (true, "Stopped speaking.".to_string()),
                    Ok(false) => (true, "Speech was already stopped.".to_string()),
                    Err(error) => (false, error),
                };
                if result.0 {
                    self.system_speech_active.store(false, Ordering::SeqCst);
                }
                result
            }
        }
    }

    fn navigate_to_editor_location(
        &mut self,
        request: crate::ross::OpenRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .code
            .read_with(cx, |code, cx| code.navigation_request(cx));
        self.navigation_history.record_jump(current, &request);
        self.open_file_request(request, window, cx);
    }

    fn navigate_editor_history(
        &mut self,
        backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .code
            .read_with(cx, |code, cx| code.navigation_request(cx));
        let destination = if backward {
            self.navigation_history.go_back(current)
        } else {
            self.navigation_history.go_forward(current)
        };
        if let Some(destination) = destination {
            self.open_file_request(destination, window, cx);
        }
    }

    fn open_file_request(
        &mut self,
        request: crate::ross::OpenRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(code) = self
            .code_tabs
            .iter()
            .find(|code| {
                same_file_path(
                    std::path::Path::new(&code.read(cx).file_path),
                    std::path::Path::new(&request.path),
                )
            })
            .cloned()
        {
            if request.permanent {
                code.update(cx, |code, cx| code.promote_preview(cx));
            }
            self.set_active_code(code.clone(), cx);
            self.show_code(window, cx);
            if let Some(location) = request.location {
                code.update(cx, |code, cx| {
                    code.reveal_location(location, window, cx);
                });
            } else {
                let focus = code.read(cx).focus.clone();
                window.focus(&focus, cx);
            }
            return;
        }

        let code = if request.permanent {
            None
        } else {
            self.code_tabs.iter().position(|code| {
                code.read(cx).is_preview
                    && (self.active_editor_group == EditorGroupSide::Right)
                        == self
                            .right_editor_tabs
                            .contains(&code.entity_id().as_u64())
            })
        };
        let code = if let Some(index) = code {
            let code = self.code_tabs[index].clone();
            code.update(cx, |code, cx| {
                code.open_file(&request.path, true, window, cx);
            });
            code
        } else {
            let proxy = self.session.read(cx).proxy.clone();
            let workspace = self.explorer.read(cx).root.clone();
            let Some(proxy) = proxy else {
                return;
            };
            let code = cx.new(|cx| {
                crate::code_panel::CodePanel::new(&request.path, window, cx)
                    .with_proxy(proxy, &workspace, window, cx)
            });
            code.update(cx, |code, cx| {
                code.is_preview = !request.permanent;
                cx.notify();
            });
            self.configure_code(code.clone(), window, cx);
            self.code_tabs.push(code.clone());
            if self.active_editor_group == EditorGroupSide::Right {
                self.right_editor_tabs.insert(code.entity_id().as_u64());
            }
            let buffers = self.code_tabs.clone();
            self.session.update(cx, |session, cx| {
                session.set_buffers(buffers.clone(), cx);
            });
            self.search.update(cx, |search, cx| {
                search.set_buffers(buffers, cx);
            });
            code
        };

        self.set_active_code(code.clone(), cx);
        self.show_code(window, cx);
        if let Some(location) = request.location {
            code.update(cx, |code, cx| {
                code.reveal_location(location, window, cx);
            });
        } else {
            let focus = code.read(cx).focus.clone();
            window.focus(&focus, cx);
        }
    }

    fn open_language_server_config(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.explorer.read(cx).root.clone();
        match ahead_core::config::ensure_workspace_config(std::path::Path::new(
            &workspace,
        )) {
            Ok(path) => self.open_file_request(
                crate::ross::OpenRequest {
                    path: path.to_string_lossy().into_owned(),
                    permanent: true,
                    location: None,
                },
                window,
                cx,
            ),
            Err(error) => {
                self.status_message =
                    format!("Could not open language-server config: {error}");
                cx.notify();
            }
        }
    }

    pub(crate) fn open_shared_buffer(
        &mut self,
        session_id: String,
        snapshot: ahead_rpc::ahead::SharedBufferSnapshot,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editable = self.session.read(cx).shared_guest_can_edit();
        if let Some(code) = self
            .code_tabs
            .iter()
            .find(|code| {
                let code = code.read(cx);
                code.shared_session_id.as_deref() == Some(session_id.as_str())
                    && code.file_path == snapshot.path
            })
            .cloned()
        {
            code.update(cx, |code, cx| {
                code.apply_shared_snapshot(snapshot, window, cx);
                code.set_shared_editable(editable, cx);
                code.start_shared_poll(window, cx);
            });
            self.set_active_code(code.clone(), cx);
            self.show_code(window, cx);
            if let Some(line) = line {
                code.update(cx, |code, cx| {
                    code.reveal_location(
                        crate::ross::OpenLocation {
                            line: line as usize,
                            column: crate::ross::OpenColumn::Character(0),
                            end_line: line as usize,
                            end_column: crate::ross::OpenColumn::Character(0),
                        },
                        window,
                        cx,
                    )
                });
            }
            return;
        }
        let Some(proxy) = self.session.read(cx).proxy.clone() else {
            return;
        };
        let workspace = self.explorer.read(cx).root.clone();
        let code = cx.new(|cx| {
            crate::code_panel::CodePanel::new_shared(
                session_id, snapshot, editable, window, cx,
            )
            .with_proxy(proxy, &workspace, window, cx)
        });
        code.update(cx, |code, cx| code.start_shared_poll(window, cx));
        self.configure_code(code.clone(), window, cx);
        self.code_tabs.push(code.clone());
        if self.active_editor_group == EditorGroupSide::Right {
            self.right_editor_tabs.insert(code.entity_id().as_u64());
        }
        let buffers = self.code_tabs.clone();
        self.session.update(cx, |session, cx| {
            session.set_buffers(buffers.clone(), cx);
        });
        self.search
            .update(cx, |search, cx| search.set_buffers(buffers, cx));
        self.set_active_code(code.clone(), cx);
        self.show_code(window, cx);
        if let Some(line) = line {
            code.update(cx, |code, cx| {
                code.reveal_location(
                    crate::ross::OpenLocation {
                        line: line as usize,
                        column: crate::ross::OpenColumn::Character(0),
                        end_line: line as usize,
                        end_column: crate::ross::OpenColumn::Character(0),
                    },
                    window,
                    cx,
                )
            });
        }
    }

    pub(crate) fn restore_shared_draft(
        &mut self,
        session_id: String,
        draft: ahead_rpc::file::EditorRecoverySnapshot,
        host_snapshot: ahead_rpc::ahead::SharedBufferSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let path = host_snapshot.path.clone();
        self.open_shared_buffer(session_id.clone(), host_snapshot, None, window, cx);
        let code = self
            .code_tabs
            .iter()
            .find(|code| {
                let code = code.read(cx);
                code.shared_session_id.as_deref() == Some(session_id.as_str())
                    && code.file_path == path
            })
            .ok_or("Shared file could not be opened")?
            .clone();
        code.update(cx, |code, cx| code.restore_shared_draft(draft, window, cx))
    }

    pub(crate) fn open_shared_code_comment(
        &mut self,
        session_id: String,
        snapshot: ahead_rpc::ahead::SharedBufferSnapshot,
        line: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = snapshot.path.clone();
        self.open_shared_buffer(
            session_id.clone(),
            snapshot,
            Some(line),
            window,
            cx,
        );
        if let Some(code) = self.code_tabs.iter().find(|code| {
            let code = code.read(cx);
            code.shared_session_id.as_deref() == Some(session_id.as_str())
                && code.file_path == path
        }) {
            code.update(cx, |code, cx| {
                code.show_code_comment_line(line.saturating_add(1), cx)
            });
        }
    }

    pub(crate) fn follow_shared_line(
        &mut self,
        session_id: &str,
        path: &str,
        line: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(code) = self
            .code_tabs
            .iter()
            .find(|code| {
                let code = code.read(cx);
                code.shared_session_id.as_deref() == Some(session_id)
                    && code.file_path == path
            })
            .cloned()
        else {
            return false;
        };
        self.set_active_code(code.clone(), cx);
        self.show_code(window, cx);
        code.update(cx, |code, cx| {
            code.reveal_location(
                crate::ross::OpenLocation {
                    line: line as usize,
                    column: crate::ross::OpenColumn::Character(0),
                    end_line: line as usize,
                    end_column: crate::ross::OpenColumn::Character(0),
                },
                window,
                cx,
            )
        });
        true
    }

    pub(crate) fn follow_host_line(
        &mut self,
        path: &str,
        line: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.explorer.read(cx).root.clone();
        self.open_file_request(
            crate::ross::OpenRequest {
                path: std::path::Path::new(&root)
                    .join(path)
                    .to_string_lossy()
                    .to_string(),
                permanent: true,
                location: Some(crate::ross::OpenLocation {
                    line: line as usize,
                    column: crate::ross::OpenColumn::Character(0),
                    end_line: line as usize,
                    end_column: crate::ross::OpenColumn::Character(0),
                }),
            },
            window,
            cx,
        );
    }

    pub(crate) fn stop_shared_buffers(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        for code in &self.code_tabs {
            if code.read(cx).shared_session_id.as_deref() == Some(session_id) {
                code.update(cx, |code, cx| code.stop_shared_poll(cx));
            }
        }
    }

    pub(crate) fn sync_shared_editor_roles(&mut self, cx: &mut Context<Self>) {
        let editable = self.session.read(cx).shared_guest_can_edit();
        for code in &self.code_tabs {
            if code.read(cx).shared_session_id.is_some() {
                code.update(cx, |code, cx| code.set_shared_editable(editable, cx));
            }
        }
    }

    pub(crate) fn shared_presence_location(
        &self,
        session_id: &str,
        guest: bool,
        cx: &App,
    ) -> Option<(String, u32)> {
        let code = self.code.read(cx);
        let line = code.current_line(cx);
        if guest {
            return (code.shared_session_id.as_deref() == Some(session_id))
                .then(|| (code.file_path.clone(), line));
        }
        if code.shared_session_id.is_some() {
            return None;
        }
        let root = self.explorer.read(cx).root.clone();
        let path = std::path::Path::new(&code.file_path)
            .strip_prefix(root)
            .ok()?;
        if path.components().any(|component| {
            component.as_os_str().to_string_lossy().starts_with('.')
        }) {
            return None;
        }
        Some((path.to_string_lossy().to_string(), line))
    }

    fn window_close_snapshot(&self, cx: &App) -> WindowCloseSnapshot {
        WindowCloseSnapshot {
            buffers: self
                .code_tabs
                .iter()
                .map(|code| (code.entity_id(), code.read(cx).request_generation))
                .collect(),
            settings_generation: self.settings.read(cx).save_generation(),
        }
    }

    fn shutdown_workspace(&mut self, cx: &mut Context<Self>) {
        for code in &self.code_tabs {
            code.update(cx, |code, _| code.release_buffer());
        }
        for terminal in &self.terminals {
            shutdown_terminal(terminal, cx);
        }
        if let Some(proxy) = &self.session.read(cx).proxy {
            proxy.shutdown();
        }
    }

    fn queue_recoveries(
        &mut self,
        discard: bool,
        cx: &mut Context<Self>,
    ) -> Vec<RecoveryReply> {
        self.code_tabs
            .iter()
            .filter_map(|code| {
                code.update(cx, |code, cx| code.queue_recovery(discard, true, cx))
            })
            .collect()
    }

    fn restore_unsaved_buffers(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.restoring_buffers {
            return;
        }
        let Some(proxy) = self.session.read(cx).proxy.clone() else {
            return;
        };
        let workspace = self.explorer.read(cx).root.clone();
        self.restoring_buffers = true;
        cx.spawn_in(window, async move |this, cx| {
            let result: Result<(), String> = async {
                let entries = crate::proxy_client::await_editor_recovery(proxy.list_editor_recoveries(), cx.background_executor()).await?;
                proxy.enable_editor_recovery();
                for entry in entries {
                    if crate::code_panel::is_shared_draft_path(&entry.path) {
                        continue;
                    }
                    let already_open = this.read_with(cx, |this, cx| this.code_tabs.iter().any(|code| code.read(cx).recovery_id() == entry.buffer_id)).map_err(|error| error.to_string())?;
                    if already_open { continue; }
                    let Some(snapshot) = crate::proxy_client::await_editor_recovery(proxy.read_editor_recovery(entry.buffer_id), cx.background_executor()).await? else { continue; };
                    this.update_in(cx, |this, window, cx| {
                        // Database contents cannot select a file outside the active project.
                        if snapshot.path.as_os_str().is_empty() || !snapshot.path.components().all(|part| matches!(part, std::path::Component::Normal(_))) {
                            this.status_message = "Invalid recovery path retained in the database; no file was opened".into();
                            cx.notify();
                            return;
                        }
                        let path = std::path::Path::new(&workspace).join(&snapshot.path).to_string_lossy().into_owned();
                        if this.code_tabs.iter().any(|code| code.read(cx).file_path == path && code.read(cx).dirty) {
                            this.status_message = format!("Additional recovery for {path} was kept. Close its current tab, then use File > Recover Unsaved Changes.");
                            cx.notify();
                            return;
                        }
                        this.open_file_request(crate::ross::OpenRequest { path, permanent: true, location: None }, window, cx);
                        let code = this.code.clone();
                        if let Err(error) = code.update(cx, |code, cx| code.restore_recovery(snapshot, window, cx)) {
                            this.status_message = error;
                        }
                        cx.notify();
                    }).map_err(|error| error.to_string())?;
                }
                Ok(())
            }.await;
            this.update(cx, |this, cx| {
                this.restoring_buffers = false;
                if let Err(error) = result {
                    this.status_message = format!("Unsaved-buffer recovery unavailable: {error}");
                }
                cx.notify();
            })
        }).detach_and_log_err(cx);
    }

    fn prepare_window_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Option<WindowCloseSnapshot>> {
        if self.closing_code_tabs {
            return Task::ready(None);
        }
        self.closing_code_tabs = true;
        let targets = self.code_tabs.clone();
        let settings = self.settings.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = async {
                let mut buffers = Vec::new();
                for code in targets {
                    let generation = code
                        .update_in(cx, |code, window, cx| {
                            code.prepare_close(window, cx)
                        })
                        .ok()?
                        .await?;
                    buffers.push((code.entity_id(), generation));
                }
                let settings_generation = match settings
                    .update(cx, |settings, cx| settings.prepare_close(cx))
                    .await
                {
                    Ok(generation) => generation,
                    Err(error) => {
                        if let Err(error) = this.update(cx, |this, cx| {
                            this.status_message = error;
                            cx.notify();
                        }) {
                            eprintln!("Reporting settings save failure: {error}");
                        }
                        return None;
                    }
                };
                let snapshot = WindowCloseSnapshot {
                    buffers,
                    settings_generation,
                };
                this.read_with(cx, |this, cx| {
                    (this.window_close_snapshot(cx) == snapshot).then_some(snapshot)
                })
                .ok()
                .flatten()
            }
            .await;
            if let Err(error) = this.update(cx, |this, cx| {
                this.closing_code_tabs = false;
                cx.notify();
            }) {
                eprintln!("Finishing window close preparation: {error}");
            }
            result
        })
    }

    fn request_window_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if cx
            .try_global::<QuitInProgress>()
            .is_some_and(|state| state.0)
        {
            return;
        }
        let preparation = self.prepare_window_close(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(snapshot) = preparation.await else { return Ok(()); };
            let replies = this.update(cx, |this, cx| {
                (this.window_close_snapshot(cx) == snapshot).then(|| this.queue_recoveries(true, cx))
            })?;
            let recovery_result = match replies {
                Some(replies) => confirm_recovery_writes(replies, cx.background_executor()).await,
                None => Err("Files or settings changed while preparing to close. Review them and try again.".into()),
            };
            this.update_in(cx, |this, window, cx| {
                if this.window_close_snapshot(cx) != snapshot || recovery_result.is_err() {
                    for code in &this.code_tabs { code.update(cx, |code, cx| code.refresh_recovery(cx)); }
                    this.status_message = recovery_result.err().unwrap_or_else(|| "Files or settings changed while preparing to close. Review them and try again.".into());
                    cx.notify();
                    return;
                }
                this.window_close_authorized = true;
                if cfg!(debug_assertions) {
                    eprintln!("AHEAD window close: shutting down workspace proxy");
                }
                this.shutdown_workspace(cx);
                window.remove_window();
            })
        }).detach_and_log_err(cx);
    }

    fn close_code_tab(
        &mut self,
        panel: gpui_kit::component::dock::PanelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_code_tab_action(
            panel,
            crate::code_panel::CodeTabAction::Close,
            window,
            cx,
        );
    }

    fn run_active_code_tab_action(
        &mut self,
        action: crate::code_panel::CodeTabAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = gpui_kit::component::dock::PanelId::from(self.code.entity_id());
        self.handle_code_tab_action(panel, action, window, cx);
    }

    fn handle_code_tab_action(
        &mut self,
        panel: gpui_kit::component::dock::PanelId,
        action: crate::code_panel::CodeTabAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.code_tabs.iter().position(|code| {
            gpui_kit::component::dock::PanelId::from(code.entity_id()) == panel
        }) else {
            return;
        };

        let file_path = self.code_tabs[index].read(cx).file_path.clone();
        let workspace = self.explorer.read(cx).root.clone();
        let set_status =
            |shell: &mut Self, status: String, cx: &mut Context<Self>| {
                if let Some(code) = shell.code_tabs.get(index) {
                    code.update(cx, |code, cx| {
                        code.status = status.into();
                        cx.notify();
                    });
                }
            };
        match action {
            crate::code_panel::CodeTabAction::MoveToOtherEditorGroup => {
                let right_count = self
                    .code_tabs
                    .iter()
                    .filter(|code| {
                        self.right_editor_tabs.contains(&code.entity_id().as_u64())
                    })
                    .count();
                let left_count = self.code_tabs.len().saturating_sub(right_count);
                if self.code_tabs.len() < 2 {
                    set_status(
                        self,
                        "Open another file before creating a split editor group"
                            .into(),
                        cx,
                    );
                    return;
                }
                let moved_to = toggle_editor_group(
                    &mut self.right_editor_tabs,
                    self.code_tabs[index].entity_id().as_u64(),
                );
                if (moved_to == EditorGroupSide::Right && left_count == 1)
                    || (moved_to == EditorGroupSide::Left && right_count == 1)
                {
                    self.right_editor_tabs.clear();
                    self.right_editor_group_id = None;
                    self.active_editor_group = EditorGroupSide::Left;
                } else {
                    self.active_editor_group = moved_to;
                }
                let code = self.code_tabs[index].clone();
                self.set_active_code(code.clone(), cx);
                self.show_code(window, cx);
                let focus = code.read(cx).focus.clone();
                window.focus(&focus, cx);
                return;
            }
            crate::code_panel::CodeTabAction::CommentOnSelection => {
                let session_id = self.session.read(cx).session_id.clone();
                if session_id.is_none() {
                    set_status(
                        self,
                        "Start an AHEAD session before commenting".to_string(),
                        cx,
                    );
                    return;
                }
                let Some(target) =
                    self.code_tabs[index].read(cx).selected_code_comment(cx)
                else {
                    set_status(self, "Select code to comment on".to_string(), cx);
                    return;
                };
                self.code_tabs[index].update(cx, |code, cx| {
                    code.begin_code_comment(target, session_id, window, cx);
                });
                return;
            }
            crate::code_panel::CodeTabAction::CopyRelativePath => {
                let relative = crate::explorer_panel::relative_path(
                    std::path::Path::new(&workspace),
                    std::path::Path::new(&file_path),
                );
                cx.write_to_clipboard(ClipboardItem::new_string(
                    relative.to_string_lossy().into_owned(),
                ));
                set_status(self, "Copied relative path".to_string(), cx);
                return;
            }
            crate::code_panel::CodeTabAction::CopyAbsolutePath => {
                cx.write_to_clipboard(ClipboardItem::new_string(file_path.clone()));
                set_status(self, "Copied absolute path".to_string(), cx);
                return;
            }
            crate::code_panel::CodeTabAction::AddToGitignore => {
                let status = match crate::explorer_panel::add_to_gitignore(
                    std::path::Path::new(&workspace),
                    std::path::Path::new(&file_path),
                ) {
                    Ok(true) => "Added to .gitignore".to_string(),
                    Ok(false) => "Already in .gitignore".to_string(),
                    Err(error) => format!("Could not update .gitignore: {error}"),
                };
                set_status(self, status, cx);
                return;
            }
            crate::code_panel::CodeTabAction::RevealInFinder => {
                cx.reveal_path(std::path::Path::new(&file_path));
                return;
            }
            crate::code_panel::CodeTabAction::DuplicateFile => {
                let status = match crate::explorer_panel::duplicate_path(
                    std::path::Path::new(&file_path),
                ) {
                    Ok(destination) => {
                        format!("Duplicated {}", destination.display())
                    }
                    Err(error) => format!("Could not duplicate: {error}"),
                };
                self.explorer
                    .update(cx, |explorer, cx| explorer.refresh(cx));
                set_status(self, status, cx);
                return;
            }
            crate::code_panel::CodeTabAction::TrashFile => {
                self.trash_workspace_path(
                    std::path::PathBuf::from(file_path),
                    window,
                    cx,
                );
                return;
            }
            crate::code_panel::CodeTabAction::ViewHistory => {
                let relative = crate::explorer_panel::relative_path(
                    std::path::Path::new(&workspace),
                    std::path::Path::new(&file_path),
                );
                let command = format!(
                    "git log --follow --oneline -- {}",
                    shell_quote(&relative.to_string_lossy()),
                );
                self.new_terminal_in(workspace, Some(command), None, window, cx);
                return;
            }
            _ => {}
        }

        if matches!(action, crate::code_panel::CodeTabAction::Promote) {
            let code = self.code_tabs[index].clone();
            code.update(cx, |code, cx| code.promote_preview(cx));
            self.set_active_code(code, cx);
            self.show_code(window, cx);
            return;
        }

        if self.closing_code_tabs {
            return;
        }
        let targets: Vec<_> = self
            .code_tabs
            .iter()
            .enumerate()
            .filter(|(candidate, code)| {
                action == crate::code_panel::CodeTabAction::CloseAll
                    || (self.right_editor_tabs.contains(&code.entity_id().as_u64())
                        == self
                            .right_editor_tabs
                            .contains(&self.code_tabs[index].entity_id().as_u64())
                        && closes_code_tab(action, index, *candidate))
            })
            .map(|(_, code)| code.clone())
            .collect();
        self.closing_code_tabs = true;
        cx.spawn_in(window, async move |this, cx| {
            for code in targets {
                let Some(generation) = code
                    .update_in(cx, |code, window, cx| {
                        code.prepare_close(window, cx)
                    })?
                    .await
                else {
                    break;
                };
                this.update_in(cx, |this, window, cx| {
                    this.close_confirmed_code_tab(
                        code.entity_id(),
                        generation,
                        window,
                        cx,
                    )
                })?
                .await;
            }
            this.update(cx, |this, cx| {
                this.closing_code_tabs = false;
                cx.notify();
            })
        })
        .detach_and_log_err(cx);
    }

    pub(crate) fn open_code_comment(
        &mut self,
        comment: &ahead_rpc::ahead::CodeComment,
        cx: &mut Context<Self>,
    ) {
        let workspace = std::path::PathBuf::from(&self.explorer.read(cx).root);
        let Ok(workspace) = workspace.canonicalize() else {
            self.status_message = "Workspace unavailable".into();
            cx.notify();
            return;
        };
        let Ok(path) = workspace.join(&comment.path).canonicalize() else {
            self.status_message = "Commented file unavailable".into();
            cx.notify();
            return;
        };
        if !path.starts_with(&workspace) || !path.is_file() {
            self.status_message = "Commented file is outside the workspace".into();
            cx.notify();
            return;
        }
        let current_source = self
            .code_tabs
            .iter()
            .find(|code| code.read(cx).file_path == path.to_string_lossy())
            .and_then(|code| {
                code.read(cx)
                    .turn_context(cx)
                    .map(|context| context.file_content)
            })
            .or_else(|| std::fs::read_to_string(&path).ok());
        let source_matches = current_source.as_deref().is_some_and(|source| {
            format!("{:x}", sha2::Sha256::digest(source.as_bytes()))
                == comment.source_sha256
        });
        let current_range = current_source.as_deref().and_then(|source| {
            crate::code_panel::current_comment_range(comment, source)
        });
        if !source_matches {
            self.status_message = if current_range.is_some() {
                "Commented source changed; found the quoted code at its new location"
            } else {
                "Commented source changed; original line may be stale"
            }
            .into();
        }
        let range = current_range.unwrap_or(comment.range);
        let location = crate::ross::OpenLocation {
            line: range.start.line as usize,
            column: crate::ross::OpenColumn::Utf16(range.start.col as usize),
            end_line: if current_range.is_some() {
                range.end.line
            } else {
                range.start.line
            } as usize,
            end_column: crate::ross::OpenColumn::Utf16(if current_range.is_some() {
                range.end.col
            } else {
                range.start.col
            } as usize),
        };
        crate::ross::request_open_at(
            self.explorer.read(cx).mailbox_id,
            &path.to_string_lossy(),
            location,
        );
        let path_text = path.to_string_lossy().into_owned();
        let line = range.start.line.saturating_add(1);
        if let Some(code) = self
            .code_tabs
            .iter()
            .find(|code| code.read(cx).file_path == path_text)
        {
            code.update(cx, |code, cx| code.show_code_comment_line(line, cx));
        } else {
            self.pending_comment = Some((path_text, line));
        }
        cx.notify();
    }

    fn trash_workspace_path(
        &mut self,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closing_code_tabs {
            return;
        }
        let Some(proxy) = self.session.read(cx).proxy.clone() else {
            self.status_message =
                "Cannot move to Trash: project connection unavailable".into();
            cx.notify();
            return;
        };
        let snapshot = self
            .code_tabs
            .iter()
            .filter(|code| {
                std::path::Path::new(&code.read(cx).file_path).starts_with(&path)
            })
            .map(|code| (code.entity_id(), code.read(cx).request_generation))
            .collect::<Vec<_>>();
        let dirty_count = self
            .code_tabs
            .iter()
            .filter(|code| {
                std::path::Path::new(&code.read(cx).file_path).starts_with(&path)
                    && code.read(cx).dirty
            })
            .count();
        let detail = if dirty_count == 0 {
            format!(
                "{}\nYou can recover it from the system Trash.",
                path.display()
            )
        } else {
            format!(
                "{}\n{dirty_count} open file(s) have unsaved changes that will be lost. Only their saved contents can be recovered from Trash.",
                path.display()
            )
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            "Move to Trash?",
            Some(&detail),
            &["Move to Trash", "Cancel"],
            cx,
        );
        self.closing_code_tabs = true;
        cx.spawn_in(window, async move |this, cx| {
            let result: anyhow::Result<Option<Result<(), String>>> = async {
                if answer.await.ok() != Some(0) { return Ok(None); }
                let unchanged = this.read_with(cx, |this, cx| {
                    let current: Vec<_> = this.code_tabs.iter().filter(|code| std::path::Path::new(&code.read(cx).file_path).starts_with(&path))
                        .map(|code| (code.entity_id(), code.read(cx).request_generation)).collect();
                    current == snapshot
                })?;
                if !unchanged { return Ok(Some(Err("Files changed while the Trash confirmation was open. Review them and try again.".to_string()))); }
                let receiver = proxy.trash_path(path);
                let mut reply = std::pin::pin!(receiver.recv());
                let mut deadline = std::pin::pin!(cx.background_executor().timer(std::time::Duration::from_secs(30)));
                let result = std::future::poll_fn(|cx| {
                    use std::task::Poll;
                    if let Poll::Ready(result) = reply.as_mut().poll(cx) {
                        return Poll::Ready(result.unwrap_or_else(|error| Err(error.to_string())));
                    }
                    if deadline.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(Err("Trash acknowledgement timed out. The path may have moved; keep these buffers open and check the filesystem before retrying.".into()));
                    }
                    Poll::Pending
                }).await;
                Ok(Some(result))
            }.await;
            if matches!(&result, Ok(Some(Ok(())))) {
                for (entity, generation) in snapshot {
                    this.update_in(cx, |this, window, cx| {
                        this.close_confirmed_code_tab(entity, generation, window, cx)
                    })?.await;
                }
            }
            this.update(cx, |this, cx| {
                this.closing_code_tabs = false;
                match result {
                    Ok(Some(Ok(()))) => {
                        this.explorer.update(cx, |explorer, cx| explorer.refresh(cx));
                        this.status_message = "Moved to Trash. Buffers that could not safely close remain open; check their status.".into();
                    }
                    Ok(None) => {}
                    Ok(Some(Err(error))) => this.status_message = format!("Could not move to Trash: {error}"),
                    Err(error) => this.status_message = format!("Could not move to Trash: {error}"),
                }
                cx.notify();
            })
        }).detach_and_log_err(cx);
    }

    fn close_confirmed_code_tab(
        &mut self,
        entity_id: EntityId,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let Some(code) = self
            .code_tabs
            .iter()
            .find(|code| {
                code.entity_id() == entity_id
                    && code.read(cx).request_generation == generation
            })
            .cloned()
        else {
            return Task::ready(());
        };
        let Some(reply) =
            code.update(cx, |code, cx| code.queue_recovery(true, true, cx))
        else {
            self.remove_confirmed_code_tab(entity_id, generation, window, cx);
            return Task::ready(());
        };
        cx.spawn_in(window, async move |this, cx| {
            let result =
                confirm_recovery_writes(vec![reply], cx.background_executor()).await;
            if let Err(error) = this.update_in(cx, |this, window, cx| {
                if result.is_ok() && code.read(cx).request_generation == generation {
                    this.remove_confirmed_code_tab(
                        entity_id, generation, window, cx,
                    );
                } else {
                    let message = result
                        .err()
                        .unwrap_or_else(|| "Newer edits were kept open".into());
                    code.update(cx, |code, cx| {
                        code.refresh_recovery(cx);
                        code.status = message.clone().into();
                        cx.notify();
                    });
                    this.status_message = message;
                    cx.notify();
                }
            }) {
                eprintln!("Closing a recovered buffer: {error}");
            }
        })
    }

    fn remove_confirmed_code_tab(
        &mut self,
        entity_id: EntityId,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .code_tabs
            .iter()
            .position(|code| code.entity_id() == entity_id)
        else {
            return;
        };
        if self.code_tabs[index].read(cx).request_generation != generation {
            return;
        }
        self.code_tabs[index].update(cx, |code, _| code.release_buffer());
        self.right_editor_tabs.remove(&entity_id.as_u64());
        self.code_tabs.remove(index);
        let right_count = self
            .code_tabs
            .iter()
            .filter(|code| {
                self.right_editor_tabs.contains(&code.entity_id().as_u64())
            })
            .count();
        if right_count == 0 || right_count == self.code_tabs.len() {
            self.right_editor_tabs.clear();
            self.right_editor_group_id = None;
            self.active_editor_group = EditorGroupSide::Left;
        }
        let buffers = self.code_tabs.clone();
        self.session.update(cx, |session, cx| {
            session.set_buffers(buffers.clone(), cx);
        });
        self.search.update(cx, |search, cx| {
            search.set_buffers(buffers, cx);
        });

        let active_code = if self
            .code_tabs
            .iter()
            .any(|code| code.entity_id() == self.code.entity_id())
        {
            Some(self.code.clone())
        } else {
            self.code_tabs
                .get(index.min(self.code_tabs.len().saturating_sub(1)))
                .cloned()
        };
        if let Some(code) = active_code {
            self.set_active_code(code, cx);
        } else if self.code_tabs.is_empty() {
            self.session.update(cx, |session, _| session.code = None);
        }
        self.refresh_center_layout(window, cx);
    }

    fn set_bottom_layout(
        &mut self,
        debug_active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let debug_active = debug_active && self.debug_visible;
        self.bottom_debug_active = debug_active;
        let terminals = self.terminals.clone();
        let debug_bar = self.debug_bar.clone();
        let debug_visible = self.debug_visible;
        let terminal_count = terminals.len();
        let active_terminal =
            self.active_terminal.min(terminal_count.saturating_sub(1));
        self.active_terminal = active_terminal;
        self.area.update(cx, |area, cx| {
            let mut bottom = DockLayout::tabs();
            for terminal in terminals {
                bottom = bottom.panel_view(panel_handle(terminal), cx);
            }
            if debug_visible {
                bottom = bottom.panel_view(panel_handle(debug_bar), cx);
            }
            let active_index = if debug_active {
                terminal_count
            } else {
                active_terminal
            };
            let bottom = bottom.active_index(active_index);
            area.set_dock(DockPlacement::Bottom, bottom, window, cx);
            area.set_dock_size(DockPlacement::Bottom, px(250.), window, cx);
        });
        cx.notify();
    }

    fn close_bottom_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bottom_debug_active = false;
        self.area.update(cx, |area, cx| {
            area.remove_dock(DockPlacement::Bottom, window, cx);
        });
        cx.notify();
    }

    fn show_debug(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.debug_visible = true;
        self.set_bottom_layout(true, window, cx);
        self.area.update(cx, |area, cx| {
            if !area.is_dock_open(DockPlacement::Bottom) {
                area.toggle_dock(DockPlacement::Bottom, window, cx);
            }
        });
        cx.notify();
    }

    fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminals.is_empty() {
            self.new_terminal(window, cx);
            return;
        }
        let bottom_open = self.area.read(cx).is_dock_open(DockPlacement::Bottom);
        if bottom_open && !self.bottom_debug_active {
            self.area.update(cx, |area, cx| {
                area.toggle_dock(DockPlacement::Bottom, window, cx);
            });
        } else {
            self.set_bottom_layout(false, window, cx);
            self.area.update(cx, |area, cx| {
                if !area.is_dock_open(DockPlacement::Bottom) {
                    area.toggle_dock(DockPlacement::Bottom, window, cx);
                }
            });
        }
        cx.notify();
    }

    fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.code.read(cx).workspace.clone();
        self.new_terminal_in(cwd, None, None, window, cx);
    }

    fn new_task_terminal(
        &mut self,
        recipe: String,
        cwd: String,
        justfile: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_terminal_in(
            cwd,
            Some(just_task_terminal_command(&justfile, &recipe, false)),
            Some(recipe),
            window,
            cx,
        );
    }

    fn watch_just_task_terminal(
        &mut self,
        recipe: String,
        cwd: String,
        justfile: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_terminal_in(
            cwd,
            Some(just_task_terminal_command(&justfile, &recipe, true)),
            Some(format!("watch: {recipe}")),
            window,
            cx,
        );
    }

    fn new_terminal_in(
        &mut self,
        cwd: String,
        command: Option<String>,
        title: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let terminal_id = self.next_terminal_id;
        self.next_terminal_id += 1;
        let terminal = cx.new(|cx| match title {
            Some(title) => {
                crate::terminal_panel::TerminalPanel::new_with_cwd_and_title(
                    terminal_id,
                    cwd,
                    title,
                    cx,
                )
            }
            None => crate::terminal_panel::TerminalPanel::new_with_cwd(
                terminal_id,
                cwd,
                cx,
            ),
        });
        self.configure_terminal(terminal.clone(), cx);
        self.terminals.push(terminal);
        self.active_terminal = self.terminals.len() - 1;
        self.set_bottom_layout(false, window, cx);
        self.area.update(cx, |area, cx| {
            if !area.is_dock_open(DockPlacement::Bottom) {
                area.toggle_dock(DockPlacement::Bottom, window, cx);
            }
        });
        let terminal = self.terminals[self.active_terminal].clone();
        let focus = terminal.read(cx).focus.clone();
        window.focus(&focus, cx);
        if let Some(command) = command {
            cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                cx.update_entity(&terminal, |terminal, cx| {
                    terminal.run_command(&command, cx);
                });
            })
            .detach();
        }
        cx.notify();
    }

    pub(crate) fn open_agent_auth_terminal(
        &mut self,
        launch: ahead_rpc::ahead::AgentTerminalAuthLaunch,
        exit_handler: impl Fn(
            Option<i32>,
            &mut Context<crate::terminal_panel::TerminalPanel>,
        ) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let terminal_id = self.next_terminal_id;
        self.next_terminal_id += 1;
        let terminal = cx.new(|cx| {
            crate::terminal_panel::TerminalPanel::new_for_agent_auth(
                terminal_id,
                launch,
                exit_handler,
                cx,
            )
        });
        if terminal.read(cx).process_id().is_none() {
            shutdown_terminal(&terminal, cx);
            return Err("Could not start the agent sign-in terminal".to_string());
        }
        self.configure_terminal(terminal.clone(), cx);
        self.terminals.push(terminal.clone());
        self.active_terminal = self.terminals.len() - 1;
        self.set_bottom_layout(false, window, cx);
        self.area.update(cx, |area, cx| {
            if !area.is_dock_open(DockPlacement::Bottom) {
                area.toggle_dock(DockPlacement::Bottom, window, cx);
            }
        });
        let focus = terminal.read(cx).focus.clone();
        window.focus(&focus, cx);
        cx.notify();
        Ok(())
    }

    fn start_debug_terminal_pump(
        &mut self,
        proxy: Arc<crate::proxy_client::ProxyClient>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let requests = proxy.subscribe_debug_terminals();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = requests.recv().await {
                if this
                    .update_in(cx, |shell, window, cx| match event {
                        DebugTerminalEvent::Request(request) => {
                            shell.open_debug_terminal(
                                request,
                                proxy.clone(),
                                window,
                                cx,
                            );
                        }
                        DebugTerminalEvent::Retire(dap_id) => {
                            if let Some(ids) = shell.debug_terminals.remove(&dap_id)
                            {
                                for id in ids {
                                    shell.close_terminal(id, window, cx);
                                }
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn open_debug_terminal(
        &mut self,
        request: ahead_rpc::dap_types::DebugTerminalRequest,
        proxy: Arc<crate::proxy_client::ProxyClient>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = proxy.debug();
        if state.dap_id != Some(request.dap_id) || !state.state.can_stop() {
            proxy.reply_debug_terminal(
                &request,
                Err("debug session is stopping".into()),
            );
            return;
        }
        let command = match debug_terminal_command(&request.arguments) {
            Ok(command) => command,
            Err(error) => {
                proxy.reply_debug_terminal(&request, Err(error));
                return;
            }
        };
        let cwd = request
            .arguments
            .cwd
            .as_deref()
            .filter(|cwd| !cwd.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| self.explorer.read(cx).root.clone());
        let terminal_id = self.next_terminal_id;
        self.next_terminal_id += 1;
        let terminal = cx.new(|cx| {
            crate::terminal_panel::TerminalPanel::new_with_cwd(terminal_id, cwd, cx)
        });
        let Some(pid) = terminal.read(cx).process_id() else {
            shutdown_terminal(&terminal, cx);
            proxy.reply_debug_terminal(
                &request,
                Err("Could not start debug terminal or obtain its process ID".into()),
            );
            return;
        };
        self.configure_terminal(terminal.clone(), cx);
        self.terminals.push(terminal.clone());
        self.debug_terminals
            .entry(request.dap_id)
            .or_default()
            .push(terminal_id);
        self.active_terminal = self.terminals.len() - 1;
        self.set_bottom_layout(false, window, cx);
        self.area.update(cx, |area, cx| {
            if !area.is_dock_open(DockPlacement::Bottom) {
                area.toggle_dock(DockPlacement::Bottom, window, cx);
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(100))
                .await;
            let result = this
                .update_in(cx, |shell, window, cx| {
                    let state = proxy.debug();
                    if state.dap_id != Some(request.dap_id)
                        || !state.state.can_stop()
                        || !shell.terminals.iter().any(|current| {
                            current.read(cx).terminal_id() == terminal_id
                        })
                    {
                        shell.close_terminal(terminal_id, window, cx);
                        return Err("debug terminal request was cancelled".into());
                    }
                    if let Some(command) = &command {
                        if let Err(error) = terminal.update(cx, |terminal, cx| {
                            terminal.run_debug_command(command, cx)
                        }) {
                            shell.close_terminal(terminal_id, window, cx);
                            return Err(error);
                        }
                    }
                    Ok(pid)
                })
                .unwrap_or_else(|error| Err(error.to_string()));
            proxy.reply_debug_terminal(&request, result);
        })
        .detach();
        cx.notify();
    }

    fn focused_terminal_id(&self, window: &Window, cx: &App) -> Option<usize> {
        self.terminals.iter().find_map(|terminal| {
            let terminal = terminal.read(cx);
            terminal
                .focus
                .contains_focused(window, cx)
                .then(|| terminal.terminal_id())
        })
    }

    fn close_terminal(
        &mut self,
        terminal_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_terminal_set(&[terminal_id], terminal_id, window, cx);
    }

    fn close_terminal_tab_action(
        &mut self,
        terminal_id: usize,
        action: crate::terminal_panel::TerminalTabAction,
        panel_order: Vec<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::terminal_panel::TerminalTabAction;

        let mut ordered_ids = panel_order
            .iter()
            .filter_map(|panel_id| {
                self.terminals
                    .iter()
                    .find(|terminal| terminal.entity_id().as_u64() == *panel_id)
                    .map(|terminal| terminal.read(cx).terminal_id())
            })
            .collect::<Vec<_>>();
        for terminal in &self.terminals {
            let id = terminal.read(cx).terminal_id();
            if !ordered_ids.contains(&id) {
                ordered_ids.push(id);
            }
        }
        let Some(target_index) =
            ordered_ids.iter().position(|id| *id == terminal_id)
        else {
            return;
        };
        let close_ids = match action {
            TerminalTabAction::Close => vec![terminal_id],
            TerminalTabAction::CloseOthers => ordered_ids
                .iter()
                .copied()
                .filter(|id| *id != terminal_id)
                .collect(),
            TerminalTabAction::CloseLeft => ordered_ids[..target_index].to_vec(),
            TerminalTabAction::CloseRight => {
                ordered_ids[target_index + 1..].to_vec()
            }
            // Terminal tabs have no unsaved buffer state, so every tab is clean.
            TerminalTabAction::CloseClean => ordered_ids.clone(),
            TerminalTabAction::CloseAll => ordered_ids.clone(),
        };
        if close_ids.is_empty() {
            return;
        }

        // Keep the dock's tab order when rebuilding its layout after a close.
        let mut remaining = std::mem::take(&mut self.terminals);
        let mut reordered = Vec::with_capacity(remaining.len());
        for id in &ordered_ids {
            if let Some(index) = remaining
                .iter()
                .position(|terminal| terminal.read(cx).terminal_id() == *id)
            {
                reordered.push(remaining.remove(index));
            }
        }
        reordered.extend(remaining);
        self.terminals = reordered;
        self.close_terminal_set(&close_ids, terminal_id, window, cx);
    }

    fn close_terminal_set(
        &mut self,
        terminal_ids: &[usize],
        preferred_active_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active_id = self
            .terminals
            .get(self.active_terminal)
            .map(|terminal| terminal.read(cx).terminal_id());
        let previous_active_index = self.active_terminal;
        for terminal_id in terminal_ids {
            let Some(index) = self.terminals.iter().position(|terminal| {
                terminal.read(cx).terminal_id() == *terminal_id
            }) else {
                continue;
            };
            for ids in self.debug_terminals.values_mut() {
                ids.retain(|id| id != terminal_id);
            }
            shutdown_terminal(&self.terminals.remove(index), cx);
        }

        if self.terminals.is_empty() {
            if self.debug_visible {
                self.set_bottom_layout(true, window, cx);
            } else {
                self.close_bottom_panel(window, cx);
            }
        } else {
            let next_active_id = active_id
                .filter(|id| {
                    self.terminals
                        .iter()
                        .any(|terminal| terminal.read(cx).terminal_id() == *id)
                })
                .or_else(|| {
                    self.terminals
                        .iter()
                        .any(|terminal| {
                            terminal.read(cx).terminal_id() == preferred_active_id
                        })
                        .then_some(preferred_active_id)
                });
            self.active_terminal = next_active_id
                .and_then(|id| {
                    self.terminals
                        .iter()
                        .position(|terminal| terminal.read(cx).terminal_id() == id)
                })
                .unwrap_or_else(|| {
                    previous_active_index.min(self.terminals.len() - 1)
                });
            self.set_bottom_layout(self.bottom_debug_active, window, cx);
        }
        cx.notify();
    }

    fn select_terminal_panel(
        &mut self,
        panel_entity_id: u64,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self
            .terminals
            .iter()
            .position(|terminal| terminal.entity_id().as_u64() == panel_entity_id)
            && self.active_terminal != index
        {
            self.active_terminal = index;
            cx.notify();
        }
    }

    fn close_debug(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.debug_visible = false;
        if self.terminals.is_empty() {
            self.close_bottom_panel(window, cx);
        } else {
            self.set_bottom_layout(false, window, cx);
        }
    }

    fn configure_terminal(
        &mut self,
        terminal: Entity<crate::terminal_panel::TerminalPanel>,
        cx: &mut Context<Self>,
    ) {
        let shell = cx.weak_entity();
        let terminal_id = terminal.read(cx).terminal_id();
        let panel_entity_id = terminal.entity_id().as_u64();
        terminal.update(cx, |terminal, _| {
            let shell_for_actions = shell.clone();
            terminal.set_tab_action_handler(
                move |action, panel_order, window, cx| {
                    _ = shell_for_actions.update(cx, |shell, cx| {
                        shell.close_terminal_tab_action(
                            terminal_id,
                            action,
                            panel_order,
                            window,
                            cx,
                        );
                    });
                },
            );
            let shell_for_active = shell.clone();
            terminal.set_active_handler(move |_, cx| {
                _ = shell_for_active.update(cx, |shell, cx| {
                    shell.select_terminal_panel(panel_entity_id, cx);
                });
            });
            let shell_for_new = shell.clone();
            terminal.set_new_terminal_handler(move |window, cx| {
                _ = shell_for_new.update(cx, |shell, cx| {
                    shell.new_terminal(window, cx);
                });
            });
        });
    }

    fn toggle_chat_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat_zoomed {
            self.chat_zoomed = false;
            self.agent_workspace
                .update(cx, |panel, cx| panel.set_chat_zoomed(false, cx));
            self.show_code(window, cx);
            self.restore_side_docks(window, cx);
        } else {
            self.left_dock_was_open =
                self.area.read(cx).is_dock_open(DockPlacement::Left);
            self.right_dock_was_open =
                self.area.read(cx).is_dock_open(DockPlacement::Right);
            self.bottom_dock_was_open =
                self.area.read(cx).is_dock_open(DockPlacement::Bottom);
            self.chat_zoomed = true;
            self.active_center_panel = None;
            self.agent_workspace
                .update(cx, |panel, cx| panel.set_chat_zoomed(true, cx));

            let session = self.session.clone();
            let agent_workspace = self.agent_workspace.clone();
            let threads_visible = self.threads_visible;
            self.area.update(cx, |area, cx| {
                area.remove_dock(DockPlacement::Left, window, cx);
                area.remove_dock(DockPlacement::Bottom, window, cx);
                area.set_center(
                    DockLayout::tabs().panel_view(panel_handle(session), cx),
                    window,
                    cx,
                );
                if threads_visible {
                    area.set_dock(
                        DockPlacement::Right,
                        DockLayout::tabs()
                            .panel_view(panel_handle(agent_workspace), cx),
                        window,
                        cx,
                    );
                    area.set_dock_size(DockPlacement::Right, px(300.), window, cx);
                } else {
                    area.remove_dock(DockPlacement::Right, window, cx);
                }
            });
            let focus = self.session.read(cx).focus.clone();
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn restore_side_docks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let agent_workspace = self.agent_workspace.clone();
        let left_dock_was_open = self.left_dock_was_open;
        let right_dock_was_open = self.right_dock_was_open;
        let bottom_dock_was_open = self.bottom_dock_was_open;

        self.activity.update(cx, |activity, cx| {
            activity.show_current(window, cx);
        });
        self.area.update(cx, |area, cx| {
            let right =
                DockLayout::tabs().panel_view(panel_handle(agent_workspace), cx);
            area.set_dock(DockPlacement::Right, right, window, cx);
            area.set_dock_size(DockPlacement::Right, px(640.), window, cx);
            area.set_dock_size(DockPlacement::Left, px(240.), window, cx);
            if !left_dock_was_open {
                area.toggle_dock(DockPlacement::Left, window, cx);
            }
            if !right_dock_was_open {
                area.toggle_dock(DockPlacement::Right, window, cx);
            }
        });
        self.set_bottom_layout(false, window, cx);
        if !bottom_dock_was_open {
            self.area.update(cx, |area, cx| {
                area.toggle_dock(DockPlacement::Bottom, window, cx);
            });
        }
    }

    pub(crate) fn set_threads_visible(
        &mut self,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changed = self.threads_visible != visible;
        self.threads_visible = visible;
        self.agent_workspace
            .update(cx, |panel, cx| panel.set_threads_visible(visible, cx));
        if !changed {
            return;
        }
        if self.chat_zoomed {
            if visible {
                let agent_workspace = self.agent_workspace.clone();
                self.area.update(cx, |area, cx| {
                    area.set_dock(
                        DockPlacement::Right,
                        DockLayout::tabs()
                            .panel_view(panel_handle(agent_workspace), cx),
                        window,
                        cx,
                    );
                    area.set_dock_size(DockPlacement::Right, px(300.), window, cx);
                });
            } else {
                self.area.update(cx, |area, cx| {
                    area.remove_dock(DockPlacement::Right, window, cx);
                });
            }
        }
        cx.notify();
    }

    fn run_shell_command(
        &mut self,
        command: ShellShortcut,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use ShellShortcut::*;
        match command {
            ToggleLeft => self.area.update(cx, |area, cx| {
                area.toggle_dock(DockPlacement::Left, window, cx)
            }),
            ToggleBottom => self.area.update(cx, |area, cx| {
                area.toggle_dock(DockPlacement::Bottom, window, cx)
            }),
            ToggleRight => self.area.update(cx, |area, cx| {
                area.toggle_dock(DockPlacement::Right, window, cx)
            }),
            Settings => self.open_center_panel(CenterPanel::Settings, window, cx),
            Extensions => self.open_extensions(window, cx),
            Help => self.open_center_panel(CenterPanel::Help, window, cx),
            Explorer => self.show_left_panel(WorkspaceView::Explorer, window, cx),
            Search => self.open_center_panel(CenterPanel::Search, window, cx),
            SourceControl => self.show_left_panel(WorkspaceView::Git, window, cx),
            StageAllChanges => {
                self.activity.update(cx, |activity, cx| {
                    activity.stage_all_changes(window, cx)
                });
                self.show_left_panel(WorkspaceView::Git, window, cx);
            }
            CommitStagedChanges => self.activity.update(cx, |activity, cx| {
                activity.commit_staged_changes(window, cx)
            }),
            Tasks => self.show_left_panel(WorkspaceView::Tasks, window, cx),
            LanguageServers => {
                self.show_left_panel(WorkspaceView::LanguageServers, window, cx)
            }
            Problems => self.open_center_panel(CenterPanel::Problems, window, cx),
            QuickOpen => self.open_quick_open(window, cx),
            GoToLine => self.open_go_to_line(window, cx),
            OpenFileDialog => request_open_path(file_prompt_options(), cx),
            OpenFolderDialog => request_open_path(workspace_prompt_options(), cx),
            SaveActiveFile => {
                if !self.code_tabs.is_empty() {
                    self.code.update(cx, |code, cx| code.save(window, cx));
                }
            }
            CloseActiveItem => {
                if let Some(terminal_id) = self.focused_terminal_id(window, cx) {
                    self.close_terminal(terminal_id, window, cx);
                } else if !self.code_tabs.is_empty()
                    && self.code.read(cx).focus.contains_focused(window, cx)
                {
                    self.close_code_tab(
                        gpui_kit::component::dock::PanelId::from(
                            self.code.entity_id(),
                        ),
                        window,
                        cx,
                    );
                }
            }
            CloseEditor => {
                if !self.code_tabs.is_empty() {
                    self.close_code_tab(
                        gpui_kit::component::dock::PanelId::from(
                            self.code.entity_id(),
                        ),
                        window,
                        cx,
                    );
                }
            }
            CloseOtherEditors => self.run_active_code_tab_action(
                crate::code_panel::CodeTabAction::CloseOthers,
                window,
                cx,
            ),
            CloseEditorsToLeft => self.run_active_code_tab_action(
                crate::code_panel::CodeTabAction::CloseLeft,
                window,
                cx,
            ),
            CloseEditorsToRight => self.run_active_code_tab_action(
                crate::code_panel::CodeTabAction::CloseRight,
                window,
                cx,
            ),
            CloseAllEditors => self.run_active_code_tab_action(
                crate::code_panel::CodeTabAction::CloseAll,
                window,
                cx,
            ),
            MoveEditorToOtherGroup => self.run_active_code_tab_action(
                crate::code_panel::CodeTabAction::MoveToOtherEditorGroup,
                window,
                cx,
            ),
            Editor(command) => self
                .code
                .update(cx, |code, cx| code.run_command(command, window, cx)),
            NavigateBack => self.navigate_editor_history(true, window, cx),
            NavigateForward => self.navigate_editor_history(false, window, cx),
            CommandPalette => self.open_command_palette(window, cx),
            NewTerminal => self.new_terminal(window, cx),
            Debug => self.show_debug(window, cx),
            DebugKey(key, shift) => {
                let (file_path, line) = self.code.read_with(cx, |code, _| {
                    (code.file_path.clone(), code.active_line)
                });
                self.debug_bar.update(cx, |debug_bar, cx| {
                    debug_bar.file_path = file_path;
                    debug_bar.active_line = line;
                    debug_bar.handle_shortcut(key, shift, cx);
                });
            }
            Agent => {
                if !self.area.read(cx).is_dock_open(DockPlacement::Right) {
                    self.area.update(cx, |area, cx| {
                        area.toggle_dock(DockPlacement::Right, window, cx)
                    });
                }
                let focus = self.session.read_with(cx, |session, cx| {
                    if session.session_id.is_some() {
                        session.chat_input.focus_handle(cx)
                    } else {
                        session.focus.clone()
                    }
                });
                window.focus(&focus, cx);
            }
            NewAheadSession => self.session.update(cx, |session, cx| {
                session.open_thread(ahead_rpc::ahead::HarnessKind::Ahead, window, cx)
            }),
            NewExternalAgent => self.session.update(cx, |session, cx| {
                session.open_thread(
                    ahead_rpc::ahead::HarnessKind::ExternalAcp,
                    window,
                    cx,
                )
            }),
            ChatZoom => self.toggle_chat_zoom(window, cx),
            Threads => {
                let next = !self.threads_visible;
                self.set_threads_visible(next, window, cx);
            }
        }
        cx.notify();
    }

    fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_editor = !self.code_tabs.is_empty();
        let text_editor = active_editor && self.code.read(cx).is_text_available();
        let language_editor = text_editor
            && self.code.read_with(cx, |code, _| {
                code.proxy.as_ref().is_some_and(|proxy| {
                    proxy.lsp_servers().iter().any(|server| server.is_ready())
                })
            });
        let debug_proxy = self.debug_bar.read(cx).proxy.clone();
        let debug = debug_proxy.debug();
        let can_step_debugging = debug.state
            == ahead_rpc::dap_types::DapSessionState::Stopped
            && debug.thread_id.is_some();
        let (can_commit_staged, tasks) =
            self.activity.read_with(cx, |activity, cx| {
                (activity.can_commit_staged(cx), activity.command_tasks(cx))
            });
        let active_editor_id = self.code.entity_id();
        let active_group_is_right =
            self.right_editor_tabs.contains(&active_editor_id.as_u64());
        let active_group_tabs = self
            .code_tabs
            .iter()
            .filter(|code| {
                self.right_editor_tabs.contains(&code.entity_id().as_u64())
                    == active_group_is_right
            })
            .collect::<Vec<_>>();
        let active_group_index = active_group_tabs
            .iter()
            .position(|code| code.entity_id() == active_editor_id);
        let context = crate::command_palette::CommandContext {
            active_editor,
            text_editor,
            language_editor,
            can_navigate_back: !self.navigation_history.backward.is_empty(),
            can_navigate_forward: !self.navigation_history.forward.is_empty(),
            can_start_or_continue_debugging: can_step_debugging
                || (!debug.state.is_active()
                    && !debug.connection_closed
                    && !debug_proxy.stored_configs().is_empty()),
            can_stop_debugging: debug.state.can_stop(),
            can_step_debugging,
            can_commit_staged,
            can_close_other_editors: active_group_tabs.len() > 1,
            can_close_editors_to_left: active_group_index
                .is_some_and(|index| index > 0),
            can_close_editors_to_right: active_group_index
                .is_some_and(|index| index + 1 < active_group_tabs.len()),
            can_move_editor_to_other_group: self.code_tabs.len() > 1,
        };
        let shell = cx.weak_entity();
        crate::command_palette::open(
            Box::new(move |command, window, cx| {
                let result = shell.update(cx, |shell, cx| match command {
                    crate::command_palette::CommandAction::Shell(command) => {
                        shell.run_shell_command(command, window, cx)
                    }
                    crate::command_palette::CommandAction::RunJustTask {
                        recipe,
                        cwd,
                        justfile,
                    } => shell.new_task_terminal(recipe, cwd, justfile, window, cx),
                });
                if let Err(error) = result {
                    eprintln!("AHEAD could not run command: {error}");
                }
            }),
            context,
            tasks,
            window,
            cx,
        );
    }

    fn show_left_panel(
        &mut self,
        view: WorkspaceView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activity
            .update(cx, |activity, cx| activity.show(view, window, cx));
        self.area.update(cx, |area, cx| {
            if !area.is_dock_open(DockPlacement::Left) {
                area.toggle_dock(DockPlacement::Left, window, cx);
            }
        });
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn open_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let (proxy, workspace, active_file) = self.code.read_with(cx, |code, _| {
            (
                code.proxy.clone(),
                code.workspace.clone(),
                code.file_path.clone(),
            )
        });
        let Some(proxy) = proxy else {
            return;
        };
        let workspace = std::path::PathBuf::from(workspace);
        let active_file = std::path::PathBuf::from(active_file);
        let relative_to = active_file
            .strip_prefix(&workspace)
            .ok()
            .and_then(|path| path.parent())
            .filter(|path| !path.as_os_str().is_empty())
            .map(std::path::Path::to_path_buf);
        let shell = cx.weak_entity();
        crate::quick_open::open(
            proxy,
            workspace,
            self.recent_files.clone(),
            (!active_file.as_os_str().is_empty()).then_some(active_file.clone()),
            relative_to,
            Box::new(move |request, window, cx| {
                if let Err(error) = shell.update(cx, |shell, cx| {
                    shell.open_file_request(request, window, cx);
                }) {
                    eprintln!("AHEAD could not open the selected file: {error}");
                }
            }),
            window,
            cx,
        );
    }

    fn open_go_to_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.code_tabs.is_empty() || !self.code.read(cx).is_text_available() {
            return;
        }
        let (path, line, column, line_count) =
            self.code.read_with(cx, |code, cx| {
                let editor = code.editor.read(cx);
                let position = editor.cursor_position();
                (
                    code.file_path.clone(),
                    position.line as usize + 1,
                    position.character as usize + 1,
                    editor.text().lines_len().max(1),
                )
            });
        let shell = cx.weak_entity();
        crate::go_to_line::open(
            line,
            column,
            line_count,
            Box::new(move |location, window, cx| {
                if let Err(error) = shell.update(cx, |shell, cx| {
                    shell.navigate_to_editor_location(
                        crate::ross::OpenRequest {
                            path: path.clone(),
                            permanent: false,
                            location: Some(location),
                        },
                        window,
                        cx,
                    );
                }) {
                    eprintln!("AHEAD could not navigate to line: {error}");
                }
            }),
            window,
            cx,
        );
    }
}

impl Render for Shell {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if reap_system_speech(&self.system_speech_process) {
            self.system_speech_active.store(false, Ordering::SeqCst);
        }
        self.poll_explorer_open(window, cx);
        self.poll_editor_presentations(window, cx);
        let branch = self
            .session
            .read(cx)
            .proxy
            .as_ref()
            .map(|proxy| proxy.diff().branch)
            .filter(|branch| !branch.is_empty())
            .unwrap_or_else(|| "No branch".to_string());
        let has_branch = branch != "No branch";
        let worktree_name =
            if self.worktree_name == self.workspace_name && has_branch {
                branch.clone()
            } else {
                self.worktree_name.clone()
            };
        let border = cx.theme().border;
        let text = cx.theme().sidebar_foreground;
        let area_right = self.area.clone();
        let area_left = self.area.clone();
        let left_open = self.area.read(cx).is_dock_open(DockPlacement::Left);
        let bottom_open = self.area.read(cx).is_dock_open(DockPlacement::Bottom);
        let right_open = self.area.read(cx).is_dock_open(DockPlacement::Right);
        let active_left = self.activity.read(cx).active();
        let active_icon = cx.theme().success;
        let active_button = |button: Button, active: bool| {
            let button = button.ghost();
            if active {
                button.text_color(active_icon)
            } else {
                button
            }
        };
        let status_divider = || div().w(px(1.)).h(px(14.)).bg(border);
        let session_label = self.session.read_with(cx, |session, _| {
            session
                .session_id
                .as_ref()
                .map(|_| {
                    format!(
                        "{} [{} · {} · {} task]",
                        session.active_work_title,
                        session.phase_id,
                        session.active_work_kind.display_name(),
                        session.active_task_intent.display_name(),
                    )
                })
                .unwrap_or_else(|| "No active session".to_string())
        });
        let lsp_servers = self.code.read_with(cx, |code, _| {
            code.proxy
                .as_ref()
                .map(|proxy| proxy.lsp_servers())
                .unwrap_or_default()
        });
        let (lsp_color, lsp_state) = if lsp_servers.is_empty() {
            (cx.theme().warning, "starting")
        } else if lsp_servers.iter().any(|server| server.is_error()) {
            (cx.theme().danger, "error")
        } else if lsp_servers.iter().all(|server| server.is_ready()) {
            (cx.theme().success, "ready")
        } else {
            (cx.theme().warning, "starting")
        };

        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .child(
                TitleBar::new().on_close_window(cx.listener(|this, _, window, cx| {
                    this.request_window_close(window, cx);
                })).child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            h_flex()
                                .items_center()
                                .child(
                                    Button::new("editor-navigation-back")
                                        .ghost()
                                        .icon(IconName::ArrowLeft)
                                        .disabled(
                                            self.navigation_history.backward.is_empty(),
                                        )
                                        .tooltip("Go Back (Ctrl+-)")
                                        .on_click(cx.listener(
                                            |this, _, window, cx| {
                                                this.navigate_editor_history(
                                                    true, window, cx,
                                                );
                                            },
                                        )),
                                )
                                .child(
                                    Button::new("editor-navigation-forward")
                                        .ghost()
                                        .icon(IconName::ArrowRight)
                                        .disabled(
                                            self.navigation_history.forward.is_empty(),
                                        )
                                        .tooltip("Go Forward (Ctrl+_)")
                                        .on_click(cx.listener(
                                            |this, _, window, cx| {
                                                this.navigate_editor_history(
                                                    false, window, cx,
                                                );
                                            },
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .id("ahead-brand")
                                .role(Role::Image)
                                .aria_label("AHEAD")
                                .child(ahead_icon(px(24.), cx)),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(text)
                                .child(self.workspace_name.clone()),
                        )
                        .when(has_branch, |bar| {
                            bar.child(div().w(px(1.)).h(px(14.)).bg(border))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        .text_size(px(12.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(IconName::Folder)
                                        .child(worktree_name),
                                )
                                .child(
                                    div()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("/"),
                                )
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        .text_size(px(12.))
                                        .text_color(text)
                                        .child(IconName::GitBranch)
                                        .child(branch.clone()),
                                )
                        }),
                ),
            )
            .child(div().flex_1().child(self.area.clone()))
            .child(
                StatusBar::new()
                    // Left Region: Dock toggles and panel quick-switchers with tooltips
                    .left(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                active_button(
                                    Button::new("toggle_left_dock")
                                        .icon(IconName::PanelLeft)
                                        .tooltip(format!("Toggle Left Dock ({})", shortcut_hint("⌘B", "Ctrl+B"))),
                                    left_open,
                                )
                                    .on_click(cx.listener(move |_, _, window, cx| {
                                        area_left.update(cx, |area, cx| area.toggle_dock(DockPlacement::Left, window, cx));
                                        cx.notify();
                                    }))
                            )
                            .child(
                                active_button(
                                    Button::new("files_btn")
                                        .icon(IconName::Folder)
                                        .tooltip(format!("Files Explorer ({})", shortcut_hint("⌘⇧E", "Ctrl+Shift+E"))),
                                    left_open && active_left == WorkspaceView::Explorer,
                                )
                                    .on_click(cx.listener({
                                        move |this: &mut Self, _, window, cx| {
                                            this.show_left_panel(WorkspaceView::Explorer, window, cx);
                                        }
                                    }))
                            )
                            .child(
                                active_button(
                                    Button::new("git_btn")
                                        .icon(IconName::GitBranch)
                                        .tooltip(format!("Source Control · {} ({})", branch, shortcut_hint("⌃⇧G", "Ctrl+Shift+G"))),
                                    left_open && active_left == WorkspaceView::Git,
                                )
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.show_left_panel(WorkspaceView::Git, window, cx);
                                        this.explorer.update(cx, |explorer, cx| explorer.refresh(cx));
                                        cx.notify();
                                    }))
                            )
                            .child(
                                div()
                                    .relative()
                                    .child(
                                        active_button(
                                            Button::new("language_servers_btn")
                                                .icon(IconName::Zap)
                                                .tooltip(format!(
                                                    "Language Servers: {lsp_state} ({})",
                                                    shortcut_hint("⌃⌥⇧L", "Ctrl+Alt+Shift+L")
                                                )),
                                            left_open && active_left == WorkspaceView::LanguageServers,
                                        )
                                            .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                                this.show_left_panel(WorkspaceView::LanguageServers, window, cx);
                                            })),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .top(px(3.))
                                            .right(px(3.))
                                            .w(px(6.))
                                            .h(px(6.))
                                            .bg(lsp_color),
                                    )
                            )
                            .child(
                                active_button(
                                    Button::new("tasks_btn")
                                        .icon(IconName::ListTodo)
                                        .tooltip(format!("Tasks from justfiles ({})", shortcut_hint("⌃⌥⇧K", "Ctrl+Alt+Shift+K"))),
                                    left_open && active_left == WorkspaceView::Tasks,
                                )
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.show_left_panel(WorkspaceView::Tasks, window, cx);
                                    })),
                            )
                            .child(status_divider())
                            .child(
                                Button::new("quick_open_btn")
                                    .icon(IconName::File)
                                    .tooltip(format!("Quick Open ({})", shortcut_hint("⌘P", "Ctrl+P")))
                                    .on_click(cx.listener(
                                        |this: &mut Self, _, window, cx| {
                                            this.open_quick_open(window, cx);
                                        },
                                    )),
                            )
                            .child(
                                active_button(
                                    Button::new("search_btn")
                                        .icon(IconName::Search)
                                        .tooltip(format!("Search Workspace ({})", shortcut_hint("⌘⇧F", "Ctrl+Shift+F"))),
                                    self.open_center_panels.contains(&CenterPanel::Search),
                                )
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.open_center_panel(CenterPanel::Search, window, cx);
                                    })),
                            )
                            .child(
                                active_button(
                                    Button::new("problems_btn")
                                        .icon(IconName::ShieldCheck)
                                        .tooltip({
                                        let n = self.code.read_with(cx, |code, _| code.diagnostics.len());
                                        format!("Problems: {n} problem{} ({})", if n == 1 { "" } else { "s" }, shortcut_hint("⌘⇧M", "Ctrl+Shift+M"))
                                    }),
                                    self.open_center_panels.contains(&CenterPanel::Problems),
                                )
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.open_center_panel(CenterPanel::Problems, window, cx);
                                        let first = this.code.read_with(cx, |code, _| code.diagnostics.first().cloned());
                                        let code_focus = this.code.read_with(cx, |code, _| code.focus.clone());
                                        if let Some(d) = first {
                                            this.code.update(cx, |code, cx| {
                                                code.active_line = d.line;
                                                code.status = format!("Ln {}: {}", d.line, d.message).into();
                                                cx.notify();
                                            });
                                            window.focus(&code_focus, cx);
                                        }
                                    }))
                            )
                            .child(
                                active_button(
                                    Button::new("extensions_btn")
                                        .icon(IconName::Puzzle)
                                        .tooltip(format!("Open Extensions ({})", shortcut_hint("⌘⇧X", "Ctrl+Shift+X"))),
                                    self.open_center_panels.contains(&CenterPanel::Extensions),
                                )
                                .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                    this.open_extensions(window, cx);
                                })),
                            )
                            .child(
                                active_button(
                                    Button::new("settings_btn")
                                        .icon(IconName::Settings)
                                        .tooltip(format!("Open Settings ({})", shortcut_hint("⌘,", "Ctrl+,"))),
                                    self.open_center_panels.contains(&CenterPanel::Settings),
                                )
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.open_center_panel(CenterPanel::Settings, window, cx);
                                    }))
                            )
                            .child(
                                active_button(
                                    Button::new("help_btn")
                                        .icon(IconName::BookOpen)
                                        .tooltip("Open AHEAD Guide"),
                                    self.open_center_panels.contains(&CenterPanel::Help),
                                )
                                .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                    this.open_center_panel(CenterPanel::Help, window, cx);
                                })),
                            )
                    )
                    // Center Region: Active work session chip
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(ahead_icon(px(16.), cx))
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(11.))
                                    .text_color(text)
                                    .child(session_label)
                            )
                            .when(!self.status_message.is_empty(), |bar| {
                                bar.child(
                                    div()
                                        .min_w_0()
                                        .max_w(px(520.))
                                        .truncate()
                                        .text_size(px(11.))
                                        .text_color(cx.theme().warning)
                                        .child(self.status_message.clone()),
                                )
                            })
                    )
                    // Right Region: Editor stats and Right dock panel switchers
                    .right(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(text)
                                    .child({
                                        let code = self.code.read(cx);
                                        if code.is_text_available() {
                                            let editor = code.editor.read(cx);
                                            let position = editor.cursor_position();
                                            let language_name =
                                                code.status_language_name(cx);
                                            format!("Ln {}, Col {} · {} · UTF-8",
                                                position.line + 1, position.character + 1, language_name)
                                        } else {
                                            "Text unavailable".to_string()
                                        }
                                    })
                            )
                            .child(
                                active_button(
                                    Button::new("terminal_btn")
                                        .icon(IconName::Terminal)
                                        .tooltip(format!("Terminal ({})", shortcut_hint("⌃`", "Ctrl+`"))),
                                    bottom_open && !self.bottom_debug_active,
                                )
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.toggle_terminal(window, cx);
                                    }))
                            )
                            .child(
                                active_button(
                                    Button::new("debug_btn")
                                        .icon(IconName::Bug)
                                        .tooltip(format!("Debug Panel ({})", shortcut_hint("⌘⇧D", "Ctrl+Shift+D"))),
                                    bottom_open && self.bottom_debug_active,
                                )
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.show_debug(window, cx);
                                    }))
                            )
                            .child(status_divider())
                            .child(
                                active_button(
                                    Button::new("agent_btn")
                                        .icon(IconName::MessageSquare)
                                        .tooltip(format!("AHEAD Agent ({})", shortcut_hint("⌃⌘I", "Ctrl+Alt+I"))),
                                    right_open,
                                )
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.run_shell_command(ShellShortcut::Agent, window, cx);
                                    }))
                            )
                            .child(
                                active_button(
                                    Button::new("threads_btn")
                                        .icon(IconName::Layers)
                                        .tooltip(format!("Threads ({})", shortcut_hint("⌃⌥⇧T", "Ctrl+Alt+Shift+T"))),
                                    right_open && self.threads_visible,
                                )
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        let next = !this.threads_visible;
                                        this.set_threads_visible(next, window, cx);
                                    }))
                            )
                            .child(
                                active_button(
                                    Button::new("toggle_right_dock")
                                        .icon(IconName::PanelRight)
                                        .tooltip(format!("Toggle Right Sidebar ({})", shortcut_hint("⌥⌘B", "Ctrl+Alt+B"))),
                                    right_open,
                                )
                                    .on_click(cx.listener(move |_, _, window, cx| {
                                        area_right.update(cx, |area, cx| area.toggle_dock(DockPlacement::Right, window, cx));
                                        cx.notify();
                                    }))
                            )
                    )
                    .border_t_1()
                    .border_color(border),
            )
            .children(gpui_kit::component::Root::render_dialog_layer(window, cx))
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        if let Err(error) = stop_system_speech(&self.system_speech_process) {
            eprintln!("AHEAD could not stop system speech during shutdown: {error}");
        }
    }
}

fn workspace_git_names(root: &str) -> (String, String) {
    let fallback = std::path::Path::new(root)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Workspace")
        .to_string();
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel", "--git-common-dir"])
        .current_dir(root)
        .output();
    let Ok(output) = output else {
        return (fallback.clone(), fallback);
    };
    if !output.status.success() {
        return (fallback.clone(), fallback);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let Some(worktree_path) = lines.next().map(std::path::PathBuf::from) else {
        return (fallback.clone(), fallback);
    };
    let Some(common_dir) = lines.next().map(std::path::PathBuf::from) else {
        return (fallback.clone(), fallback);
    };
    let common_dir = if common_dir.is_absolute() {
        common_dir
    } else {
        std::path::Path::new(root).join(common_dir)
    };
    let workspace_path = common_dir
        .canonicalize()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf));
    let workspace_name = workspace_path
        .as_deref()
        .and_then(std::path::Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or(&fallback)
        .to_string();
    let worktree_path = worktree_path.canonicalize().unwrap_or(worktree_path);
    let worktree_name = worktree_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&fallback)
        .to_string();
    (workspace_name, worktree_name)
}

fn configure_ahead_theme(cx: &mut App) -> anyhow::Result<()> {
    let (dark, light) = crate::theme::default_themes()?;
    let theme = gpui_kit::component::Theme::global_mut(cx);
    theme.dark_theme = std::rc::Rc::new(dark);
    theme.light_theme = std::rc::Rc::new(light);
    Ok(())
}

fn should_prompt_for_workspace(
    current_dir: &std::path::Path,
    file_path: &str,
    root_arg: Option<&str>,
) -> bool {
    file_path.is_empty() && root_arg.is_none() && current_dir.parent().is_none()
}

fn canonical_workspace_directory(
    path: &std::path::Path,
) -> std::result::Result<std::path::PathBuf, String> {
    let canonical = path.canonicalize().map_err(|error| {
        format!("AHEAD could not open workspace {}: {error}", path.display())
    })?;
    if !canonical.is_dir() {
        return Err(format!(
            "AHEAD workspace selection is not a directory: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn workspace_prompt_options() -> PathPromptOptions {
    PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Open Workspace".into()),
    }
}

fn file_prompt_options() -> PathPromptOptions {
    PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some("Open File".into()),
    }
}

fn launch_paths(
    mut file_path: String,
    root_arg: Option<String>,
    current_dir: &std::path::Path,
) -> std::result::Result<(String, String), String> {
    let directory_arg =
        !file_path.is_empty() && std::path::Path::new(&file_path).is_dir();
    let directory_root = if directory_arg {
        Some(
            canonical_workspace_directory(std::path::Path::new(&file_path))?
                .to_string_lossy()
                .into_owned(),
        )
    } else {
        None
    };
    let explorer_root = root_arg
        .or(directory_root)
        .or_else(|| {
            if file_path.is_empty() {
                Some(current_dir.to_string_lossy().into_owned())
            } else {
                std::path::Path::new(&file_path)
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .and_then(|parent| parent.to_str())
                    .map(str::to_string)
            }
        })
        .unwrap_or_else(|| current_dir.to_string_lossy().into_owned());
    if directory_arg {
        file_path.clear();
    }
    Ok((file_path, explorer_root))
}

pub fn launch() {
    let args: Vec<String> = std::env::args().collect();
    let mut file_path = String::new();
    let mut root_arg: Option<String> = None;

    for arg in args.into_iter().skip(1) {
        if arg.starts_with('-') {
            continue;
        }
        if file_path.is_empty() {
            file_path = arg;
        } else if root_arg.is_none() {
            root_arg = Some(arg);
        }
    }

    let current_dir =
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let prompt_for_workspace =
        should_prompt_for_workspace(&current_dir, &file_path, root_arg.as_deref());
    let (file_path, explorer_root) =
        match launch_paths(file_path, root_arg, &current_dir) {
            Ok(paths) => paths,
            Err(error) => {
                eprintln!("{error}");
                return;
            }
        };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.set_global(QuitInProgress::default());
            cx.on_action(request_quit);
            cx.on_action(request_close_window);
            cx.on_action(request_recovery);
            cx.on_action(request_new_window);
            cx.on_action(|_: &OpenFile, cx| {
                request_open_path(file_prompt_options(), cx);
            });
            cx.on_action(|_: &OpenFolder, cx| {
                request_open_path(workspace_prompt_options(), cx);
            });
            cx.on_action(|_: &OpenHelp, cx| {
                request_shell_command(ShellShortcut::Help, cx);
            });
            cx.on_action(|_: &GoToFile, cx| {
                request_shell_command(ShellShortcut::QuickOpen, cx);
            });
            cx.on_action(|_: &GoToLine, cx| {
                request_shell_command(ShellShortcut::GoToLine, cx);
            });
            cx.on_action(|_: &GoBack, cx| {
                request_shell_command(ShellShortcut::NavigateBack, cx);
            });
            cx.on_action(|_: &GoForward, cx| {
                request_shell_command(ShellShortcut::NavigateForward, cx);
            });
            cx.on_action(|_: &StartDebugging, cx| {
                request_shell_command(ShellShortcut::DebugKey("f5", false), cx);
            });
            cx.on_action(|_: &StopDebugging, cx| {
                request_shell_command(ShellShortcut::DebugKey("f5", true), cx);
            });
            cx.on_action(|_: &ToggleBreakpoint, cx| {
                request_shell_command(ShellShortcut::DebugKey("f9", false), cx);
            });
            cx.on_action(|_: &StepOver, cx| {
                request_shell_command(ShellShortcut::DebugKey("f10", false), cx);
            });
            cx.on_action(|_: &StepInto, cx| {
                request_shell_command(ShellShortcut::DebugKey("f11", false), cx);
            });
            cx.on_action(|_: &StepOut, cx| {
                request_shell_command(ShellShortcut::DebugKey("f11", true), cx);
            });
            cx.on_action(minimize_window);
            cx.on_action(zoom_window);
            cx.on_app_quit(|cx| {
                if cfg!(debug_assertions) {
                    eprintln!("AHEAD app-quit callback shutting down workspace proxies");
                }
                let executor = cx.background_executor().clone();
                let windows = editor_windows(cx);
                for (_, shell) in &windows {
                    for terminal in shell.read(cx).terminals.clone() {
                        shutdown_terminal(&terminal, cx);
                    }
                }
                let terminal_shutdowns = std::mem::take(
                    &mut cx.default_global::<PendingTerminalShutdowns>().0,
                );
                let proxies: Vec<_> = windows
                    .iter()
                    .filter_map(|(_, shell)| {
                        shell.read(cx).session.read(cx).proxy.clone()
                    })
                    .collect();
                let replies = windows
                    .into_iter()
                    .flat_map(|(_, shell)| {
                        shell.update(cx, |shell, cx| {
                            if shell.window_close_authorized {
                                Vec::new()
                            } else {
                                shell.queue_recoveries(false, cx)
                            }
                        })
                    })
                    .collect();
                async move {
                    if let Err(error) =
                        confirm_recovery_writes(replies, &executor).await
                    {
                        eprintln!("Final editor recovery flush failed: {error}");
                    }
                    for proxy in proxies {
                        if cfg!(debug_assertions) {
                            eprintln!("AHEAD app-quit callback stopping proxy child");
                        }
                        proxy.shutdown();
                    }
                    wait_for_terminal_shutdowns(terminal_shutdowns, &executor).await;
                }
            })
            .detach();
            cx.bind_keys([
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-q"
                    } else {
                        "ctrl-q"
                    },
                    Quit,
                    None,
                ),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-shift-w"
                    } else {
                        "ctrl-shift-w"
                    },
                    CloseWindow,
                    None,
                ),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-s"
                    } else {
                        "ctrl-s"
                    },
                    SaveFile,
                    None,
                ),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-o"
                    } else {
                        "ctrl-o"
                    },
                    OpenFile,
                    None,
                ),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-shift-n"
                    } else {
                        "ctrl-shift-n"
                    },
                    NewWindow,
                    None,
                ),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-p"
                    } else {
                        "ctrl-p"
                    },
                    GoToFile,
                    None,
                ),
                KeyBinding::new("ctrl-g", GoToLine, Some("AHEADCodeEditor > Input")),
                KeyBinding::new("ctrl--", GoBack, None),
                KeyBinding::new("ctrl-_", GoForward, None),
                KeyBinding::new("f12", GoToDefinition, None),
                KeyBinding::new("f2", RenameSymbol, Some("AHEADCodeEditor > Input")),
                KeyBinding::new("shift-f12", FindReferences, None),
                KeyBinding::new(
                    "shift-alt-f",
                    FormatDocument,
                    Some("AHEADCodeEditor > Input"),
                ),
                KeyBinding::new(
                    "ctrl-space",
                    ShowCompletions,
                    Some("AHEADCodeEditor > Input"),
                ),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-shift-space"
                    } else {
                        "ctrl-shift-space"
                    },
                    ShowSignatureHelp,
                    Some("AHEADCodeEditor > Input"),
                ),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-."
                    } else {
                        "ctrl-."
                    },
                    ShowCodeActions,
                    Some("AHEADCodeEditor > Input"),
                ),
                KeyBinding::new(
                    "tab",
                    NextSnippetTabstop,
                    Some("(AHEADCodeEditor && tab_action == true) > Input"),
                ),
                KeyBinding::new(
                    "shift-tab",
                    PreviousSnippetTabstop,
                    Some("(AHEADCodeEditor && tab_action == true) > Input"),
                ),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-f12"
                    } else {
                        "ctrl-f12"
                    },
                    GoToImplementation,
                    None,
                ),
                KeyBinding::new("f5", StartDebugging, None),
                KeyBinding::new("shift-f5", StopDebugging, None),
                KeyBinding::new("f9", ToggleBreakpoint, None),
                KeyBinding::new("f10", StepOver, None),
                KeyBinding::new("f11", StepInto, None),
                KeyBinding::new("shift-f11", StepOut, None),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-m"
                    } else {
                        "ctrl-m"
                    },
                    MinimizeWindow,
                    None,
                ),
            ]);
            cx.set_menus([
                Menu::new("AHEAD").items([MenuItem::action("Quit AHEAD", Quit)]),
                Menu::new("File").items([
                    MenuItem::action("New Window", NewWindow),
                    MenuItem::separator(),
                    MenuItem::action("Open File…", OpenFile),
                    MenuItem::action("Open Folder…", OpenFolder),
                    MenuItem::separator(),
                    MenuItem::action("Save", SaveFile),
                    MenuItem::separator(),
                    MenuItem::action(
                        "Recover Unsaved Changes",
                        RecoverUnsavedChanges,
                    ),
                ]),
                Menu::new("Edit").items([
                    MenuItem::action(
                        "Copy",
                        gpui_kit::component::input::Copy,
                    ),
                    MenuItem::action(
                        "Paste",
                        gpui_kit::component::input::Paste,
                    ),
                    MenuItem::separator(),
                    MenuItem::action(
                        "Select All",
                        gpui_kit::component::input::SelectAll,
                    ),
                ]),
                Menu::new("Go").items([
                    MenuItem::action("Go to File", GoToFile),
                    MenuItem::action("Go to Line", GoToLine),
                    MenuItem::action("Go Back", GoBack),
                    MenuItem::action("Go Forward", GoForward),
                    MenuItem::action("Go to Definition", GoToDefinition),
                    MenuItem::action("Find All References", FindReferences),
                    MenuItem::action("Go to Implementation", GoToImplementation),
                ]),
                Menu::new("Editor").items([
                    MenuItem::action("Rename Symbol", RenameSymbol),
                    MenuItem::action("Format Document", FormatDocument),
                    MenuItem::action("Show Signature Help", ShowSignatureHelp),
                    MenuItem::action("Show Code Actions", ShowCodeActions),
                ]),
                Menu::new("Run").items([
                    MenuItem::action("Start or Continue Debugging", StartDebugging),
                    MenuItem::action("Stop Debugging", StopDebugging),
                    MenuItem::separator(),
                    MenuItem::action("Toggle Breakpoint", ToggleBreakpoint),
                    MenuItem::action("Step Over", StepOver),
                    MenuItem::action("Step Into", StepInto),
                    MenuItem::action("Step Out", StepOut),
                ]),
                Menu::new("Window").items([
                    MenuItem::action("Minimize", MinimizeWindow),
                    MenuItem::action("Zoom", ZoomWindow),
                    MenuItem::separator(),
                    MenuItem::action("Close Window", CloseWindow),
                ]),
                Menu::new("Help").items([MenuItem::action("AHEAD Guide", OpenHelp)]),
            ]);
            #[cfg(target_os = "macos")]
            if let Err(error) = set_application_icon() {
                eprintln!("Could not set the AHEAD Dock icon: {error}");
            }
            if let Err(error) = configure_ahead_theme(cx) {
                eprintln!("Failed to load AHEAD themes: {error:#}");
            }
            gpui_kit::component::Theme::change(
                gpui_kit::component::ThemeMode::Dark,
                None,
                cx,
            );
            let path = file_path.clone();
            let workspace_prompt = prompt_for_workspace
                .then(|| cx.prompt_for_paths(workspace_prompt_options()));
            let explorer_root = explorer_root.clone();
            cx.spawn(async move |cx| {
                let explorer_root = if let Some(workspace_prompt) = workspace_prompt
                {
                    let selected_path = match workspace_prompt.await {
                        Ok(Ok(Some(paths))) => paths.into_iter().next(),
                        Ok(Ok(None)) => None,
                        Ok(Err(error)) => {
                            eprintln!("AHEAD workspace picker failed: {error}");
                            None
                        }
                        Err(_) => {
                            eprintln!(
                                "AHEAD workspace picker response was interrupted"
                            );
                            None
                        }
                    };
                    let workspace_root = selected_path.and_then(|path| {
                        match canonical_workspace_directory(&path) {
                            Ok(path) => Some(path),
                            Err(error) => {
                                eprintln!("{error}");
                                None
                            }
                        }
                    });
                    let Some(workspace_root) = workspace_root else {
                        cx.update(|cx| cx.quit());
                        return;
                    };
                    workspace_root.to_string_lossy().into_owned()
                } else {
                    explorer_root
                };
                cx.open_window(TitleBar::window_options(), |window, cx| {
                    let (area, skin) =
                        DockSkin::dock_area("ahead-shell", None, window, cx);
                    skin.set_panel_style(PanelStyle::TabBar, cx);
                    let proxy = crate::proxy_client::ProxyClient::new(
                        std::path::PathBuf::from(&explorer_root),
                    );
                    let code = cx.new(|cx| {
                        crate::code_panel::CodePanel::new(&path, window, cx)
                            .with_proxy(proxy.clone(), &explorer_root, window, cx)
                    });
                    let session = cx.new(|cx| {
                        crate::session_panel::SessionPanel::new(
                            std::path::PathBuf::from(&explorer_root),
                            window,
                            cx,
                        )
                        .with_code(code.clone())
                        .with_proxy_client(proxy.clone())
                    });
                    session.update(cx, |panel, cx| {
                        panel.watch_config_changes(window, cx);
                    });
                    let threads = cx.new(|cx| {
                        crate::threads_panel::ThreadsPanel::new(window, cx)
                            .with_proxy(proxy.clone())
                            .with_session(session.clone())
                    });
                    cx.spawn({
                        let proxy = proxy.clone();
                        let threads = threads.clone();
                        let session = session.clone();
                        async move |cx| {
                            let restore = cx
                                .background_spawn(async move {
                                    proxy.durable_session_restore()
                                })
                                .await;
                            match restore {
                                Ok(restore) => {
                                    cx.update_entity(&threads, |panel, cx| {
                                        panel.restore_durable_sessions(
                                            Ok(restore.sessions),
                                            cx,
                                        )
                                    });
                                    if let Some(active) = restore.active {
                                        cx.update_entity(&session, |panel, cx| {
                                            panel.restore_durable_session(active, cx)
                                        });
                                    }
                                }
                                Err(error) => {
                                    cx.update_entity(&threads, |panel, cx| {
                                        panel
                                            .restore_durable_sessions(Err(error), cx)
                                    });
                                }
                            }
                        }
                    })
                    .detach();
                    session.update(cx, |panel, cx| {
                        panel.set_thread_launcher(threads.clone(), cx);
                    });
                    let explorer = cx.new(|cx| {
                        crate::explorer_panel::ExplorerPanel::new(
                            &explorer_root,
                            window,
                            cx,
                        )
                    });
                    explorer.update(cx, |panel, cx| {
                        panel.watch_workspace_changes(proxy.clone(), cx)
                    });
                    let explorer_id = explorer.read(cx).mailbox_id;
                    let settings = cx.new(|cx| {
                        crate::settings_panel::SettingsPanel::new(
                            std::path::PathBuf::from(&explorer_root),
                            window,
                            cx,
                        )
                        .with_proxy(proxy.clone())
                    });
                    settings.update(cx, |settings, cx| {
                        settings.watch_config_changes(window, cx);
                        settings.load_mcp_server_declarations(cx);
                    });
                    let extensions = cx.new(|cx| {
                        crate::extensions_panel::ExtensionsPanel::new(
                            proxy.clone(),
                            window,
                            cx,
                        )
                    });
                    let terminal = cx.new(|cx| {
                        crate::terminal_panel::TerminalPanel::new_with_cwd(
                            1,
                            explorer_root.clone(),
                            cx,
                        )
                    });
                    let terminals = vec![terminal.clone()];
                    let debug_bar = cx.new(|cx| {
                        crate::debug_bar::DebugBar::new(
                            proxy.clone(),
                            &path,
                            window,
                            cx,
                        )
                    });
                    let git = cx.new(|cx| {
                        GitPanel::new(&explorer_root, window, cx)
                            .with_proxy(proxy.clone())
                    });
                    let search = cx.new(|cx| {
                        SearchPanel::new(
                            &explorer_root,
                            explorer_id,
                            code.clone(),
                            window,
                            cx,
                        )
                    });
                    let problems = cx.new(|cx| ProblemsPanel::new(code.clone(), cx));
                    let help = cx.new(crate::help_panel::HelpPanel::new);
                    let language_servers = cx.new(|cx| {
                        LanguageServersPanel::new(
                            std::path::Path::new(&explorer_root),
                            proxy.clone(),
                            cx,
                        )
                    });
                    let tasks = cx.new(|cx| JustTasksPanel::new(&explorer_root, cx));
                    tasks.update(cx, |tasks, cx| tasks.refresh(cx));
                    let agent_workspace = cx.new(|cx| {
                        AgentWorkspacePanel::new(
                            session.clone(),
                            threads.clone(),
                            cx,
                        )
                    });
                    let activity = cx.new(|_| {
                        ActivityBar::new(
                            area.clone(),
                            explorer.clone(),
                            git.clone(),
                            tasks.clone(),
                            language_servers.clone(),
                        )
                    });
                    let shell = cx.new(|cx| {
                        Shell::new(
                            area.clone(),
                            session.clone(),
                            threads.clone(),
                            code.clone(),
                            explorer.clone(),
                            git.clone(),
                            debug_bar.clone(),
                            terminals.clone(),
                            problems.clone(),
                            settings.clone(),
                            extensions.clone(),
                            help.clone(),
                            search.clone(),
                            agent_workspace.clone(),
                            activity.clone(),
                            cx,
                        )
                    });
                    let shell_for_git = shell.downgrade();
                    git.update(cx, |git, _| {
                        git.set_open_file_handler(move |path, window, cx| {
                            let path = path.to_string_lossy().into_owned();
                            let request = crate::ross::OpenRequest {
                                path: path.clone(),
                                permanent: true,
                                location: None,
                            };
                            if let Err(error) = shell_for_git.update(cx, |shell, cx| {
                                shell.open_file_request(request, window, cx);
                                if let Some(code) = shell
                                    .code_tabs
                                    .iter()
                                    .find(|code| code.read(cx).file_path == path)
                                    .cloned()
                                {
                                    code.update(cx, |code, cx| {
                                        code.reveal_first_git_hunk(cx);
                                    });
                                }
                            }) {
                                eprintln!(
                                    "AHEAD could not open changed file: {error}"
                                );
                            }
                        });
                    });
                    let shell_for_language_servers = shell.downgrade();
                    language_servers.update(cx, |language_servers, _| {
                        language_servers.set_open_settings_handler(
                            move |_path, window, cx| {
                                if let Err(error) =
                                    shell_for_language_servers.update(
                                        cx,
                                        |shell, cx| shell.open_language_server_config(window, cx),
                                    )
                                {
                                    eprintln!(
                                        "AHEAD could not open language-server settings: {error}"
                                    );
                                }
                            },
                        );
                    });
                    session.update(cx, |panel, _| panel.set_shell(shell.clone()));
                    let shell_for_settings_reload = shell.downgrade();
                    let session_for_settings_reload = session.downgrade();
                    let workspace_for_settings_reload = explorer_root.clone();
                    settings.update(cx, |settings, _| {
                        settings.set_save_handler(move |window, cx| {
                            if let Err(error) = session_for_settings_reload.update(
                                cx,
                                |session, cx| {
                                    session.reload_configured_models(window, cx)
                                },
                            ) {
                                eprintln!(
                                    "AHEAD model picker reload failed: {error}"
                                );
                            }
                            let inline_blame =
                                crate::settings_panel::inline_blame_enabled(
                                    std::path::Path::new(
                                        &workspace_for_settings_reload,
                                    ),
                                );
                            if let Err(error) =
                                shell_for_settings_reload.update(cx, |shell, cx| {
                                    for code in &shell.code_tabs {
                                        code.update(cx, |code, cx| {
                                            code.set_inline_blame_enabled(
                                                inline_blame,
                                                cx,
                                            )
                                        });
                                    }
                                })
                            {
                                eprintln!(
                                    "AHEAD inline blame reload failed: {error}"
                                );
                            }
                        });
                    });
                    Shell::install_window_close_handler(&shell, window, cx);
                    shell.update(cx, |shell, cx| {
                        shell.start_debug_terminal_pump(proxy.clone(), window, cx);
                    });
                    cx.spawn({
                        let shell = shell.downgrade();
                        let session = session.downgrade();
                        let proxy = proxy.clone();
                        async move |cx| loop {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(250))
                                .await;
                            if session
                                .update(cx, |panel, cx| panel.poll_stream(cx))
                                .is_err()
                            {
                                break;
                            }
                            if shell
                                .update(cx, |shell, cx| {
                                    for code in &shell.code_tabs {
                                        code.update(cx, |code, cx| {
                                            code.poll_recovery(cx)
                                        });
                                    }
                                    shell.sync_shared_editor_roles(cx);
                                })
                                .is_err()
                            {
                                break;
                            }
                            if let Some(message) = proxy.take_core_message()
                                && shell
                                    .update(cx, |shell, cx| {
                                        shell.status_message = message;
                                        cx.notify();
                                    })
                                    .is_err()
                            {
                                break;
                            }
                        }
                    })
                    .detach();
                    shell.update(cx, |shell, cx| {
                        shell.configure_code(code.clone(), window, cx);
                        shell.restore_unsaved_buffers(window, cx);
                    });
                    let shell_for_trash = shell.downgrade();
                    explorer.update(cx, |explorer, _| {
                        explorer.set_trash_handler(move |path, window, cx| {
                            if let Err(error) =
                                shell_for_trash.update(cx, |shell, cx| {
                                    shell.trash_workspace_path(path, window, cx);
                                })
                            {
                                eprintln!("Moving explorer entry to Trash: {error}");
                            }
                        });
                    });
                    let shell_for_tasks = shell.downgrade();
                    tasks.update(cx, |tasks, _| {
                        tasks.set_run_handler(move |recipe, cwd, justfile, window, cx| {
                            _ = shell_for_tasks.update(cx, |shell, cx| {
                                shell.new_task_terminal(recipe, cwd, justfile, window, cx);
                            });
                        });
                        let shell_for_tasks = shell.downgrade();
                        tasks.set_watch_handler(
                            move |recipe, cwd, justfile, window, cx| {
                                _ = shell_for_tasks.update(cx, |shell, cx| {
                                    shell.watch_just_task_terminal(
                                        recipe, cwd, justfile, window, cx,
                                    );
                                });
                            },
                        );
                    });
                    let shell_for_session_settings = shell.downgrade();
                    session.update(cx, |session, _| {
                        session.set_open_settings_handler(move |window, cx| {
                            _ = shell_for_session_settings.update(
                                cx,
                                |shell, cx| {
                                    shell.open_center_panel(
                                        CenterPanel::Settings,
                                        window,
                                        cx,
                                    );
                                },
                            );
                        });
                    });
                    let shell_for_settings = shell.downgrade();
                    settings.update(cx, |settings, _| {
                        settings.set_close_handler(move |panel, window, cx| {
                            _ = shell_for_settings.update(cx, |shell, cx| {
                                shell.close_center_panel(panel, window, cx);
                            });
                        });
                    });
                    let shell_for_extensions = shell.downgrade();
                    extensions.update(cx, |extensions, _| {
                        extensions.set_close_handler(move |panel, window, cx| {
                            _ = shell_for_extensions.update(cx, |shell, cx| {
                                shell.close_center_panel(panel, window, cx);
                            });
                        });
                    });
                    let shell_for_search = shell.downgrade();
                    let shell_for_help = shell.downgrade();
                    help.update(cx, |help, _| {
                        help.set_close_handler(move |panel, window, cx| {
                            _ = shell_for_help.update(cx, |shell, cx| {
                                shell.close_center_panel(panel, window, cx);
                            });
                        });
                    });
                    search.update(cx, |search, _| {
                        search.set_close_handler(move |panel, window, cx| {
                            _ = shell_for_search.update(cx, |shell, cx| {
                                shell.close_center_panel(panel, window, cx);
                            });
                        });
                    });
                    let shell_for_search_replace = shell.downgrade();
                    search.update(cx, |search, _| {
                        search.set_replace_handler(move |request, window, cx| {
                            shell_for_search_replace
                                .update(cx, |shell, cx| {
                                    shell.apply_search_replacements(
                                        request, window, cx,
                                    )
                                })
                                .map_err(|error| error.to_string())?
                        });
                    });
                    let shell_for_problems = shell.downgrade();
                    problems.update(cx, |problems, _| {
                        problems.set_close_handler(move |panel, window, cx| {
                            _ = shell_for_problems.update(cx, |shell, cx| {
                                shell.close_center_panel(panel, window, cx);
                            });
                        });
                    });
                    shell.update(cx, |shell, cx| {
                        shell.configure_terminal(terminal.clone(), cx);
                    });
                    let shell_for_debug = shell.downgrade();
                    debug_bar.update(cx, |debug_bar, _| {
                        let shell_for_close = shell_for_debug.clone();
                        let shell_for_navigation = shell_for_debug.clone();
                        debug_bar.set_close_handler(move |window, cx| {
                            _ = shell_for_debug.update(cx, |shell, cx| {
                                shell.close_debug(window, cx);
                            });
                        });
                        debug_bar.set_new_terminal_handler(move |window, cx| {
                            _ = shell_for_close.update(cx, |shell, cx| {
                                shell.new_terminal(window, cx);
                            });
                        });
                        debug_bar.set_source_navigation_handler(
                            move |path, line, window, cx| {
                                _ = shell_for_navigation.update(
                                    cx,
                                    |shell, cx| {
                                        shell.open_file_request(
                                            crate::ross::OpenRequest {
                                                path,
                                                permanent: true,
                                                location: Some(
                                                    crate::ross::OpenLocation {
                                                        line: line
                                                            .saturating_sub(1)
                                                            as usize,
                                                        column:
                                                            crate::ross::OpenColumn::Character(0),
                                                        end_line: line
                                                            .saturating_sub(1)
                                                            as usize,
                                                        end_column:
                                                            crate::ross::OpenColumn::Character(0),
                                                    },
                                                ),
                                            },
                                            window,
                                            cx,
                                        );
                                    },
                                );
                            },
                        );
                    });
                    // Center: editor tabs. Bottom utility dock: Terminal and Debug tabs.
                    shell.update(cx, |shell, cx| {
                        shell.show_code(window, cx);
                        shell.set_bottom_layout(false, window, cx);
                    });

                    // Right dock: Agent and Threads share one panel header and always stay together.
                    let right = DockLayout::tabs()
                        .panel_view(panel_handle(agent_workspace.clone()), cx);

                    area.update(cx, |area, cx| {
                        area.set_dock(DockPlacement::Right, right, window, cx);
                        area.set_dock_size(
                            DockPlacement::Right,
                            px(640.),
                            window,
                            cx,
                        );
                        area.set_dock_size(
                            DockPlacement::Left,
                            px(240.),
                            window,
                            cx,
                        );
                    });
                    activity.update(cx, |activity, cx| {
                        activity.show(WorkspaceView::Explorer, window, cx)
                    });
                    let window_handle = window.window_handle();
                    let shell_for_shortcuts = shell.downgrade();
                    let shortcut_interceptor =
                        cx.intercept_keystrokes(move |event, window, cx| {
                            if window.window_handle() != window_handle
                                || window.has_active_dialog(cx)
                            {
                                return;
                            }
                            let modifiers = event.keystroke.modifiers;
                            let Some(command) = shell_shortcut(
                                event.keystroke.key.as_str(),
                                modifiers.platform,
                                modifiers.control,
                                modifiers.alt,
                                modifiers.shift,
                            ) else {
                                return;
                            };
                            if command == ShellShortcut::CloseActiveItem
                                && !cfg!(target_os = "macos")
                                && event.keystroke.key == "w"
                                && matches!(
                                    shell_for_shortcuts.read_with(cx, |shell, cx| {
                                        shell.focused_terminal_id(window, cx).is_some()
                                    }),
                                    Ok(true)
                                )
                            {
                                return;
                            }
                            if let Err(error) =
                                shell_for_shortcuts.update(cx, |shell, cx| {
                                    shell.run_shell_command(command, window, cx);
                                })
                            {
                                eprintln!("AHEAD could not run shortcut: {error}");
                            }
                            window.prevent_default();
                            cx.stop_propagation();
                        });
                    shell.update(cx, |shell, _| {
                        shell._shortcut_interceptor = Some(shortcut_interceptor);
                    });
                    let shell_focus = shell.read(cx).focus.clone();
                    let root = cx
                        .new(|cx| gpui_kit::component::Root::new(shell, window, cx));
                    window.focus(&shell_focus, cx);
                    let language_servers_for_prompt = language_servers.downgrade();
                    window.defer(cx, move |window, cx| {
                        if let Err(error) = language_servers_for_prompt.update(
                            cx,
                            |panel, cx| panel.prompt_if_undecided(window, cx),
                        ) {
                            eprintln!("AHEAD language-server prompt was unavailable: {error}");
                        }
                    });
                    root
                })
                .expect("Failed to open AHEAD window");
                cx.update(|cx| cx.activate(true));
            })
            .detach();
        });
}

#[cfg(test)]
#[path = "app_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
mod shortcut_tests {
    #[test]
    fn close_actions_only_select_their_original_targets() {
        use crate::code_panel::CodeTabAction::*;
        for (action, expected) in [
            (Close, vec![2]),
            (CloseOthers, vec![0, 1, 3, 4]),
            (CloseLeft, vec![0, 1]),
            (CloseRight, vec![3, 4]),
            (CloseAll, vec![0, 1, 2, 3, 4]),
            (Promote, vec![]),
        ] {
            assert_eq!(
                (0..5)
                    .filter(|candidate| super::closes_code_tab(
                        action, 2, *candidate
                    ))
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn moving_a_tab_toggles_only_its_editor_group() {
        let mut right_group = std::collections::HashSet::from([2_u64]);

        assert_eq!(
            super::toggle_editor_group(&mut right_group, 1),
            super::EditorGroupSide::Right
        );
        assert_eq!(right_group, std::collections::HashSet::from([1, 2]));
        assert_eq!(
            super::toggle_editor_group(&mut right_group, 1),
            super::EditorGroupSide::Left
        );
        assert_eq!(right_group, std::collections::HashSet::from([2]));
    }

    #[test]
    fn dock_group_changes_update_tab_group_membership() {
        let mut right_group = std::collections::HashSet::from([2_u64]);
        let mut left_group_id = Some(10);
        let mut right_group_id = Some(20);

        assert_eq!(
            super::assign_editor_group(
                &mut right_group,
                &mut left_group_id,
                &mut right_group_id,
                1,
                20,
            ),
            super::EditorGroupSide::Right
        );
        assert!(right_group.contains(&1));
        assert_eq!(
            super::assign_editor_group(
                &mut right_group,
                &mut left_group_id,
                &mut right_group_id,
                1,
                10,
            ),
            super::EditorGroupSide::Left
        );
        assert!(!right_group.contains(&1));
    }

    use super::{ShellShortcut, shell_shortcut};

    #[test]
    fn vscode_shortcuts_do_not_open_ahead_panels() {
        let control = !cfg!(target_os = "macos");
        let primary =
            |key, alt, shift| shell_shortcut(key, true, control, alt, shift);

        assert_eq!(primary("p", false, false), Some(ShellShortcut::QuickOpen));
        assert_eq!(primary("m", false, true), Some(ShellShortcut::Problems));
        assert_eq!(primary("e", false, true), Some(ShellShortcut::Explorer));
        assert_eq!(primary("f", false, true), Some(ShellShortcut::Search));
        assert_eq!(primary("d", false, true), Some(ShellShortcut::Debug));
        assert_eq!(
            primary("w", false, false),
            Some(ShellShortcut::CloseActiveItem)
        );
        assert_eq!(primary("x", false, true), Some(ShellShortcut::Extensions));
        assert_eq!(primary("b", true, false), Some(ShellShortcut::ToggleRight));
        assert_eq!(
            primary("p", false, true),
            Some(ShellShortcut::CommandPalette)
        );
        assert_eq!(
            shell_shortcut("f1", false, false, false, false),
            Some(ShellShortcut::CommandPalette)
        );
        assert_eq!(
            shell_shortcut("f9", false, false, false, false),
            Some(ShellShortcut::DebugKey("f9", false))
        );
        for key in ["1", "2", "3", "k", "l", "r", "t"] {
            assert_eq!(
                primary(key, false, key == "k" || key == "l" || key == "t"),
                None
            );
        }

        assert_eq!(
            shell_shortcut("g", !cfg!(target_os = "macos"), true, false, true),
            Some(ShellShortcut::SourceControl)
        );
        assert_eq!(
            shell_shortcut("`", !cfg!(target_os = "macos"), true, false, false),
            Some(ShellShortcut::ToggleBottom)
        );
        if !cfg!(target_os = "macos") {
            assert_eq!(
                shell_shortcut("f4", true, true, false, false),
                Some(ShellShortcut::CloseActiveItem)
            );
        }
    }
}

#[cfg(test)]
mod just_task_terminal_command_tests {
    use super::just_task_terminal_command;

    #[test]
    fn watched_tasks_use_watchexec_without_changing_direct_runs() {
        let justfile = "/workspace/task's directory/.justfile";

        assert_eq!(
            just_task_terminal_command(justfile, "test's recipe", false),
            "just --justfile '.justfile' 'test'\\''s recipe'"
        );
        assert_eq!(
            just_task_terminal_command(justfile, "test's recipe", true),
            "watchexec --restart --clear --shell=none -- just --justfile '.justfile' 'test'\\''s recipe'"
        );
    }
}

#[cfg(test)]
mod editor_navigation_history_tests {
    use super::EditorNavigationHistory;
    use crate::ross::{OpenColumn, OpenLocation, OpenRequest};

    fn location(line: usize) -> OpenRequest {
        OpenRequest {
            path: "src/lib.rs".into(),
            permanent: true,
            location: Some(OpenLocation {
                line,
                column: OpenColumn::Character(0),
                end_line: line,
                end_column: OpenColumn::Character(0),
            }),
        }
    }

    #[test]
    fn back_and_forward_restore_the_previous_editor_location() {
        let mut history = EditorNavigationHistory::default();
        history.record_jump(Some(location(10)), &location(20));

        assert_eq!(history.go_back(Some(location(20))), Some(location(10)));
        assert_eq!(history.go_forward(Some(location(10))), Some(location(20)));
    }

    #[test]
    fn a_new_jump_after_going_back_discards_the_forward_branch() {
        let mut history = EditorNavigationHistory::default();
        history.record_jump(Some(location(10)), &location(20));
        assert_eq!(history.go_back(Some(location(20))), Some(location(10)));

        history.record_jump(Some(location(10)), &location(30));

        assert_eq!(history.go_forward(Some(location(30))), None);
        assert_eq!(history.go_back(Some(location(30))), Some(location(10)));
    }
}

#[cfg(test)]
mod presentation_request_tests {
    use super::{
        EditorPresentationRequest, presentation_request_matches_active_turn,
    };

    #[test]
    fn stale_session_and_turn_requests_are_rejected() {
        let request = EditorPresentationRequest {
            session_id: "session-a".to_string(),
            turn_id: "turn-a".to_string(),
            request_id: "request-a".to_string(),
            action: ahead_rpc::ahead::AgentPresentationAction::StopSpeaking,
        };

        assert!(presentation_request_matches_active_turn(
            &request,
            Some("session-a"),
            Some("turn-a"),
        ));
        assert!(!presentation_request_matches_active_turn(
            &request,
            Some("session-a"),
            Some("turn-b"),
        ));
        assert!(!presentation_request_matches_active_turn(
            &request,
            Some("session-b"),
            Some("turn-a"),
        ));
    }
}

#[cfg(test)]
mod workspace_startup_tests {
    use super::{
        canonical_workspace_directory, launch_paths, should_prompt_for_workspace,
        workspace_prompt_options,
    };
    use gpui_kit::TestAppContext;
    use std::path::Path;

    #[test]
    fn only_prompts_when_launch_has_no_workspace_and_cwd_is_a_filesystem_root() {
        assert!(should_prompt_for_workspace(Path::new("/"), "", None));
        assert!(!should_prompt_for_workspace(
            Path::new("/workspace"),
            "",
            None,
        ));
        assert!(!should_prompt_for_workspace(
            Path::new("/"),
            "",
            Some("/workspace"),
        ));
        assert!(!should_prompt_for_workspace(
            Path::new("/"),
            "/workspace/src/main.rs",
            None,
        ));
    }

    #[test]
    fn selected_workspace_must_be_a_canonical_directory() {
        let workspace =
            canonical_workspace_directory(Path::new(env!("CARGO_MANIFEST_DIR")))
                .expect("manifest directory is a workspace");
        assert!(workspace.is_absolute());
        assert!(workspace.is_dir());

        let file = std::env::current_exe().expect("test executable path");
        assert!(canonical_workspace_directory(&file).is_err());
    }

    #[test]
    fn directory_argument_opens_that_workspace_without_a_file_tab() {
        let project = tempfile::tempdir().expect("disposable project");
        let project_path = project.path().to_string_lossy().into_owned();
        let canonical_project_path = project
            .path()
            .canonicalize()
            .expect("canonical disposable project")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            launch_paths(project_path.clone(), None, Path::new("/"))
                .expect("launch paths"),
            (String::new(), canonical_project_path)
        );
    }

    #[gpui_kit::test]
    async fn workspace_picker_selects_one_directory_or_cancels(
        cx: &mut TestAppContext,
    ) {
        let selected = vec![Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()];
        let receiver =
            cx.update(|cx| cx.prompt_for_paths(workspace_prompt_options()));
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response({
            let selected = selected.clone();
            move |options| {
                assert!(!options.files);
                assert!(options.directories);
                assert!(!options.multiple);
                assert_eq!(options.prompt.as_deref(), Some("Open Workspace"));
                Some(selected)
            }
        });
        assert_eq!(receiver.await.unwrap().unwrap(), Some(selected));

        let receiver =
            cx.update(|cx| cx.prompt_for_paths(workspace_prompt_options()));
        cx.simulate_path_prompt_response(|options| {
            assert!(!options.files);
            assert!(options.directories);
            assert!(!options.multiple);
            None
        });
        assert_eq!(receiver.await.unwrap().unwrap(), None);
    }
}
