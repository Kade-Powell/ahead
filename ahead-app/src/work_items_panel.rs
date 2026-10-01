//! AHEAD Work Items Panel - The Living AI-Assisted Todo List & Workflow Gate.
//!
//! Grounded in Sections 4.2, 4.4, and 8.2 of `ahead-editor-mvp.md`.
//! Embodies the human-in-the-loop engineering workflow:
//! - Living checklist with status cycling (Open, Doing, Done, Dropped)
//! - Session workflow pipeline stepper (Plan -> Invariants -> Implement -> Verify -> Review)
//! - AI-assisted task proposal ("AI Propose Work Items")
//! - Streaming voice controls

use gpui_kit::*;
use gpui_kit::prelude::*;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent,
};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::stepper::{Stepper, StepperItem};
use gpui_kit::component::{v_flex, h_flex, ActiveTheme};
use gpui_kit_assets::IconName;

const WORKFLOW_PHASES: [(&str, &str); 5] = [
    ("plan", "Plan"),
    ("invariants", "Invariants"),
    ("implement", "Implement"),
    ("verify", "Verify"),
    ("review", "Review"),
];

pub struct WorkItemsPanel {
    pub focus: FocusHandle,
    pub work_item_input: Entity<InputState>,
    pub work_items: Vec<ahead_rpc::ahead::WorkItem>,
    pub phase_id: &'static str,
    pub work_title: String,
    pub work_kind: ahead_rpc::ahead::WorkKind,
    pub voice: ahead_viewmodel::VoiceIntent,
    pub status: SharedString,
    workflow_collapsed: bool,
}

impl WorkItemsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let work_item_input = cx.new(|cx| InputState::new(window, cx));

        let initial_items = vec![
            ahead_rpc::ahead::WorkItem {
                id: "wi-1".to_string(),
                session_id: "sess-current".to_string(),
                title: "Define scope and map affected modules".to_string(),
                status: ahead_rpc::ahead::WorkItemStatus::Done,
                position: 0,
                created_by: "human".to_string(),
                created_at: "2026-09-17T12:00:00Z".to_string(),
                closed_at: Some("2026-09-17T12:05:00Z".to_string()),
            },
            ahead_rpc::ahead::WorkItem {
                id: "wi-2".to_string(),
                session_id: "sess-current".to_string(),
                title: "Author minimal, invariant-safe implementation".to_string(),
                status: ahead_rpc::ahead::WorkItemStatus::InProgress,
                position: 1,
                created_by: "human".to_string(),
                created_at: "2026-09-17T12:05:00Z".to_string(),
                closed_at: None,
            },
            ahead_rpc::ahead::WorkItem {
                id: "wi-3".to_string(),
                session_id: "sess-current".to_string(),
                title: "Verify concurrent retry backoff under network dropout".to_string(),
                status: ahead_rpc::ahead::WorkItemStatus::Open,
                position: 2,
                created_by: "ai-assistant".to_string(),
                created_at: "2026-09-17T12:10:00Z".to_string(),
                closed_at: None,
            },
        ];

        Self {
            focus: cx.focus_handle(),
            work_item_input,
            work_items: initial_items,
            phase_id: "plan",
            work_title: "Implement resilient retry logic".to_string(),
            work_kind: ahead_rpc::ahead::WorkKind::ProductChange,
            voice: ahead_viewmodel::VoiceIntent::new(),
            status: "Work items checklist active".into(),
            workflow_collapsed: false,
        }
    }

    pub fn add_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.work_item_input.read(cx).value().to_string();
        if title.trim().is_empty() {
            return;
        }

        let new_item = ahead_rpc::ahead::WorkItem {
            id: format!("wi-{}", self.work_items.len() + 1),
            session_id: "sess-current".to_string(),
            title: title.clone(),
            status: ahead_rpc::ahead::WorkItemStatus::Open,
            position: self.work_items.len() as i64,
            created_by: "human".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            closed_at: None,
        };

        self.work_items.push(new_item);
        self.work_item_input.update(cx, |input, cx| input.set_value("", window, cx));
        self.status = format!("Added work item: {title}").into();
        cx.notify();
    }

    pub fn cycle_status(&mut self, item_id: &str, cx: &mut Context<Self>) {
        if let Some(item) = self.work_items.iter_mut().find(|i| i.id == item_id) {
            item.status = match item.status {
                ahead_rpc::ahead::WorkItemStatus::Open => ahead_rpc::ahead::WorkItemStatus::InProgress,
                ahead_rpc::ahead::WorkItemStatus::InProgress => ahead_rpc::ahead::WorkItemStatus::Done,
                ahead_rpc::ahead::WorkItemStatus::Done => ahead_rpc::ahead::WorkItemStatus::Dropped,
                ahead_rpc::ahead::WorkItemStatus::Dropped => ahead_rpc::ahead::WorkItemStatus::Open,
            };
            self.status = format!("Updated status: {:?}", item.status).into();
        }
        cx.notify();
    }

    pub fn ai_propose_next_items(&mut self, cx: &mut Context<Self>) {
        let count = self.work_items.len();
        let suggestions = match self.phase_id {
            "plan" => vec![
                "Map error domains across workspace crates",
                "Define timeout and cancellation preconditions",
            ],
            "invariants" => vec![
                "Document idempotency under network reconnect",
                "Audit concurrency bounds and lock ordering",
            ],
            "implement" => vec![
                "Implement exponential backoff calculation",
                "Wire retry budget into transport client",
            ],
            "verify" => vec![
                "Add chaos test dropping 50% of synthetic packets",
                "Run full workspace test suite with zero warnings",
            ],
            _ => vec![
                "Prepare close-out summary and verification evidence",
                "Publish review snapshot to GitHub tracker outbox",
            ],
        };

        for (i, title) in suggestions.into_iter().enumerate() {
            self.work_items.push(ahead_rpc::ahead::WorkItem {
                id: format!("wi-ai-{}", count + i + 1),
                session_id: "sess-current".to_string(),
                title: title.to_string(),
                status: ahead_rpc::ahead::WorkItemStatus::Open,
                position: (count + i) as i64,
                created_by: "ai-assistant".to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                closed_at: None,
            });
        }

        self.status = "AI proposed next work items for active phase".into();
        cx.notify();
    }

    pub fn advance_phase(&mut self, cx: &mut Context<Self>) {
        let (next_id, next_title) = ahead_viewmodel::next_phase(self.phase_id);
        self.phase_id = next_id;
        self.status = format!("Advanced workflow phase to {next_title}").into();
        cx.notify();
    }

    pub fn toggle_voice(&mut self, cx: &mut Context<Self>) {
        ahead_viewmodel::voice::toggle(&mut self.voice);
        self.status = format!(
            "Voice: {}",
            if self.voice.active { "LIVE" } else { "OFF" }
        )
        .into();
        cx.notify();
    }

    pub fn barge_in(&mut self, cx: &mut Context<Self>) {
        ahead_viewmodel::voice::barge_in(&mut self.voice);
        self.status = format!("Barge-in (gen {})", self.voice.generation).into();
        cx.notify();
    }

    pub fn toggle_mic_mute(&mut self, cx: &mut Context<Self>) {
        self.voice.mic_muted = !self.voice.mic_muted;
        self.status = format!(
            "Mic: {}",
            if self.voice.mic_muted { "MUTED" } else { "ACTIVE" }
        )
        .into();
        cx.notify();
    }
}

