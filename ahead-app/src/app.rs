//! AHEAD Application Shell - GPUI Application Lifecycle
//!
//! 4-column Zed-style layout:
//! - Left dock: File explorer (toggleable via ⌘B or bottom icon)
//! - Center: Code editor and settings tabs
//! - Bottom dock: interactive PTY terminal and Zed-style debugger tabs (toggleable via ⌃~ / ⌘J)
//! - Right dock: AHEAD Agent conversation and Threads rail together (toggleable via ⌘R or bottom icon)
//! - Top: TitleBar chrome with brand and native traffic lights
//! - Bottom: StatusBar with shortcut tooltips, branch, dock toggles, and metadata

use gpui_kit::component::TitleBar;
use gpui_kit::component::button::Button;
use gpui_kit::component::dock::{
    BasePanel, DockArea, DockLayout, DockPlacement, DockSkin, Panel, PanelControl,
    PanelEvent, PanelStyle, panel_handle,
};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit_assets::IconName;

use crate::workspace_panels::{
    ActivityBar, GitPanel, LanguageServersPanel, ProblemsPanel, SearchPanel,
    WorkspaceView,
};

pub struct AgentWorkspacePanel {
    focus: FocusHandle,
    session: Entity<crate::session_panel::SessionPanel>,
    threads: Entity<crate::threads_panel::ThreadsPanel>,
    threads_visible: bool,
    chat_zoomed: bool,
    threads_width: Pixels,
    resize_origin: Option<(Pixels, Pixels)>,
}

