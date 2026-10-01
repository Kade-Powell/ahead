//! Workspace activity bar and editor-side utility panels.

use std::path::{Path, PathBuf};
use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, DockArea, DockLayout, DockPlacement, Panel, PanelControl, PanelEvent,
    PanelId, TabGroup, panel_handle,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, Selectable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit_assets::IconName;

use crate::code_panel::CodePanel;
use crate::explorer_panel::ExplorerPanel;
use crate::proxy_client::ProxyClient;

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
                WorkspaceView::Explorer => panel_handle(explorer),
                WorkspaceView::Git => panel_handle(git),
                WorkspaceView::Tasks => panel_handle(tasks),
                WorkspaceView::LanguageServers => panel_handle(language_servers),
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
    run_handler: Option<Rc<dyn Fn(String, String, &mut Window, &mut App)>>,
}

impl JustTasksPanel {
    pub fn new(root: &str, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            root: PathBuf::from(root),
            tasks: Vec::new(),
            status: "Looking for justfiles…".into(),
            run_handler: None,
        }
    }

    pub fn set_run_handler<F>(&mut self, handler: F)
    where
        F: Fn(String, String, &mut Window, &mut App) + 'static,
    {
        self.run_handler = Some(Rc::new(handler));
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
                            v_flex()
                                .min_w_0()
                                .child(
                                    div()
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
    pub files: Vec<GitFile>,
    pub commit_message: Entity<InputState>,
    pub status: SharedString,
}

#[derive(Clone, Debug)]
pub struct GitFile {
    pub path: String,
    pub state: String,
    pub selected: bool,
}

impl GitPanel {
    pub fn new(root: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let commit_message = cx.new(|cx| InputState::new(window, cx));
        let mut panel = Self {
            focus: cx.focus_handle(),
            root: root.to_string(),
            branch: String::new(),
            files: Vec::new(),
            commit_message,
            status: "Source control ready".into(),
        };
        panel.refresh();
        panel
    }

    pub fn refresh(&mut self) {
        self.branch = git_output(&self.root, ["branch", "--show-current"])
            .unwrap_or_else(|| "HEAD".to_string());
        self.files = git_output(&self.root, ["status", "--porcelain", "-uall"])
            .map(|output| output.lines().filter_map(parse_git_file).collect())
            .unwrap_or_default();
        self.status = format!(
            "{} change{}",
            self.files.len(),
            if self.files.len() == 1 { "" } else { "s" }
        )
        .into();
    }

    fn stage_all(&mut self, cx: &mut Context<Self>) {
        let result = std::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(&self.root)
            .output();
        self.status = if result
            .map(|output| output.status.success())
            .unwrap_or(false)
        {
            "All changes staged".into()
        } else {
            "Could not stage changes".into()
        };
        self.refresh();
        cx.notify();
    }
}

fn git_output<const N: usize>(root: &str, args: [&str; N]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn parse_git_file(line: &str) -> Option<GitFile> {
    if line.len() < 4 {
        return None;
    }
    let state = line[..2].trim().to_string();
    let path = line[3..].trim().trim_matches('"').to_string();
    (!path.is_empty()).then_some(GitFile {
        path,
        state,
        selected: false,
    })
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
                            .primary(),
                    )
                    .child(Button::new("git_history").label("History").ghost()),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Button::new("git_diff")
                            .icon(IconName::GitCompare)
                            .label("View Diff")
                            .ghost(),
                    )
                    .child(
                        Button::new("git_stage_all")
                            .icon(IconName::Plus)
                            .label("Stage All")
                            .ghost()
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.stage_all(cx)
                            })),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(IconName::GitBranch)
                    .child(div().text_color(text).child(self.branch.clone()))
                    .child(
                        Button::new("git_publish")
                            .icon(IconName::Upload)
                            .label("Publish")
                            .ghost(),
                    ),
            )
            .child(v_flex().flex_1().min_h_0().overflow_y_scrollbar().children(
                self.files.iter().enumerate().map(|(index, file)| {
                    let icon = if file.state.contains('?') {
                        IconName::FilePlus
                    } else {
                        IconName::FileCode
                    };
                    h_flex()
                        .id(("git_file", index))
                        .debug_selector(move || format!("git-file-{index}").into())
                        .w_full()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .child(icon)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis_start()
                                .text_color(text)
                                .child(file.path.clone()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .debug_selector(move || {
                                    format!("git-state-{index}").into()
                                })
                                .text_color(muted)
                                .child(file.state.clone()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .debug_selector(move || {
                                    format!("git-select-{index}").into()
                                })
                                .child(
                                    Button::new(("git_select", index))
                                        .icon(if file.selected {
                                            IconName::SquareCheck
                                        } else {
                                            IconName::Square
                                        })
                                        .ghost(),
                                ),
                        )
                }),
            ))
            .child(Input::new(&self.commit_message).aria_label("Commit message"))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_color(muted)
                            .text_size(px(11.))
                            .child(self.status.clone()),
                    )
                    .child(
                        Button::new("git_commit")
                            .icon(IconName::Check)
                            .label("Commit Tracked")
                            .primary(),
                    ),
            )
            .child(div().text_size(px(10.)).text_color(muted).child(root))
    }
}

