//! Workspace activity bar and editor-side utility panels.

use std::path::{Path, PathBuf};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use ahead_rpc::source_control::GitWorkspaceStatus;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, DockArea, DockLayout, DockPlacement, Panel, PanelControl, PanelEvent,
    PanelId, TabGroup, panel_handle,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, Disableable, Selectable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit_assets::IconName;

use crate::code_panel::CodePanel;
use crate::explorer_panel::ExplorerPanel;
use crate::proxy_client::ProxyClient;

// Expire the UI prompt before the askpass bridge's five-minute request timeout.
const GIT_CREDENTIAL_PROMPT_TIMEOUT: Duration = Duration::from_secs(270);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceView {
    Explorer,
    Git,
    Tasks,
    LanguageServers,
}

pub struct ActivityBar {
    area: Entity<DockArea>,
    explorer: Entity<ExplorerPanel>,
    git: Entity<GitPanel>,
    tasks: Entity<JustTasksPanel>,
    language_servers: Entity<LanguageServersPanel>,
    active: WorkspaceView,
}

impl ActivityBar {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        area: Entity<DockArea>,
        explorer: Entity<ExplorerPanel>,
        git: Entity<GitPanel>,
        tasks: Entity<JustTasksPanel>,
        language_servers: Entity<LanguageServersPanel>,
    ) -> Self {
        Self {
            area,
            explorer,
            git,
            tasks,
            language_servers,
            active: WorkspaceView::Explorer,
        }
    }

    pub fn show(
        &mut self,
        view: WorkspaceView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active = view;
        let area = self.area.clone();
        let explorer = self.explorer.clone();
        let git = self.git.clone();
        let tasks = self.tasks.clone();
        let language_servers = self.language_servers.clone();
        window.defer(cx, move |window, cx| {
            let panel = match view {
                WorkspaceView::Explorer => {
                    explorer.update(cx, |explorer, cx| explorer.refresh(cx));
                    panel_handle(explorer)
                }
                WorkspaceView::Git => {
                    git.update(cx, |git, cx| git.refresh(cx));
                    panel_handle(git)
                }
                WorkspaceView::Tasks => panel_handle(tasks),
                WorkspaceView::LanguageServers => {
                    language_servers.update(cx, |panel, cx| panel.refresh_trust(cx));
                    panel_handle(language_servers)
                }
            };
            area.update(cx, |area, cx| {
                area.set_dock(
                    DockPlacement::Left,
                    DockLayout::tabs().panel_view(panel, cx),
                    window,
                    cx,
                );
                area.set_dock_size(DockPlacement::Left, px(240.), window, cx);
            });
        });
        cx.notify();
    }

    pub fn show_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show(self.active, window, cx);
    }

    pub fn active(&self) -> WorkspaceView {
        self.active
    }

    pub(crate) fn stage_all_changes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.git.update(cx, |git, cx| git.stage_all(window, cx));
    }

    pub(crate) fn can_commit_staged(&self, cx: &App) -> bool {
        self.git.read(cx).can_commit_staged(cx)
    }

    pub(crate) fn commit_staged_changes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.git.update(cx, |git, cx| git.commit_staged(window, cx));
    }

    pub(crate) fn command_tasks(&self, cx: &App) -> Vec<JustTask> {
        self.tasks.read(cx).tasks.clone()
    }

    pub fn refresh_active_git_status(&mut self, cx: &mut Context<Self>) {
        match self.active {
            WorkspaceView::Explorer => self
                .explorer
                .update(cx, |panel, cx| panel.refresh_git_badges(cx)),
            WorkspaceView::Git => self
                .git
                .update(cx, |panel, cx| panel.refresh_from_event(cx)),
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JustTask {
    pub justfile: PathBuf,
    pub cwd: PathBuf,
    pub name: String,
    pub description: Option<String>,
}

pub struct JustTasksPanel {
    pub focus: FocusHandle,
    root: PathBuf,
    tasks: Vec<JustTask>,
    status: SharedString,
    run_handler: Option<Rc<dyn Fn(String, String, String, &mut Window, &mut App)>>,
    watch_handler: Option<Rc<dyn Fn(String, String, String, &mut Window, &mut App)>>,
}

impl JustTasksPanel {
    pub fn new(root: &str, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            root: PathBuf::from(root),
            tasks: Vec::new(),
            status: "Looking for justfiles…".into(),
            run_handler: None,
            watch_handler: None,
        }
    }

    pub fn set_run_handler<F>(&mut self, handler: F)
    where
        F: Fn(String, String, String, &mut Window, &mut App) + 'static,
    {
        self.run_handler = Some(Rc::new(handler));
    }

    pub fn set_watch_handler<F>(&mut self, handler: F)
    where
        F: Fn(String, String, String, &mut Window, &mut App) + 'static,
    {
        self.watch_handler = Some(Rc::new(handler));
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.status = "Looking for justfiles…".into();
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let tasks = cx
                .background_spawn(async move {
                    discover_justfiles(&root)
                        .into_iter()
                        .flat_map(|justfile| list_just_tasks(&justfile))
                        .collect::<Vec<_>>()
                })
                .await;
            if let Err(error) = this.update(cx, |panel, cx| {
                panel.tasks = tasks;
                panel.status = if panel.tasks.is_empty() {
                    "No just recipes found".into()
                } else {
                    format!(
                        "{} recipe{} from justfiles",
                        panel.tasks.len(),
                        if panel.tasks.len() == 1 { "" } else { "s" }
                    )
                    .into()
                };
                cx.notify();
            }) {
                eprintln!("Could not publish discovered Just tasks: {error:#}");
            }
        })
        .detach();
        cx.notify();
    }

    fn run_task(&self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.get(index) else {
            return;
        };
        let Some(handler) = self.run_handler.as_ref() else {
            return;
        };
        handler(
            task.name.clone(),
            task.cwd.to_string_lossy().into_owned(),
            task.justfile.to_string_lossy().into_owned(),
            window,
            cx,
        );
    }

    fn watch_task(&self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.get(index) else {
            return;
        };
        let Some(handler) = self.watch_handler.as_ref() else {
            return;
        };
        handler(
            task.name.clone(),
            task.cwd.to_string_lossy().into_owned(),
            task.justfile.to_string_lossy().into_owned(),
            window,
            cx,
        );
    }
}

fn discover_justfiles(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if root.parent().is_some() {
        collect_justfiles(root, &mut files);
    } else {
        for name in ["justfile", "Justfile", ".justfile"] {
            let path = root.join(name);
            if path.is_file() {
                files.push(path);
            }
        }
    }
    let mut ancestor = root.parent();
    while let Some(directory) = ancestor {
        for name in ["justfile", "Justfile", ".justfile"] {
            let path = directory.join(name);
            if path.is_file() {
                files.push(path);
            }
        }
        ancestor = directory.parent();
    }
    files.sort();
    files.dedup();
    files
}

fn collect_justfiles(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_file()
            && matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("justfile" | "Justfile" | ".justfile")
            )
        {
            files.push(path);
        } else if file_type.is_dir()
            && !matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some(".git" | "target" | "node_modules")
            )
        {
            collect_justfiles(&path, files);
        }
    }
}

fn list_just_tasks(justfile: &Path) -> Vec<JustTask> {
    let Some(cwd) = justfile.parent() else {
        return Vec::new();
    };
    let Some(justfile) = justfile.to_str() else {
        return Vec::new();
    };
    let Ok(output) = std::process::Command::new("just")
        .args(["--list", "--justfile", justfile])
        .current_dir(cwd)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    parse_just_list(
        &String::from_utf8_lossy(&output.stdout),
        Path::new(justfile),
        cwd,
    )
}

fn parse_just_list(output: &str, justfile: &Path, cwd: &Path) -> Vec<JustTask> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line == "Available recipes:" {
                return None;
            }
            let (recipe, description) = line.split_once(" # ").map_or(
                (line, None),
                |(recipe, description)| {
                    (recipe, Some(description.trim().to_string()))
                },
            );
            let name = recipe.split_whitespace().next()?.to_string();
            Some(JustTask {
                justfile: justfile.to_path_buf(),
                cwd: cwd.to_path_buf(),
                name,
                description,
            })
        })
        .collect()
}

impl BasePanel for JustTasksPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_just_tasks"
    }
}

impl Panel for JustTasksPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        "Tasks"
    }

    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}

impl EventEmitter<PanelEvent> for JustTasksPanel {}

impl Focusable for JustTasksPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for JustTasksPanel {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let text = cx.theme().sidebar_foreground;
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let tasks = self.tasks.clone();
        let tasks_empty = tasks.is_empty();
        v_flex()
            .size_full()
            .min_h_0()
            .gap_2()
            .p_3()
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(border)
                    .pb_2()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(IconName::ListTodo)
                            .child(div().text_color(text).child("Tasks")),
                    )
                    .child(
                        Button::new("refresh_just_tasks")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .tooltip("Refresh Tasks")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.refresh(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(muted)
                    .child(self.status.clone()),
            )
            .child(v_flex().flex_1().min_h_0().overflow_y_scrollbar().children(
                tasks.into_iter().enumerate().map(|(index, task)| {
                    let recipe = task.name.clone();
                    let cwd = task.cwd.to_string_lossy().into_owned();
                    let description = task.description.unwrap_or_default();
                    h_flex()
                        .id(("just_task", index))
                        .items_start()
                        .gap_2()
                        .p_2()
                        .border_b_1()
                        .border_color(border)
                        .child(
                            Button::new(("run_just_task", index))
                                .icon(IconName::Play)
                                .label(recipe.clone())
                                .ghost()
                                .tooltip(format!("Run just {recipe}"))
                                .on_click(cx.listener(
                                    move |this: &mut Self, _, window, cx| {
                                        this.run_task(index, window, cx);
                                    },
                                )),
                        )
                        .child(
                            Button::new(format!("watch_just_task_{index}"))
                                .debug_selector(move || {
                                    format!("watch_just_task_{index}")
                                })
                                .icon(IconName::RefreshCw)
                                .ghost()
                                .accessibility_label(format!("Watch {recipe}"))
                                .tooltip(format!(
                                    "Watch {recipe} and restart it when files change (requires watchexec)"
                                ))
                                .on_click(cx.listener(
                                    move |this: &mut Self, _, window, cx| {
                                        this.watch_task(index, window, cx);
                                    },
                                )),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(11.))
                                        .text_color(muted)
                                        .child(cwd),
                                )
                                .child(div().text_color(text).child(description)),
                        )
                }),
            ))
            .when(tasks_empty, |el| {
                el.child(div().text_color(muted).child(
                    "Add a justfile with public recipes to use editor tasks.",
                ))
            })
    }
}

pub struct GitPanel {
    pub focus: FocusHandle,
    pub root: String,
    pub branch: String,
    branches: Vec<String>,
    branch_has_upstream: bool,
    remotes: Vec<String>,
    repository_dirty: bool,
    repository_has_conflicts: bool,
    pub files: Vec<GitFile>,
    commits: Vec<ahead_rpc::source_control::GitCommitSummary>,
    selected_commit: Option<ahead_rpc::source_control::GitCommitSummary>,
    worktree_diff_target: Option<WorktreeDiffTarget>,
    commit_patch: Option<ahead_rpc::source_control::GitPatch>,
    commit_patch_lines: Arc<Vec<String>>,
    commit_patch_loading: bool,
    commit_patch_error: Option<String>,
    commit_patch_copied: bool,
    commit_patch_generation: u64,
    commit_patch_scroll: UniformListScrollHandle,
    show_history: bool,
    history_loading: bool,
    history_error: Option<String>,
    history_generation: u64,
    pub commit_message: Entity<InputState>,
    branch_create_input: Entity<InputState>,
    git_username_input: Entity<InputState>,
    git_secret_input: Entity<InputState>,
    git_credential_prompt: Option<(u64, String, bool)>,
    branch_create_open: bool,
    pub status: SharedString,
    proxy: Option<Arc<ProxyClient>>,
    open_file_handler: Option<Rc<dyn Fn(PathBuf, &mut Window, &mut App)>>,
    git_refresh_task: Option<Task<()>>,
    stage_pending: bool,
    commit_pending: bool,
    checkout_pending: bool,
    branch_create_pending: bool,
    branch_delete_pending: bool,
    push_pending: bool,
    repository_missing: bool,
    git_init_pending: bool,
}

