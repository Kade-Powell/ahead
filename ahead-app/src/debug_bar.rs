//! DAP debug control bar: breakpoint list + step controls.
//!
//! Backed by `ProxyClient` + `ahead-proxy` DAP dispatch. Gutter breakpoint
//! toggling lives in `CodePanel`'s gutter rail state; this bar surfaces the
//! same breakpoint set with start/stop/step controls and stop reason.

use ahead_rpc::dap_types::DapSessionState;
use std::{collections::HashMap, path::PathBuf, rc::Rc, sync::Arc};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent, PanelId, TabGroup,
};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, Disableable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit_assets::IconName;

use crate::proxy_client::ProxyClient;

type SpeechInterruptionHandler = Rc<dyn Fn()>;

pub struct DebugBar {
    pub focus: FocusHandle,
    pub proxy: Arc<ProxyClient>,
    pub file_path: String,
    pub status: SharedString,
    pub active_line: u32,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
    close_handler: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    new_terminal_handler: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    speech_interruption_handler: Option<SpeechInterruptionHandler>,
    _debug_updates: Task<()>,
}

impl DebugBar {
    pub fn new(
        proxy: Arc<ProxyClient>,
        file_path: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let receiver = proxy.subscribe_diagnostics();
        let debug_updates = cx.spawn(async move |this, cx| {
            while receiver.recv().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        Self {
            focus: cx.focus_handle(),
            proxy,
            file_path: file_path.to_string(),
            status: "Debugger idle · F9 toggles breakpoint".into(),
            active_line: 3,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
            close_handler: None,
            new_terminal_handler: None,
            speech_interruption_handler: None,
            _debug_updates: debug_updates,
        }
    }

    pub fn set_close_handler<F>(&mut self, handler: F)
    where
        F: Fn(&mut Window, &mut App) + 'static,
    {
        self.close_handler = Some(Rc::new(handler));
    }

    pub fn set_new_terminal_handler<F>(&mut self, handler: F)
    where
        F: Fn(&mut Window, &mut App) + 'static,
    {
        self.new_terminal_handler = Some(Rc::new(handler));
    }

    pub fn set_speech_interruption_handler(&mut self, handler: impl Fn() + 'static) {
        self.speech_interruption_handler = Some(Rc::new(handler));
    }

    fn interrupt_speech(&self) {
        if let Some(handler) = self.speech_interruption_handler.as_ref() {
            handler();
        }
    }

    fn breakpoints(&self) -> Vec<u32> {
        let mut bps: Vec<u32> = self
            .proxy
            .breakpoints_for(PathBuf::from(&self.file_path).as_path())
            .into_iter()
            .collect();
        bps.sort_unstable();
        bps
    }

    fn status_text(&self) -> SharedString {
        let debug = self.proxy.debug();
        if let Some(error) = debug.error {
            return error.into();
        }
        match debug.state {
            DapSessionState::Idle => self.status.clone(),
            DapSessionState::Starting => "Starting debug adapter…".into(),
            DapSessionState::Running => "Debug session running".into(),
            DapSessionState::Stopped => format!("Stopped: {}", debug.reason).into(),
            DapSessionState::Stopping => "Stopping debug adapter…".into(),
            DapSessionState::Terminated => "Debug session ended".into(),
            DapSessionState::Failed(message) => message.into(),
        }
    }

    fn toggle_current(&mut self, cx: &mut Context<Self>) {
        self.interrupt_speech();
        self.proxy.toggle_breakpoint(
            PathBuf::from(&self.file_path).as_path(),
            self.active_line,
        );
        self.status = format!("Breakpoint Ln {}", self.active_line).into();
        cx.notify();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self.interrupt_speech();
        let configs = self.proxy.stored_configs();
        let Some(config) = configs.first().cloned() else {
            self.status = "No debug configuration is available".into();
            cx.notify();
            return;
        };
        let bps: Vec<u32> = self.breakpoints();
        let mut map: HashMap<PathBuf, Vec<ahead_rpc::dap_types::SourceBreakpoint>> =
            HashMap::new();
        map.insert(
            PathBuf::from(&self.file_path),
            bps.into_iter()
                .map(|line| ahead_rpc::dap_types::SourceBreakpoint {
                    line: line as usize,
                    ..Default::default()
                })
                .collect(),
        );
        if self.proxy.dap_start(config, map) {
            self.status = "Debug session starting via ahead-proxy".into();
        }
        cx.notify();
    }

    pub(crate) fn handle_shortcut(
        &mut self,
        key: &str,
        shift: bool,
        cx: &mut Context<Self>,
    ) {
        let debug = self.proxy.debug();
        let can_step =
            debug.state == DapSessionState::Stopped && debug.thread_id.is_some();
        match (key, shift) {
            ("f5", false)
                if !debug.state.is_active() && !debug.connection_closed =>
            {
                self.start(cx)
            }
            ("f5", false) if can_step => {
                self.interrupt_speech();
                if self.proxy.dap_continue() {
                    self.status = "Continue requested".into();
                }
                cx.notify();
            }
            ("f9", false) => self.toggle_current(cx),
            ("f10", false) if can_step => {
                self.interrupt_speech();
                if self.proxy.dap_step_over() {
                    self.status = "Step over requested".into();
                }
                cx.notify();
            }
            ("f11", false) if can_step => {
                self.interrupt_speech();
                if self.proxy.dap_step_into() {
                    self.status = "Step into requested".into();
                }
                cx.notify();
            }
            ("f11", true) if can_step => {
                self.interrupt_speech();
                if self.proxy.dap_step_out() {
                    self.status = "Step out requested".into();
                }
                cx.notify();
            }
            ("f5", true) if debug.state.can_stop() => {
                self.interrupt_speech();
                if self.proxy.dap_stop() {
                    self.status = "Stop requested".into();
                }
                cx.notify();
            }
            _ => {}
        }
    }
}

impl BasePanel for DebugBar {
    fn panel_name(&self) -> &'static str {
        "ahead_debug_bar"
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

impl Panel for DebugBar {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let group = self.tab_group.clone();
        let panel_id = self.panel_id;
        let close_handler = self.close_handler.clone();
        h_flex().items_center().gap_1().child("Debug").child(
            Button::new("close_debug_panel")
                .icon(IconName::X)
                .ghost()
                .tooltip("Close Debug")
                .on_click(move |_, window, cx| {
                    if let Some(handler) = close_handler.as_ref() {
                        handler(window, cx);
                    } else if let Some(group) = group.as_ref() {
                        let is_last = group
                            .read_with(cx, |group, _| group.panels().len() == 1)
                            .unwrap_or(false);
                        if !is_last {
                            _ = group.update(cx, |group, cx| {
                                group.close_panel(panel_id, cx);
                            });
                        }
                    }
                }),
        )
    }
    fn toolbar_buttons(
        &mut self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        let handler = self.new_terminal_handler.clone();
        Some(vec![
            Button::new("new_terminal_from_debug")
                .icon(IconName::Plus)
                .tooltip("New Terminal")
                .on_click(move |_, window, cx| {
                    if let Some(handler) = handler.as_ref() {
                        handler(window, cx);
                    }
                }),
        ])
    }
    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        None
    }
}

impl EventEmitter<PanelEvent> for DebugBar {}

impl Focusable for DebugBar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DebugBar {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let text = cx.theme().sidebar_foreground;
        let border = cx.theme().border;
        let panel = cx.theme().sidebar;
        let muted = cx.theme().muted_foreground;
        let dbg = self.proxy.debug();
        let can_step =
            dbg.state == DapSessionState::Stopped && dbg.thread_id.is_some();
        let bps = self.breakpoints();
        let state_label = if dbg.connection_closed {
            "Disconnected".to_string()
        } else {
            match &dbg.state {
                DapSessionState::Idle => "Idle".into(),
                DapSessionState::Starting => "Starting".into(),
                DapSessionState::Running => "Running".into(),
                DapSessionState::Stopped => format!("Stopped: {}", dbg.reason),
                DapSessionState::Stopping => "Stopping".into(),
                DapSessionState::Terminated => "Session ended".into(),
                DapSessionState::Failed(_) => "Failed".into(),
            }
        };
        let status = self.status_text();