impl BasePanel for WorkItemsPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_work_items"
    }
}

impl Panel for WorkItemsPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        "Work Items"
    }
    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}

impl EventEmitter<PanelEvent> for WorkItemsPanel {}

impl Focusable for WorkItemsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for WorkItemsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let phase_name = match self.phase_id {
            "plan" => "1. Plan",
            "invariants" => "2. Invariants",
            "implement" => "3. Implement",
            "verify" => "4. Verify",
            "review" => "5. Review",
            "complete" => "Completed",
            _ => "Plan",
        };

        let phase_goal = match self.phase_id {
            "plan" => "Define scope, map affected crates, author problem framing.",
            "invariants" => "Document state preconditions, postconditions, and idempotency.",
            "implement" => "Author minimal, strictly-bounded code changes adhering to invariants.",
            "verify" => "Run cargo test --workspace, inspect edge cases, zero regressions.",
            "review" => "Review behavior, evidence, findings, and the team's PR policy.",
            _ => "Milestone complete.",
        };
        let phase_index = WORKFLOW_PHASES
            .iter()
            .position(|(id, _)| *id == self.phase_id)
            .unwrap_or(0);

        let border_color = cx.theme().border;
        let text_color = cx.theme().sidebar_foreground;
        let group_box = cx.theme().group_box;
        let panel_bg = cx.theme().sidebar;
        let open_count = self.work_items.iter().filter(|i| matches!(i.status, ahead_rpc::ahead::WorkItemStatus::Open | ahead_rpc::ahead::WorkItemStatus::InProgress)).count();

        v_flex()
            .size_full()
            .min_h_0()
            .overflow_y_scrollbar()
            .p_3()
            .gap_2()
            .track_focus(&self.focus)
            // Session Header Card
            .child(
                v_flex()
                    .p_3()
                    .gap_1()
                    .bg(panel_bg)
                    .border_1()
                    .border_color(border_color)
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                v_flex()
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .items_center()
                                            .child(IconName::Zap)
                                            .child(
                                                div()
                                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                                    .text_color(cx.theme().warning)
                                                    .child("ACTIVE SESSION")
                                            )
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(text_color)
                                            .child(format!("{} · {}", self.work_kind.display_name(), self.work_title))
                                    )
                            )
                    )
            )
            // Workflow Stepper Card
            .child(
                v_flex()
                    .p_3()
                    .gap_1()
                    .bg(panel_bg)
                    .border_1()
                    .border_color(border_color)
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(IconName::Workflow)
                                    .child(
                                        div()
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .text_size(px(11.))
                                            .text_color(text_color)
                                            .child(format!("Workflow: {phase_name}"))
                                    )
                            )
                            .child(
                                Button::new("wi_workflow_toggle")
                                    .icon(if self.workflow_collapsed {
                                        IconName::ChevronRight
                                    } else {
                                        IconName::ChevronDown
                                    })
                                    .tooltip("Expand or collapse workflow")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                        this.workflow_collapsed = !this.workflow_collapsed;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("wi_advance_btn")
                                    .icon(IconName::ArrowRight)
                                    .label("Advance Phase")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| this.advance_phase(cx)))
                            )
                    )
                    .child(
                        Collapsible::new()
                            .open(!self.workflow_collapsed)
                            .motion_id("workflow-stepper-reveal")
                            .content(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        Stepper::new("workflow-stepper")
                                            .selected_index(phase_index)
                                            .items(WORKFLOW_PHASES.iter().map(|(_, label)| {
                                                StepperItem::new().child(*label)
                                            }))
                                            .on_click(cx.listener(|this: &mut Self, index, _, cx| {
                                                if let Some((phase_id, _)) = WORKFLOW_PHASES.get(*index) {
                                                    this.phase_id = phase_id;
                                                    this.status = format!("Workflow phase: {phase_id}").into();
                                                    cx.notify();
                                                }
                                            })),
                                    )
                                    .child(
                                        div()
                                            .p_1()
                                            .text_size(px(11.))
                                            .text_color(text_color)
                                            .child(phase_goal),
                                    ),
                            ),
                    )
            )
            // Voice Bar
            .child(
                h_flex()
                    .p_2()
                    .gap_2()
                    .items_center()
                    .bg(panel_bg)
                    .border_1()
                    .border_color(border_color)
                    .child(
                        Button::new("wi_voice_toggle")
                            .icon(if self.voice.active { IconName::Mic } else { IconName::MicOff })
                            .label(if self.voice.active { "Voice: Live" } else { "Voice: Off" })
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.toggle_voice(cx)))
                    )
                    .child(
                        Button::new("wi_barge_in")
                            .icon(IconName::Square)
                            .label("Barge In")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.barge_in(cx)))
                    )
                    .child(
                        Button::new("wi_mute_mic")
                            .icon(if self.voice.mic_muted { IconName::MicOff } else { IconName::Mic })
                            .label(if self.voice.mic_muted { "Unmute" } else { "Mute" })
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.toggle_mic_mute(cx)))
                    )
            )
            // Work Items Header & AI Propose Action
            .child(
                v_flex()
                    .p_3()
                    .gap_2()
                .bg(panel_bg)
                    .border_1()
                    .border_color(border_color)
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(IconName::ListTodo)
                                    .child(
                                        div()
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .text_size(px(12.))
                                            .text_color(text_color)
                                            .child(format!("CHECKLIST ({open_count} open / {} total)", self.work_items.len()))
                                    )
                            )
                            .child(
                                Button::new("ai_propose_btn")
                                    .icon(IconName::Sparkles)
                                    .label("AI Suggest Items")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| this.ai_propose_next_items(cx)))
                            )
                    )
                    // Inline Add Box
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Input::new(&self.work_item_input)
                                    .aria_label("Add work item...")
                                    .flex_1(),
                            )
                            .child(
                                Button::new("wi_add_btn")
                                    .primary()
                                    .icon(IconName::Plus)
                                    .label("Add")
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| this.add_item(window, cx)))
                            )
                    )
                    // Work Items List
                    .child(
                        v_flex()
                            .gap_1()
                            .children(
                                self.work_items.iter().map(|item| {
                                    let item_id = item.id.clone();
                                    let (status_icon, status_label) = match item.status {
                                        ahead_rpc::ahead::WorkItemStatus::Open => (IconName::Circle, "Open"),
                                        ahead_rpc::ahead::WorkItemStatus::InProgress => (IconName::CircleDashed, "Doing"),
                                        ahead_rpc::ahead::WorkItemStatus::Done => (IconName::CircleCheck, "Done"),
                                        ahead_rpc::ahead::WorkItemStatus::Dropped => (IconName::CircleX, "Dropped"),
                                    };

                                    let is_ai_authored = item.created_by == "ai-assistant";

                                    h_flex()
                                        .items_center()
                                        .justify_between()
                                        .p_2()
                                        .bg(group_box)
                                        .border_1()
                                        .border_color(border_color)
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .text_size(px(12.))
                                                        .text_color(text_color)
                                                        .child(item.title.clone())
                                                )
                                                .when(is_ai_authored, |el| {
                                                    el.child(
                                                        div()
                                                            .text_size(px(10.))
                                                            .text_color(cx.theme().warning)
                                                            .child("(AI suggested)")
                                                    )
                                                })
                                        )
                                        .child(
                                            Button::new(SharedString::from(format!("wi_cycle_{}", item_id)))
                                                .icon(status_icon)
                                                .label(status_label)
                                                .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                    this.cycle_status(&item_id, cx);
                                                }))
                                        )
                                })
                            )
                    )
            )
            // Code Proposals Section
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
