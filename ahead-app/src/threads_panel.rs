//! AHEAD Agent Threads Sidebar - GPUI Implementation
//!
//! Mirrors the multi-thread sidebar layout from Zed/AHEAD:
//! - Search threads input at top
//! - Unified thread list under the workspace root (`ahead`)
//! - Threads can be AHEAD collaborative threads or external agent threads
//! - Each thread shows title, relative time, close button, and active state

use gpui_kit::*;
use gpui_kit::component::button::Button;
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent,
};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{v_flex, h_flex, ActiveTheme};
use gpui_kit_assets::IconName;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadKind {
    Ahead { item_id: String, shared: bool },
    External { harness: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentThread {
    pub id: String,
    pub title: String,
    pub kind: ThreadKind,
    pub time_str: String,
    pub is_active: bool,
}

pub struct ThreadsPanel {
    pub focus: FocusHandle,
    pub search_input: Entity<InputState>,
    pub threads: Vec<AgentThread>,
    pub status: SharedString,
}

impl ThreadsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx));
        let threads = vec![
            AgentThread {
                id: "thread-1".into(),
                title: "helo".into(),
                kind: ThreadKind::Ahead { item_id: "wi-1".into(), shared: false },
                time_str: "1m".into(),
                is_active: true,
            },
            AgentThread {
                id: "thread-2".into(),
                title: "Implement resilient retry logic".into(),
                kind: ThreadKind::Ahead { item_id: "wi-2".into(), shared: true },
                time_str: "12m".into(),
                is_active: false,
            },
            AgentThread {
                id: "thread-3".into(),
                title: "Audit third-party/codex runtime bloat".into(),
                kind: ThreadKind::External { harness: "Subagent".into() },
                time_str: "45m".into(),
                is_active: false,
            },
            AgentThread {
                id: "thread-4".into(),
                title: "Tree-sitter queries synchronization".into(),
                kind: ThreadKind::External { harness: "External".into() },
                time_str: "2h".into(),
                is_active: false,
            },
        ];

        Self {
            focus: cx.focus_handle(),
            search_input,
            threads,
            status: "Threads".into(),
        }
    }

    pub fn select_thread(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        for thread in &mut self.threads {
            thread.is_active = thread.id == thread_id;
        }
        self.status = format!("Thread: {thread_id}").into();
        cx.notify();
    }

    pub fn close_thread(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        self.threads.retain(|t| t.id != thread_id);
        if let Some(first) = self.threads.first_mut() {
            first.is_active = true;
        }
        cx.notify();
    }

    pub fn new_thread(&mut self, cx: &mut Context<Self>) {
        let count = self.threads.len() + 1;
        let new_t = AgentThread {
            id: format!("thread-{count}"),
            title: format!("Thread {count}"),
            kind: ThreadKind::Ahead { item_id: format!("wi-{count}"), shared: false },
            time_str: "Just now".into(),
            is_active: true,
        };
        for t in &mut self.threads {
            t.is_active = false;
        }
        self.threads.insert(0, new_t);
        cx.notify();
    }
}

impl BasePanel for ThreadsPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_threads"
    }
}

impl Panel for ThreadsPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        "Threads"
    }
    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}

impl EventEmitter<PanelEvent> for ThreadsPanel {}

impl Focusable for ThreadsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ThreadsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let search_query = self.search_input.read(cx).value().to_lowercase();
        let border_color = cx.theme().border;
        let text_color = cx.theme().sidebar_foreground;
        let active_bg = cx.theme().sidebar_accent;
        let active_fg = cx.theme().sidebar_accent_foreground;
        let group_box = cx.theme().group_box;

        v_flex()
            .size_full()
            .min_h_0()
            .p_2()
            .gap_1()
            .track_focus(&self.focus)
            // Top Search Bar & New Thread button
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Input::new(&self.search_input)
                            .aria_label("Search threads...")
                    )
                    .child(
                        Button::new("new_thread_icon_btn")
                            .icon(IconName::Plus)
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.new_thread(cx)))
                    )
            )
            // Workspace Header
            .child(
                div()
                    .px_2()
                    .pt_2()
                    .pb_1()
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_size(px(11.))
                    .text_color(text_color)
                    .child("ahead")
            )
            // Unified Thread List (scrollable)
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_1()
                    .overflow_y_scrollbar()
                    .children(
                        self.threads.iter()
                            .filter(move |t| search_query.is_empty() || t.title.to_lowercase().contains(&search_query))
                            .map(|thread| {
                                let thread_id = thread.id.clone();
                                let thread_id_close = thread.id.clone();
                                let is_active = thread.is_active;

                                h_flex()
                                    .id(ElementId::Name(thread_id.clone().into()))
                                    .items_center()
                                    .justify_between()
                                    .px_2()
                                    .py_1()
                                    .bg(if is_active { active_bg } else { group_box })
                                    .border_1()
                                    .border_color(if is_active { active_bg } else { border_color })
                                    .cursor(gpui_kit::CursorStyle::PointingHand)
                                    .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                        this.select_thread(&thread_id, cx);
                                    }))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .font_weight(if is_active { gpui_kit::FontWeight::BOLD } else { gpui_kit::FontWeight::NORMAL })
                                                    .text_size(px(12.))
                                                    .text_color(if is_active { active_fg } else { text_color })
                                                    .child(thread.title.clone())
                                            )
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_size(px(10.))
                                                    .text_color(text_color)
                                                    .child(thread.time_str.clone())
                                            )
                                            .child(
                                                Button::new(SharedString::from(format!("close_{}", thread_id_close)))
                                                    .icon(IconName::X)
                                                    .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                        this.close_thread(&thread_id_close, cx);
                                                    }))
                                            )
                                    )
                            })
                    )
            )
    }
}