#[derive(Clone, Debug)]
pub struct GitFile {
    pub path: String,
    pub state: String,
    staged: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum WorktreeDiffTarget {
    File(String),
    All,
}

impl GitFile {
    fn is_conflicted(&self) -> bool {
        let mut status = self.state.bytes();
        let Some(index_status) = status.next() else {
            return false;
        };
        let Some(worktree_status) = status.next() else {
            return false;
        };
        index_status == b'U'
            || worktree_status == b'U'
            || matches!((index_status, worktree_status), (b'A', b'A') | (b'D', b'D'))
    }
}

impl GitPanel {
    pub fn new(root: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let commit_message = cx.new(|cx| InputState::new(window, cx));
        let branch_create_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("New branch name"));
        let git_username_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Username"));
        let git_secret_input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Password or access token")
        });
        cx.subscribe_in(
            &branch_create_input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change if this.branch_create_open => cx.notify(),
                InputEvent::PressEnter { shift: false, .. }
                    if this.branch_create_open =>
                {
                    this.create_branch(window, cx);
                }
                _ => {}
            },
        )
        .detach();
        for input in [&git_username_input, &git_secret_input] {
            cx.subscribe_in(
                input,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change if this.git_credential_prompt.is_some() => {
                        cx.notify();
                    }
                    InputEvent::PressEnter { shift: false, .. }
                        if this.git_credential_prompt.is_some() =>
                    {
                        this.submit_git_credential(window, cx);
                    }
                    _ => {}
                },
            )
            .detach();
        }
        let panel = Self {
            focus: cx.focus_handle(),
            root: root.to_string(),
            branch: String::new(),
            branches: Vec::new(),
            branch_has_upstream: false,
            remotes: Vec::new(),
            repository_dirty: false,
            repository_has_conflicts: false,
            files: Vec::new(),
            commits: Vec::new(),
            selected_commit: None,
            worktree_diff_target: None,
            commit_patch: None,
            commit_patch_lines: Arc::new(Vec::new()),
            commit_patch_loading: false,
            commit_patch_error: None,
            commit_patch_copied: false,
            commit_patch_generation: 0,
            commit_patch_scroll: UniformListScrollHandle::new(),
            show_history: false,
            history_loading: false,
            history_error: None,
            history_generation: 0,
            commit_message,
            branch_create_input,
            git_username_input,
            git_secret_input,
            git_credential_prompt: None,
            branch_create_open: false,
            status: "Git status not loaded".into(),
            proxy: None,
            open_file_handler: None,
            git_refresh_task: None,
            stage_pending: false,
            commit_pending: false,
            checkout_pending: false,
            branch_create_pending: false,
            branch_delete_pending: false,
            push_pending: false,
            repository_missing: false,
            git_init_pending: false,
        };
        panel
    }

    pub fn with_proxy(mut self, proxy: Arc<ProxyClient>) -> Self {
        self.proxy = Some(proxy);
        self
    }

    pub(crate) fn request_git_credential(
        &mut self,
        request_id: u64,
        prompt: String,
        secret: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((previous_id, _, _)) = self.git_credential_prompt.take() {
            if let Some(proxy) = self.proxy.as_ref() {
                proxy.respond_core_request(
                    previous_id,
                    ahead_rpc::core::CoreResponse::GitCredentialAnswer {
                        answer: None,
                    },
                );
            }
        }
        self.clear_git_credential_inputs(window, cx);
        let prompt = prompt
            .chars()
            .filter(|character| !character.is_control())
            .take(1024)
            .collect::<String>();
        let input = if secret {
            &self.git_secret_input
        } else {
            &self.git_username_input
        };
        self.git_credential_prompt = Some((request_id, prompt, secret));
        self.status = "Waiting for Git credentials".into();
        input.focus_handle(cx).focus(window, cx);
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(GIT_CREDENTIAL_PROMPT_TIMEOUT).await;
            if let Err(error) = this.update_in(cx, |panel, window, cx| {
                if panel
                    .git_credential_prompt
                    .as_ref()
                    .is_some_and(|(active_id, _, _)| *active_id == request_id)
                {
                    panel.cancel_git_credential(window, cx);
                    panel.status = "Git authentication timed out".into();
                    cx.notify();
                }
            }) {
                eprintln!(
                    "Source Control panel closed before Git authentication timed out: {error}"
                );
            }
        })
        .detach();
    }

    fn clear_git_credential_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.git_username_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.git_secret_input
            .update(cx, |input, cx| input.set_value("", window, cx));
    }

    fn submit_git_credential(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((request_id, _, secret)) = self.git_credential_prompt.as_ref()
        else {
            return;
        };
        let request_id = *request_id;
        let secret = *secret;
        let answer = if secret {
            self.git_secret_input.read(cx).value().to_string()
        } else {
            self.git_username_input.read(cx).value().to_string()
        };
        if answer.len() > 8 * 1024 {
            self.status = "Git credentials must be 8 KiB or smaller".into();
            cx.notify();
            return;
        }
        self.git_credential_prompt = None;
        if let Some(proxy) = self.proxy.as_ref() {
            proxy.respond_core_request(
                request_id,
                ahead_rpc::core::CoreResponse::GitCredentialAnswer {
                    answer: Some(answer),
                },
            );
        }
        self.clear_git_credential_inputs(window, cx);
        self.status = "Sending Git credentials…".into();
        cx.notify();
    }

    fn cancel_git_credential(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((request_id, _, _)) = self.git_credential_prompt.take() else {
            return;
        };
        if let Some(proxy) = self.proxy.as_ref() {
            proxy.respond_core_request(
                request_id,
                ahead_rpc::core::CoreResponse::GitCredentialAnswer { answer: None },
            );
        }
        self.clear_git_credential_inputs(window, cx);
        self.status = "Git authentication canceled".into();
        cx.notify();
    }

    pub fn set_open_file_handler<F>(&mut self, handler: F)
    where
        F: 'static + Fn(PathBuf, &mut Window, &mut App),
    {
        self.open_file_handler = Some(Rc::new(handler));
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.git_refresh_task = None;
        let Some(proxy) = self.proxy.as_ref() else {
            self.status = "Git service unavailable".into();
            cx.notify();
            return;
        };
        self.status = "Refreshing Git status…".into();
        cx.notify();
        let response = proxy.git_status();
        self.git_refresh_task = Some(cx.spawn(async move |this, cx| {
            let result = response
                .recv()
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            if let Err(error) = this.update(cx, |panel, cx| {
                match result {
                    Ok(status) => panel.apply_git_status(status),
                    Err(error) => {
                        panel.repository_missing = git_repository_not_found(&error);
                        panel.git_init_pending = false;
                        panel.status = if panel.repository_missing {
                            "This workspace is not a Git repository yet".into()
                        } else {
                            format!("Git status failed: {error}").into()
                        };
                    }
                }
                cx.notify();
            }) {
                eprintln!("Source Control panel closed during Git refresh: {error}");
            }
        }));
    }

    fn show_changes(&mut self, cx: &mut Context<Self>) {
        if self.worktree_diff_target.take().is_some() {
            self.selected_commit = None;
            self.commit_patch = None;
            self.commit_patch_lines = Arc::new(Vec::new());
            self.commit_patch_loading = false;
            self.commit_patch_error = None;
            self.commit_patch_copied = false;
            self.commit_patch_generation =
                self.commit_patch_generation.wrapping_add(1);
        }
        self.show_history = false;
        cx.notify();
    }

    fn show_history(&mut self, cx: &mut Context<Self>) {
        if self.worktree_diff_target.take().is_some() {
            self.selected_commit = None;
            self.commit_patch = None;
            self.commit_patch_lines = Arc::new(Vec::new());
            self.commit_patch_loading = false;
            self.commit_patch_error = None;
            self.commit_patch_copied = false;
            self.commit_patch_generation =
                self.commit_patch_generation.wrapping_add(1);
        }
        self.show_history = true;
        if self.commits.is_empty() && !self.history_loading {
            self.load_history(cx);
        }
        cx.notify();
    }

    fn open_history_commit(
        &mut self,
        commit: ahead_rpc::source_control::GitCommitSummary,
        cx: &mut Context<Self>,
    ) {
        self.worktree_diff_target = None;
        let Some(proxy) = self.proxy.as_ref() else {
            self.selected_commit = Some(commit);
            self.commit_patch_error = Some("Git service unavailable".into());
            self.commit_patch_loading = false;
            cx.notify();
            return;
        };
        self.selected_commit = Some(commit.clone());
        self.commit_patch = None;
        self.commit_patch_lines = Arc::new(Vec::new());
        self.commit_patch_error = None;
        self.commit_patch_copied = false;
        self.commit_patch_loading = true;
        self.commit_patch_generation = self.commit_patch_generation.wrapping_add(1);
        let generation = self.commit_patch_generation;
        let response = proxy.git_commit_patch(commit.id.clone());
        cx.spawn(async move |this, cx| {
            let result = response
                .recv()
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            if let Err(error) = this.update(cx, |panel, cx| {
                if panel.commit_patch_generation != generation {
                    return;
                }
                panel.commit_patch_loading = false;
                match result {
                    Ok(patch) => {
                        panel.commit_patch_lines = Arc::new(
                            patch.patch.lines().map(str::to_string).collect(),
                        );
                        panel.commit_patch = Some(patch);
                        panel
                            .commit_patch_scroll
                            .scroll_to_item(0, ScrollStrategy::Top);
                    }
                    Err(error) => panel.commit_patch_error = Some(error),
                }
                cx.notify();
            }) {
                eprintln!("Git panel closed before commit diff response: {error}");
            }
        })
        .detach();
        cx.notify();
    }

    fn open_worktree_file_diff(&mut self, path: String, cx: &mut Context<Self>) {
        self.open_worktree_diff(WorktreeDiffTarget::File(path), cx);
    }

    fn open_worktree_diff(
        &mut self,
        target: WorktreeDiffTarget,
        cx: &mut Context<Self>,
    ) {
        let Some(proxy) = self.proxy.as_ref() else {
            self.status = "Git service unavailable".into();
            cx.notify();
            return;
        };
        self.show_history = true;
        self.worktree_diff_target = Some(target.clone());
        let (id, subject) = match &target {
            WorktreeDiffTarget::File(path) => {
                (path.clone(), format!("Uncommitted changes · {path}"))
            }
            WorktreeDiffTarget::All => (
                "working-tree".into(),
                "Uncommitted changes · all files".into(),
            ),
        };
        self.selected_commit = Some(ahead_rpc::source_control::GitCommitSummary {
            id,
            author: "Working tree".into(),
            timestamp: chrono::Utc::now().timestamp(),
            subject,
        });
        self.status = match &target {
            WorktreeDiffTarget::File(path) => {
                format!("Loading working-tree diff for {path}").into()
            }
            WorktreeDiffTarget::All => "Loading working-tree diff…".into(),
        };
        self.commit_patch = None;
        self.commit_patch_lines = Arc::new(Vec::new());
        self.commit_patch_error = None;
        self.commit_patch_copied = false;
        self.commit_patch_loading = true;
        self.commit_patch_generation = self.commit_patch_generation.wrapping_add(1);
        let generation = self.commit_patch_generation;
        let response = match target {
            WorktreeDiffTarget::File(path) => {
                proxy.git_worktree_file_patch(PathBuf::from(path))
            }
            WorktreeDiffTarget::All => proxy.git_worktree_patch(),
        };
        cx.spawn(async move |this, cx| {
            let result = response
                .recv()
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            if let Err(error) = this.update(cx, |panel, cx| {
                if panel.commit_patch_generation != generation {
                    return;
                }
                panel.commit_patch_loading = false;
                match result {
                    Ok(patch) => {
                        panel.commit_patch_lines = Arc::new(
                            patch.patch.lines().map(str::to_string).collect(),
                        );
                        panel.commit_patch = Some(patch);
                        panel
                            .commit_patch_scroll
                            .scroll_to_item(0, ScrollStrategy::Top);
                    }
                    Err(error) => panel.commit_patch_error = Some(error),
                }
                cx.notify();
            }) {
                eprintln!("Git panel closed before working diff response: {error}");
            }
        })
        .detach();
        cx.notify();
    }

    fn close_history_commit(&mut self, cx: &mut Context<Self>) {
        let return_to_changes = self.worktree_diff_target.take().is_some();
        self.commit_patch_generation = self.commit_patch_generation.wrapping_add(1);
        self.selected_commit = None;
        self.commit_patch = None;
        self.commit_patch_lines = Arc::new(Vec::new());
        self.commit_patch_loading = false;
        self.commit_patch_error = None;
        self.commit_patch_copied = false;
        if return_to_changes {
            self.show_history = false;
        }
        cx.notify();
    }

    fn load_history(&mut self, cx: &mut Context<Self>) {
        if self.history_loading {
            return;
        }
        let Some(proxy) = self.proxy.as_ref() else {
            self.history_error = Some("Git service unavailable".to_string());
            self.status = "Git service unavailable".into();
            cx.notify();
            return;
        };
        self.history_loading = true;
        self.history_error = None;
        self.status = "Loading commit history…".into();
        self.history_generation = self.history_generation.wrapping_add(1);
        let generation = self.history_generation;
        let response = proxy.git_history(100);
        cx.spawn(async move |this, cx| {
            let result = response
                .recv()
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            if let Err(error) = this.update(cx, |panel, cx| {
                if panel.history_generation != generation {
                    return;
                }
                panel.history_loading = false;
                match result {
                    Ok(commits) => {
                        let count = commits.len();
                        panel.commits = commits;
                        panel.repository_missing = false;
                        panel.history_error = None;
                        panel.status = if count == 0 {
                            "No commits in this repository yet".into()
                        } else {
                            format!("Showing the latest {count} commit{}", if count == 1 { "" } else { "s" }).into()
                        };
                    }
                    Err(error) => {
                        panel.repository_missing = git_repository_not_found(&error);
                        panel.history_error = Some(error.clone());
                        panel.status = format!("Git history failed: {error}").into();
                    }
                }
                cx.notify();
            }) {
                eprintln!("Source Control panel closed before Git history response: {error}");
            }
        })
        .detach();
        cx.notify();
    }

    fn refresh_from_event(&mut self, cx: &mut Context<Self>) {
        self.commits.clear();
        if self.show_history {
            self.load_history(cx);
        } else {
            self.refresh(cx);
        }
    }

    fn apply_git_status(&mut self, status: GitWorkspaceStatus) {
        self.repository_missing = false;
        self.git_init_pending = false;
        self.branches = status.branches;
        self.branch_has_upstream = status.branch_has_upstream;
        self.remotes = status.remotes;
        self.repository_dirty = status.repository_dirty;
        self.repository_has_conflicts = status.repository_has_conflicts;
        let files = status
            .files
            .into_iter()
            .map(|file| GitFile {
                path: file.path,
                state: file.state,
                staged: file.staged,
            })
            .collect::<Vec<_>>();
        self.branch = status.branch;
        self.files = files;
        if !self.stage_pending
            && !self.commit_pending
            && !self.checkout_pending
            && !self.branch_create_pending
            && !self.branch_delete_pending
            && !self.push_pending
        {
            self.status = if self.repository_has_conflicts {
                "Resolve all repository conflicts before committing".into()
            } else {
                format!(
                    "{} change{}",
                    self.files.len(),
                    if self.files.len() == 1 { "" } else { "s" }
                )
                .into()
            };
        }
    }

    fn checkout_branch(&mut self, reference: String, cx: &mut Context<Self>) {
        if reference == self.branch || self.checkout_pending {
            return;
        }
        if self.stage_pending
            || self.commit_pending
            || self.branch_create_pending
            || self.branch_delete_pending
            || self.push_pending
        {
            return;
        }
        if self.repository_dirty || !self.files.is_empty() {
            self.status =
                "Commit or stash repository changes before switching branches"
                    .into();
            cx.notify();
            return;
        }
        if !self.branches.iter().any(|branch| branch == &reference) {
            self.status =
                "Branch list changed; refresh Source Control and try again".into();
            cx.notify();
            return;
        }
        let Some(proxy) = self.proxy.as_ref() else {
            self.status = "Git service unavailable".into();
            cx.notify();
            return;
        };

        self.checkout_pending = true;
        self.status = format!("Switching to {reference}…").into();
        let response = proxy.git_checkout(reference.clone());
        cx.spawn(async move |this, cx| {
            let result = response
                .recv()
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            if let Err(error) = this.update(cx, |panel, cx| {
                panel.checkout_pending = false;
                match result {
                    Ok(()) => {
                        panel.status = format!("Switched to {reference}").into();
                        panel.refresh(cx);
                    }
                    Err(error) => {
                        panel.status = format!("Branch switch failed: {error}").into();
                    }
                }
                cx.notify();
            }) {
                eprintln!("Source Control panel closed before branch switch response: {error}");
            }
        })
        .detach();
        cx.notify();
    }

    fn begin_branch_creation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.repository_dirty || !self.files.is_empty() {
            self.status =
                "Commit or stash repository changes before creating a branch".into();
            cx.notify();
            return;
        }
        if self.repository_missing
            || self.checkout_pending
            || self.stage_pending
            || self.commit_pending
            || self.branch_create_pending
            || self.branch_delete_pending
            || self.push_pending
        {
            return;
        }
        self.branch_create_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.branch_create_open = true;
        self.branch_create_input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn cancel_branch_creation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.branch_create_pending {
            return;
        }
        self.branch_create_open = false;
        self.branch_create_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        cx.notify();
    }

    fn create_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.branch_create_pending
            || self.branch_delete_pending
            || self.push_pending
            || self.repository_dirty
            || !self.files.is_empty()
        {
            return;
        }
        let reference = self.branch_create_input.read(cx).value().trim().to_string();
        if reference.is_empty() {
            self.status = "Enter a branch name".into();
            cx.notify();
            return;
        }
        let Some(proxy) = self.proxy.as_ref() else {
            self.status = "Git service unavailable".into();
            cx.notify();
            return;
        };

        self.branch_create_pending = true;
        self.status = format!("Creating branch {reference}…").into();
        let response = proxy.git_create_branch(reference.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = response
                .recv()
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            if let Err(error) = this.update_in(cx, |panel, window, cx| {
                panel.branch_create_pending = false;
                match result {
                    Ok(()) => {
                        panel.branch_create_open = false;
                        panel.branch_create_input.update(cx, |input, cx| {
                            input.set_value("", window, cx);
                        });
                        panel.status = format!("Created branch {reference}").into();
                        panel.refresh(cx);
                    }
                    Err(error) => {
                        panel.status = format!("Could not create branch: {error}").into();
                    }
                }
                cx.notify();
            }) {
                eprintln!("Source Control panel closed before branch creation response: {error}");
            }
        })
        .detach();
        cx.notify();
    }

    fn delete_branch(&mut self, reference: String, cx: &mut Context<Self>) {
        if self.branch_delete_pending
            || reference == self.branch
            || self.checkout_pending
            || self.branch_create_pending
            || self.stage_pending
            || self.commit_pending
            || self.push_pending
        {
            return;
        }
        if self.repository_dirty || !self.files.is_empty() {
            self.status =
                "Commit or stash repository changes before deleting a branch".into();
            cx.notify();
            return;
        }
        if !self.branches.iter().any(|branch| branch == &reference) {
            self.status =
                "Branch list changed; refresh Source Control and try again".into();
            cx.notify();
            return;
        }
        let Some(proxy) = self.proxy.as_ref() else {
            self.status = "Git service unavailable".into();
            cx.notify();
            return;
        };

        self.branch_delete_pending = true;
        self.status = format!("Deleting merged branch {reference}…").into();
        let response = proxy.git_delete_branch(reference.clone());
        cx.spawn(async move |this, cx| {
            let result = response
                .recv()
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            if let Err(error) = this.update(cx, |panel, cx| {
                panel.branch_delete_pending = false;
                match result {
                    Ok(()) => {
                        panel.status = format!("Deleted branch {reference}").into();
                        panel.refresh(cx);
                    }
                    Err(error) => {
                        panel.status = format!("Could not delete branch: {error}").into();
                    }
                }
                cx.notify();
            }) {
                eprintln!(
                    "Source Control panel closed before branch deletion response: {error}"
                );
            }
        })
        .detach();
        cx.notify();
    }

    fn can_push(&self) -> bool {
        !self.repository_missing
            && !self.remotes.is_empty()
            && !self.branch.is_empty()
            && self.branch != "HEAD"
            && !self.stage_pending
            && !self.commit_pending
            && !self.checkout_pending
            && !self.branch_create_pending
            && !self.branch_delete_pending
            && !self.push_pending
            && !self.branch_create_open
            && self.proxy.is_some()
    }

    fn push_branch(&mut self, remote: Option<String>, cx: &mut Context<Self>) {
        if self.push_pending
            || self.stage_pending
            || self.commit_pending
            || self.checkout_pending
            || self.branch_create_pending
            || self.branch_delete_pending
        {
            return;
        }
        if self.remotes.is_empty() {
            self.status = "Add a Git remote before publishing this branch".into();
            cx.notify();
            return;
        }
        if self.branch.is_empty() || self.branch == "HEAD" {
            self.status = "Create or select a local branch before publishing".into();
            cx.notify();
            return;
        }
        let Some(proxy) = self.proxy.as_ref() else {
            self.status = "Git service unavailable".into();
            cx.notify();
            return;
        };

        let branch = self.branch.clone();
        let publishing = !self.branch_has_upstream;
        self.push_pending = true;
        self.status = if publishing {
            format!("Publishing {branch}…").into()
        } else {
            format!("Pushing {branch}…").into()
        };
        let response = proxy.git_push(remote);
        cx.spawn(async move |this, cx| {
            let result = response
                .recv()
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            if let Err(error) = this.update_in(cx, |panel, window, cx| {
                panel.push_pending = false;
                panel.git_credential_prompt = None;
                panel
                    .git_username_input
                    .update(cx, |input, cx| input.set_value("", window, cx));
                panel
                    .git_secret_input
                    .update(cx, |input, cx| input.set_value("", window, cx));
                match result {
                    Ok(()) => {
                        panel.status = if publishing {
                            format!("Published branch {branch}").into()
                        } else {
                            format!("Pushed branch {branch}").into()
                        };
                        panel.refresh(cx);
                    }
                    Err(error) => {
                        panel.status = git_push_failure_status(&error).into();
                    }
                }
                cx.notify();
            }) {
                eprintln!(
                    "Source Control panel closed before Git push response: {error}"
                );
            }
        })
        .detach();
        cx.notify();
    }

    fn initialize_repository(&mut self, cx: &mut Context<Self>) {
        if !self.repository_missing || self.git_init_pending {
            return;
        }
        let Some(proxy) = self.proxy.as_ref() else {
            self.status = "Git service unavailable".into();
            cx.notify();
            return;
        };
        self.git_init_pending = true;
        self.status = "Initializing Git repository…".into();
        proxy.git_init();
        self.refresh(cx);
        cx.notify();
    }

    fn stage_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.stage_pending
            || self.commit_pending
            || self.checkout_pending
            || self.branch_create_pending
            || self.branch_delete_pending
            || self.push_pending
        {
            return;
        }
        let Some(proxy) = self.proxy.as_ref() else {
            self.status = "Git service unavailable".into();
            cx.notify();
            return;
        };
        self.stage_pending = true;
        self.status = "Staging changes…".into();
        let response = proxy.git_stage_all();
        cx.spawn_in(window, async move |this, cx| {
            let result = response
                .recv()
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            if let Err(error) = this.update_in(cx, |panel, _, cx| {
                panel.stage_pending = false;
                match result {
                    Ok(()) => {
                        panel.refresh(cx);
                        panel.status = "All changes staged".into();
                    }
                    Err(error) => {
                        panel.status = format!("Stage failed: {error}").into();
                    }
                }
                cx.notify();
            }) {
                eprintln!(
                    "Source Control panel closed before stage response: {error}"
                );
            }
        })
        .detach();
        cx.notify();
    }

    fn set_file_staged(
        &mut self,
        path: String,
        staged: bool,
        cx: &mut Context<Self>,
    ) {
        if self.stage_pending
            || self.commit_pending
            || self.checkout_pending
            || self.branch_create_pending
            || self.branch_delete_pending
            || self.push_pending
        {
            return;
        }
        let Some(proxy) = self.proxy.as_ref() else {
            self.status = "Git service unavailable".into();
            cx.notify();
            return;
        };
        self.stage_pending = true;
        self.status =
            format!("{} {path}…", if staged { "Staging" } else { "Unstaging" })
                .into();
        let response = proxy.git_stage_paths(vec![path.clone()], staged);
        cx.spawn(async move |this, cx| {
            let result = response
                .recv()
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            if let Err(error) = this.update(cx, |panel, cx| {
                panel.stage_pending = false;
                match result {
                    Ok(()) => {
                        panel.refresh(cx);
                        panel.status = format!(
                            "{} {path}",
                            if staged { "Staged" } else { "Unstaged" }
                        )
                        .into();
                    }
                    Err(error) => {
                        panel.status = format!(
                            "Could not {} {path}: {error}",
                            if staged { "stage" } else { "unstage" }
                        )
                        .into();
                    }
                }
                cx.notify();
            }) {
                eprintln!(
                    "Source Control panel closed before Git path response: {error}"
                );
            }
        })
        .detach();
        cx.notify();
    }

    fn can_commit_staged(&self, cx: &App) -> bool {
        !self.stage_pending
            && !self.commit_pending
            && !self.checkout_pending
            && !self.branch_create_pending
            && !self.branch_delete_pending
            && !self.push_pending
            && self.proxy.is_some()
            && !self.repository_has_conflicts
            && self.files.iter().any(|file| file.staged)
            && !self.commit_message.read(cx).value().trim().is_empty()
    }

    fn view_workspace_diff(&mut self, cx: &mut Context<Self>) {
        if self.files.is_empty() {
            self.status = "No changed files to review".into();
            cx.notify();
            return;
        }
        self.open_worktree_diff(WorktreeDiffTarget::All, cx);
    }

    fn commit_staged(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.stage_pending
            || self.commit_pending
            || self.checkout_pending
            || self.branch_create_pending
            || self.branch_delete_pending
            || self.push_pending
        {
            return;
        }
        if self.repository_has_conflicts {
            self.status =
                "Resolve and stage all repository conflicts before committing"
                    .into();
            cx.notify();
            return;
        }
        let message = self.commit_message.read(cx).value().trim().to_string();
        if message.is_empty() {
            self.status = "Enter a commit message".into();
            cx.notify();
            return;
        }
        let Some(proxy) = self.proxy.as_ref() else {
            self.status = "Git service unavailable".into();
            cx.notify();
            return;
        };
        self.commit_pending = true;
        self.status = "Committing staged changes…".into();
        let response = proxy.git_commit(message.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = response
                .recv()
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            if let Err(error) = this.update_in(cx, |panel, window, cx| {
                panel.commit_pending = false;
                match result {
                    Ok(()) => {
                        if panel.commit_message.read(cx).value().trim() == message {
                            panel.commit_message.update(cx, |input, cx| {
                                input.set_value("", window, cx)
                            });
                        }
                        panel.commits.clear();
                        panel.refresh(cx);
                        panel.status = "Commit created".into();
                    }
                    Err(error) => {
                        panel.status = format!("Commit failed: {error}").into();
                    }
                }
                cx.notify();
            }) {
                eprintln!(
                    "Source Control panel closed before commit response: {error}"
                );
            }
        })
        .detach();
        cx.notify();
    }
}