        v_flex()
            .size_full()
            .min_h_0()
            .bg(panel)
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .h(px(40.))
                    .px_3()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(IconName::Bug)
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_size(px(12.))
                            .text_color(text)
                            .child("Debug"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(10.))
                            .text_color(muted)
                            .child(format!("{state_label} · {} breakpoint{}", bps.len(), if bps.len() == 1 { "" } else { "s" })),
                    )
                    .child(
                        Button::new("dbg_start")
                            .disabled(dbg.state.is_active() || dbg.connection_closed)
                            .icon(IconName::Play)
                            .label("Start")
                            .tooltip("Start Debug Session (F5)")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.start(cx))),
                    )
                    .child(
                        Button::new("dbg_toggle_bp")
                            .icon(IconName::CircleDot)
                            .label("Breakpoint")
                            .tooltip("Toggle Breakpoint at current line (F9)")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.handle_shortcut("f9", false, cx))),
                    )
                    .child(
                        Button::new("dbg_continue")
                            .disabled(!can_step)
                            .icon(IconName::Play)
                            .label("Continue")
                            .tooltip("Continue (F5)")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.handle_shortcut("f5", false, cx))),
                    )
                    .child(
                        Button::new("dbg_step_over")
                            .disabled(!can_step)
                            .icon(IconName::StepForward)
                            .label("Step Over")
                            .tooltip("Step Over (F10)")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.handle_shortcut("f10", false, cx))),
                    )
                    .child(
                        Button::new("dbg_step_into")
                            .disabled(!can_step)
                            .icon(IconName::ArrowDownToLine)
                            .label("Step Into")
                            .tooltip("Step Into (F11)")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.handle_shortcut("f11", false, cx))),
                    )
                    .child(
                        Button::new("dbg_step_out")
                            .disabled(!can_step)
                            .icon(IconName::ArrowUpFromLine)
                            .label("Step Out")
                            .tooltip("Step Out (⇧F11)")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.handle_shortcut("f11", true, cx))),
                    )
                    .child(
                        Button::new("dbg_stop")
                            .disabled(!dbg.state.can_stop())
                            .icon(IconName::Square)
                            .tooltip("Stop Session (⇧F5)")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.handle_shortcut("f5", true, cx))),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .w(px(260.))
                            .min_h_0()
                            .p_3()
                            .gap_2()
                            .border_r_1()
                            .border_color(border)
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(IconName::CircleDot)
                                    .child(
                                        div()
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .text_size(px(11.))
                                            .text_color(text)
                                            .child("Breakpoints"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(muted)
                                    .child("Click a breakpoint to focus its source line."),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scrollbar()
                                    .when(bps.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .py_2()
                                                .text_size(px(11.))
                                                .text_color(muted)
                                                .child("No breakpoints"),
                                        )
                                    })
                                    .children(bps.iter().map(|line| {
                                        let line = *line;
                                        h_flex()
                                            .h(px(28.))
                                            .px_2()
                                            .gap_2()
                                            .items_center()
                                            .id(("breakpoint", line as usize))
                                            .cursor(CursorStyle::PointingHand)
                                            .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                this.active_line = line;
                                                this.status = format!("Breakpoint Ln {line}").into();
                                                cx.notify();
                                            }))
                                            .child(IconName::CircleDot)
                                            .child(
                                                div()
                                                    .text_size(px(11.))
                                                    .text_color(text)
                                                    .child(format!("{}:{}", self.file_path, line)),
                                            )
                                    })),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .p_4()
                            .gap_3()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(IconName::Bug)
                                    .child(
                                        div()
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .text_size(px(12.))
                                            .text_color(text)
                                            .child(if dbg.state.is_active() { "Debug session" } else { "No debug session" }),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(muted)
                                    .child(if dbg.connection_closed {
                                        "AHEAD proxy connection closed. Restart AHEAD to reconnect.".to_string()
                                    } else if dbg.state.is_active() {
                                        state_label.clone()
                                    } else {
                                        "Start a debug session to inspect threads, stack frames, and variables here.".to_string()
                                    }),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .items_center()
                                    .justify_center()
                                    .gap_2()
                                    .child(IconName::Bug)
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(muted)
                                            .child("Debugger output will appear here when the session stops."),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(muted)
                                    .child(status),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{DebugBar, ProxyClient};
    use ahead_rpc::{
        core::CoreNotification,
        dap_types::DapSessionState,
        proxy::{ProxyNotification, ProxyRpc},
    };
    use gpui_kit::TestAppContext;
    use std::{cell::Cell, collections::HashMap, rc::Rc};

    #[gpui_kit::test(iterations = 20)]
    fn debugger_shows_failures_and_waits_for_confirmed_cleanup(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let proxy = ProxyClient::new_for_test(directory.path().to_path_buf());
        let (bar, cx) = cx.add_window_view(|window, cx| {
            DebugBar::new(proxy.clone(), "src/main.ts", window, cx)
        });
        bar.update(cx, |bar, cx| bar.handle_shortcut("f5", false, cx));
        let first = proxy.debug().dap_id.expect("session");
        assert_eq!(
            bar.read_with(cx, |bar, _| bar.status_text()),
            "Starting debug adapter…"
        );
        proxy.route_core(CoreNotification::DapSessionState {
            dap_id: first,
            state: DapSessionState::Failed("Adapter not installed".into()),
        });
        cx.run_until_parked();
        assert_eq!(
            bar.read_with(cx, |bar, _| bar.status_text()),
            "Adapter not installed"
        );
        assert!(!proxy.debug().state.is_active());
        cx.update(|window, cx| window.draw(cx).clear(cx));

        bar.update(cx, |bar, cx| bar.handle_shortcut("f5", false, cx));
        let second = proxy.debug().dap_id.expect("retry session");
        assert_ne!(first, second);
        proxy.route_core(CoreNotification::DapSessionState {
            dap_id: second,
            state: DapSessionState::Running,
        });
        proxy.route_core(CoreNotification::DapSessionState {
            dap_id: first,
            state: DapSessionState::Failed("Old failure".into()),
        });
        proxy.route_core(CoreNotification::DapError {
            dap_id: first,
            message: "Old command error".into(),
        });
        assert_eq!(
            bar.read_with(cx, |bar, _| bar.status_text()),
            "Debug session running"
        );
        proxy.route_core(CoreNotification::DapError {
            dap_id: second,
            message: "Pause rejected by adapter".into(),
        });
        cx.run_until_parked();
        assert_eq!(
            bar.read_with(cx, |bar, _| bar.status_text()),
            "Pause rejected by adapter"
        );

        bar.update(cx, |bar, cx| bar.handle_shortcut("f5", true, cx));
        assert_eq!(
            bar.read_with(cx, |bar, _| bar.status_text()),
            "Stopping debug adapter…"
        );
        bar.update(cx, |bar, cx| bar.handle_shortcut("f5", false, cx));
        assert_eq!(
            proxy.debug().dap_id,
            Some(second),
            "do not restart before cleanup"
        );
        proxy.route_core(CoreNotification::DapSessionState {
            dap_id: second,
            state: DapSessionState::Running,
        });
        assert_eq!(
            proxy.debug().state,
            DapSessionState::Stopping,
            "late launch ack"
        );
        proxy.route_core(CoreNotification::DapSessionState {
            dap_id: second,
            state: DapSessionState::Terminated,
        });
        cx.run_until_parked();
        assert_eq!(
            bar.read_with(cx, |bar, _| bar.status_text()),
            "Debug session ended"
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui_kit::test(iterations = 20)]
    fn debugger_shortcuts_use_the_stopped_session_and_notifications_wake_the_bar(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let proxy = ProxyClient::new_for_test(directory.path().to_path_buf());
        let rpc = proxy.rpc_for_test();
        let notifications = rpc.rx();
        let (bar, cx) = cx.add_window_view(|window, cx| {
            DebugBar::new(proxy.clone(), "src/main.rs", window, cx)
        });
        let updates = Rc::new(Cell::new(0));
        let _subscription = cx.update(|_, cx| {
            cx.observe(&bar, {
                let updates = updates.clone();
                move |_, _| updates.set(updates.get() + 1)
            })
        });
        bar.update(cx, |bar, cx| {
            for (key, shift) in
                [("f10", false), ("f11", false), ("f11", true), ("f5", true)]
            {
                bar.handle_shortcut(key, shift, cx);
            }
        });
        assert!(
            notifications.try_recv().is_err(),
            "idle shortcuts must not send controls"
        );
        bar.update(cx, |bar, cx| bar.handle_shortcut("f5", false, cx));
        let ProxyRpc::Notification(ProxyNotification::DapStart { config, .. }) =
            notifications.try_recv().expect("start")
        else {
            panic!("start notification");
        };
        bar.update(cx, |bar, cx| bar.handle_shortcut("f5", false, cx));
        assert!(
            notifications.try_recv().is_err(),
            "do not start duplicate sessions"
        );
        cx.run_until_parked();
        let previous_updates = updates.get();
        proxy.route_core(CoreNotification::DapStopped {
            dap_id: config.dap_id,
            stopped: serde_json::from_value(serde_json::json!({
                "reason": "breakpoint", "threadId": 51,
            }))
            .expect("stopped event"),
            stack_frames: HashMap::new(),
            variables: Vec::new(),
        });
        cx.run_until_parked();
        assert!(
            updates.get() > previous_updates,
            "debug notifications must wake the view"
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
        bar.update(cx, |bar, cx| bar.handle_shortcut("f10", false, cx));
        let ProxyRpc::Notification(ProxyNotification::DapStepOver {
            dap_id,
            thread_id,
        }) = notifications.try_recv().expect("step")
        else {
            panic!("step notification");
        };
        assert_eq!(dap_id, config.dap_id);
        assert_eq!(serde_json::to_value(thread_id).expect("thread ID"), 51);
        bar.update(cx, |bar, cx| bar.handle_shortcut("f5", true, cx));
        assert!(matches!(notifications.try_recv().expect("stop"),
            ProxyRpc::Notification(ProxyNotification::DapStop { dap_id }) if dap_id == config.dap_id
        ));
        bar.update(cx, |bar, cx| bar.handle_shortcut("f11", false, cx));
        assert!(notifications.try_recv().is_err());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
}