pub struct SearchPanel {
    pub focus: FocusHandle,
    pub root: String,
    pub query: Entity<InputState>,
    pub include_glob: Entity<InputState>,
    pub exclude_glob: Entity<InputState>,
    case_sensitive: bool,
    whole_word: bool,
    is_regex: bool,
    pub results: Vec<SearchResult>,
    selected_result_index: usize,
    pub mailbox_id: usize,
    proxy: Option<Arc<ProxyClient>>,
    cached_paths: Option<(u64, Arc<Vec<PathBuf>>)>,
    active_workspace_file_request: Option<u64>,
    _workspace_changes: Option<Task<()>>,
    buffers: Vec<Entity<CodePanel>>,
    buffer_observers: Vec<Subscription>,
    last_buffer_states: Vec<(String, usize, usize)>,
    search_generation: Arc<AtomicU64>,
    debounced_search: Option<Task<()>>,
    searching: bool,
    search_limit_reached: bool,
    search_error: Option<String>,
    close_handler: Option<Rc<dyn Fn(PanelId, &mut Window, &mut App)>>,
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

impl SearchPanel {
    pub fn new(
        root: &str,
        mailbox_id: usize,
        code: Entity<CodePanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx));
        let include_glob = cx.new(|cx| InputState::new(window, cx));
        let exclude_glob = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe(&query, |this: &mut Self, _state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.refresh(cx);
            }
        })
        .detach();
        for input in [&include_glob, &exclude_glob] {
            cx.subscribe(
                input,
                |this: &mut Self, _state, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.refresh(cx);
                    }
                },
            )
            .detach();
        }
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
            include_glob,
            exclude_glob,
            case_sensitive: false,
            whole_word: false,
            is_regex: false,
            results: Vec::new(),
            selected_result_index: 0,
            mailbox_id,
            proxy,
            cached_paths: None,
            active_workspace_file_request: None,
            _workspace_changes: workspace_changes,
            buffers: Vec::new(),
            buffer_observers: Vec::new(),
            last_buffer_states: Vec::new(),
            search_generation: Arc::new(AtomicU64::new(0)),
            debounced_search: None,
            searching: false,
            search_limit_reached: false,
            search_error: None,
            close_handler: None,
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

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().to_string();
        let include_glob = self.include_glob.read(cx).value().to_string();
        let exclude_glob = self.exclude_glob.read(cx).value().to_string();
        let case_sensitive = self.case_sensitive;
        let whole_word = self.whole_word;
        let is_regex = self.is_regex;
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
        if let Some(request_id) = self.active_workspace_file_request.take()
            && let Some(proxy) = &self.proxy
        {
            proxy.cancel_workspace_files(request_id);
        }
        if !has_query {
            self.debounced_search = None;
            self.results.clear();
            self.selected_result_index = 0;
            self.searching = false;
            self.search_limit_reached = false;
            self.search_error = None;
            cx.notify();
            return;
        }

        self.results.clear();
        self.selected_result_index = 0;
        self.searching = true;
        self.search_limit_reached = false;
        self.search_error = None;
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
                    max_results: 500,
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
                this.update(cx, |panel, cx| {
                    let selected = panel.results.get(panel.selected_result_index).map(
                        |result| (result.path.clone(), result.line, result.start),
                    );
                    panel.results.extend(matches.into_iter().map(|matched| {
                        SearchResult {
                            path: path.to_string_lossy().to_string(),
                            line: matched.line + 1,
                            end_line: matched.end_line + 1,
                            start: matched.start,
                            end: matched.end,
                            text: matched.line_content.clone(),
                            preview_match: matched.preview_match.clone(),
                        }
                    }));
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
                    match result {
                        Ok(limit_reached) => {
                            panel.search_limit_reached = limit_reached;
                        }
                        Err(ahead_core::search::FileSearchError::Cancelled) => {}
                        Err(error) => panel.search_error = Some(error.to_string()),
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
            if let Some(result) = self.results.get(self.selected_result_index) {
                self.open_result(result, window, cx);
            }
            return;
        }
        if let Some(index) = search_result_index_after_key(
            self.selected_result_index,
            self.results.len(),
            key,
        ) {
            self.selected_result_index = index;
            cx.notify();
        }
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

fn highlighted_search_preview(
    text: &str,
    preview_match: Range<usize>,
    foreground: Hsla,
    accent: Hsla,
) -> AnyElement {
    h_flex()
        .flex_1()
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
        let mut grouped: BTreeMap<String, Vec<&SearchResult>> = BTreeMap::new();
        for result in &self.results {
            grouped.entry(result.path.clone()).or_default().push(result);
        }
        let mut first_result_index = 0;
        let grouped = grouped
            .into_iter()
            .map(|(path, matches)| {
                let first_index = first_result_index;
                first_result_index += matches.len();
                (path, matches, first_index)
            })
            .collect::<Vec<_>>();
        let selected_result_index = self.selected_result_index;
        v_flex()
            .size_full()
            .gap_1()
            .p_2()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::handle_result_key_down))
            .child(Input::new(&self.query).aria_label("Search workspace"))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("search_match_case")
                            .label("Case")
                            .ghost()
                            .selected(self.case_sensitive)
                            .tooltip("Match case")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.case_sensitive = !this.case_sensitive;
                                this.refresh(cx);
                            })),
                    )
                    .child(
                        Button::new("search_whole_word")
                            .label("Word")
                            .ghost()
                            .selected(self.whole_word)
                            .tooltip("Match whole words")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.whole_word = !this.whole_word;
                                this.refresh(cx);
                            })),
                    )
                    .child(
                        Button::new("search_regex")
                            .label("Regex")
                            .ghost()
                            .selected(self.is_regex)
                            .tooltip("Match with regular expression")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.is_regex = !this.is_regex;
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
            .child(div().px_1().text_size(px(11.)).text_color(muted).child(
                if query.is_empty() {
                    "Search workspace".to_string()
                } else if let Some(error) = &self.search_error {
                    error.clone()
                } else if self.searching {
                    format!("Searching… {} matches", self.results.len())
                } else if self.search_limit_reached {
                    format!("{} results (limit reached)", self.results.len())
                } else {
                    format!(
                        "{} result{}",
                        self.results.len(),
                        if self.results.len() == 1 { "" } else { "s" }
                    )
                },
            ))
            .child(v_flex().flex_1().min_h_0().overflow_y_scrollbar().children(
                grouped.into_iter().map(|(path, matches, first_index)| {
                    let rows = matches
                        .into_iter()
                        .enumerate()
                        .map(|(match_index, result)| {
                            let result_index = first_index + match_index;
                            let click_result = result.clone();
                            let location_label = if result.line == result.end_line {
                                result.line.to_string()
                            } else {
                                format!("{}–{}", result.line, result.end_line)
                            };
                            ListItem::new(result_index)
                                .w_full()
                                .selected(result_index == selected_result_index)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.selected_result_index = result_index;
                                    this.open_result(&click_result, window, cx);
                                }))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .pl_4()
                                        .child(
                                            div()
                                                .w(px(56.))
                                                .text_color(muted)
                                                .child(location_label),
                                        )
                                        .child(highlighted_search_preview(
                                            &result.text,
                                            result.preview_match.clone(),
                                            text,
                                            accent,
                                        )),
                                )
                        })
                        .collect::<Vec<_>>();
                    v_flex()
                        .gap_1()
                        .py_1()
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .child(IconName::FileCode)
                                .child(div().text_color(text).child(path.clone())),
                        )
                        .children(rows)
                }),
            ))
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
    pub root: String,
    proxy: Arc<ProxyClient>,
    _updates: Task<()>,
}