impl AgentWorkspacePanel {
    pub fn new(
        session: Entity<crate::session_panel::SessionPanel>,
        threads: Entity<crate::threads_panel::ThreadsPanel>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            session,
            threads,
            threads_visible: true,
            chat_zoomed: false,
            threads_width: px(220.),
            resize_origin: None,
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
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if let Some((origin_x, origin_width)) = this.resize_origin {
                    this.threads_width = (origin_width + origin_x
                        - event.position.x)
                        .max(px(160.))
                        .min(px(520.));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.resize_origin = None;
                    cx.notify();
                }),
            )
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
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(
                                    |this, event: &MouseDownEvent, _, cx| {
                                        this.resize_origin = Some((
                                            event.position.x,
                                            this.threads_width,
                                        ));
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

pub struct Shell {
    pub area: Entity<DockArea>,
    pub focus: FocusHandle,
    pub branch: String,
    pub session: Entity<crate::session_panel::SessionPanel>,
    pub threads: Entity<crate::threads_panel::ThreadsPanel>,
    pub code: Entity<crate::code_panel::CodePanel>,
    pub code_tabs: Vec<Entity<crate::code_panel::CodePanel>>,
    pub explorer: Entity<crate::explorer_panel::ExplorerPanel>,
    pub debug_bar: Entity<crate::debug_bar::DebugBar>,
    pub terminals: Vec<Entity<crate::terminal_panel::TerminalPanel>>,
    pub problems: Entity<ProblemsPanel>,
    pub settings: Entity<crate::settings_panel::SettingsPanel>,
    pub search: Entity<SearchPanel>,
    pub agent_workspace: Entity<AgentWorkspacePanel>,
    pub activity: Entity<ActivityBar>,
    pub threads_visible: bool,
    pub chat_zoomed: bool,
    left_dock_was_open: bool,
    right_dock_was_open: bool,
    bottom_dock_was_open: bool,
    bottom_debug_active: bool,
    debug_visible: bool,
    active_terminal: usize,
    next_terminal_id: usize,
    settings_active: bool,
    search_active: bool,
    problems_active: bool,
}

impl Shell {
    pub fn new(
        area: Entity<DockArea>,
        branch: &str,
        session: Entity<crate::session_panel::SessionPanel>,
        threads: Entity<crate::threads_panel::ThreadsPanel>,
        code: Entity<crate::code_panel::CodePanel>,
        explorer: Entity<crate::explorer_panel::ExplorerPanel>,
        debug_bar: Entity<crate::debug_bar::DebugBar>,
        terminals: Vec<Entity<crate::terminal_panel::TerminalPanel>>,
        problems: Entity<ProblemsPanel>,
        settings: Entity<crate::settings_panel::SettingsPanel>,
        search: Entity<SearchPanel>,
        agent_workspace: Entity<AgentWorkspacePanel>,
        activity: Entity<ActivityBar>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            area,
            focus: cx.focus_handle(),
            branch: branch.to_string(),
            session,
            threads,
            code_tabs: vec![code.clone()],
            code,
            explorer,
            debug_bar,
            terminals,
            problems,
            settings,
            search,
            agent_workspace,
            activity,
            threads_visible: true,
            chat_zoomed: false,
            left_dock_was_open: true,
            right_dock_was_open: true,
            bottom_dock_was_open: true,
            bottom_debug_active: false,
            debug_visible: true,
            active_terminal: 0,
            next_terminal_id: 2,
            settings_active: false,
            search_active: false,
            problems_active: false,
        }
    }

    fn set_active_code(
        &mut self,
        code: Entity<crate::code_panel::CodePanel>,
        cx: &mut Context<Self>,
    ) {
        self.code = code.clone();
        self.session.update(cx, |session, _| {
            session.code = Some(code.clone());
        });
        self.problems.update(cx, |problems, _| {
            problems.code = code;
        });
    }

    fn configure_code(
        &mut self,
        code: Entity<crate::code_panel::CodePanel>,
        cx: &mut Context<Self>,
    ) {
        let shell = cx.entity().downgrade();
        let shell_for_close = shell.clone();
        let shell_for_tabs = shell;
        code.update(cx, |code, _| {
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
        });
    }

    fn set_center_layout(
        &mut self,
        settings_active: bool,
        search_active: bool,
        problems_active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings_active = settings_active;
        self.search_active = search_active;
        self.problems_active = problems_active;
        let settings = self.settings.clone();
        let search = self.search.clone();
        let problems = self.problems.clone();
        let code_tabs = self.code_tabs.clone();
        let code_count = code_tabs.len();
        let active_code_id = self.code.entity_id();
        let active_code_index = self
            .code_tabs
            .iter()
            .position(|code| code.entity_id() == active_code_id)
            .unwrap_or(0);
        self.area.update(cx, |area, cx| {
            let mut editor_tabs = DockLayout::tabs();
            for code in code_tabs {
                editor_tabs = editor_tabs.panel_view(panel_handle(code), cx);
            }
            let editor_tabs = if settings_active {
                editor_tabs.panel_view(panel_handle(settings), cx)
            } else if search_active {
                editor_tabs.panel_view(panel_handle(search), cx)
            } else if problems_active {
                editor_tabs.panel_view(panel_handle(problems), cx)
            } else {
                editor_tabs
            };
            let active_index = if settings_active || search_active || problems_active
            {
                code_count
            } else {
                active_code_index
            };
            let editor_tabs = editor_tabs.active_index(active_index);
            area.set_center(editor_tabs, window, cx);
        });
        cx.notify();
    }

    fn close_center_panel(
        &mut self,
        _: gpui_kit::component::dock::PanelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_center_view(window, cx);
    }

    fn close_center_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_active = false;
        self.search_active = false;
        self.problems_active = false;
        self.set_center_layout(false, false, false, window, cx);
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

    fn open_file_request(
        &mut self,
        request: crate::ross::OpenRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(code) = self
            .code_tabs
            .iter()
            .find(|code| code.read(cx).file_path == request.path)
            .cloned()
        {
            if request.permanent {
                code.update(cx, |code, cx| code.promote_preview(cx));
            }
            self.set_active_code(code.clone(), cx);
            self.set_center_layout(false, false, false, window, cx);
            let focus = code.read(cx).focus.clone();
            window.focus(&focus, cx);
            return;
        }

        let code = if request.permanent {
            None
        } else {
            self.code_tabs
                .iter()
                .position(|code| code.read(cx).is_preview)
        };
        let code = if let Some(index) = code {
            let code = self.code_tabs[index].clone();
            code.update(cx, |code, cx| {
                code.open_file(&request.path, true, window, cx);
            });
            code
        } else {
            let (proxy, workspace) = self.code.read_with(cx, |code, _| {
                (code.proxy.clone(), code.workspace.clone())
            });
            let Some(proxy) = proxy else {
                return;
            };
            let code = cx.new(|cx| {
                crate::code_panel::CodePanel::new(&request.path, window, cx)
                    .with_proxy(proxy, &workspace)
            });
            code.update(cx, |code, cx| {
                code.is_preview = !request.permanent;
                cx.notify();
            });
            self.configure_code(code.clone(), cx);
            self.code_tabs.push(code.clone());
            code
        };

        self.set_active_code(code.clone(), cx);
        self.set_center_layout(false, false, false, window, cx);
        let focus = code.read(cx).focus.clone();
        window.focus(&focus, cx);
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

        if matches!(action, crate::code_panel::CodeTabAction::Promote) {
            let code = self.code_tabs[index].clone();
            code.update(cx, |code, cx| code.promote_preview(cx));
            self.set_active_code(code, cx);
            self.set_center_layout(false, false, false, window, cx);
            return;
        }

        let target_id = self.code_tabs[index].entity_id();
        match action {
            crate::code_panel::CodeTabAction::Close => {
                self.code_tabs.remove(index);
            }
            crate::code_panel::CodeTabAction::CloseOthers => {
                self.code_tabs.retain(|code| code.entity_id() == target_id);
            }
            crate::code_panel::CodeTabAction::CloseLeft => {
                self.code_tabs.drain(..index);
            }
            crate::code_panel::CodeTabAction::CloseRight => {
                self.code_tabs.truncate(index + 1);
            }
            crate::code_panel::CodeTabAction::CloseAll => self.code_tabs.clear(),
            crate::code_panel::CodeTabAction::Promote => unreachable!(),
        }

        let active_code = if matches!(
            action,
            crate::code_panel::CodeTabAction::CloseOthers
                | crate::code_panel::CodeTabAction::CloseLeft
                | crate::code_panel::CodeTabAction::CloseRight
        ) {
            self.code_tabs
                .iter()
                .find(|code| code.entity_id() == target_id)
                .cloned()
        } else if action == crate::code_panel::CodeTabAction::Close {
            self.code_tabs
                .get(index.min(self.code_tabs.len().saturating_sub(1)))
                .cloned()
        } else {
            None
        };
        if let Some(code) = active_code {
            self.set_active_code(code, cx);
        } else if self.code_tabs.is_empty() {
            self.session.update(cx, |session, _| session.code = None);
        }
        self.set_center_layout(false, false, false, window, cx);
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
            area.set_dock_size(DockPlacement::Bottom, px(320.), window, cx);
        });
    }

    fn close_bottom_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bottom_debug_active = false;
        self.area.update(cx, |area, cx| {
            area.remove_dock(DockPlacement::Bottom, window, cx);
        });
    }

    fn show_debug(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.debug_visible = true;
        self.set_bottom_layout(true, window, cx);
        self.area.update(cx, |area, cx| {
            if !area.is_dock_open(DockPlacement::Bottom) {
                area.toggle_dock(DockPlacement::Bottom, window, cx);
            }
        });
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
    }

    fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let terminal_id = self.next_terminal_id;
        self.next_terminal_id += 1;
        let terminal = cx.new(|cx| {
            crate::terminal_panel::TerminalPanel::new_with_id(terminal_id, cx)
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
    }

    fn close_terminal(
        &mut self,
        terminal_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .terminals
            .iter()
            .position(|terminal| terminal.read(cx).terminal_id() == terminal_id)
        else {
            return;
        };
        self.terminals.remove(index);
        if self.active_terminal > index {
            self.active_terminal -= 1;
        }
        if self.terminals.is_empty() {
            if self.debug_visible {
                self.set_bottom_layout(true, window, cx);
            } else {
                self.close_bottom_panel(window, cx);
            }
        } else {
            self.set_bottom_layout(self.bottom_debug_active, window, cx);
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
        terminal.update(cx, |terminal, _| {
            let shell_for_close = shell.clone();
            terminal.set_close_handler(move |window, cx| {
                _ = shell_for_close.update(cx, |shell, cx| {
                    shell.close_terminal(terminal_id, window, cx);
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
            self.set_center_layout(false, false, false, window, cx);
            self.restore_side_docks(window, cx);
        } else {
            self.left_dock_was_open =
                self.area.read(cx).is_dock_open(DockPlacement::Left);
            self.right_dock_was_open =
                self.area.read(cx).is_dock_open(DockPlacement::Right);
            self.bottom_dock_was_open =
                self.area.read(cx).is_dock_open(DockPlacement::Bottom);
            self.chat_zoomed = true;
            self.settings_active = false;
            self.search_active = false;
            self.problems_active = false;
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
                    area.set_dock_size(DockPlacement::Right, px(220.), window, cx);
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
            area.set_dock_size(DockPlacement::Right, px(720.), window, cx);
            area.set_dock_size(DockPlacement::Left, px(380.), window, cx);
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

    fn set_threads_visible(
        &mut self,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.threads_visible == visible {
            return;
        }
        self.threads_visible = visible;
        self.agent_workspace
            .update(cx, |panel, cx| panel.set_threads_visible(visible, cx));
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
                    area.set_dock_size(DockPlacement::Right, px(220.), window, cx);
                });
            } else {
                self.area.update(cx, |area, cx| {
                    area.remove_dock(DockPlacement::Right, window, cx);
                });
            }
        }
        cx.notify();
    }

    pub fn handle_global_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let cmd =
            event.keystroke.modifiers.platform || event.keystroke.modifiers.control;
        let ctrl = event.keystroke.modifiers.control;
        let shift = event.keystroke.modifiers.shift;

        if cmd {
            match key {
                "b" => {
                    self.area.update(cx, |a, cx| {
                        a.toggle_dock(DockPlacement::Left, window, cx)
                    });
                }
                "j" => {
                    self.area.update(cx, |a, cx| {
                        a.toggle_dock(DockPlacement::Bottom, window, cx)
                    });
                }
                "r" => {
                    self.area.update(cx, |a, cx| {
                        a.toggle_dock(DockPlacement::Right, window, cx)
                    });
                }
                "," => self.set_center_layout(true, false, false, window, cx),
                "e" if shift => {
                    self.show_left_panel(WorkspaceView::Explorer, window, cx)
                }
                "f" if shift => {
                    self.set_center_layout(false, true, false, window, cx)
                }
                "g" if shift => self.show_left_panel(WorkspaceView::Git, window, cx),
                "l" if shift => {
                    self.show_left_panel(WorkspaceView::LanguageServers, window, cx)
                }
                "p" if shift => {
                    self.set_center_layout(false, false, true, window, cx)
                }
                "1" => {
                    self.area.update(cx, |a, cx| {
                        if !a.is_dock_open(DockPlacement::Left) {
                            a.toggle_dock(DockPlacement::Left, window, cx);
                        }
                    });
                }
                "2" => {
                    self.area.update(cx, |a, cx| {
                        if !a.is_dock_open(DockPlacement::Bottom) {
                            a.toggle_dock(DockPlacement::Bottom, window, cx);
                        }
                    });
                }
                "3" => {
                    self.area.update(cx, |a, cx| {
                        if !a.is_dock_open(DockPlacement::Right) {
                            a.toggle_dock(DockPlacement::Right, window, cx);
                        }
                    });
                }
                "d" if shift => self.show_debug(window, cx),
                "m" if shift => {
                    self.toggle_chat_zoom(window, cx);
                }
                "t" if shift => {
                    let next = !self.threads_visible;
                    self.set_threads_visible(next, window, cx);
                }
                _ => {}
            }
        } else if ctrl && (key == "`" || key == "~") {
            self.area.update(cx, |a, cx| {
                a.toggle_dock(DockPlacement::Bottom, window, cx)
            });
        }
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
    }
}

impl Render for Shell {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.poll_explorer_open(window, cx);
        let border = cx.theme().border;
        let text = cx.theme().sidebar_foreground;
        let area_right = self.area.clone();
        let area_left = self.area.clone();
        let left_open = self.area.read(cx).is_dock_open(DockPlacement::Left);
        let bottom_open = self.area.read(cx).is_dock_open(DockPlacement::Bottom);
        let right_open = self.area.read(cx).is_dock_open(DockPlacement::Right);
        let active_left = self.activity.read(cx).active();
        let active_icon = gpui_kit::rgb(0x34D399);
        let active_button = |button: Button, active: bool| {
            if active {
                button.text_color(active_icon)
            } else {
                button
            }
        };
        let session_label = self.session.read_with(cx, |session, _| {
            format!(
                "AHEAD: {} [{} · {} · {} task]",
                session.active_work_title,
                session.phase_id,
                session.active_work_kind.display_name(),
                session.active_task_intent.display_name(),
            )
        });
        let lsp_servers = self.code.read_with(cx, |code, _| {
            code.proxy
                .as_ref()
                .map(|proxy| proxy.lsp_servers())
                .unwrap_or_default()
        });
        let (lsp_color, lsp_state) = if lsp_servers.is_empty() {
            (gpui_kit::rgb(0xFBBF24), "starting")
        } else if lsp_servers.iter().all(|server| server.is_ready()) {
            (gpui_kit::rgb(0x34D399), "ready")
        } else {
            (gpui_kit::rgb(0xF87171), "error")
        };

        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::handle_global_key))
            .child(
                TitleBar::new().child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(IconName::Zap)
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::BOLD)
                                .text_size(px(12.))
                                .text_color(text)
                                .child("AHEAD"),
                        ),
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
                                        .tooltip("Toggle Left Dock (⌘B)"),
                                    left_open,
                                )
                                    .on_click(cx.listener(move |_, _, window, cx| {
                                        area_left.update(cx, |area, cx| area.toggle_dock(DockPlacement::Left, window, cx));
                                    }))
                            )
                            .child(
                                active_button(
                                    Button::new("files_btn")
                                        .icon(IconName::Folder)
                                        .tooltip("Files Explorer (⌘1)"),
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
                                        .tooltip(format!("Source Control · {} (⌘⇧G)", self.branch)),
                                    left_open && active_left == WorkspaceView::Git,
                                )
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.show_left_panel(WorkspaceView::Git, window, cx);
                                        let workspace = this.explorer.read_with(cx, |explorer, _| explorer.root.clone());
                                        let branch = std::process::Command::new("git")
                                            .args(["branch", "--show-current"])
                                            .current_dir(workspace)
                                            .output()
                                            .ok()
                                            .and_then(|o| if o.status.success() { Some(String::from_utf8_lossy(&o.stdout).trim().to_string()) } else { None })
                                            .filter(|s| !s.is_empty())
                                            .unwrap_or_else(|| this.branch.clone());
                                        this.branch = branch;
                                        this.explorer.update(cx, |explorer, cx| explorer.refresh(cx));
                                        this.code.update(cx, |code, _| {
                                            if let Some(proxy) = code.proxy.as_ref() {
                                                proxy.refresh_diff_local();
                                            }
                                        });
                                        cx.notify();
                                    }))
                            )
                            .child(
                                active_button(
                                    Button::new("search_btn")
                                        .icon(IconName::Search)
                                        .tooltip("Search Workspace (⌘⇧F)"),
                                    self.search_active,
                                )
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.set_center_layout(false, true, false, window, cx);
                                    })),
                            )
                            .child(
                                active_button(
                                    Button::new("problems_btn")
                                        .icon(IconName::ShieldCheck)
                                        .tooltip({
                                        let n = self.code.read_with(cx, |code, _| code.diagnostics.len());
                                        format!("Diagnostics: {n} problem{} · Jump to first issue (F8)", if n == 1 { "" } else { "s" })
                                    }),
                                    self.problems_active,
                                )
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.set_center_layout(false, false, true, window, cx);
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
                                h_flex()
                                    .items_center()
                                    .gap(px(1.))
                                    .child(
                                        active_button(
                                            Button::new("language_servers_btn")
                                                .icon(IconName::Zap)
                                                .tooltip(format!(
                                                    "Language Servers: {lsp_state} (⌘⇧L)"
                                                )),
                                            left_open && active_left == WorkspaceView::LanguageServers,
                                        )
                                            .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                                this.show_left_panel(WorkspaceView::LanguageServers, window, cx);
                                            })),
                                    )
                                    .child(
                                        div()
                                            .w(px(6.))
                                            .h(px(6.))
                                            .rounded_full()
                                            .bg(lsp_color),
                                    )
                            )
                    )
                    // Center Region: Active work session chip
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(IconName::Zap)
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(11.))
                                    .text_color(text)
                                    .child(session_label)
                            )
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
                                    .child("Ln 1, Col 1 · Rust · UTF-8")
                            )
                            .child(
                                active_button(
                                    Button::new("terminal_btn")
                                        .icon(IconName::Terminal)
                                        .tooltip("Terminal (⌘J)"),
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
                                        .tooltip("Debug Panel (⌘⇧D)"),
                                    bottom_open && self.bottom_debug_active,
                                )
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.show_debug(window, cx);
                                    }))
                            )
                            .child(
                                active_button(
                                    Button::new("settings_btn")
                                        .icon(IconName::Settings)
                                        .tooltip("Open Settings (⌘,)"),
                                    self.settings_active,
                                )
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.set_center_layout(true, false, false, window, cx);
                                    }))
                            )
                            .child(
                                active_button(
                                    Button::new("agent_btn")
                                        .icon(IconName::MessageSquare)
                                        .tooltip("AHEAD Agent (⌘3)"),
                                    right_open,
                                )
                                    .on_click(cx.listener({
                                        let area = self.area.clone();
                                        move |_, _, window, cx| {
                                            area.update(cx, |area, cx| {
                                                if !area.is_dock_open(DockPlacement::Right) {
                                                    area.toggle_dock(DockPlacement::Right, window, cx);
                                                }
                                            });
                                        }
                                    }))
                            )
                            .child(
                                active_button(
                                    Button::new("threads_btn")
                                        .icon(IconName::Layers)
                                        .tooltip("Threads (⌘⇧T)"),
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
                                        .tooltip("Toggle Right Sidebar (⌘R)"),
                                    right_open,
                                )
                                    .on_click(cx.listener(move |_, _, window, cx| {
                                        area_right.update(cx, |area, cx| area.toggle_dock(DockPlacement::Right, window, cx));
                                    }))
                            )
                    )
                    .border_t_1()
                    .border_color(border),
            )
    }
}