fn relative_git_time(timestamp: i64, now: i64) -> String {
    let seconds = now.saturating_sub(timestamp);
    let (count, unit) = if seconds < 60 {
        return "just now".to_string();
    } else if seconds < 3_600 {
        (seconds / 60, "minute")
    } else if seconds < 86_400 {
        (seconds / 3_600, "hour")
    } else if seconds < 2_592_000 {
        (seconds / 86_400, "day")
    } else if seconds < 31_536_000 {
        (seconds / 2_592_000, "month")
    } else {
        (seconds / 31_536_000, "year")
    };
    format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" })
}

fn git_repository_not_found(error: &str) -> bool {
    error.contains("could not find repository")
}

fn git_push_failure_status(error: &str) -> String {
    if is_non_fast_forward_error(error) {
        format!(
            "Push rejected: fetch and integrate remote changes before pushing again. {error}"
        )
    } else {
        format!("Push failed: {error}")
    }
}

fn is_non_fast_forward_error(error: &str) -> bool {
    (error.contains("[rejected]")
        && (error.contains("(non-fast-forward)") || error.contains("(fetch first)")))
        || (error.contains("Updates were rejected")
            && (error.contains("behind") || error.contains("remote")))
}

impl BasePanel for GitPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_git"
    }
}
impl Panel for GitPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        "Source Control"
    }
    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}
impl EventEmitter<PanelEvent> for GitPanel {}
impl Focusable for GitPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for GitPanel {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let text = cx.theme().sidebar_foreground;
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let root = self.root.clone();
        v_flex()
            .size_full()
            .gap_1()
            .p_2()
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        Button::new("git_changes")
                            .label(format!("Changes ({})", self.files.len()))
                            .when(!self.show_history, |button| button.primary())
                            .when(self.show_history, |button| button.ghost())
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.show_changes(cx);
                            })),
                    )
                    .child(
                        Button::new("git_history")
                            .label("History")
                            .when(self.show_history, |button| button.primary())
                            .when(!self.show_history, |button| button.ghost())
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.show_history(cx);
                            })),
                    )
                    .child(
                        Button::new("git_refresh")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .tooltip(if self.show_history {
                                if self.worktree_diff_target.is_some() {
                                    "Refresh working-tree diff"
                                } else {
                                    "Refresh commit history"
                                }
                            } else {
                                "Refresh source control"
                            })
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                if let Some(target) = this.worktree_diff_target.clone() {
                                    this.open_worktree_diff(target, cx);
                                } else if this.show_history {
                                    this.commits.clear();
                                    this.load_history(cx);
                                } else {
                                    this.refresh(cx);
                                }
                            })),
                    ),
            )
            .when(self.branch_create_open, |panel| {
                panel.child(
                    h_flex()
                        .items_center()
                        .gap_1()
                        .child(
                            Input::new(&self.branch_create_input)
                                .aria_label("New local branch name")
                                .flex_1(),
                        )
                        .child(
                            Button::new("git_create_branch")
                                .icon(IconName::Plus)
                                .tooltip("Create and switch to branch (Enter)")
                                .disabled(
                                    self.branch_create_pending
                                        || self.branch_delete_pending
                                        || self.push_pending
                                        || self
                                            .branch_create_input
                                            .read(cx)
                                            .value()
                                            .trim()
                                            .is_empty(),
                                )
                                .on_click(cx.listener(
                                    |this: &mut Self, _, window, cx| {
                                        this.create_branch(window, cx);
                                    },
                                )),
                        )
                        .child(
                            Button::new("git_cancel_create_branch")
                                .icon(IconName::X)
                                .tooltip("Cancel branch creation")
                                .disabled(self.branch_create_pending)
                                .on_click(cx.listener(
                                    |this: &mut Self, _, window, cx| {
                                        this.cancel_branch_creation(window, cx);
                                    },
                                )),
                        ),
                )
            })
            .when_some(
                self.git_credential_prompt.as_ref(),
                |panel, (_, prompt, secret)| {
                    let input = if *secret {
                        &self.git_secret_input
                    } else {
                        &self.git_username_input
                    };
                    panel.child(
                        v_flex()
                            .debug_selector(|| "git-credential-prompt".into())
                            .w_full()
                            .min_w_0()
                            .gap_2()
                            .border_b_1()
                            .border_color(border)
                            .px_2()
                            .py_2()
                            .child(
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .text_color(text)
                                            .child("Git authentication"),
                                    )
                                    .child(
                                        Button::new("git_credential_cancel")
                                            .icon(IconName::X)
                                            .ghost()
                                            .tooltip("Cancel Git authentication")
                                            .on_click(cx.listener(
                                                |this, _, window, cx| {
                                                    this.cancel_git_credential(
                                                        window, cx,
                                                    );
                                                },
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .text_size(px(11.))
                                    .text_color(muted)
                                    .child(prompt.clone()),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .min_w_0()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        Input::new(input)
                                            .aria_label(if *secret {
                                                "Git password or access token"
                                            } else {
                                                "Git username"
                                            })
                                            .flex_1(),
                                    )
                                    .child(
                                        Button::new("git_credential_submit")
                                            .primary()
                                            .label("Continue")
                                            .tooltip("Send credential to Git for this push only")
                                            .on_click(cx.listener(
                                                |this, _, window, cx| {
                                                    this.submit_git_credential(
                                                        window, cx,
                                                    );
                                                },
                                            )),
                                    ),
                            ),
                    )
                },
            )
            .when(!self.show_history, |this| this.child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Button::new("git_diff")
                            .icon(IconName::GitCompare)
                            .label("View Diff")
                            .ghost()
                            .disabled(self.repository_missing)
                            .tooltip(
                                "Review all staged, unstaged and untracked workspace changes",
                            )
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.view_workspace_diff(cx)
                            })),
                    )
                    .child(
                        Button::new("git_stage_all")
                            .icon(IconName::Plus)
                            .label(if self.stage_pending {
                                "Staging…"
                            } else {
                                "Stage All"
                            })
                            .ghost()
                            .disabled(
                                self.repository_missing
                                    || self.stage_pending
                                    || self.commit_pending
                                    || self.checkout_pending
                                    || self.branch_create_pending
                                    || self.branch_delete_pending
                                    || self.push_pending,
                            )
                            .tooltip(
                                "Stage workspace changes; unresolved conflicts are left untouched",
                            )
                            .on_click(cx.listener(
                                |this: &mut Self, _, window, cx| {
                                    this.stage_all(window, cx)
                                },
                            )),
                    ),
            ))
            .when(!self.show_history, |this| this.child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child({
                        let branches = self.branches.clone();
                        let selected_branch = self.branch.clone();
                        let panel = cx.entity().downgrade();
                        let unavailable = self.repository_missing
                            || self.checkout_pending
                            || self.branch_create_pending
                            || self.branch_delete_pending
                            || self.push_pending
                            || self.stage_pending
                            || self.commit_pending
                            || self.repository_dirty
                            || !self.files.is_empty()
                            || self.branch_create_open;
                        let tooltip = if self.repository_has_conflicts {
                            "Resolve repository conflicts before switching branches"
                        } else if self.repository_dirty || !self.files.is_empty() {
                            "Commit or stash repository changes before switching branches"
                        } else {
                            "Switch local branch"
                        };
                        Button::new("git_branch")
                            .icon(IconName::GitBranch)
                            .label(if self.branch.is_empty() {
                                "Select branch".to_string()
                            } else {
                                self.branch.clone()
                            })
                            .ghost()
                            .disabled(unavailable)
                            .tooltip(tooltip)
                            .dropdown_menu(move |menu, window, cx| {
                                let create_panel = panel.clone();
                                let delete_panel = panel.clone();
                                let menu = branches.iter().fold(menu, |menu, branch| {
                                    let panel = panel.clone();
                                    let reference = branch.clone();
                                    let label = if branch == &selected_branch {
                                        format!("{branch} · current")
                                    } else {
                                        branch.clone()
                                    };
                                    menu.item(PopupMenuItem::new(label).on_click(
                                        move |_, _, cx| {
                                            if let Err(error) = panel.update(cx, |panel, cx| {
                                                panel.checkout_branch(
                                                    reference.clone(),
                                                    cx,
                                                );
                                            }) {
                                                eprintln!(
                                                    "Source Control panel closed before branch selection: {error}"
                                                );
                                            }
                                        },
                                    ))
                                })
                                .separator()
                                .item(PopupMenuItem::new("Create branch…").on_click(
                                    move |_, _window, cx| {
                                        if let Err(error) = create_panel.update_in(
                                            cx,
                                            |panel, window, cx| {
                                                panel.begin_branch_creation(window, cx);
                                            },
                                        ) {
                                            eprintln!(
                                                "Source Control panel closed before branch creation opened: {error}"
                                            );
                                        }
                                    },
                                ));
                                let deletable_branches = branches
                                    .iter()
                                    .filter(|branch| *branch != &selected_branch)
                                    .cloned()
                                    .collect::<Vec<_>>();
                                if deletable_branches.is_empty() {
                                    menu
                                } else {
                                    menu.separator().submenu(
                                        "Delete merged branch…",
                                        window,
                                        cx,
                                        move |menu, _, _| {
                                            deletable_branches.iter().fold(
                                                menu,
                                                |menu, reference| {
                                                    let panel = delete_panel.clone();
                                                    let reference = reference.clone();
                                                    menu.item(
                                                        PopupMenuItem::new(
                                                            reference.clone(),
                                                        )
                                                        .on_click(move |_, _, cx| {
                                                            if let Err(error) = panel
                                                                .update(cx, |panel, cx| {
                                                                    panel.delete_branch(
                                                                        reference.clone(),
                                                                        cx,
                                                                    );
                                                                })
                                                            {
                                                                eprintln!(
                                                                    "Source Control panel closed before branch deletion: {error}"
                                                                );
                                                            }
                                                        }),
                                                    )
                                                },
                                            )
                                        },
                                    )
                                }
                            })
                    })
                    .child(
                        Button::new("git_push")
                            .icon(IconName::ArrowUpFromLine)
                            .label(if self.push_pending {
                                if self.branch_has_upstream {
                                    "Pushing…"
                                } else {
                                    "Publishing…"
                                }
                            } else if self.branch_has_upstream {
                                "Push"
                            } else {
                                "Publish"
                            })
                            .when(self.branch_has_upstream, |button| button.ghost())
                            .when(!self.branch_has_upstream, |button| button.primary())
                            .disabled(!self.can_push())
                            .tooltip(if self.remotes.is_empty() {
                                "Add a Git remote before publishing this branch"
                            } else if self.branch.is_empty() || self.branch == "HEAD" {
                                "Create or select a local branch before publishing"
                            } else if self.branch_has_upstream {
                                "Push the current branch to its configured upstream"
                            } else {
                                "Publish this branch and set its upstream remote"
                            })
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.push_branch(None, cx);
                            })),
                    )
                    .child({
                        let remotes = self.remotes.clone();
                        let panel = cx.entity().downgrade();
                        Button::new("git_push_remote")
                            .icon(IconName::ChevronDown)
                            .ghost()
                            .disabled(!self.can_push())
                            .tooltip("Choose a Git remote to push to")
                            .dropdown_menu(move |menu, _, _| {
                                remotes.iter().fold(menu, |menu, remote| {
                                    let panel = panel.clone();
                                    let remote = remote.clone();
                                    menu.item(
                                        PopupMenuItem::new(remote.clone()).on_click(
                                            move |_, _, cx| {
                                                if let Err(error) = panel.update(
                                                    cx,
                                                    |panel, cx| {
                                                        panel.push_branch(
                                                            Some(remote.clone()),
                                                            cx,
                                                        );
                                                    },
                                                ) {
                                                    eprintln!(
                                                        "Source Control panel closed before remote selection: {error}"
                                                    );
                                                }
                                            },
                                        ),
                                    )
                                })
                            })
                    }),
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .when(self.show_history && self.selected_commit.is_none(), |list| {
                        list.when(self.history_loading, |list| {
                            list.child(
                                div()
                                    .debug_selector(|| {
                                        "git-history-loading".to_string()
                                    })
                                    .px_2()
                                    .py_1()
                                    .text_color(muted)
                                    .child("Loading commit history…"),
                            )
                        })
                        .when_some(self.history_error.as_ref(), |list, error| {
                            list.child(
                                div()
                                    .debug_selector(|| {
                                        "git-history-error".to_string()
                                    })
                                    .p_3()
                                    .text_color(muted)
                                    .child(format!(
                                        "Unable to load commit history: {error}"
                                    )),
                            )
                        })
                        .when(
                            self.commits.is_empty()
                                && !self.history_loading
                                && self.history_error.is_none(),
                            |list| {
                                list.child(
                                    div()
                                        .debug_selector(|| {
                                            "git-history-empty".to_string()
                                        })
                                        .p_3()
                                        .text_color(muted)
                                        .child(if self.repository_missing {
                                            "No repository found"
                                        } else {
                                            "No commits in this repository yet"
                                        }),
                                )
                            },
                        )
                        .children(
                            self.commits
                                .iter()
                                .enumerate()
                                .map(|(index, commit)| {
                                    let short_id =
                                        commit.id.chars().take(7).collect::<String>();
                                    let timestamp = relative_git_time(
                                        commit.timestamp,
                                        chrono::Utc::now().timestamp(),
                                    );
                                    let commit_for_open = commit.clone();
                                    Button::new(format!("git-history-commit-{index}"))
                                        .ghost()
                                        .w_full()
                                        .min_w_0()
                                        .justify_start()
                                        .debug_selector(move || {
                                            format!("git-history-commit-{index}")
                                        })
                                        .on_click(cx.listener(
                                            move |this: &mut Self, _, _, cx| {
                                                this.open_history_commit(
                                                    commit_for_open.clone(),
                                                    cx,
                                                );
                                            },
                                        ))
                                        .child(
                                            v_flex()
                                                .w_full()
                                                .min_w_0()
                                                .gap_1()
                                                .py_1()
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .truncate()
                                                        .text_color(text)
                                                        .child(commit.subject.clone()),
                                                )
                                                .child(
                                                    h_flex()
                                                        .items_center()
                                                        .gap_1()
                                                        .text_size(px(11.))
                                                        .text_color(muted)
                                                        .child(commit.author.clone())
                                                        .child("·")
                                                        .child(timestamp)
                                                        .child("·")
                                                        .child(short_id),
                                                ),
                                        )
                                }),
                        )
                    })
                    .when_some(
                        self.selected_commit
                            .as_ref()
                            .filter(|_| self.show_history),
                        |panel, commit| {
                            let commit = commit.clone();
                            let worktree_diff_target = self.worktree_diff_target.clone();
                            let is_worktree_diff = worktree_diff_target.is_some();
                            let is_all_worktree_diff = matches!(
                                worktree_diff_target,
                                Some(WorktreeDiffTarget::All)
                            );
                            let commit_author = commit.author.clone();
                            let commit_id = commit.id.clone();
                            let patch_lines = self.commit_patch_lines.clone();
                            let patch_line_count = patch_lines.len();
                            let patch_truncated = self
                                .commit_patch
                                .as_ref()
                                .is_some_and(|patch| patch.truncated);
                            let mono_font = cx.theme().mono_font_family.clone();
                            panel.child(
                                v_flex()
                                    .size_full()
                                    .min_h_0()
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_2()
                                            .border_b_1()
                                            .border_color(border)
                                            .pb_2()
                                            .child(
                                                Button::new("git_commit_diff_back")
                                                    .debug_selector(|| {
                                                        "git_commit_diff_back".into()
                                                    })
                                                    .icon(IconName::ArrowLeft)
                                                    .ghost()
                                                    .tooltip(if is_worktree_diff {
                                                        "Back to changes"
                                                    } else {
                                                        "Back to commit history"
                                                    })
                                                    .on_click(cx.listener(
                                                        |this: &mut Self, _, _, cx| {
                                                            this.close_history_commit(cx);
                                                        },
                                                    )),
                                            )
                                            .child(
                                                v_flex()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .truncate()
                                                            .text_color(text)
                                                            .child(commit.subject),
                                                    )
                                                    .child(
                                                        h_flex()
                                                            .items_center()
                                                            .gap_1()
                                                            .text_size(px(11.))
                                                            .text_color(muted)
                                                            .child(commit_author)
                                                            .child("·")
                                                            .child(
                                                                if is_worktree_diff {
                                                                    if is_all_worktree_diff {
                                                                        "All files".to_string()
                                                                    } else {
                                                                        "File diff".to_string()
                                                                    }
                                                                } else {
                                                                    commit_id
                                                                        .chars()
                                                                        .take(7)
                                                                        .collect::<String>()
                                                                },
                                                            ),
                                                    ),
                                            )
                                                    .when_some(
                                                self.commit_patch.as_ref(),
                                                |header, patch| {
                                                    let patch_text = patch.patch.clone();
                                                    let tooltip = if patch.truncated {
                                                        "Copy displayed patch (truncated)"
                                                                } else if is_worktree_diff {
                                                        "Copy working-tree diff"
                                                    } else {
                                                        "Copy commit diff"
                                                    };
                                                    header.child(
                                                        Button::new("git_commit_diff_copy")
                                                            .debug_selector(|| {
                                                                "git_commit_diff_copy".into()
                                                            })
                                                            .icon(IconName::Copy)
                                                            .ghost()
                                                            .tooltip(tooltip)
                                                            .on_click(cx.listener(
                                                                move |this: &mut Self, _, _, cx| {
                                                                    cx.write_to_clipboard(
                                                                        ClipboardItem::new_string(
                                                                            patch_text.clone(),
                                                                        ),
                                                                    );
                                                                    this.commit_patch_copied = true;
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    )
                                                },
                                            ),
                                    )
                                    .when(self.commit_patch_copied, |panel| {
                                        panel.child(
                                            div()
                                                .px_2()
                                                .py_1()
                                                .text_size(px(11.))
                                                .text_color(muted)
                                                .child(if is_worktree_diff {
                                                    "Copied working-tree diff to clipboard."
                                                } else {
                                                    "Copied commit diff to clipboard."
                                                }),
                                        )
                                    })
                                    .when(self.commit_patch_loading, |panel| {
                                        panel.child(
                                            div()
                                                .p_2()
                                                .text_color(muted)
                                                .child(if is_worktree_diff {
                                                    "Loading working-tree diff…"
                                                } else {
                                                    "Loading commit diff…"
                                                }),
                                        )
                                    })
                                    .when_some(
                                        self.commit_patch_error.as_ref(),
                                        |panel, error| {
                                            panel.child(
                                                div()
                                                    .p_2()
                                                    .text_color(cx.theme().danger)
                                                    .child(format!(
                                                        "Unable to load {} diff: {error}",
                                                        if is_worktree_diff {
                                                            "working-tree"
                                                        } else {
                                                            "commit"
                                                        }
                                                    )),
                                            )
                                        },
                                    )
                                    .when(patch_truncated, |panel| {
                                        panel.child(
                                            div()
                                                .px_2()
                                                .py_1()
                                                .text_size(px(11.))
                                                .text_color(muted)
                                                .child(
                                                    "Diff is truncated after 2 MiB.",
                                                ),
                                        )
                                    })
                                    .when(
                                        !self.commit_patch_loading
                                            && self.commit_patch_error.is_none()
                                            && self.commit_patch.is_some()
                                            && patch_line_count == 0,
                                        |panel| {
                                            panel.child(
                                                div()
                                                    .p_2()
                                                    .text_color(muted)
                                                    .child(if is_worktree_diff {
                                                        if is_all_worktree_diff {
                                                            "No uncommitted workspace changes."
                                                        } else {
                                                            "This file has no working-tree changes."
                                                        }
                                                    } else {
                                                        "This commit has no file changes."
                                                    }),
                                            )
                                        },
                                    )
                                    .when(patch_line_count > 0, |panel| {
                                        panel.child(
                                            div()
                                                .size_full()
                                                .overflow_x_scrollbar()
                                                .child(uniform_list(
                                                "git-commit-patch-lines",
                                                patch_line_count,
                                                cx.processor(
                                                    move |_this,
                                                          range: Range<usize>,
                                                          _window,
                                                          _cx| {
                                                        range
                                                            .filter_map(|line_index| {
                                                                let line = patch_lines
                                                                    .get(line_index)?;
                                                                let added = line
                                                                    .starts_with('+')
                                                                    && !line
                                                                        .starts_with("+++");
                                                                let removed = line
                                                                    .starts_with('-')
                                                                    && !line
                                                                        .starts_with("---");
                                                                let hunk = line
                                                                    .starts_with("@@");
                                                                let line_color = if added {
                                                                    _cx.theme().success
                                                                } else if removed {
                                                                    _cx.theme().danger
                                                                } else if hunk {
                                                                    _cx.theme().accent
                                                                } else {
                                                                    _cx.theme().muted_foreground
                                                                };
                                                                let background = if added {
                                                                    Some(_cx.theme().success.opacity(0.12))
                                                                } else if removed {
                                                                    Some(_cx.theme().danger.opacity(0.12))
                                                                } else {
                                                                    None
                                                                };
                                                                let row = div()
                                                                    .id(("git_commit_patch_line", line_index))
                                                                    .w_full()
                                                                    .min_w_0()
                                                                    .h(px(20.))
                                                                    .px_2()
                                                                    .font_family(mono_font.clone())
                                                                    .whitespace_nowrap()
                                                                    .text_color(line_color);
                                                                let row = if let Some(background) = background {
                                                                    row.bg(background)
                                                                } else {
                                                                    row
                                                                };
                                                                Some(
                                                                    row.child(line.clone())
                                                                        .into_any_element(),
                                                                )
                                                            })
                                                            .collect::<Vec<_>>()
                                                    },
                                                ),
                                            )
                                                .track_scroll(&self.commit_patch_scroll)
                                                .on_scroll_wheel(|_, _, cx| {
                                                    cx.stop_propagation()
                                                })
                                                .size_full(),
                                            ),
                                        )
                                    }),
                            )
                        },
                    )
                    .when(!self.show_history, |list| {
                        list.children(self.files.iter().enumerate().map(|(index, file)| {
                    let path = file.path.clone();
                    let open_path = Path::new(&root).join(&path);
                    let path_for_deleted_diff = path.clone();
                    let status_for_deleted_diff = file.state.clone();
                    let staged = file.staged;
                    let conflicted = file.is_conflicted();
                    let state_label = if conflicted {
                        "Conflict".to_string()
                    } else {
                        file.state.clone()
                    };
                    let state_color = if conflicted {
                        cx.theme().danger
                    } else {
                        muted
                    };
                    let icon = if file.state.contains('?') {
                        IconName::FilePlus
                    } else {
                        IconName::FileCode
                    };
                    h_flex()
                        .id(("git_file", index))
                        .debug_selector(move || format!("git-file-{index}"))
                        .w_full()
                        .min_w_0()
                        .flex_nowrap()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .child(icon)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .debug_selector(move || format!("git-path-{index}"))
                                .child(
                                    Button::new(format!("git-open-file-{index}"))
                                        .ghost()
                                        .w_full()
                                        .min_w_0()
                                        .justify_start()
                                        .whitespace_nowrap()
                                        .text_ellipsis_start()
                                        .label(file.path.clone())
                                        .disabled(
                                            self.open_file_handler.is_none()
                                                && self.proxy.is_none(),
                                        )
                                        .tooltip(file.path.clone())
                                        .on_click(cx.listener(
                                            move |this: &mut Self, _, window, cx| {
                                                let exists = std::fs::symlink_metadata(
                                                    &open_path,
                                                )
                                                .is_ok_and(|metadata| {
                                                    metadata.file_type().is_file()
                                                });
                                                if !exists
                                                    && status_for_deleted_diff
                                                        .contains('D')
                                                {
                                                    this.open_worktree_file_diff(
                                                        path_for_deleted_diff.clone(),
                                                        cx,
                                                    );
                                                    return;
                                                }
                                                if let Some(handler) =
                                                    this.open_file_handler.as_ref()
                                                {
                                                    handler(
                                                        open_path.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                }
                                            },
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .debug_selector(move || format!("git-state-{index}"))
                                .text_color(state_color)
                                .child(state_label),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .debug_selector(move || {
                                    format!("git-select-{index}")
                                })
                                .child(
                                    Button::new(("git_select", index))
                                        .icon(if staged {
                                            IconName::SquareCheck
                                        } else {
                                            IconName::Square
                                        })
                                        .ghost()
                                        .disabled(
                                            self.repository_missing
                                                || self.stage_pending
                                                || self.commit_pending,
                                        )
                                        .tooltip(if conflicted {
                                            "Stage the resolved file to mark this conflict resolved"
                                        } else if staged {
                                            "Unstage file"
                                        } else {
                                            "Stage file"
                                        })
                                        .on_click(cx.listener(
                                            move |this: &mut Self, _, _, cx| {
                                                this.set_file_staged(
                                                    path.clone(),
                                                    !staged,
                                                    cx,
                                                );
                                            },
                                        )),
                                ),
                        )
                    }))
                    .when(self.repository_missing, |list| {
                    list.child(
                        v_flex()
                            .debug_selector(|| "git-init-prompt".to_string())
                            .items_center()
                            .gap_2()
                            .p_3()
                            .child(
                                div()
                                    .text_color(muted)
                                    .child("Initialize Git to track changes in this workspace."),
                            )
                            .child(
                                Button::new("git_initialize_repository")
                                    .icon(IconName::GitBranch)
                                    .label(if self.git_init_pending {
                                        "Initializing…"
                                    } else {
                                        "Initialize Repository"
                                    })
                                    .primary()
                                    .disabled(self.git_init_pending)
                                    .tooltip("Initialize a Git repository in this workspace")
                                    .on_click(cx.listener(
                                        |this: &mut Self, _, _, cx| {
                                            this.initialize_repository(cx);
                                        },
                                    )),
                            ),
                    )
                    })
                    })
            )
            .when(!self.show_history, |this| {
                this.child(Input::new(&self.commit_message).aria_label("Commit message"))
            })
            .when(!self.show_history, |this| this.child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .w_full()
                            .debug_selector(|| "git-commit-status".to_string())
                            .text_color(if self.repository_has_conflicts {
                                cx.theme().danger
                            } else {
                                muted
                            })
                            .text_size(px(11.))
                            .child(self.status.clone()),
                    )
                    .child(
                        h_flex().justify_end().child(
                            div()
                                .debug_selector(|| "git-commit-action".to_string())
                                .child(
                                    Button::new("git_commit")
                                        .icon(IconName::Check)
                                        .label(if self.commit_pending {
                                            "Committing…"
                                        } else {
                                            "Commit Staged"
                                        })
                                    .primary()
                                        .disabled(!self.can_commit_staged(cx))
                                        .tooltip("Commit staged changes")
                                        .on_click(cx.listener(
                                            |this: &mut Self, _, window, cx| {
                                                this.commit_staged(window, cx)
                                            },
                                        )),
                                ),
                        ),
                    ),
            ))
            .child(div().text_size(px(10.)).text_color(muted).child(root))
    }
}