impl LanguageServersPanel {
    pub fn new(root: &str, proxy: Arc<ProxyClient>, cx: &mut Context<Self>) -> Self {
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
            root: root.to_string(),
            proxy,
            _updates: task,
        }
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
        v_flex()
            .size_full()
            .gap_2()
            .p_3()
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(IconName::Zap)
                    .child(div().flex_1().text_color(text).child("Language Servers"))
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
            .child(div().text_color(muted).child("Workspace language services"))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .p_2()
                    .bg(cx.theme().group_box)
                    .child(IconName::Activity)
                    .child(
                        v_flex()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .text_color(text)
                                    .child("Managed by ahead-proxy"),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(11.))
                                    .text_color(muted)
                                    .child(self.root.clone()),
                            ),
                    ),
            )
            .children(servers.iter().map(|server| {
                let (icon, state, color) = if server.is_ready() {
                    (IconName::CircleCheck, "Ready", cx.theme().success)
                } else if server.message.is_some() {
                    (IconName::CircleX, "Error", cx.theme().danger)
                } else {
                    (IconName::CircleDashed, "Starting", cx.theme().warning)
                };
                let name = server
                    .name
                    .strip_prefix("lsp-")
                    .map(|language| format!("{language} language server"))
                    .unwrap_or_else(|| server.name.clone());
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
                                        .truncate()
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
            }))
            .when(servers.is_empty(), |el| {
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
        GitFile, GitPanel, parse_just_list, search_preview_segments,
        search_result_index_after_key,
    };
    use gpui_kit::prelude::*;
    use gpui_kit::{Entity, Render, TestAppContext, Window, div, px};
    use std::path::Path;

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
    fn narrow_git_row_keeps_status_and_selection_visible(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            NarrowGitPanel(cx.new(|cx| GitPanel::new(".", window, cx)))
        });
        let panel = cx.update(|_, cx| view.read(cx).0.clone());
        panel.update(cx, |panel, cx| {
            panel.files = vec![GitFile {
                path: "deeply/nested/path/to/a/long/descriptive/filename.rs".into(),
                state: "??".into(),
                selected: false,
            }];
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let row = cx.debug_bounds("git-file-0").expect("Git row");
        let state = cx.debug_bounds("git-state-0").expect("Git status");
        let select = cx.debug_bounds("git-select-0").expect("Git selection");
        assert!(row.size.width <= px(260.));
        assert!(state.origin.x + state.size.width <= select.origin.x);
        assert!(
            select.origin.x + select.size.width <= row.origin.x + row.size.width
        );
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

    #[test]
    fn search_result_navigation_wraps_and_ignores_empty_lists() {
        assert_eq!(search_result_index_after_key(0, 3, "up"), Some(2));
        assert_eq!(search_result_index_after_key(2, 3, "down"), Some(0));
        assert_eq!(search_result_index_after_key(0, 3, "down"), Some(1));
        assert_eq!(search_result_index_after_key(0, 0, "down"), None);
        assert_eq!(search_result_index_after_key(0, 3, "enter"), None);
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