fn configure_ahead_theme(cx: &mut App) {
    let theme = gpui_kit::component::Theme::global_mut(cx);

    let mut dark_theme = (*theme.dark_theme).clone();
    dark_theme.colors.list_active = Some("#10B98133".into());
    dark_theme.colors.list_active_border = Some("#10B981".into());
    theme.dark_theme = std::rc::Rc::new(dark_theme);

    let mut light_theme = (*theme.light_theme).clone();
    light_theme.colors.list_active = Some("#05966933".into());
    light_theme.colors.list_active_border = Some("#059669".into());
    theme.light_theme = std::rc::Rc::new(light_theme);
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

    if file_path.is_empty() {
        file_path = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join("README.md")
            .to_string_lossy()
            .to_string();
    }

    let explorer_root = root_arg
        .or_else(|| {
            std::path::Path::new(&file_path)
                .parent()
                .and_then(|p| p.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| {
            "/Users/kpowel859@cable.comcast.com/dev/ahead".to_string()
        });

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            configure_ahead_theme(cx);
            gpui_kit::component::Theme::change(
                gpui_kit::component::ThemeMode::Dark,
                None,
                cx,
            );
            let path = file_path.clone();
            let explorer_root = explorer_root.clone();
            cx.spawn(async move |cx| {
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds {
                            origin: Point::new(px(40.), px(40.)),
                            size: Size::new(px(1600.), px(1000.)),
                        })),
                        ..TitleBar::window_options()
                    },
                    |window, cx| {
                        let (area, skin) =
                            DockSkin::dock_area("ahead-shell", None, window, cx);
                        skin.set_panel_style(PanelStyle::TabBar, cx);
                        let proxy = crate::proxy_client::ProxyClient::new(
                            std::path::PathBuf::from(&explorer_root),
                        );
                        let proxy_branch = proxy.diff().branch.clone();
                        let code = cx.new(|cx| {
                            crate::code_panel::CodePanel::new(&path, window, cx)
                                .with_proxy(proxy.clone(), &explorer_root)
                        });
                        // Start a real durable AHEAD work session through the
                        // proxy host and bind the chat panel to it. Reopen the
                        // newest active session after an app restart.
                        let session_result =
                            proxy.open_work_session("AHEAD session", &explorer_root);
                        let session = cx.new(|cx| {
                            let panel =
                                crate::session_panel::SessionPanel::new(window, cx)
                                    .with_code(code.clone());
                            match &session_result {
                                Ok(session_id) => panel
                                    .with_proxy(proxy.clone(), session_id.clone()),
                                Err(error) => {
                                    panel.with_startup_error(error.message.clone())
                                }
                            }
                        });
                        // Poll streamed harness output on a UI timer so the
                        // conversation appears incrementally.
                        cx.spawn({
                            let session = session.clone();
                            async move |cx| loop {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(250))
                                    .await;
                                cx.update_entity(&session, |panel, cx| {
                                    panel.poll_stream(cx)
                                });
                            }
                        })
                        .detach();
                        let threads = cx.new(|cx| {
                            crate::threads_panel::ThreadsPanel::new(window, cx)
                                .with_proxy(proxy.clone())
                                .with_session(session.clone())
                        });
                        session.update(cx, |panel, cx| {
                            panel.set_thread_launcher(threads.clone(), cx);
                        });
                        if let Ok(session_id) = &session_result {
                            let thread_title = proxy
                                .session_view(session_id)
                                .map(|view| view.session.title)
                                .unwrap_or_else(|| "AHEAD work session".to_string());
                            let harness = proxy.harness_kind(session_id);
                            threads.update(cx, |threads, cx| {
                                threads.set_active_session(
                                    session_id,
                                    &thread_title,
                                    harness,
                                    cx,
                                );
                            });
                        }
                        let explorer = cx.new(|cx| {
                            crate::explorer_panel::ExplorerPanel::new(
                                &explorer_root,
                                window,
                                cx,
                            )
                        });
                        let explorer_id = explorer.read(cx).mailbox_id;
                        code.update(cx, |code, cx| {
                            code.explorer_id = Some(explorer_id);
                            cx.notify();
                        });
                        let settings = cx.new(|cx| {
                            crate::settings_panel::SettingsPanel::new(
                                std::path::PathBuf::from(&explorer_root),
                                window,
                                cx,
                            )
                        });
                        let terminal = cx.new(|cx| {
                            crate::terminal_panel::TerminalPanel::new_with_id(1, cx)
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
                        let git =
                            cx.new(|cx| GitPanel::new(&explorer_root, window, cx));
                        let search = cx.new(|cx| {
                            SearchPanel::new(&explorer_root, explorer_id, window, cx)
                        });
                        let problems =
                            cx.new(|cx| ProblemsPanel::new(code.clone(), cx));
                        let language_servers = cx.new(|cx| {
                            LanguageServersPanel::new(
                                &explorer_root,
                                proxy.clone(),
                                cx,
                            )
                        });
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
                                language_servers.clone(),
                            )
                        });
                        let shell = cx.new(|cx| {
                            Shell::new(
                                area.clone(),
                                &proxy_branch,
                                session.clone(),
                                threads.clone(),
                                code.clone(),
                                explorer.clone(),
                                debug_bar.clone(),
                                terminals.clone(),
                                problems.clone(),
                                settings.clone(),
                                search.clone(),
                                agent_workspace.clone(),
                                activity.clone(),
                                cx,
                            )
                        });
                        shell.update(cx, |shell, cx| {
                            shell.configure_code(code.clone(), cx);
                        });
                        let shell_for_settings = shell.downgrade();
                        settings.update(cx, |settings, _| {
                            settings.set_close_handler(move |panel, window, cx| {
                                _ = shell_for_settings.update(cx, |shell, cx| {
                                    shell.close_center_panel(panel, window, cx);
                                });
                            });
                        });
                        let shell_for_search = shell.downgrade();
                        search.update(cx, |search, _| {
                            search.set_close_handler(move |panel, window, cx| {
                                _ = shell_for_search.update(cx, |shell, cx| {
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
                        });
                        // Center: editor tabs. Bottom utility dock: Terminal and Debug tabs.
                        shell.update(cx, |shell, cx| {
                            shell.set_center_layout(false, false, false, window, cx);
                            shell.set_bottom_layout(false, window, cx);
                        });

                        // Right dock: Agent and Threads share one panel header and always stay together.
                        let right = DockLayout::tabs()
                            .panel_view(panel_handle(agent_workspace.clone()), cx);

                        area.update(cx, |area, cx| {
                            area.set_dock(DockPlacement::Right, right, window, cx);
                            area.set_dock_size(
                                DockPlacement::Right,
                                px(720.),
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
                        cx.new(|cx| {
                            gpui_kit::component::Root::new(shell, window, cx)
                        })
                    },
                )
                .expect("Failed to open AHEAD window");
            })
            .detach();
        });
}
