//! AHEAD Agent Threads Sidebar - GPUI Implementation
//!
//! Mirrors the multi-thread sidebar layout from modern AI-native editors (Zed/AHEAD),
//! supporting both collaborative AHEAD work-item threads and delegated harness tasks.
//! Uses gpui-kit components, Lucide icons, and dynamic Dark/Light theming.

use gpui_kit::*;
use gpui_kit::prelude::*;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent,
};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{v_flex, h_flex, ActiveTheme};
use gpui_kit_assets::IconName;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadKind {
    AheadWorkItem { item_id: String, shared_collaborators: Vec<String> },
    DelegatedTask { harness: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentThread {
    pub id: String,
    pub title: String,
    pub kind: ThreadKind,
    pub message_count: usize,
    pub is_active: bool,
    pub updated_at: String,
}

pub struct ThreadsPanel {
    pub focus: FocusHandle,
    pub search_input: Entity<InputState>,
    pub threads: Vec<AgentThread>,
    pub status: SharedString,
    pub filter: String,
}

impl ThreadsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx));
        let threads = vec![
            AgentThread {
                id: "thread-ahead-1".into(),
                title: "Implement resilient retry logic".into(),
                kind: ThreadKind::AheadWorkItem {
                    item_id: "wi-2".into(),
                    shared_collaborators: vec!["kpowel859".into(), "alice".into()],
                },
                message_count: 5,
                is_active: true,
                updated_at: "Just now".into(),
            },
            AgentThread {
                id: "thread-ahead-2".into(),
                title: "Invariants & bounds audit".into(),
                kind: ThreadKind::AheadWorkItem {
                    item_id: "wi-1".into(),
                    shared_collaborators: vec!["kpowel859".into()],
                },
                message_count: 12,
                is_active: false,
                updated_at: "10m ago".into(),
            },
            AgentThread {
                id: "thread-task-1".into(),
                title: "Audit third-party/codex runtime bloat".into(),
                kind: ThreadKind::DelegatedTask {
                    harness: "ACP Subagent".into(),
                },
                message_count: 8,
                is_active: false,
                updated_at: "15m ago".into(),
            },
            AgentThread {
                id: "thread-task-2".into(),
                title: "Sync Tree-sitter grammar queries".into(),
                kind: ThreadKind::DelegatedTask {
                    harness: "External ACP".into(),
                },
                message_count: 3,
                is_active: false,
                updated_at: "1h ago".into(),
            },
        ];

        Self {
            focus: cx.focus_handle(),
            search_input,
            threads,
            status: "Threads loaded".into(),
            filter: String::new(),
        }
    }

    pub fn select_thread(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        for thread in &mut self.threads {
            thread.is_active = thread.id == thread_id;
        }
        self.status = format!("Active thread: {thread_id}").into();
        cx.notify();
    }

    pub fn new_ahead_thread(&mut self, cx: &mut Context<Self>) {
        let count = self.threads.len() + 1;
        let new_thread = AgentThread {
            id: format!("thread-ahead-{count}"),
            title: format!("New AHEAD Work Thread {count}"),
            kind: ThreadKind::AheadWorkItem {
                item_id: format!("wi-{count}"),
                shared_collaborators: vec!["kpowel859".into()],
            },
            message_count: 0,
            is_active: true,
            updated_at: "Just now".into(),
        };

        for thread in &mut self.threads {
            thread.is_active = false;
        }

        self.threads.insert(0, new_thread);
        self.status = "Created new collaborative AHEAD thread".into();
        cx.notify();
    }

    pub fn new_delegated_task(&mut self, cx: &mut Context<Self>) {
        let count = self.threads.len() + 1;
        let new_thread = AgentThread {
            id: format!("thread-task-{count}"),
            title: format!("Delegated Task #{count}"),
            kind: ThreadKind::DelegatedTask {
                harness: "Background Subagent".into(),
            },
            message_count: 0,
            is_active: true,
            updated_at: "Just now".into(),
        };

        for thread in &mut self.threads {
            thread.is_active = false;
        }

        self.threads.insert(0, new_thread);
        self.status = "Spawned new delegated task thread".into();
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
        let is_dark = cx.theme().mode.is_dark();

        v_flex()
            .size_full()
            .p_3()
            .gap_2()
            .track_focus(&self.focus)
            // Top Search Bar & New Thread Actions
            .child(
                v_flex()
                    .gap_2()
                    .p_2()
                    .rounded_lg()
                    .bg(group_box)
                    .border_1()
                    .border_color(border_color)
                    .child(
                        Input::new(&self.search_input)
                            .aria_label("Search threads")
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("new_ahead_thread_btn")
                                    .primary()
                                    .icon(IconName::MessageSquare)
                                    .label("AHEAD Thread")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| this.new_ahead_thread(cx)))
                            )
                            .child(
                                Button::new("new_task_thread_btn")
                                    .icon(IconName::Bot)
                                    .label("Task")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| this.new_delegated_task(cx)))
                            )
                    )
            )
            // Workspace Group Header
            .child(
                div()
                    .px_2()
                    .pt_2()
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_size(px(11.))
                    .text_color(text_color)
                    .child("ahead")
            )
            // Thread List Entries
            .child(
                v_flex()
                    .flex_1()
                    .gap_1()
                    .children(
                        self.threads.iter()
                            .filter(move |t| search_query.is_empty() || t.title.to_lowercase().contains(&search_query))
                            .map(|thread| {
                                let thread_id = thread.id.clone();
                                let is_active = thread.is_active;

                                let (badge_text, badge_color, is_shared) = match &thread.kind {
                                    ThreadKind::AheadWorkItem { shared_collaborators, .. } => (
                                        "AHEAD".to_string(),
                                        if is_dark { gpui_kit::rgb(0x10B981) } else { gpui_kit::rgb(0x059669) },
                                        shared_collaborators.len() > 1,
                                    ),
                                    ThreadKind::DelegatedTask { harness } => (
                                        harness.clone(),
                                        if is_dark { gpui_kit::rgb(0x60A5FA) } else { gpui_kit::rgb(0x2563EB) },
                                        false,
                                    ),
                                };

                                h_flex()
                                    .id(ElementId::Name(thread_id.clone().into()))
                                    .items_center()
                                    .justify_between()
                                    .p_3()
                                    .rounded_lg()
                                    .bg(if is_active { active_bg } else { group_box })
                                    .border_1()
                                    .border_color(if is_active { active_bg } else { border_color })
                                    .cursor(gpui_kit::CursorStyle::PointingHand)
                                    .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                        this.select_thread(&thread_id, cx);
                                    }))
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .font_weight(if is_active { gpui_kit::FontWeight::BOLD } else { gpui_kit::FontWeight::NORMAL })
                                                    .text_size(px(12.))
                                                    .text_color(if is_active { active_fg } else { text_color })
                                                    .child(thread.title.clone())
                                            )
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .items_center()
                                                    .child(
                                                        div()
                                                            .text_size(px(10.))
                                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                                            .text_color(badge_color)
                                                            .child(badge_text)
                                                    )
                                                    .when(is_shared, |el| {
                                                        el.child(
                                                            h_flex()
                                                                .gap_1()
                                                                .items_center()
                                                                .child(IconName::Users)
                                                                .child(
                                                                    div()
                                                                        .text_size(px(10.))
                                                                        .text_color(if is_dark { gpui_kit::rgb(0x34D399) } else { gpui_kit::rgb(0x059669) })
                                                                        .child("Shared")
                                                                )
                                                        )
                                                    })
                                                    .child(
                                                        div()
                                                            .text_size(px(10.))
                                                            .text_color(text_color)
                                                            .child(format!("{} msgs · {}", thread.message_count, thread.updated_at))
                                                    )
                                            )
                                    )
                            })
                    )
            )
            // Footer status
            .child(
                div()
                    .pt_1()
                    .text_size(px(10.))
                    .text_color(text_color)
                    .child(self.status.clone())
            )
    }
}