const SEARCH_RESULT_PAGE_SIZE: usize = 100;
// Mirrors Zed's MAX_SEARCH_RESULT_RANGES safety ceiling.
const SEARCH_RESULT_LIMIT: usize = ahead_core::search::MAX_SEARCH_RESULT_RANGES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SearchMode {
    Text,
    Word,
    Regex,
}

pub struct SearchPanel {
    pub focus: FocusHandle,
    pub root: String,
    pub query: Entity<InputState>,
    pub replacement: Entity<InputState>,
    pub include_glob: Entity<InputState>,
    pub exclude_glob: Entity<InputState>,
    case_sensitive: bool,
    search_mode: SearchMode,
    show_replace: bool,
    replacing: bool,
    replacement_status: Option<String>,
    pub results: Vec<SearchResult>,
    selected_result_index: usize,
    pending_next_match: Option<(String, usize, usize)>,
    visible_result_count: usize,
    search_results_scroll: UniformListScrollHandle,
    pub mailbox_id: usize,
    proxy: Option<Arc<ProxyClient>>,
    cached_paths: Option<(u64, Arc<Vec<PathBuf>>)>,
    active_workspace_file_request: Option<u64>,
    _workspace_changes: Option<Task<()>>,
    buffers: Vec<Entity<CodePanel>>,
    buffer_observers: Vec<Subscription>,
    last_buffer_states: Vec<(String, usize, usize)>,
    search_generation: Arc<AtomicU64>,
    replacement_generation: Arc<AtomicU64>,
    debounced_search: Option<Task<()>>,
    replacement_task: Option<Task<()>>,
    searching: bool,
    search_limit_reached: bool,
    search_error: Option<String>,
    close_handler: Option<Rc<dyn Fn(PanelId, &mut Window, &mut App)>>,
    replace_handler: Option<
        Rc<
            dyn Fn(
                SearchReplaceRequest,
                &mut Window,
                &mut App,
            ) -> Result<(), String>,
        >,
    >,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
}

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub path: String,
    pub line: usize,
    pub end_line: usize,
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub preview_match: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SearchDisplayRow {
    FileHeader { result_index: usize },
    Match { result_index: usize },
}

