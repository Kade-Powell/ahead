//! Workspace activity bar and editor-side utility panels.

use std::path::Path;
use std::{collections::BTreeMap, rc::Rc, sync::Arc};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, DockArea, DockLayout, DockPlacement, Panel, PanelControl, PanelEvent,
    PanelId, TabGroup, panel_handle,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
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
    LanguageServers,
}

pub struct ActivityBar {
    area: Entity<DockArea>,
    explorer: Entity<ExplorerPanel>,
    git: Entity<GitPanel>,
    language_servers: Entity<LanguageServersPanel>,
    active: WorkspaceView,
}

impl ActivityBar {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        area: Entity<DockArea>,
        explorer: Entity<ExplorerPanel>,
        git: Entity<GitPanel>,
        language_servers: Entity<LanguageServersPanel>,
    ) -> Self {
        Self {
            area,
            explorer,
            git,
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
        let language_servers = self.language_servers.clone();
        window.defer(cx, move |window, cx| {
            let panel = match view {
                WorkspaceView::Explorer => panel_handle(explorer),
                WorkspaceView::Git => panel_handle(git),
                WorkspaceView::LanguageServers => panel_handle(language_servers),
            };
            area.update(cx, |area, cx| {
                area.set_dock(
                    DockPlacement::Left,
                    DockLayout::tabs().panel_view(panel, cx),
                    window,
                    cx,
                );
                area.set_dock_size(DockPlacement::Left, px(380.), window, cx);
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
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .child(icon)
                        .child(
                            div().flex_1().text_color(text).child(file.path.clone()),
                        )
                        .child(div().text_color(muted).child(file.state.clone()))
                        .child(
                            Button::new(("git_select", index))
                                .icon(if file.selected {
                                    IconName::SquareCheck
                                } else {
                                    IconName::Square
                                })
                                .ghost(),
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
    pub results: Vec<SearchResult>,
    pub mailbox_id: usize,
    close_handler: Option<Rc<dyn Fn(PanelId, &mut Window, &mut App)>>,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
}

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub path: String,
    pub line: usize,
    pub text: String,
}

impl SearchPanel {
    pub fn new(
        root: &str,
        mailbox_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe(&query, |this: &mut Self, _state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.refresh(cx);
            }
        })
        .detach();
        let mut panel = Self {
            focus: cx.focus_handle(),
            root: root.to_string(),
            query,
            results: Vec::new(),
            mailbox_id,
            close_handler: None,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
        };
        panel.refresh(cx);
        panel
    }

    pub fn set_close_handler<F>(&mut self, handler: F)
    where
        F: Fn(PanelId, &mut Window, &mut App) + 'static,
    {
        self.close_handler = Some(Rc::new(handler));
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().to_string();
        self.results = if query.trim().is_empty() {
            Vec::new()
        } else {
            search_files(Path::new(&self.root), &query)
        };
        cx.notify();
    }
}

fn search_files(root: &Path, query: &str) -> Vec<SearchResult> {
    let query = query.to_lowercase();
    let mut results = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (line_index, line) in contents.lines().enumerate() {
                if line.to_lowercase().contains(&query) {
                    results.push(SearchResult {
                        path: path.to_string_lossy().to_string(),
                        line: line_index + 1,
                        text: line.trim().to_string(),
                    });
                    if results.len() >= 500 {
                        return results;
                    }
                }
            }
        }
    }
    results.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    results
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
                    let can_close = group.as_ref().is_some_and(|group| {
                        group
                            .read_with(cx, |group, cx| {
                                group.context(cx).is_draggable()
                            })
                            .unwrap_or(false)
                    });
                    if can_close {
                        if let Some(group) = group.as_ref() {
                            _ = group.update(cx, |group, cx| {
                                group.close_panel(panel_id, cx)
                            });
                        }
                    } else if let Some(handler) = close_handler.as_ref() {
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
        let query = self.query.read(cx).value().to_string();
        let mailbox_id = self.mailbox_id;
        let mut grouped: BTreeMap<String, Vec<&SearchResult>> = BTreeMap::new();
        for result in &self.results {
            grouped.entry(result.path.clone()).or_default().push(result);
        }
        v_flex()
            .size_full()
            .gap_1()
            .p_2()
            .track_focus(&self.focus)
            .child(Input::new(&self.query).aria_label("Search workspace"))
            .child(div().px_1().text_size(px(11.)).text_color(muted).child(
                if query.is_empty() {
                    "Search buffers".to_string()
                } else {
                    format!(
                        "{} result{}",
                        self.results.len(),
                        if self.results.len() == 1 { "" } else { "s" }
                    )
                },
            ))
            .child(v_flex().flex_1().min_h_0().overflow_y_scrollbar().children(
                grouped.into_iter().map(|(path, matches)| {
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
                        .children(matches.into_iter().enumerate().map(
                            |(match_index, result)| {
                                let target = result.path.clone();
                                let click_target = target.clone();
                                h_flex()
                                    .id(("search_result", match_index))
                                    .gap_2()
                                    .pl_4()
                                    .cursor(CursorStyle::PointingHand)
                                    .on_click(cx.listener(
                                        move |_: &mut Self, _, window, cx| {
                                            crate::ross::request_open(
                                                mailbox_id,
                                                &click_target,
                                            );
                                            cx.focus_self(window);
                                        },
                                    ))
                                    .child(
                                        div()
                                            .w(px(34.))
                                            .text_color(muted)
                                            .child(result.line.to_string()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_color(text)
                                            .child(result.text.clone()),
                                    )
                            },
                        ))
                }),
            ))
    }
}

pub struct ProblemsPanel {
    pub focus: FocusHandle,
    pub code: Entity<CodePanel>,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
}

impl ProblemsPanel {
    pub fn new(code: Entity<CodePanel>, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            code,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
        }
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
        h_flex().items_center().gap_1().child("Problems").child(
            Button::new("close_problems_panel")
                .icon(IconName::X)
                .ghost()
                .tooltip("Close Problems")
                .on_click(move |_, _, cx| {
                    if let Some(group) = group.as_ref() {
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
}

impl LanguageServersPanel {
    pub fn new(root: &str, proxy: Arc<ProxyClient>, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            root: root.to_string(),
            proxy,
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
                    .child(div().text_color(text).child("Language Servers")),
            )
            .child(div().text_color(muted).child("Workspace language services"))
            .child(
                h_flex()
                    .gap_2()
                    .p_2()
                    .bg(cx.theme().group_box)
                    .child(IconName::Activity)
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_color(text)
                                    .child("Managed by ahead-proxy"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(muted)
                                    .child(self.root.clone()),
                            ),
                    ),
            )
            .children(servers.iter().map(|server| {
                let (icon, state, color) = if server.is_ready() {
                    (IconName::CircleCheck, "Ready", gpui_kit::rgb(0x34D399))
                } else if server.message.is_some() {
                    (IconName::CircleX, "Error", gpui_kit::rgb(0xF87171))
                } else {
                    (IconName::CircleDashed, "Starting", gpui_kit::rgb(0xFBBF24))
                };
                let name = server
                    .name
                    .strip_prefix("lsp-")
                    .map(|language| format!("{language} language server"))
                    .unwrap_or_else(|| server.name.clone());
                h_flex()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .bg(cx.theme().group_box)
                    .child(div().text_color(color).child(icon))
                    .child(div().flex_1().text_color(text).child(name))
                    .child(div().text_size(px(11.)).text_color(color).child(state))
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