#[derive(Clone, Debug)]
pub(crate) struct SearchReplaceFile {
    pub path: PathBuf,
    pub original_text: String,
    pub replacement_text: String,
    pub match_count: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct SearchReplaceRequest {
    pub files: Vec<SearchReplaceFile>,
    pub match_count: usize,
}

impl SearchPanel {
    pub fn new(
        root: &str,
        mailbox_id: usize,
        code: Entity<CodePanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx));
        let replacement = cx.new(|cx| InputState::new(window, cx));
        let include_glob = cx.new(|cx| InputState::new(window, cx));
        let exclude_glob = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe(&query, |this: &mut Self, _state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.pending_next_match = None;
                this.refresh(cx);
            }
        })
        .detach();
        for input in [&include_glob, &exclude_glob] {
            cx.subscribe(
                input,
                |this: &mut Self, _state, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.pending_next_match = None;
                        this.refresh(cx);
                    }
                },
            )
            .detach();
        }
        cx.subscribe(
            &replacement,
            |this: &mut Self, _state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.pending_next_match = None;
                    this.replacement_generation.fetch_add(1, Ordering::SeqCst);
                    this.replacement_task = None;
                    this.replacing = false;
                    this.replacement_status = None;
                    cx.notify();
                }
            },
        )
        .detach();
        let proxy = code.read(cx).proxy.clone();
        let workspace_changes = proxy.as_ref().map(|proxy| {
            let receiver = proxy.subscribe_workspace_file_changes();
            let proxy = proxy.clone();
            cx.spawn(async move |this, cx| {
                while receiver.recv().await.is_ok() {
                    if this
                        .update(cx, |panel, cx| {
                            if panel.cached_paths.as_ref().is_some_and(
                                |(generation, _)| {
                                    *generation != proxy.workspace_file_generation()
                                },
                            ) {
                                panel.cached_paths = None;
                            }
                            panel.refresh(cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
        });
        let mut panel = Self {
            focus: cx.focus_handle(),
            root: root.to_string(),
            query,
            replacement,
            include_glob,
            exclude_glob,
            case_sensitive: false,
            search_mode: SearchMode::Text,
            show_replace: false,
            replacing: false,
            replacement_status: None,
            results: Vec::new(),
            selected_result_index: 0,
            pending_next_match: None,
            visible_result_count: SEARCH_RESULT_PAGE_SIZE,
            search_results_scroll: UniformListScrollHandle::new(),
            mailbox_id,
            proxy,
            cached_paths: None,
            active_workspace_file_request: None,
            _workspace_changes: workspace_changes,
            buffers: Vec::new(),
            buffer_observers: Vec::new(),
            last_buffer_states: Vec::new(),
            search_generation: Arc::new(AtomicU64::new(0)),
            replacement_generation: Arc::new(AtomicU64::new(0)),
            debounced_search: None,
            replacement_task: None,
            searching: false,
            search_limit_reached: false,
            search_error: None,
            close_handler: None,
            replace_handler: None,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
        };
        panel.set_buffers(vec![code], cx);
        panel
    }

    pub fn set_buffers(
        &mut self,
        buffers: Vec<Entity<CodePanel>>,
        cx: &mut Context<Self>,
    ) {
        self.buffer_observers = buffers
            .iter()
            .map(|buffer| {
                cx.observe(buffer, |this, _, cx| {
                    let states = this
                        .buffers
                        .iter()
                        .map(|buffer| {
                            let code = buffer.read(cx);
                            (code.file_path.clone(), code.dirty_rev, code.saved_rev)
                        })
                        .collect::<Vec<_>>();
                    if this.last_buffer_states != states {
                        this.refresh(cx);
                    }
                })
            })
            .collect();
        self.buffers = buffers;
        self.refresh(cx);
    }

    pub fn set_close_handler<F>(&mut self, handler: F)
    where
        F: Fn(PanelId, &mut Window, &mut App) + 'static,
    {
        self.close_handler = Some(Rc::new(handler));
    }

    pub(crate) fn set_replace_handler<F>(&mut self, handler: F)
    where
        F: Fn(SearchReplaceRequest, &mut Window, &mut App) -> Result<(), String>
            + 'static,
    {
        self.replace_handler = Some(Rc::new(handler));
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().to_string();
        let include_glob = self.include_glob.read(cx).value().to_string();
        let exclude_glob = self.exclude_glob.read(cx).value().to_string();
        let case_sensitive = self.case_sensitive;
        let whole_word = self.search_mode == SearchMode::Word;
        let is_regex = self.search_mode == SearchMode::Regex;
        let has_query = !query.trim().is_empty();
        self.last_buffer_states = self
            .buffers
            .iter()
            .map(|buffer| {
                let code = buffer.read(cx);
                (code.file_path.clone(), code.dirty_rev, code.saved_rev)
            })
            .collect();
        let generation = self.search_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.search_results_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        self.replacement_generation.fetch_add(1, Ordering::SeqCst);
        self.replacement_task = None;
        if let Some(request_id) = self.active_workspace_file_request.take()
            && let Some(proxy) = &self.proxy
        {
            proxy.cancel_workspace_files(request_id);
        }
        if !has_query {
            self.debounced_search = None;
            self.results.clear();
            self.selected_result_index = 0;
            self.visible_result_count = SEARCH_RESULT_PAGE_SIZE;
            self.searching = false;
            self.search_limit_reached = false;
            self.search_error = None;
            self.replacing = false;
            self.replacement_status = None;
            cx.notify();
            return;
        }

        self.results.clear();
        self.selected_result_index = 0;
        self.visible_result_count = SEARCH_RESULT_PAGE_SIZE;
        self.searching = true;
        self.search_limit_reached = false;
        self.search_error = None;
        self.replacing = false;
        self.replacement_status = None;
        cx.notify();
        let root = PathBuf::from(&self.root);
        let proxy = self.proxy.clone();
        let cached_paths = self.cached_paths.clone();
        let search_generation = self.search_generation.clone();
        self.debounced_search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            if search_generation.load(Ordering::SeqCst) != generation {
                return;
            }
            let Ok(overrides) = this.update(cx, |panel, cx| {
                panel
                    .buffers
                    .iter()
                    .filter_map(|buffer| {
                        let code = buffer.read(cx);
                        code.dirty.then(|| {
                            (
                                PathBuf::from(&code.file_path),
                                code.editor.read(cx).value().to_string(),
                            )
                        })
                    })
                    .collect::<HashMap<_, _>>()
            }) else {
                return;
            };
            let indexed_files = if let Some(proxy) = proxy {
                if let Some((cached_generation, files)) = cached_paths.filter(
                    |(cached_generation, _)| {
                        *cached_generation == proxy.workspace_file_generation()
                    },
                ) {
                    Some((cached_generation, files))
                } else {
                    let request_id =
                        crate::proxy_client::next_workspace_file_request_id();
                    let request = proxy.workspace_files(request_id);
                    if this
                        .update(cx, |panel, _| {
                            panel.active_workspace_file_request = Some(request_id);
                        })
                        .is_err()
                    {
                        proxy.cancel_workspace_files(request_id);
                        return;
                    }
                    let response = cx
                        .background_spawn(async move { request.wait() })
                        .await;
                    let _ = this.update(cx, |panel, _| {
                        if panel.active_workspace_file_request == Some(request_id) {
                            panel.active_workspace_file_request = None;
                        }
                    });
                    if search_generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    match response {
                        Ok((snapshot_generation, files))
                            if snapshot_generation
                                >= proxy.workspace_file_generation() =>
                        {
                            let files = Arc::new(files);
                            if this
                                .update(cx, |panel, _| {
                                    panel.cached_paths = Some((
                                        snapshot_generation,
                                        files.clone(),
                                    ));
                                })
                                .is_err()
                            {
                                return;
                            }
                            Some((snapshot_generation, files))
                        }
                        Ok(_) => return,
                        Err(error) => {
                            if let Err(update_error) = this.update(cx, |panel, cx| {
                                panel.searching = false;
                                panel.search_error = Some(error.message);
                                cx.notify();
                            }) {
                                eprintln!("Search panel closed before proxy error was shown: {update_error}");
                            }
                            return;
                        }
                    }
                }
            } else {
                None
            };
            let active_generation = search_generation.clone();
            let (sender, receiver) = async_channel::bounded(16);
            let scan = cx.background_spawn(async move {
                let path_filter = ahead_core::search::WorkspacePathFilter::new(
                    &root,
                    Some(&include_glob),
                    Some(&exclude_glob),
                )?;
                let options = ahead_core::search::FileSearchOptions {
                    pattern: query,
                    case_sensitive,
                    whole_word,
                    is_regex,
                    max_results: SEARCH_RESULT_LIMIT,
                };
                let disk_paths: Box<dyn Iterator<Item = PathBuf> + '_> =
                    if let Some((_, files)) = indexed_files.as_ref() {
                        Box::new(files.iter().cloned())
                    } else {
                        Box::new(ahead_core::search::workspace_paths(&root))
                    };
                ahead_core::search::search_paths_with_overrides_stream(
                    ahead_core::search::SearchScope::Workspace(&root),
                    ahead_core::search::new_open_buffer_paths(&root, &overrides)
                        .into_iter()
                        .chain(disk_paths)
                        .filter(|path| path_filter.matches(path)),
                    &overrides,
                    &options,
                    || active_generation.load(Ordering::SeqCst) == generation,
                    |path, matches| sender.send_blocking((path, matches)).is_ok(),
                )
            });
            while let Ok((path, matches)) = receiver.recv().await {
                if search_generation.load(Ordering::SeqCst) != generation {
                    break;
                }
                let mut pending = vec![(path, matches)];
                for _ in 1..32 {
                    match receiver.try_recv() {
                        Ok(result) => pending.push(result),
                        Err(_) => break,
                    }
                }
                this.update(cx, |panel, cx| {
                    if panel.search_generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    let selected = panel.results.get(panel.selected_result_index).map(
                        |result| (result.path.clone(), result.line, result.start),
                    );
                    for (path, matches) in pending {
                        let path = path.to_string_lossy().into_owned();
                        panel.results.extend(matches.into_iter().map(|matched| {
                            SearchResult {
                                path: path.clone(),
                                line: matched.line + 1,
                                end_line: matched.end_line + 1,
                                start: matched.start,
                                end: matched.end,
                                text: matched.line_content,
                                preview_match: matched.preview_match,
                            }
                        }));
                    }
                    panel.results.sort_by(|left, right| {
                        left.path
                            .cmp(&right.path)
                            .then_with(|| left.line.cmp(&right.line))
                            .then_with(|| left.start.cmp(&right.start))
                            .then_with(|| left.end_line.cmp(&right.end_line))
                            .then_with(|| left.end.cmp(&right.end))
                    });
                    panel.selected_result_index = selected
                        .and_then(|(path, line, start)| {
                            panel.results.iter().position(|result| {
                                result.path == path
                                    && result.line == line
                                    && result.start == start
                            })
                        })
                        .unwrap_or(0);
                    cx.notify();
                })
                .ok();
            }
            let result = scan.await;
            this.update(cx, |panel, cx| {
                if panel.search_generation.load(Ordering::SeqCst) == generation {
                    panel.searching = false;
                    let succeeded = result.is_ok();
                    match result {
                        Ok(limit_reached) => {
                            panel.search_limit_reached = limit_reached;
                        }
                        Err(ahead_core::search::FileSearchError::Cancelled) => {}
                        Err(error) => panel.search_error = Some(error.to_string()),
                    }
                    if succeeded
                        && let Some(next_match) = panel.pending_next_match.take()
                    {
                        panel.selected_result_index =
                            next_search_result_index(&panel.results, &next_match);
                        panel.visible_result_count = panel
                            .visible_result_count
                            .max(panel.selected_result_index.saturating_add(1))
                            .min(panel.results.len());
                        if let Some(row_index) = search_display_row_for_result(
                            &panel.results,
                            panel.selected_result_index,
                        ) {
                            panel.search_results_scroll.scroll_to_item(
                                row_index,
                                ScrollStrategy::Nearest,
                            );
                        }
                    } else {
                        panel.pending_next_match = None;
                    }
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn handle_result_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        if key == "enter" {
            self.pending_next_match = None;
            if let Some(result) = self.results.get(self.selected_result_index) {
                self.open_result(result, window, cx);
            }
            return;
        }
        if let Some(index) = search_result_index_after_key(
            self.selected_result_index,
            self.visible_result_count.min(self.results.len()),
            key,
        ) {
            self.pending_next_match = None;
            self.selected_result_index = index;
            if let Some(row_index) =
                search_display_row_for_result(&self.results, index)
            {
                self.search_results_scroll
                    .scroll_to_item(row_index, ScrollStrategy::Nearest);
            }
            cx.notify();
        }
    }

    fn replace_current_match(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected) = self.results.get(self.selected_result_index).cloned()
        else {
            self.replacement_status =
                Some("Select a search match to replace".into());
            cx.notify();
            return;
        };
        self.replace_matches(Some(selected), window, cx);
    }

    fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_matches(None, window, cx);
    }

    fn replace_matches(
        &mut self,
        selected: Option<SearchResult>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.replacing {
            return;
        }
        let query = self.query.read(cx).value().to_string();
        if query.trim().is_empty() {
            return;
        }
        let Some(handler) = self.replace_handler.clone() else {
            self.replacement_status =
                Some("Search replacement is unavailable".into());
            cx.notify();
            return;
        };

        let generation =
            self.replacement_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let replacement_generation = self.replacement_generation.clone();
        self.search_generation.fetch_add(1, Ordering::SeqCst);
        if let Some(request_id) = self.active_workspace_file_request.take()
            && let Some(proxy) = &self.proxy
        {
            proxy.cancel_workspace_files(request_id);
        }
        let root = PathBuf::from(&self.root);
        let replacement = self.replacement.read(cx).value().to_string();
        let replacing_current_match = selected.is_some();
        let next_match = selected.as_ref().map(|selected| {
            search_location_after_replacement(selected, &replacement)
        });
        let include_glob = self.include_glob.read(cx).value().to_string();
        let exclude_glob = self.exclude_glob.read(cx).value().to_string();
        let options = ahead_core::search::FileSearchOptions {
            pattern: query,
            case_sensitive: self.case_sensitive,
            whole_word: self.search_mode == SearchMode::Word,
            is_regex: self.search_mode == SearchMode::Regex,
            max_results: usize::MAX,
        };
        let proxy = self.proxy.clone();
        let cached_paths =
            self.cached_paths.clone().filter(|(cache_generation, _)| {
                proxy.as_ref().is_none_or(|proxy| {
                    *cache_generation == proxy.workspace_file_generation()
                })
            });
        let overrides = self
            .buffers
            .iter()
            .filter_map(|buffer| {
                let code = buffer.read(cx);
                code.dirty.then(|| {
                    (
                        PathBuf::from(&code.file_path),
                        code.editor.read(cx).value().to_string(),
                    )
                })
            })
            .collect::<HashMap<_, _>>();
        self.replacing = true;
        self.searching = false;
        self.replacement_status = Some(if replacing_current_match {
            "Finding selected match to replace…".into()
        } else {
            "Finding matches to replace…".into()
        });
        self.replacement_task = Some(cx.spawn_in(window, async move |this, cx| {
            let replacements = cx
                .background_spawn(async move {
                    let path_filter = ahead_core::search::WorkspacePathFilter::new(
                        &root,
                        (!include_glob.trim().is_empty())
                            .then_some(include_glob.as_str()),
                        (!exclude_glob.trim().is_empty())
                            .then_some(exclude_glob.as_str()),
                    )
                    .map_err(|error| error.to_string())?;
                    if let Some(selected) = selected {
                        let path = PathBuf::from(&selected.path);
                        if !path_filter.matches(&path) {
                            return Ok(Vec::new());
                        }
                        let matched = ahead_core::search::FileSearchMatch {
                            line: selected.line.saturating_sub(1),
                            start: selected.start,
                            end_line: selected.end_line.saturating_sub(1),
                            end: selected.end,
                            line_content: selected.text,
                            preview_match: selected.preview_match,
                        };
                        ahead_core::search::replace_path_match_with_overrides(
                            ahead_core::search::SearchScope::Workspace(&root),
                            &path,
                            &overrides,
                            &options,
                            &replacement,
                            &matched,
                            || replacement_generation.load(Ordering::SeqCst) == generation,
                        )
                        .map(|replacement| replacement.into_iter().collect())
                        .map_err(|error| error.to_string())
                    } else {
                        let paths: Box<dyn Iterator<Item = PathBuf> + '_> =
                            if let Some((_, paths)) = cached_paths {
                                let mut index = 0;
                                Box::new(std::iter::from_fn(move || {
                                    let path = paths.get(index).cloned();
                                    index += 1;
                                    path
                                }))
                            } else {
                                Box::new(ahead_core::search::workspace_paths(&root))
                            };
                        let paths = ahead_core::search::new_open_buffer_paths(
                            &root,
                            &overrides,
                        )
                        .into_iter()
                        .chain(paths)
                        .filter(|path| path_filter.matches(path));
                        ahead_core::search::replace_paths_with_overrides(
                            ahead_core::search::SearchScope::Workspace(&root),
                            paths,
                            &overrides,
                            &options,
                            &replacement,
                            || replacement_generation.load(Ordering::SeqCst) == generation,
                        )
                        .map_err(|error| error.to_string())
                    }
                })
                .await;
            if let Err(error) = this.update_in(cx, |panel, window, cx| {
                if panel.replacement_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                panel.replacing = false;
                match replacements {
                    Ok(files) if files.is_empty() => {
                        panel.replacement_status = Some(if replacing_current_match {
                            "The selected match is no longer available; refresh search".into()
                        } else {
                            "No matches to replace".into()
                        });
                    }
                    Ok(files)
                        if replacing_current_match
                            && files.iter().all(|file| {
                                file.original_text == file.replacement_text
                            }) =>
                    {
                        panel.replacement_status =
                            Some("Replacement would not change this match".into());
                    }
                    Ok(files) => {
                        let match_count =
                            files.iter().map(|file| file.match_count).sum();
                        let file_count = files.len();
                        let request = SearchReplaceRequest {
                            files: files
                                .into_iter()
                                .map(|file| SearchReplaceFile {
                                    path: file.path,
                                    original_text: file.original_text,
                                    replacement_text: file.replacement_text,
                                    match_count: file.match_count,
                                })
                                .collect(),
                            match_count,
                        };
                        panel.replacement_status = Some(format!(
                            "Applying {match_count} replacement{} in {file_count} file{}…",
                            if match_count == 1 { "" } else { "s" },
                            if file_count == 1 { "" } else { "s" }
                        ));
                        let weak_panel = this.clone();
                        window.defer(cx, move |window, cx| {
                            let result = handler(request, window, cx);
                            let applied = result.is_ok();
                            let status = result.map_or_else(
                                |error| format!("Replace failed: {error}"),
                                |_| {
                                    format!(
                                        "Replaced {match_count} occurrence{} in {file_count} file{}; review and save the changed tabs",
                                        if match_count == 1 { "" } else { "s" },
                                        if file_count == 1 { "" } else { "s" }
                                    )
                                },
                            );
                            if let Err(error) = weak_panel.update(cx, |panel, cx| {
                                let advance_after_replace =
                                    applied && replacing_current_match;
                                panel.pending_next_match =
                                    if advance_after_replace {
                                        next_match
                                    } else {
                                        None
                                    };
                                if advance_after_replace {
                                    panel.refresh(cx);
                                }
                                panel.replacement_status = Some(status);
                                cx.notify();
                            }) {
                                eprintln!("Search panel closed before replacement status was shown: {error}");
                            }
                        });
                    }
                    Err(error) => {
                        panel.replacement_status = Some(format!("Replace failed: {error}"));
                    }
                }
                cx.notify();
            }) {
                eprintln!("Search panel closed before replacement results were shown: {error}");
            }
        }));
        cx.notify();
    }

    fn open_result(
        &self,
        result: &SearchResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::ross::request_open_at(
            self.mailbox_id,
            &result.path,
            crate::ross::OpenLocation {
                line: result.line.saturating_sub(1),
                column: crate::ross::OpenColumn::Utf8Byte(result.start),
                end_line: result.end_line.saturating_sub(1),
                end_column: crate::ross::OpenColumn::Utf8Byte(result.end),
            },
        );
        cx.focus_self(window);
    }
}

fn search_result_index_after_key(
    current_index: usize,
    result_count: usize,
    key: &str,
) -> Option<usize> {
    if result_count == 0 {
        return None;
    }
    let current_index = current_index.min(result_count - 1);
    match key {
        "down" => Some(if current_index + 1 == result_count {
            0
        } else {
            current_index + 1
        }),
        "up" => Some(if current_index == 0 {
            result_count - 1
        } else {
            current_index - 1
        }),
        _ => None,
    }
}

fn search_location_after_replacement(
    selected: &SearchResult,
    replacement: &str,
) -> (String, usize, usize) {
    let added_lines = replacement.matches('\n').count();
    let (line, column) = match replacement.rsplit_once('\n') {
        Some((_, last_line)) => (
            selected.line.saturating_sub(1).saturating_add(added_lines),
            last_line.strip_suffix('\r').unwrap_or(last_line).len(),
        ),
        None => (
            selected.line.saturating_sub(1),
            selected.start.saturating_add(replacement.len()),
        ),
    };
    (selected.path.clone(), line, column)
}

fn next_search_result_index(
    results: &[SearchResult],
    location: &(String, usize, usize),
) -> usize {
    results
        .iter()
        .position(|result| {
            (
                result.path.as_str(),
                result.line.saturating_sub(1),
                result.start,
            ) >= (location.0.as_str(), location.1, location.2)
        })
        .unwrap_or(0)
}

fn next_search_result_page_size(visible_count: usize, result_count: usize) -> usize {
    visible_count
        .saturating_add(SEARCH_RESULT_PAGE_SIZE)
        .min(result_count)
}

fn search_display_rows(
    results: &[SearchResult],
    visible_result_count: usize,
) -> Vec<SearchDisplayRow> {
    let mut rows = Vec::with_capacity(
        visible_result_count.min(results.len()).saturating_mul(2),
    );
    let mut previous_path = None;
    for (result_index, result) in
        results.iter().take(visible_result_count).enumerate()
    {
        if previous_path != Some(result.path.as_str()) {
            rows.push(SearchDisplayRow::FileHeader { result_index });
            previous_path = Some(result.path.as_str());
        }
        rows.push(SearchDisplayRow::Match { result_index });
    }
    rows
}

fn search_display_row_for_result(
    results: &[SearchResult],
    target_result_index: usize,
) -> Option<usize> {
    let mut display_row_index = 0;
    let mut previous_path = None;
    for (result_index, result) in results.iter().enumerate() {
        if previous_path != Some(result.path.as_str()) {
            display_row_index += 1;
            previous_path = Some(result.path.as_str());
        }
        if result_index == target_result_index {
            return Some(display_row_index);
        }
        display_row_index += 1;
    }
    None
}

fn highlighted_search_preview(
    text: &str,
    preview_match: Range<usize>,
    foreground: Hsla,
    accent: Hsla,
) -> AnyElement {
    h_flex()
        .flex_1()
        .min_w_0()
        .truncate()
        .overflow_hidden()
        .children(
            search_preview_segments(text, preview_match)
                .into_iter()
                .map(|(matched, segment)| {
                    let element = div()
                        .text_color(if matched { accent } else { foreground })
                        .child(segment);
                    if matched {
                        element.font_weight(FontWeight::BOLD)
                    } else {
                        element
                    }
                }),
        )
        .into_any_element()
}

fn search_preview_segments(
    text: &str,
    preview_match: Range<usize>,
) -> Vec<(bool, String)> {
    if preview_match.is_empty() {
        return vec![(false, text.to_string())];
    }
    let (Some(before), Some(matched), Some(after)) = (
        text.get(..preview_match.start),
        text.get(preview_match.clone()),
        text.get(preview_match.end..),
    ) else {
        return vec![(false, text.to_string())];
    };

    [(false, before), (true, matched), (false, after)]
        .into_iter()
        .filter(|(_, segment)| !segment.is_empty())
        .map(|(matched, segment)| (matched, segment.to_string()))
        .collect()
}

impl BasePanel for SearchPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_search"
    }

    fn on_added_to(
        &mut self,
        group: WeakEntity<TabGroup>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.tab_group = Some(group);
    }
}
impl Panel for SearchPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let group = self.tab_group.clone();
        let panel_id = self.panel_id;
        let close_handler = self.close_handler.clone();
        h_flex().items_center().gap_1().child("Search").child(
            Button::new("close_search")
                .icon(IconName::X)
                .ghost()
                .tooltip("Close Search")
                .on_click(move |_, window, cx| {
                    if let Some(handler) = close_handler.as_ref() {
                        handler(panel_id, window, cx);
                    } else if let Some(group) = group.as_ref() {
                        _ = group
                            .update(cx, |group, cx| group.close_panel(panel_id, cx));
                    }
                }),
        )
    }

    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}
impl EventEmitter<PanelEvent> for SearchPanel {}

impl Drop for SearchPanel {
    fn drop(&mut self) {
        if let (Some(request_id), Some(proxy)) = (
            self.active_workspace_file_request.take(),
            self.proxy.as_ref(),
        ) {
            proxy.cancel_workspace_files(request_id);
        }
    }
}

impl Focusable for SearchPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SearchPanel {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let text = cx.theme().sidebar_foreground;
        let muted = cx.theme().muted_foreground;
        let accent = cx.theme().primary;
        let query = self.query.read(cx).value().to_string();
        let has_query = !query.trim().is_empty();
        let show_replace = self.show_replace;
        let replacing = self.replacing;
        let can_replace_current = has_query
            && !replacing
            && !self.searching
            && self.results.get(self.selected_result_index).is_some();
        let can_replace_all = has_query && !replacing && !self.searching;
        let replacement_status = self.replacement_status.clone();
        let result_count = self.results.len();
        let visible_result_count = self.visible_result_count.min(self.results.len());
        let display_rows = search_display_rows(&self.results, visible_result_count);
        v_flex()
            .size_full()
            .gap_1()
            .p_2()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::handle_result_key_down))
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.query).aria_label("Search workspace")),
                    )
                    .child(
                        Button::new("toggle_search_replace")
                            .label("Replace")
                            .ghost()
                            .selected(show_replace)
                            .tooltip("Show or hide workspace replacement")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.show_replace = !this.show_replace;
                                if !this.show_replace {
                                    this.replacement_status = None;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .when(show_replace, |panel| {
                panel.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .debug_selector(|| "search-replace-toolbar".into())
                                .w_full()
                                .gap_1()
                                .items_center()
                                .child(
                                    div()
                                        .debug_selector(|| "search-replace-input".into())
                                        .flex_1()
                                        .min_w_0()
                                        .child(Input::new(&self.replacement).aria_label("Replace with")),
                                )
                                .child(
                                    Button::new("replace_current_search_match")
                                        .debug_selector(|| "search-replace-current-action".into())
                                        .icon(IconName::Replace)
                                        .label("Replace Match")
                                        .ghost()
                                        .flex_shrink_0()
                                        .disabled(!can_replace_current)
                                        .tooltip("Replace this match and select the next one")
                                        .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                            this.replace_current_match(window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("replace_all_search_matches")
                                        .debug_selector(|| "search-replace-all-action".into())
                                        .icon(IconName::Replace)
                                        .label(if replacing {
                                            "Replacing…"
                                        } else {
                                            "Replace All"
                                        })
                                        .primary()
                                        .flex_shrink_0()
                                        .disabled(!can_replace_all)
                                        .tooltip("Replace all workspace matches in editor buffers")
                                        .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                            this.replace_all(window, cx);
                                        })),
                                ),
                        )
                        .when_some(replacement_status, |row, status| {
                            let color = if status.starts_with("Replace failed:") {
                                cx.theme().danger
                            } else {
                                muted
                            };
                            row.child(
                                div()
                                    .px_1()
                                    .text_size(px(11.))
                                    .text_color(color)
                                    .child(status),
                            )
                        }),
                )
            })
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        h_flex()
                            .debug_selector(|| "search-mode-group".into())
                            .gap_1()
                            .items_center()
                            .px_1()
                            .rounded_md()
                            .border_1()
                            .border_color(cx.theme().border)
                            .child(
                                Button::new("search_mode_text")
                                    .debug_selector(|| "search-mode-text".into())
                                    .label("Text")
                                    .ghost()
                                    .selected(self.search_mode == SearchMode::Text)
                                    .tooltip("Search for literal text")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                        this.search_mode = SearchMode::Text;
                                        this.pending_next_match = None;
                                        this.refresh(cx);
                                    })),
                            )
                            .child(
                                Button::new("search_mode_word")
                                    .debug_selector(|| "search-mode-word".into())
                                    .label("Word")
                                    .ghost()
                                    .selected(self.search_mode == SearchMode::Word)
                                    .tooltip("Match the query as a whole word")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                        this.search_mode = SearchMode::Word;
                                        this.pending_next_match = None;
                                        this.refresh(cx);
                                    })),
                            )
                            .child(
                                Button::new("search_mode_regex")
                                    .debug_selector(|| "search-mode-regex".into())
                                    .label("Regex")
                                    .ghost()
                                    .selected(self.search_mode == SearchMode::Regex)
                                    .tooltip("Interpret the query as a regular expression")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                        this.search_mode = SearchMode::Regex;
                                        this.pending_next_match = None;
                                        this.refresh(cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .w(px(1.))
                            .h(px(18.))
                            .bg(cx.theme().border),
                    )
                    .child(
                        Button::new("search_match_case")
                            .debug_selector(|| "search-match-case".into())
                            .label("Match case")
                            .ghost()
                            .selected(self.case_sensitive)
                            .tooltip("Also match capitalization in Text, Word, or Regex mode")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.case_sensitive = !this.case_sensitive;
                                this.pending_next_match = None;
                                this.refresh(cx);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(muted)
                            .child("Include paths (comma-separated)"),
                    )
                    .child(
                        Input::new(&self.include_glob)
                            .aria_label("Include path patterns"),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(muted)
                            .child("Exclude paths (comma-separated)"),
                    )
                    .child(
                        Input::new(&self.exclude_glob)
                            .aria_label("Exclude path patterns"),
                    ),
            )
            .child(div().px_1().text_size(px(11.)).text_color(muted).child({
                let count_summary = if visible_result_count < result_count {
                    format!(
                        "Showing {visible_result_count} of {result_count} matches"
                    )
                } else {
                    format!(
                        "{result_count} result{}",
                        if result_count == 1 { "" } else { "s" }
                    )
                };
                if query.is_empty() {
                    "Search workspace".to_string()
                } else if let Some(error) = &self.search_error {
                    error.clone()
                } else if self.searching {
                    format!("Searching… {count_summary}")
                } else if self.search_limit_reached {
                    format!("{count_summary} (search limit reached)")
                } else {
                    count_summary
                }
            }))
            .child(
                v_flex().flex_1().min_h_0().child(
                    uniform_list(
                        "workspace-search-results",
                        display_rows.len(),
                        cx.processor(move |this, range: Range<usize>, _window, cx| {
                            range
                                .filter_map(|row_index| {
                                    match display_rows.get(row_index)? {
                                        SearchDisplayRow::FileHeader {
                                            result_index,
                                        } => {
                                            let path = this
                                                .results
                                                .get(*result_index)?
                                                .path
                                                .clone();
                                            Some(
                                                h_flex()
                                                    .w_full()
                                                    .h(px(32.))
                                                    .gap_1()
                                                    .items_center()
                                                    .child(IconName::FileCode)
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .truncate()
                                                            .text_color(text)
                                                            .child(path),
                                                    )
                                                    .into_any_element(),
                                            )
                                        }
                                        SearchDisplayRow::Match {
                                            result_index,
                                        } => {
                                            let result_index = *result_index;
                                            let result =
                                                this.results.get(result_index)?;
                                            let click_result = result.clone();
                                            let location_label =
                                                if result.line == result.end_line {
                                                    result.line.to_string()
                                                } else {
                                                    format!(
                                                        "{}–{}",
                                                        result.line,
                                                        result.end_line
                                                    )
                                                };
                                            Some(
                                                ListItem::new(result_index)
                                                    .w_full()
                                                    .h(px(32.))
                                                    .selected(
                                                        result_index
                                                            == this.selected_result_index,
                                                    )
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            this.pending_next_match = None;
                                                            this.selected_result_index =
                                                                result_index;
                                                            this.open_result(
                                                                &click_result,
                                                                window,
                                                                cx,
                                                            );
                                                        },
                                                    ))
                                                    .child(
                                                        h_flex()
                                                            .h_full()
                                                            .gap_2()
                                                            .pl_4()
                                                            .items_center()
                                                            .child(
                                                                div()
                                                                    .w(px(56.))
                                                                    .text_color(muted)
                                                                    .child(location_label),
                                                            )
                                                            .child(
                                                                highlighted_search_preview(
                                                                    &result.text,
                                                                    result.preview_match.clone(),
                                                                    text,
                                                                    accent,
                                                                ),
                                                            ),
                                                    )
                                                    .into_any_element(),
                                            )
                                        }
                                    }
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .size_full()
                    .track_scroll(&self.search_results_scroll),
                ),
            )
            .when(visible_result_count < result_count, |panel| {
                panel.child(
                    Button::new("show_more_search_results")
                        .label(format!(
                            "Show more ({visible_result_count} of {result_count})"
                        ))
                        .tooltip("Show more search results")
                        .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                            this.visible_result_count = next_search_result_page_size(
                                this.visible_result_count,
                                this.results.len(),
                            );
                            cx.notify();
                        })),
                )
            })
    }
}

pub struct ProblemsPanel {
    pub focus: FocusHandle,
    pub code: Entity<CodePanel>,
    close_handler: Option<Rc<dyn Fn(PanelId, &mut Window, &mut App)>>,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
}

impl ProblemsPanel {
    pub fn new(code: Entity<CodePanel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&code, |_, _, cx| cx.notify()).detach();
        Self {
            focus: cx.focus_handle(),
            code,
            close_handler: None,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
        }
    }

    pub fn set_close_handler<F>(&mut self, handler: F)
    where
        F: Fn(PanelId, &mut Window, &mut App) + 'static,
    {
        self.close_handler = Some(Rc::new(handler));
    }
}

impl BasePanel for ProblemsPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_problems"
    }

    fn on_added_to(
        &mut self,
        group: WeakEntity<TabGroup>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.tab_group = Some(group);
    }
}
impl Panel for ProblemsPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let group = self.tab_group.clone();
        let panel_id = self.panel_id;
        let close_handler = self.close_handler.clone();
        h_flex().items_center().gap_1().child("Problems").child(
            Button::new("close_problems_panel")
                .icon(IconName::X)
                .ghost()
                .tooltip("Close Problems")
                .on_click(move |_, window, cx| {
                    if let Some(handler) = close_handler.as_ref() {
                        handler(panel_id, window, cx);
                    } else if let Some(group) = group.as_ref() {
                        _ = group.update(cx, |group, cx| {
                            group.close_panel(panel_id, cx);
                        });
                    }
                }),
        )
    }
    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}
impl EventEmitter<PanelEvent> for ProblemsPanel {}
impl Focusable for ProblemsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ProblemsPanel {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let text = cx.theme().sidebar_foreground;
        let muted = cx.theme().muted_foreground;
        let (code_focus, proxy, active_file, active_diagnostics) =
            self.code.read_with(cx, |code, _| {
                (
                    code.focus.clone(),
                    code.proxy.clone(),
                    code.file_path.clone(),
                    code.diagnostics.clone(),
                )
            });
        let mut problems = proxy
            .map(|proxy| {
                proxy
                    .all_diagnostics()
                    .into_iter()
                    .flat_map(|(path, diagnostics)| {
                        let path = path.to_string_lossy().to_string();
                        diagnostics.into_iter().map(move |diagnostic| {
                            (
                                path.clone(),
                                diagnostic.range.start.line + 1,
                                diagnostic.message,
                                matches!(
                                    diagnostic.severity,
                                    Some(lsp_types::DiagnosticSeverity::ERROR)
                                ),
                            )
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if problems.is_empty() {
            problems = active_diagnostics
                .into_iter()
                .map(|diagnostic| {
                    (
                        active_file.clone(),
                        diagnostic.line,
                        diagnostic.message,
                        diagnostic.is_error,
                    )
                })
                .collect();
        }
        v_flex()
            .size_full()
            .gap_1()
            .p_2()
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(IconName::ShieldAlert)
                    .child(
                        div()
                            .text_color(text)
                            .child(format!("Problems ({})", problems.len())),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(muted)
                    .child("Project diagnostics and active-file issues"),
            )
            .child(v_flex().flex_1().min_h_0().overflow_y_scrollbar().children(
                problems.into_iter().enumerate().map(
                    |(index, (path, line, message, is_error))| {
                        let problem_focus = code_focus.clone();
                        h_flex()
                            .id(("problem", index))
                            .gap_2()
                            .px_2()
                            .py_1()
                            .cursor(CursorStyle::PointingHand)
                            .on_click(cx.listener(
                                move |this: &mut Self, _, window, cx| {
                                    this.code.update(cx, |code, cx| {
                                        code.active_line = line;
                                        cx.notify();
                                    });
                                    window.focus(&problem_focus, cx);
                                },
                            ))
                            .child(if is_error {
                                IconName::CircleX
                            } else {
                                IconName::TriangleAlert
                            })
                            .child(
                                div()
                                    .w(px(34.))
                                    .text_color(muted)
                                    .child(format!("{}", line)),
                            )
                            .child(div().w(px(150.)).text_color(muted).child(path))
                            .child(div().flex_1().text_color(text).child(message))
                    },
                ),
            ))
    }
}

pub struct LanguageServersPanel {
    pub focus: FocusHandle,
    root: PathBuf,
    proxy: Arc<ProxyClient>,
    trusted: bool,
    undecided: bool,
    trust_pending: bool,
    trust_error: Option<String>,
    expanded_schemas: HashSet<String>,
    open_settings_handler: Option<Rc<dyn Fn(PathBuf, &mut Window, &mut App)>>,
    _updates: Task<()>,
}

fn language_server_schema_properties(
    schema: &serde_json::Value,
) -> Vec<(String, String)> {
    schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flat_map(|properties| properties.iter())
        .map(|(name, property)| {
            let description = property
                .get("description")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    property
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "configuration option".into());
            (name.clone(), description)
        })
        .collect()
}

fn workspace_trust_prompt(root: &Path) -> (String, String) {
    let workspace_name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "this workspace".into());
    (
        format!("Allow language servers for {workspace_name}?"),
        format!(
            "Workspace path: {}\n\nProject settings can choose language-server commands. Installed extensions may run processes with your user permissions. Allow only projects you recognize.",
            root.display()
        ),
    )
}

impl LanguageServersPanel {
    pub fn new(
        root: &Path,
        proxy: Arc<ProxyClient>,
        cx: &mut Context<Self>,
    ) -> Self {
        use ahead_core::workspace_trust::TrustDecision;
        let (trusted, undecided, trust_error) =
            match ahead_core::workspace_trust::decision(root) {
                Ok(decision) => (
                    decision == TrustDecision::Allowed,
                    decision == TrustDecision::Undecided,
                    None,
                ),
                Err(error) => (false, false, Some(error.to_string())),
            };
        let updates = proxy.subscribe_diagnostics();
        let task = cx.spawn(async move |this, cx| {
            while updates.recv().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        Self {
            focus: cx.focus_handle(),
            root: root.to_path_buf(),
            proxy,
            trusted,
            undecided,
            trust_pending: false,
            trust_error,
            expanded_schemas: HashSet::new(),
            open_settings_handler: None,
            _updates: task,
        }
    }

    pub fn set_open_settings_handler<F>(&mut self, handler: F)
    where
        F: 'static + Fn(PathBuf, &mut Window, &mut App),
    {
        self.open_settings_handler = Some(Rc::new(handler));
    }

    fn refresh_trust(&mut self, cx: &mut Context<Self>) {
        use ahead_core::workspace_trust::TrustDecision;
        if self.trust_pending {
            return;
        }
        match ahead_core::workspace_trust::decision(&self.root) {
            Ok(decision) => {
                let trusted = decision == TrustDecision::Allowed;
                if self.trusted != trusted {
                    self.proxy.restart_language_servers();
                }
                self.trusted = trusted;
                self.undecided = decision == TrustDecision::Undecided;
                self.trust_error = None;
            }
            Err(error) => {
                if self.trusted {
                    self.proxy.restart_language_servers();
                }
                self.trusted = false;
                self.undecided = false;
                self.trust_error = Some(error.to_string());
            }
        }
        cx.notify();
    }

    pub fn prompt_if_undecided(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refresh_trust(cx);
        if self.undecided && self.trust_error.is_none() {
            self.change_trust(window, cx);
        }
    }

    fn change_trust(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.trust_pending {
            return;
        }
        let confirmation = (!self.trusted).then(|| {
            let (title, message) = workspace_trust_prompt(&self.root);
            window.prompt(
                PromptLevel::Warning,
                &title,
                Some(&message),
                &["Allow & Start", "Keep Blocked"],
                cx,
            )
        });
        self.trust_pending = true;
        self.trust_error = None;
        let root = self.root.clone();
        let proxy = self.proxy.clone();
        cx.spawn_in(window, async move |this, cx| {
            let trusted = if let Some(confirmation) = confirmation {
                match confirmation.await {
                    Ok(0) => true,
                    Ok(1) => false,
                    _ => {
                        if let Err(error) = this.update_in(cx, |panel, _, cx| {
                            panel.trust_pending = false;
                            cx.notify();
                        }) {
                            eprintln!(
                                "Language Servers panel closed during trust prompt: {error}"
                            );
                        }
                        return;
                    }
                }
            } else {
                false
            };
            let result = cx
                .background_spawn(async move {
                    ahead_core::workspace_trust::set_trusted(&root, trusted)?;
                    if ahead_core::workspace_trust::is_trusted(&root)? != trusted {
                        return Err(std::io::Error::other(
                            "Workspace trust decision could not be verified",
                        ));
                    }
                    Ok(())
                })
                .await;
            if let Err(error) = this.update_in(cx, |panel, _, cx| {
                panel.trust_pending = false;
                match result {
                    Ok(()) => {
                        panel.trusted = trusted;
                        panel.undecided = false;
                        proxy.restart_language_servers();
                    }
                    Err(error) => panel.trust_error = Some(error.to_string()),
                }
                cx.notify();
            }) {
                eprintln!(
                    "Language Servers panel closed before trust update: {error}"
                );
            }
        })
        .detach();
        cx.notify();
    }
}

impl BasePanel for LanguageServersPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_language_servers"
    }
}
impl Panel for LanguageServersPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        "Language Servers"
    }
    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}
impl EventEmitter<PanelEvent> for LanguageServersPanel {}
impl Focusable for LanguageServersPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for LanguageServersPanel {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let text = cx.theme().sidebar_foreground;
        let muted = cx.theme().muted_foreground;
        let servers = self.proxy.lsp_servers();
        let server_schemas = self.proxy.language_server_schemas();
        let expanded_schemas = self.expanded_schemas.clone();
        let extension_issues = self.proxy.language_extension_issues();
        v_flex()
            .size_full()
            .gap_2()
            .p_3()
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("edit_language_server_settings")
                            .icon(IconName::Settings)
                            .label("Config")
                            .ghost()
                            .disabled(self.open_settings_handler.is_none())
                            .tooltip(
                                "Edit shareable language-server routing in .ahead/config.toml",
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(handler) =
                                    this.open_settings_handler.clone()
                                {
                                    handler(
                                        this.root.join(".ahead/config.toml"),
                                        window,
                                        cx,
                                    );
                                }
                            })),
                    )
                    .child(
                        Button::new("restart_language_servers")
                            .icon(IconName::RefreshCw)
                            .label("Restart")
                            .ghost()
                            .tooltip("Restart workspace language servers")
                            .on_click(cx.listener(|this, _, _, _| {
                                this.proxy.restart_language_servers()
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(if self.trusted {
                                IconName::ShieldCheck
                            } else {
                                IconName::ShieldAlert
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_color(text)
                                    .child(if self.trusted {
                                        "Language servers allowed"
                                    } else {
                                        "Language servers blocked"
                                    }),
                            )
                            .child(
                                Button::new("workspace_trust")
                                    .label(if self.trust_pending {
                                        "Saving…"
                                    } else if self.trusted {
                                        "Block"
                                    } else {
                                        "Allow"
                                    })
                                    .ghost()
                                    .disabled(self.trust_pending)
                                    .tooltip(if self.trusted {
                                        "Block workspace language servers"
                                    } else {
                                        "Allow language servers to run for this workspace"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.change_trust(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(muted)
                            .child(if self.trusted {
                                "Project language servers may run."
                            } else {
                                "Language servers will not start for this project."
                            }),
                    ),
            )
            .when_some(self.trust_error.clone(), |el, error| {
                el.child(
                    div()
                        .text_size(px(11.))
                        .text_color(cx.theme().danger)
                        .child(format!("Trust settings: {error}")),
                )
            })
            .children(servers.iter().map(|server| {
                let (icon, state, color) = if server.is_ready() {
                    (IconName::CircleCheck, "Ready", cx.theme().success)
                } else if server.is_error() {
                    (IconName::CircleX, "Error", cx.theme().danger)
                } else {
                    (IconName::CircleDashed, "Starting", cx.theme().warning)
                };
                let server_id = server
                    .name
                    .strip_prefix("lsp-")
                    .unwrap_or(&server.name)
                    .to_owned();
                let name = format!("{server_id} language server");
                let schema = server_schemas
                    .get(&server_id)
                    .filter(|schemas| {
                        schemas.initialization_options.is_some()
                            || schemas.settings.is_some()
                    })
                    .cloned();
                let schema_expanded = expanded_schemas.contains(&server_id);
                let server_id_for_click = server_id.clone();
                v_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .p_2()
                            .bg(cx.theme().group_box)
                            .child(div().text_color(color).child(icon))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(text)
                                            .child(name),
                                    )
                                    .when_some(server.message.clone(), |el, message| {
                                        el.child(
                                            div()
                                                .min_w_0()
                                                .text_size(px(11.))
                                                .text_color(muted)
                                                .child(message),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(11.))
                                    .text_color(color)
                                    .child(state),
                            )
                            .when_some(schema.clone(), |row, _| {
                                row.child(
                                    Button::new(format!("lsp_schema_{server_id}"))
                                        .icon(if schema_expanded {
                                            IconName::ChevronDown
                                        } else {
                                            IconName::ChevronRight
                                        })
                                        .label("Options")
                                        .ghost()
                                        .tooltip(
                                            "Show configuration options supported by this extension",
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if !this
                                                .expanded_schemas
                                                .remove(&server_id_for_click)
                                            {
                                                this.expanded_schemas
                                                    .insert(server_id_for_click.clone());
                                            }
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                    .when_some(schema, |item, schemas| {
                        item.when(schema_expanded, |item| {
                            let initialization_options = schemas
                                .initialization_options
                                .as_ref()
                                .map(language_server_schema_properties)
                                .unwrap_or_default();
                            let settings = schemas
                                .settings
                                .as_ref()
                                .map(language_server_schema_properties)
                                .unwrap_or_default();
                            item.child(
                                v_flex()
                                    .w_full()
                                    .gap_2()
                                    .p_2()
                                    .pl_8()
                                    .bg(cx.theme().group_box)
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(muted)
                                            .child(format!(
                                                "Edit [lsp.{server_id}] in .ahead/config.toml with Config above"
                                            )),
                                    )
                                    .when_some(
                                        schemas.initialization_options.as_ref(),
                                        |details, _| {
                                            details.child(
                                                v_flex()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .text_color(text)
                                                            .child("Initialization options"),
                                                    )
                                                    .children(
                                                        initialization_options
                                                            .iter()
                                                            .take(8)
                                                            .map(|(name, description)| {
                                                                h_flex()
                                                                    .gap_2()
                                                                    .child(
                                                                        div()
                                                                            .flex_shrink_0()
                                                                            .text_color(text)
                                                                            .child(name.clone()),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .min_w_0()
                                                                            .truncate()
                                                                            .text_color(muted)
                                                                            .child(description.clone()),
                                                                    )
                                                            }),
                                                    )
                                                    .when(
                                                        initialization_options.len() > 8,
                                                        |details| {
                                                            details.child(
                                                                div()
                                                                    .text_size(px(11.))
                                                                    .text_color(muted)
                                                                    .child(format!(
                                                                        "{} more options",
                                                                        initialization_options.len() - 8
                                                                    )),
                                                            )
                                                        },
                                                    )
                                            )
                                        },
                                    )
                                    .when_some(schemas.settings.as_ref(), |details, _| {
                                        details.child(
                                            v_flex()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .text_color(text)
                                                        .child("Server settings"),
                                                )
                                                .children(
                                                    settings.iter().take(8).map(
                                                        |(name, description)| {
                                                            h_flex()
                                                                .gap_2()
                                                                .child(
                                                                    div()
                                                                        .flex_shrink_0()
                                                                        .text_color(text)
                                                                        .child(name.clone()),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .min_w_0()
                                                                        .truncate()
                                                                        .text_color(muted)
                                                                        .child(description.clone()),
                                                                )
                                                        },
                                                    ),
                                                )
                                                .when(settings.len() > 8, |details| {
                                                    details.child(
                                                        div()
                                                            .text_size(px(11.))
                                                            .text_color(muted)
                                                            .child(format!(
                                                                "{} more options",
                                                                settings.len() - 8
                                                            )),
                                                    )
                                                }),
                                        )
                                    }),
                            )
                        })
                    })
            }))
            .when(!extension_issues.is_empty(), |el| {
                el.child(
                    div()
                        .pt_2()
                        .text_size(px(11.))
                        .text_color(muted)
                        .child("Extension issues"),
                )
            })
            .children(extension_issues.iter().map(|issue| {
                v_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .child(
                                div()
                                    .text_color(cx.theme().danger)
                                    .child(IconName::TriangleAlert),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(text)
                                    .child(issue.name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(muted)
                            .child(issue.message.clone()),
                    )
            }))
            .when(self.trusted && servers.is_empty() && extension_issues.is_empty(), |el| {
                el.child(
                    div()
                        .text_size(px(11.))
                        .text_color(muted)
                        .child("Waiting for language servers to initialize…"),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GitFile, GitPanel, GitWorkspaceStatus, JustTask, JustTasksPanel,
        LanguageServersPanel, SEARCH_RESULT_PAGE_SIZE, SearchDisplayRow, SearchMode,
        SearchPanel, SearchResult, git_push_failure_status,
        git_repository_not_found, is_non_fast_forward_error,
        language_server_schema_properties, next_search_result_index,
        next_search_result_page_size, parse_just_list,
        search_display_row_for_result, search_display_rows,
        search_location_after_replacement, search_preview_segments,
        search_result_index_after_key, workspace_trust_prompt,
    };
    use crate::code_panel::CodePanel;
    use ahead_rpc::source_control::GitStatusFile;
    use gpui_kit::prelude::*;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{Entity, Focusable, Render, TestAppContext, Window, div, px};
    use std::{cell::RefCell, path::Path, rc::Rc, sync::Arc};

    #[test]
    fn language_server_schema_preview_lists_fields_and_descriptions() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "enabled": {
                    "type": "boolean",
                    "description": "Enable the feature"
                },
                "mode": { "type": "string" }
            }
        });

        assert_eq!(
            language_server_schema_properties(&schema),
            vec![
                ("enabled".into(), "Enable the feature".into()),
                ("mode".into(), "string".into()),
            ]
        );
    }

    #[gpui_kit::test]
    fn language_server_config_action_opens_tracked_workspace_config(
        cx: &mut TestAppContext,
    ) {
        let workspace = tempfile::tempdir().expect("workspace");
        let root = workspace.path().to_path_buf();
        let proxy = crate::proxy_client::ProxyClient::new_for_test(root.clone());
        let opened_path = Rc::new(RefCell::new(None));
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(move |_, cx| {
            LanguageServersPanel::new(&root, proxy.clone(), cx)
        });
        let opened_path_for_handler = opened_path.clone();
        panel.update(cx, |panel, _| {
            panel.set_open_settings_handler(move |path, _, _| {
                *opened_path_for_handler.borrow_mut() = Some(path);
            });
        });

        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| {
            window.click("edit_language_server_settings", cx);
        });

        assert_eq!(
            opened_path.borrow().as_ref(),
            Some(&workspace.path().join(".ahead/config.toml"))
        );
    }

    #[test]
    fn workspace_trust_prompt_identifies_project_and_full_path() {
        let root = Path::new("/private/tmp/ahead-disposable-project");
        let (title, message) = workspace_trust_prompt(root);

        assert_eq!(
            title,
            "Allow language servers for ahead-disposable-project?"
        );
        assert!(
            message
                .contains("Workspace path: /private/tmp/ahead-disposable-project")
        );
        assert!(message.contains(
            "Installed extensions may run processes with your user permissions."
        ));
    }

    #[test]
    fn git_push_failure_status_explains_stale_remote_and_preserves_details() {
        let error = "error: failed to push some refs to 'origin'\n! [rejected] main -> main (non-fast-forward)";

        let status = git_push_failure_status(error);

        assert!(status.starts_with(
            "Push rejected: fetch and integrate remote changes before pushing again."
        ));
        assert!(status.contains(error));
    }

    #[test]
    fn git_push_failure_status_keeps_other_git_errors_unchanged() {
        assert_eq!(
            git_push_failure_status("fatal: Authentication failed"),
            "Push failed: fatal: Authentication failed"
        );
    }

    #[test]
    fn non_fast_forward_detection_handles_git_rejection_variants() {
        assert!(is_non_fast_forward_error("! [rejected] main (fetch first)"));
        assert!(is_non_fast_forward_error(
            "Updates were rejected because the remote contains work you do not have locally"
        ));
        assert!(!is_non_fast_forward_error("fatal: Authentication failed"));
        assert!(!is_non_fast_forward_error("error: RPC failed; curl 56"));
    }

    struct NarrowGitPanel(Entity<GitPanel>);

    impl Render for NarrowGitPanel {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut Context<Self>,
        ) -> impl IntoElement {
            div().w(px(260.)).h(px(400.)).child(self.0.clone())
        }
    }

    #[gpui_kit::test]
    fn narrow_git_row_keeps_status_and_staging_visible(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            NarrowGitPanel(cx.new(|cx| GitPanel::new(".", window, cx)))
        });
        let panel = cx.update(|_, cx| view.read(cx).0.clone());
        panel.update(cx, |panel, cx| {
            panel.files = vec![GitFile {
                path: "deeply/nested/path/to/a/long/descriptive/filename.rs".into(),
                state: "??".into(),
                staged: false,
            }];
            panel.status = "Commit failed: Git identity is not configured for this disposable project; set user.name and user.email before retrying".into();
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let row = cx.debug_bounds("git-file-0").expect("Git row");
        let path = cx.debug_bounds("git-path-0").expect("Git path");
        let state = cx.debug_bounds("git-state-0").expect("Git status");
        let select = cx.debug_bounds("git-select-0").expect("Git selection");
        assert!(row.size.width <= px(260.));
        assert!(path.origin.x + path.size.width <= state.origin.x);
        assert!(state.origin.x + state.size.width <= select.origin.x);
        assert!(
            select.origin.x + select.size.width <= row.origin.x + row.size.width
        );
        let status = cx.debug_bounds("git-commit-status").expect("Git status");
        let action = cx.debug_bounds("git-commit-action").expect("Commit action");
        assert!(status.origin.y + status.size.height <= action.origin.y);
        assert!(
            action.origin.x + action.size.width <= row.origin.x + row.size.width
        );
    }

    #[gpui_kit::test]
    fn git_credential_prompt_is_inline_masked_and_submittable(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("workspace");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            workspace.path().to_path_buf(),
        );
        let proxy_for_panel = proxy.clone();
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            GitPanel::new(".", window, cx).with_proxy(proxy_for_panel)
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.request_git_credential(
                42,
                "Password for https://example.test".into(),
                true,
                window,
                cx,
            );
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(cx.debug_bounds("git-credential-prompt").is_some());
        let input = panel.update(cx, |panel, _| panel.git_secret_input.clone());
        assert!(cx.update(|_, cx| input.read(cx).presentation().is_masked()));
        input.update_in(cx, |input, window, cx| {
            input.set_value("disposable-test-token", window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.click("git_credential_submit", cx));

        assert!(panel.update(cx, |panel, _| panel.git_credential_prompt.is_none()));
        assert_eq!(
            panel.update(cx, |panel, _| panel.status.to_string()),
            "Sending Git credentials…"
        );
        assert_eq!(
            cx.update(|_, cx| input.read(cx).value().to_owned()),
            "",
            "credential input is cleared immediately after submission"
        );
    }

    #[gpui_kit::test]
    fn canceling_git_credential_clears_inputs_and_answers_none(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("workspace");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            workspace.path().to_path_buf(),
        );
        let proxy_for_panel = proxy.clone();
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            GitPanel::new(".", window, cx).with_proxy(proxy_for_panel)
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.request_git_credential(
                43,
                "Password for https://example.test".into(),
                true,
                window,
                cx,
            );
        });
        let input = panel.update(cx, |panel, _| panel.git_secret_input.clone());
        input.update_in(cx, |input, window, cx| {
            input.set_value("disposable-test-token", window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.click("git_credential_cancel", cx));

        assert!(panel.update(cx, |panel, _| panel.git_credential_prompt.is_none()));
        assert_eq!(cx.update(|_, cx| input.read(cx).value().to_owned()), "");
    }

    #[gpui_kit::test]
    fn git_history_tab_shows_commits_and_returns_to_changes(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            NarrowGitPanel(cx.new(|cx| GitPanel::new(".", window, cx)))
        });
        let panel = cx.update(|_, cx| view.read(cx).0.clone());
        panel.update(cx, |panel, cx| {
            panel.show_history = true;
            panel.commits = vec![ahead_rpc::source_control::GitCommitSummary {
                id: "0123456789abcdef".into(),
                author: "Ada Lovelace".into(),
                timestamp: chrono::Utc::now().timestamp(),
                subject: "Add a useful feature".into(),
            }];
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(cx.debug_bounds("git-history-commit-0").is_some());

        cx.update(|window, cx| window.click("git-history-commit-0", cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.update(|_, cx| panel.read(cx).selected_commit.is_some()));
        assert!(cx.debug_bounds("git_commit_diff_back").is_some());
        assert!(cx.debug_bounds("git-history-commit-0").is_none());

        panel.update(cx, |panel, cx| {
            panel.commit_patch_error = None;
            panel.commit_patch = Some(ahead_rpc::source_control::GitPatch {
                id: "0123456789abcdef".into(),
                patch: "diff --git a/main.rs b/main.rs\n+added\n".into(),
                truncated: false,
            });
            panel.commit_patch_lines = Arc::new(vec![
                "diff --git a/main.rs b/main.rs".into(),
                "+added".into(),
            ]);
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("git_commit_diff_copy").is_some());
        cx.update(|window, cx| window.click("git_commit_diff_copy", cx));
        assert!(cx.update(|_, cx| panel.read(cx).commit_patch_copied));

        cx.update(|window, cx| window.click("git_commit_diff_back", cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.update(|_, cx| panel.read(cx).selected_commit.is_none()));
        assert!(cx.debug_bounds("git-history-commit-0").is_some());

        cx.update(|window, cx| window.click("git_changes", cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("git-history-commit-0").is_none());
        assert!(!cx.update(|_, cx| panel.read(cx).show_history));
    }

    #[gpui_kit::test]
    fn missing_git_repository_offers_initialization(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            NarrowGitPanel(cx.new(|cx| GitPanel::new(".", window, cx)))
        });
        let panel = cx.update(|_, cx| view.read(cx).0.clone());
        panel.update(cx, |panel, cx| {
            panel.repository_missing = true;
            panel.status = "This workspace is not a Git repository yet".into();
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(cx.debug_bounds("git-init-prompt").is_some());
        cx.update(|window, cx| window.click("git_initialize_repository", cx));
        assert_eq!(
            cx.update(|_, cx| panel.read(cx).status.as_ref().to_string()),
            "Git service unavailable"
        );
    }

    #[test]
    fn git_repository_error_detection_does_not_hide_other_failures() {
        assert!(git_repository_not_found(
            "could not find repository at '/tmp/project'; class=Repository (6); code=NotFound (-3)"
        ));
        assert!(!git_repository_not_found("permission denied"));
    }

    #[gpui_kit::test]
    fn search_modes_are_exclusive_and_case_is_independent(cx: &mut TestAppContext) {
        let workspace = tempfile::tempdir().expect("workspace");
        let file_path = workspace.path().join("main.js");
        std::fs::write(&file_path, "console.log(42)\nconsoleXlog(42)\n")
            .expect("source file");
        let root = workspace.path().to_string_lossy().into_owned();
        let file_path = file_path.to_string_lossy().into_owned();
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            let code = cx.new(|cx| CodePanel::new(&file_path, window, cx));
            SearchPanel::new(&root, 0, code, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let mode_group = cx
            .debug_bounds("search-mode-group")
            .expect("exclusive search modes are grouped");
        let text_mode = cx.debug_bounds("search-mode-text").expect("Text mode");
        let regex_mode = cx.debug_bounds("search-mode-regex").expect("Regex mode");
        let match_case = cx
            .debug_bounds("search-match-case")
            .expect("independent case modifier");
        assert!(text_mode.origin.x >= mode_group.origin.x);
        assert!(
            regex_mode.origin.x + regex_mode.size.width
                <= mode_group.origin.x + mode_group.size.width
        );
        assert!(
            mode_group.origin.x + mode_group.size.width < match_case.origin.x,
            "Match case should be visually separate from the exclusive mode group"
        );

        cx.update(|window, cx| {
            window.click("search_mode_word", cx);
        });
        assert_eq!(
            cx.update(|_, cx| panel.read(cx).search_mode),
            SearchMode::Word
        );
        assert!(!cx.update(|_, cx| panel.read(cx).case_sensitive));

        cx.update(|window, cx| {
            window.click("search_match_case", cx);
            window.click("search_mode_regex", cx);
        });
        assert_eq!(
            cx.update(|_, cx| panel.read(cx).search_mode),
            SearchMode::Regex
        );
        assert!(cx.update(|_, cx| panel.read(cx).case_sensitive));

        cx.update(|window, cx| window.click("search_mode_text", cx));
        assert_eq!(
            cx.update(|_, cx| panel.read(cx).search_mode),
            SearchMode::Text
        );
        assert!(cx.update(|_, cx| panel.read(cx).case_sensitive));

        panel.update_in(cx, |panel, window, cx| {
            panel.query.update(cx, |input, cx| {
                input.focus_handle(cx).focus(window, cx);
            });
        });
        cx.simulate_keystrokes("console.log");
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(250));
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.search_mode, SearchMode::Text);
            assert!(panel.search_error.is_none());
            assert_eq!(panel.results.len(), 1);
        });

        cx.update(|window, cx| window.click("search_mode_regex", cx));
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(250));
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.search_mode, SearchMode::Regex);
            assert_eq!(panel.results.len(), 2);
            assert!(panel.search_error.is_none());
        });

        cx.simulate_keystrokes("(");
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(250));
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.search_mode, SearchMode::Regex);
            assert!(panel.search_error.is_some());
        });

        cx.update(|window, cx| window.click("search_mode_text", cx));
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(250));
        cx.run_until_parked();
        panel.read_with(cx, |panel, app| {
            assert_eq!(panel.search_mode, SearchMode::Text);
            assert_eq!(panel.query.read(app).value().as_ref(), "console.log(");
            assert!(panel.search_error.is_none());
            assert_eq!(panel.results.len(), 1);
            assert_eq!(panel.results[0].text, "console.log(42)");
        });
    }

    #[gpui_kit::test]
    fn search_panel_treats_regex_metacharacters_as_literal_text(
        cx: &mut TestAppContext,
    ) {
        let workspace = tempfile::tempdir().expect("workspace");
        let file_path = workspace.path().join("main.js");
        std::fs::write(&file_path, "console.log(42)\nconsoleXlog(42)\n")
            .expect("source file");
        let root = workspace.path().to_string_lossy().into_owned();
        let file_path = file_path.to_string_lossy().into_owned();
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            let code = cx.new(|cx| CodePanel::new(&file_path, window, cx));
            SearchPanel::new(&root, 0, code, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.update_in(cx, |panel, window, cx| {
            panel.query.update(cx, |input, cx| {
                input.focus_handle(cx).focus(window, cx);
            });
        });
        cx.simulate_keystrokes("console.log");
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(250));
        cx.run_until_parked();

        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.search_mode, SearchMode::Text);
            assert!(panel.search_error.is_none());
            assert_eq!(panel.results.len(), 1);
            assert!(
                panel
                    .results
                    .iter()
                    .all(|result| { result.text.contains("console.log(") })
            );
        });

        // Enter the opening parenthesis in a distinct input event, as users do
        // when completing a literal call expression.
        cx.simulate_keystrokes("(");
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(250));
        cx.run_until_parked();

        panel.read_with(cx, |panel, app| {
            assert_eq!(panel.search_mode, SearchMode::Text);
            assert_eq!(panel.query.read(app).value().as_ref(), "console.log(");
            assert!(!panel.searching);
            assert!(panel.search_error.is_none());
            assert_eq!(panel.results.len(), 1);
            assert_eq!(panel.results[0].text, "console.log(42)");
        });
    }

    #[gpui_kit::test]
    fn search_replace_controls_are_collapsible(cx: &mut TestAppContext) {
        let workspace = tempfile::tempdir().expect("workspace");
        let file_path = workspace.path().join("main.rs");
        std::fs::write(&file_path, "needle\n").expect("source file");
        let root = workspace.path().to_string_lossy().into_owned();
        let file_path = file_path.to_string_lossy().into_owned();
        cx.update(gpui_kit::component::init);
        let (_panel, cx) = cx.add_window_view(move |window, cx| {
            let code = cx.new(|cx| CodePanel::new(&file_path, window, cx));
            SearchPanel::new(&root, 0, code, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        cx.update(|window, cx| window.click("toggle_search_replace", cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.update(|_, cx| _panel.read(cx).show_replace));
        let replacement_input = cx
            .debug_bounds("search-replace-input")
            .expect("replacement input is visible");
        let replacement_toolbar = cx
            .debug_bounds("search-replace-toolbar")
            .expect("replacement actions are visible");
        let replace_match = cx
            .debug_bounds("search-replace-current-action")
            .expect("Replace Match is visible");
        let replace_all = cx
            .debug_bounds("search-replace-all-action")
            .expect("Replace All is visible");
        let replacement_input_bottom =
            replacement_input.origin.y + replacement_input.size.height;
        let replace_match_bottom =
            replace_match.origin.y + replace_match.size.height;
        let replace_all_bottom = replace_all.origin.y + replace_all.size.height;
        assert!(
            replacement_input.origin.y < replace_match_bottom
                && replace_match.origin.y < replacement_input_bottom,
            "replacement input and Replace Match share the Zed-style toolbar row"
        );
        assert!(
            replacement_input.origin.y < replace_all_bottom
                && replace_all.origin.y < replacement_input_bottom,
            "Replace All shares the same toolbar row"
        );
        assert!(
            replacement_input.origin.x + replacement_input.size.width
                <= replace_match.origin.x,
            "replacement field stays before the replace actions"
        );
        assert_eq!(replace_match.origin.y, replace_all.origin.y);
        assert!(
            replace_match.origin.x + replace_match.size.width
                <= replace_all.origin.x
        );
        assert!(
            replace_all.origin.x + replace_all.size.width
                <= replacement_toolbar.origin.x + replacement_toolbar.size.width
        );

        cx.update(|window, cx| window.click("toggle_search_replace", cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(!cx.update(|_, cx| _panel.read(cx).show_replace));
    }

    #[gpui_kit::test]
    fn search_replace_all_emits_unsaved_editor_changes_without_writing_disk(
        cx: &mut TestAppContext,
    ) {
        let workspace = tempfile::tempdir().expect("workspace");
        let file_path = workspace.path().join("main.rs");
        std::fs::write(&file_path, "AHEAD_REPLACE_NEEDLE\n").expect("source file");
        let root = workspace.path().to_string_lossy().into_owned();
        let file_path = file_path.to_string_lossy().into_owned();
        let disk_file_path = file_path.clone();
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            let code = cx.new(|cx| CodePanel::new(&file_path, window, cx));
            SearchPanel::new(&root, 0, code, window, cx)
        });
        let captured_request = Rc::new(RefCell::new(None));
        let capture = captured_request.clone();
        panel.update(cx, |panel, _| {
            panel.set_replace_handler(move |request, _, _| {
                *capture.borrow_mut() = Some(request);
                Ok(())
            });
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.query.update(cx, |input, cx| {
                input.set_value("AHEAD_REPLACE_NEEDLE", window, cx);
            });
            panel.replacement.update(cx, |input, cx| {
                input.set_value("AHEAD_REPLACE_DONE", window, cx);
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.click("toggle_search_replace", cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| {
            window.click("replace_all_search_matches", cx);
        });
        cx.run_until_parked();

        let request = captured_request
            .borrow()
            .clone()
            .expect("replace request reaches the editor handler");
        assert_eq!(request.match_count, 1);
        assert_eq!(request.files.len(), 1);
        assert_eq!(request.files[0].original_text, "AHEAD_REPLACE_NEEDLE\n");
        assert_eq!(request.files[0].replacement_text, "AHEAD_REPLACE_DONE\n");
        assert_eq!(
            std::fs::read_to_string(disk_file_path)
                .expect("disk source remains readable"),
            "AHEAD_REPLACE_NEEDLE\n"
        );
    }

    #[gpui_kit::test]
    fn search_replace_match_emits_only_the_selected_result(cx: &mut TestAppContext) {
        let workspace = tempfile::tempdir().expect("workspace");
        let file_path = workspace.path().join("main.rs");
        let source = "AHEAD_REPLACE_NEEDLE then AHEAD_REPLACE_NEEDLE\n";
        std::fs::write(&file_path, source).expect("source file");
        let root = workspace.path().to_string_lossy().into_owned();
        let result_path = file_path.to_string_lossy().into_owned();
        let disk_file_path = file_path.clone();
        let code_path = result_path.clone();
        let match_start = source.find("AHEAD_REPLACE_NEEDLE").expect("match");
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            let code = cx.new(|cx| CodePanel::new(&code_path, window, cx));
            SearchPanel::new(&root, 0, code, window, cx)
        });
        let captured_request = Rc::new(RefCell::new(None));
        let capture = captured_request.clone();
        panel.update(cx, |panel, _| {
            panel.set_replace_handler(move |request, _, _| {
                *capture.borrow_mut() = Some(request);
                Ok(())
            });
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.query.update(cx, |input, cx| {
                input.set_value("AHEAD_REPLACE_NEEDLE", window, cx);
            });
            panel.replacement.update(cx, |input, cx| {
                input.set_value("AHEAD_REPLACE_DONE", window, cx);
            });
            panel.results = vec![SearchResult {
                path: result_path.clone(),
                line: 1,
                end_line: 1,
                start: match_start,
                end: match_start + "AHEAD_REPLACE_NEEDLE".len(),
                text: source.trim_end().into(),
                preview_match: match_start
                    ..match_start + "AHEAD_REPLACE_NEEDLE".len(),
            }];
            panel.selected_result_index = 0;
            panel.show_replace = true;
            panel.searching = false;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.click("replace_current_search_match", cx));
        cx.run_until_parked();

        let request = captured_request
            .borrow()
            .clone()
            .expect("single-match replacement reaches the editor handler");
        assert_eq!(request.match_count, 1);
        assert_eq!(request.files.len(), 1);
        assert_eq!(
            request.files[0].replacement_text,
            "AHEAD_REPLACE_DONE then AHEAD_REPLACE_NEEDLE\n"
        );
        assert_eq!(
            std::fs::read_to_string(disk_file_path)
                .expect("disk source remains readable"),
            source
        );
    }

    #[gpui_kit::test]
    fn search_panel_loads_the_next_result_page_on_click(cx: &mut TestAppContext) {
        let workspace = tempfile::tempdir().expect("workspace");
        let file_path = workspace.path().join("main.rs");
        std::fs::write(&file_path, "match\n").expect("source file");
        let root = workspace.path().to_string_lossy().into_owned();
        let file_path = file_path.to_string_lossy().into_owned();
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            let code = cx.new(|cx| CodePanel::new(&file_path, window, cx));
            SearchPanel::new(&root, 0, code, window, cx)
        });
        panel.update(cx, |panel, cx| {
            panel.results = (0..SEARCH_RESULT_PAGE_SIZE + 1)
                .map(|index| SearchResult {
                    path: "main.rs".into(),
                    line: index + 1,
                    end_line: index + 1,
                    start: 0,
                    end: 5,
                    text: "match".into(),
                    preview_match: 0..5,
                })
                .collect();
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        cx.update(|window, cx| window.click("show_more_search_results", cx));

        assert_eq!(
            cx.update(|_, cx| panel.read(cx).visible_result_count),
            SEARCH_RESULT_PAGE_SIZE + 1
        );
    }

    #[gpui_kit::test]
    fn applying_git_status_tracks_index_state_and_preserves_pending_status(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let (panel, cx) =
            cx.add_window_view(|window, cx| GitPanel::new(".", window, cx));
        panel.update(cx, |panel, _| {
            panel.apply_git_status(GitWorkspaceStatus {
                branch: "main".into(),
                branches: vec!["main".into()],
                branch_has_upstream: false,
                remotes: Vec::new(),
                repository_dirty: true,
                repository_has_conflicts: false,
                files: vec![GitStatusFile {
                    path: "first.rs".into(),
                    state: "??".into(),
                    staged: false,
                }],
            });
            panel.apply_git_status(GitWorkspaceStatus {
                branch: "main".into(),
                branches: vec!["main".into()],
                branch_has_upstream: true,
                remotes: vec!["origin".into()],
                repository_dirty: true,
                repository_has_conflicts: false,
                files: vec![GitStatusFile {
                    path: "first.rs".into(),
                    state: "A".into(),
                    staged: true,
                }],
            });
            assert_eq!(panel.files.len(), 1);
            assert!(panel.files[0].staged);
            assert!(panel.branch_has_upstream);
            assert_eq!(panel.remotes, vec!["origin".to_string()]);

            panel.apply_git_status(GitWorkspaceStatus {
                branch: "main".into(),
                branches: vec!["main".into()],
                branch_has_upstream: false,
                remotes: vec!["origin".into()],
                repository_dirty: true,
                repository_has_conflicts: true,
                files: vec![GitStatusFile {
                    path: "conflict.rs".into(),
                    state: "UU".into(),
                    staged: false,
                }],
            });
            assert!(panel.repository_has_conflicts);
            assert!(panel.files[0].is_conflicted());
            assert_eq!(
                panel.status.as_ref(),
                "Resolve all repository conflicts before committing"
            );

            panel.stage_pending = true;
            panel.status = "Staging changes…".into();
            panel.apply_git_status(GitWorkspaceStatus::default());
            assert_eq!(panel.status.as_ref(), "Staging changes…");
        });
    }

    #[gpui_kit::test]
    fn branch_switch_is_blocked_by_repository_changes_outside_workspace(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let (panel, cx) =
            cx.add_window_view(|window, cx| GitPanel::new(".", window, cx));
        panel.update(cx, |panel, cx| {
            panel.branch = "main".into();
            panel.branches = vec!["main".into(), "feature".into()];
            panel.repository_dirty = true;

            panel.checkout_branch("feature".into(), cx);

            assert!(!panel.checkout_pending);
            assert_eq!(
                panel.status.as_ref(),
                "Commit or stash repository changes before switching branches"
            );
        });
    }

    #[gpui_kit::test]
    fn branch_creation_requires_a_name(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        let (panel, cx) =
            cx.add_window_view(|window, cx| GitPanel::new(".", window, cx));
        panel.update_in(cx, |panel, window, cx| {
            panel.branch_create_open = true;
            panel.create_branch(window, cx);

            assert!(!panel.branch_create_pending);
            assert_eq!(panel.status.as_ref(), "Enter a branch name");
        });
    }

    #[gpui_kit::test]
    fn branch_deletion_is_blocked_for_dirty_repository_and_current_branch(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let (panel, cx) =
            cx.add_window_view(|window, cx| GitPanel::new(".", window, cx));
        panel.update(cx, |panel, cx| {
            panel.branch = "main".into();
            panel.branches = vec!["feature".into(), "main".into()];
            panel.repository_dirty = true;
            panel.delete_branch("feature".into(), cx);
            assert!(!panel.branch_delete_pending);
            assert_eq!(
                panel.status.as_ref(),
                "Commit or stash repository changes before deleting a branch"
            );

            panel.repository_dirty = false;
            panel.delete_branch("main".into(), cx);
            assert!(!panel.branch_delete_pending);
        });
    }

    #[gpui_kit::test]
    fn git_file_row_opens_path_relative_to_the_workspace(cx: &mut TestAppContext) {
        let workspace = tempfile::tempdir().expect("workspace");
        let expected_path = workspace.path().join("nested/file.rs");
        std::fs::create_dir_all(expected_path.parent().expect("parent"))
            .expect("nested folder");
        std::fs::write(&expected_path, "fn main() {}\n").expect("source");
        let root = workspace.path().to_string_lossy().into_owned();
        let opened_path = Rc::new(RefCell::new(None));
        let opened_path_from_handler = opened_path.clone();
        cx.update(gpui_kit::component::init);
        let (_view, cx) = cx.add_window_view(move |window, cx| {
            let panel = cx.new(|cx| GitPanel::new(&root, window, cx));
            panel.update(cx, |panel, _| {
                panel.files = vec![GitFile {
                    path: "nested/file.rs".into(),
                    state: "M".into(),
                    staged: false,
                }];
                panel.set_open_file_handler(move |path, _, _| {
                    *opened_path_from_handler.borrow_mut() = Some(path);
                });
            });
            NarrowGitPanel(panel)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        cx.update(|window, cx| window.click("git-open-file-0", cx));

        assert_eq!(*opened_path.borrow(), Some(expected_path));
    }

    #[gpui_kit::test]
    fn git_view_diff_opens_aggregate_worktree_patch(cx: &mut TestAppContext) {
        use ahead_rpc::proxy::{ProxyRequest, ProxyResponse, ProxyRpc};
        use gpui_kit::test::TestWindowExt;

        let workspace = tempfile::tempdir().expect("workspace");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            workspace.path().to_owned(),
        );
        let proxy_rpc = proxy.rpc_for_test();
        let root = workspace.path().to_string_lossy().into_owned();
        let proxy_for_panel = proxy.clone();
        cx.update(gpui_kit::component::init);
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let panel = cx.new(|cx| {
                GitPanel::new(&root, window, cx).with_proxy(proxy_for_panel)
            });
            panel.update(cx, |panel, cx| {
                panel.files = vec![GitFile {
                    path: "nested/file.rs".into(),
                    state: "M".into(),
                    staged: false,
                }];
                cx.notify();
            });
            NarrowGitPanel(panel)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        cx.update(|window, cx| window.click("git_diff", cx));

        let panel = cx.update(|_, cx| view.read(cx).0.clone());
        assert!(cx.update(|_, cx| panel.read(cx).show_history));
        assert_eq!(
            cx.update(|_, cx| {
                panel
                    .read(cx)
                    .selected_commit
                    .as_ref()
                    .map(|commit| commit.subject.clone())
            }),
            Some("Uncommitted changes · all files".into())
        );
        let ProxyRpc::Request(id, ProxyRequest::GitWorktreePatch {}) =
            proxy_rpc.rx().try_recv().expect("aggregate diff request")
        else {
            panic!("expected aggregate worktree diff request");
        };
        proxy_rpc.handle_response(
            id,
            Ok(ProxyResponse::GitWorktreePatch {
                patch: ahead_rpc::source_control::GitPatch {
                    id: "working-tree".into(),
                    patch: "diff --git a/one.rs b/one.rs\n+one\ndiff --git a/two.rs b/two.rs\n+two\n".into(),
                    truncated: false,
                },
            }),
        );
        cx.run_until_parked();
        assert!(cx.update(|_, cx| {
            panel
                .read(cx)
                .commit_patch_lines
                .iter()
                .any(|line| line.contains("one.rs"))
        }));
        assert!(cx.update(|_, cx| {
            panel
                .read(cx)
                .commit_patch_lines
                .iter()
                .any(|line| line.contains("two.rs"))
        }));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("git_commit_diff_back").is_some());
        assert!(cx.debug_bounds("git_commit_diff_copy").is_some());

        cx.update(|window, cx| window.click("git_commit_diff_back", cx));
        assert!(!cx.update(|_, cx| panel.read(cx).show_history));
    }

    #[gpui_kit::test]
    fn commit_requires_message_and_connected_service(cx: &mut TestAppContext) {
        let workspace = tempfile::tempdir().expect("workspace");
        let root = workspace.path().to_string_lossy().into_owned();
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            workspace.path().to_owned(),
        );
        cx.update(gpui_kit::component::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            NarrowGitPanel(cx.new(|cx| GitPanel::new(&root, window, cx)))
        });
        let panel = cx.update(|_, cx| view.read(cx).0.clone());
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.commit_staged(window, cx);
                assert_eq!(panel.status.as_ref(), "Enter a commit message");
                panel.commit_message.update(cx, |input, cx| {
                    input.set_value("Ready to commit", window, cx)
                });
                assert!(!panel.can_commit_staged(cx));
                panel.commit_staged(window, cx);
                assert_eq!(panel.status.as_ref(), "Git service unavailable");
                panel.stage_all(window, cx);
                assert_eq!(panel.status.as_ref(), "Git service unavailable");
                panel.proxy = Some(proxy);
                assert!(!panel.can_commit_staged(cx));
                panel.files.push(GitFile {
                    path: "staged.rs".into(),
                    state: "A".into(),
                    staged: true,
                });
                assert!(panel.can_commit_staged(cx));
                panel.repository_has_conflicts = true;
                assert!(!panel.can_commit_staged(cx));
                panel.commit_staged(window, cx);
                assert_eq!(
                    panel.status.as_ref(),
                    "Resolve and stage all repository conflicts before committing"
                );
                panel.repository_has_conflicts = false;
                panel.commit_pending = true;
                assert!(!panel.can_commit_staged(cx));
            });
        });
    }

    #[test]
    fn git_file_identifies_porcelain_conflicts() {
        for state in ["DD", "AU", "UD", "UA", "DU", "AA", "UU"] {
            assert!(
                GitFile {
                    path: "conflict.rs".into(),
                    state: state.into(),
                    staged: false,
                }
                .is_conflicted(),
                "{state} should be shown as a conflict"
            );
        }
        for state in ["M", "MM", "A", "??"] {
            assert!(
                !GitFile {
                    path: "ordinary.rs".into(),
                    state: state.into(),
                    staged: false,
                }
                .is_conflicted(),
                "{state} should not be shown as a conflict"
            );
        }
    }

    #[test]
    fn parses_recipe_names_and_descriptions() {
        let tasks = parse_just_list(
            "    build # Build the application\n    test <args> # Run tests\n",
            Path::new("/workspace/justfile"),
            Path::new("/workspace"),
        );

        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].name, "build");
        assert_eq!(
            tasks[0].description.as_deref(),
            Some("Build the application")
        );
        assert_eq!(tasks[1].name, "test");
        assert_eq!(tasks[1].description.as_deref(), Some("Run tests"));
    }

    #[gpui_kit::test]
    fn watch_task_button_dispatches_the_selected_recipe(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        let (panel, cx) =
            cx.add_window_view(|_, cx| JustTasksPanel::new("/workspace", cx));
        let dispatched = Rc::new(RefCell::new(None));
        panel.update(cx, |panel, cx| {
            panel.tasks.push(JustTask {
                justfile: Path::new("/workspace/justfile").into(),
                cwd: Path::new("/workspace").into(),
                name: "test".into(),
                description: Some("Run tests".into()),
            });
            let dispatched = dispatched.clone();
            panel.set_watch_handler(move |recipe, cwd, justfile, _, _| {
                *dispatched.borrow_mut() = Some((recipe, cwd, justfile));
            });
            cx.notify();
        });

        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert!(window.try_find("watch_just_task_0").is_some());
            window.click("watch_just_task_0", cx);
        });

        assert_eq!(
            *dispatched.borrow(),
            Some((
                "test".into(),
                "/workspace".into(),
                "/workspace/justfile".into()
            ))
        );
    }

    #[test]
    fn search_result_navigation_wraps_and_ignores_empty_lists() {
        assert_eq!(search_result_index_after_key(0, 3, "up"), Some(2));
        assert_eq!(search_result_index_after_key(2, 3, "down"), Some(0));
        assert_eq!(search_result_index_after_key(0, 3, "down"), Some(1));
        assert_eq!(search_result_index_after_key(0, 0, "down"), None);
        assert_eq!(search_result_index_after_key(0, 3, "enter"), None);
    }

    #[test]
    fn replace_match_advances_from_replacement_end_and_wraps() {
        let selected = SearchResult {
            path: "src/a.rs".into(),
            line: 2,
            end_line: 2,
            start: 4,
            end: 10,
            text: "    needle first needle".into(),
            preview_match: 4..10,
        };
        let location = search_location_after_replacement(&selected, "longer\ntext");
        assert_eq!(location, ("src/a.rs".into(), 2, 4));

        let results = vec![
            SearchResult {
                path: "src/a.rs".into(),
                line: 1,
                end_line: 1,
                start: 0,
                end: 6,
                text: "    needle".into(),
                preview_match: 0..6,
            },
            SearchResult {
                path: "src/a.rs".into(),
                line: 3,
                end_line: 3,
                start: 11,
                end: 17,
                text: "text first needle".into(),
                preview_match: 11..17,
            },
            SearchResult {
                path: "src/b.rs".into(),
                line: 1,
                end_line: 1,
                start: 0,
                end: 6,
                text: "needle".into(),
                preview_match: 0..6,
            },
        ];
        assert_eq!(next_search_result_index(&results, &location), 1);
        assert_eq!(
            next_search_result_index(&results, &("src/z.rs".into(), 0, 0)),
            0
        );
    }

    #[test]
    fn search_result_virtual_rows_group_paths_and_map_keyboard_selection() {
        let results = vec![
            SearchResult {
                path: "src/a.rs".into(),
                line: 1,
                end_line: 1,
                start: 0,
                end: 1,
                text: "a".into(),
                preview_match: 0..1,
            },
            SearchResult {
                path: "src/a.rs".into(),
                line: 3,
                end_line: 3,
                start: 0,
                end: 1,
                text: "a".into(),
                preview_match: 0..1,
            },
            SearchResult {
                path: "src/b.rs".into(),
                line: 2,
                end_line: 2,
                start: 0,
                end: 1,
                text: "b".into(),
                preview_match: 0..1,
            },
        ];

        assert_eq!(
            search_display_rows(&results, 3),
            vec![
                SearchDisplayRow::FileHeader { result_index: 0 },
                SearchDisplayRow::Match { result_index: 0 },
                SearchDisplayRow::Match { result_index: 1 },
                SearchDisplayRow::FileHeader { result_index: 2 },
                SearchDisplayRow::Match { result_index: 2 },
            ]
        );
        assert_eq!(search_display_rows(&results, 1).len(), 2);
        assert_eq!(search_display_row_for_result(&results, 0), Some(1));
        assert_eq!(search_display_row_for_result(&results, 1), Some(2));
        assert_eq!(search_display_row_for_result(&results, 2), Some(4));
        assert_eq!(search_display_row_for_result(&results, 3), None);
    }

    #[test]
    fn search_result_pagination_advances_without_exceeding_results() {
        assert_eq!(next_search_result_page_size(100, 250), 200);
        assert_eq!(next_search_result_page_size(200, 250), 250);
        assert_eq!(next_search_result_page_size(100, 80), 80);
        assert_eq!(
            next_search_result_page_size(usize::MAX, usize::MAX),
            usize::MAX
        );
    }

    #[test]
    fn search_preview_highlights_utf8_byte_ranges() {
        assert_eq!(
            search_preview_segments("é needle", 3..9),
            vec![(false, "é ".to_string()), (true, "needle".to_string()),]
        );
        assert_eq!(
            search_preview_segments("é needle", 1..2),
            vec![(false, "é needle".to_string())]
        );
    }
}
