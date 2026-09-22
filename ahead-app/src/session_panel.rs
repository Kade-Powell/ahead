//! AHEAD AI Agent Conversation Panel - GPUI Implementation
//!
//! True conversational AI interface matching Zed and modern assistant panels:
//! - Thread header with title (`helo`), model badge, and action buttons.
//! - Chat stream with user message bubbles, agent responses, and inline plan cards.
//! - Inline code proposals gate within the conversation flow.
//! - Modern bottom composer: "Message the AHEAD Agent, @ to include context, / for commands".

use gpui_kit::*;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent,
};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{v_flex, h_flex, ActiveTheme};
use gpui_kit_assets::IconName;

pub struct SessionPanel {
    pub focus: FocusHandle,
    pub chat_input: Entity<InputState>,
    pub session: ahead_viewmodel::SessionSnapshot,
    pub voice: ahead_viewmodel::VoiceIntent,
    pub mode_assist: bool,
    pub phase_id: &'static str,
    pub active_work_title: String,
    pub active_work_kind: ahead_rpc::ahead::WorkKind,
    pub status: SharedString,
    pub work_items: Vec<ahead_rpc::ahead::WorkItem>,
    pub pending_proposals: Vec<ahead_rpc::ahead::ChangeProposal>,
}

impl SessionPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let chat_input = cx.new(|cx| InputState::new(window, cx));
        let mut session = ahead_viewmodel::SessionSnapshot::default();

        ahead_viewmodel::push_human_message(&mut session, "helo");
        ahead_viewmodel::push_agent_message(
            &mut session,
            ahead_viewmodel::AgentMessage {
                sender: "AHEAD Agent".into(),
                text: "I've analyzed the problem. Let's work through the implementation together. State your next step or ask for an edge-case challenge.".into(),
                is_challenge: false,
            },
        );

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
        ];

        let initial_proposals = vec![
            ahead_rpc::ahead::ChangeProposal {
                id: "prop-scaffold-1".to_string(),
                session_id: "sess-current".to_string(),
                path: "src/service.rs".to_string(),
                original_sha256: "00000000".to_string(),
                patch: "@@ -1,3 +1,12 @@\n+#[derive(Debug, Clone)]\n+pub struct RetryPolicy {\n+    pub max_retries: u32,\n+    pub backoff_ms: u64,\n+}\n".to_string(),
                is_mechanical: true,
                description: "Scaffold mechanical RetryPolicy struct boilerplate".to_string(),
                recommended_cursor: None,
            },
        ];

        Self {
            focus: cx.focus_handle(),
            chat_input,
            session,
            voice: ahead_viewmodel::VoiceIntent::new(),
            mode_assist: true,
            phase_id: "plan",
            active_work_title: "Implement resilient retry logic".to_string(),
            active_work_kind: ahead_rpc::ahead::WorkKind::ProductChange,
            status: "Connected".into(),
            work_items: initial_items,
            pending_proposals: initial_proposals,
        }
    }

    pub fn send_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.chat_input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }

        if ahead_viewmodel::push_human_message(&mut self.session, &text) {
            self.chat_input.update(cx, |input, cx| input.set_value("", window, cx));
            let text_lower = text.to_lowercase();

            if text_lower.contains("edge") || text_lower.contains("challenge") {
                ahead_viewmodel::push_agent_message(
                    &mut self.session,
                    ahead_viewmodel::AgentMessage {
                        sender: "Socratic Guide".into(),
                        text: "Challenge: What happens if network timeout occurs during state mutation? Is idempotency preserved?".into(),
                        is_challenge: true,
                    },
                );
            } else if text_lower.contains("scaffold") || text_lower.contains("boilerplate") {
                if self.mode_assist {
                    ahead_viewmodel::push_agent_message(
                        &mut self.session,
                        ahead_viewmodel::AgentMessage {
                            sender: "AHEAD Agent".into(),
                            text: "I have prepared mechanical scaffolding in the proposal gate below. Review the diff before authorizing.".into(),
                            is_challenge: false,
                        },
                    );
                    self.pending_proposals.push(ahead_rpc::ahead::ChangeProposal {
                        id: format!("prop-{}", &uuid::Uuid::new_v4().to_string()[..8]),
                        session_id: "sess-current".to_string(),
                        path: "src/retry.rs".to_string(),
                        original_sha256: "00000000".to_string(),
                        patch: "@@ -0,0 +1,8 @@\n+pub fn calculate_backoff(attempt: u32) -> u64 {\n+    (2u64.pow(attempt) * 100).min(5000)\n+}\n".to_string(),
                        is_mechanical: true,
                        description: "Generate exponential backoff calculation helper".to_string(),
                        recommended_cursor: None,
                    });
                } else {
                    ahead_viewmodel::push_agent_message(
                        &mut self.session,
                        ahead_viewmodel::AgentMessage {
                            sender: "AHEAD Agent".into(),
                            text: "Learn mode policy denies agent-authored scaffolding. Let's reason through the implementation together.".into(),
                            is_challenge: false,
                        },
                    );
                }
            } else {
                ahead_viewmodel::push_agent_message(
                    &mut self.session,
                    ahead_viewmodel::AgentMessage {
                        sender: "AHEAD Agent".into(),
                        text: format!("Analyzed: '{}'. Verify state preconditions before proceeding.", text),
                        is_challenge: false,
                    },
                );
            }

            self.status = format!("{} messages", self.session.chat.len()).into();
        }
        cx.notify();
    }

    pub fn toggle_mode(&mut self, cx: &mut Context<Self>) {
        self.mode_assist = !self.mode_assist;
        cx.notify();
    }

    pub fn advance_phase(&mut self, cx: &mut Context<Self>) {
        let (next_id, next_title) = ahead_viewmodel::next_phase(self.phase_id);
        self.phase_id = next_id;
        ahead_viewmodel::push_agent_message(
            &mut self.session,
            ahead_viewmodel::AgentMessage {
                sender: "AHEAD Agent".into(),
                text: format!("Phase advanced: {next_title}."),
                is_challenge: false,
            },
        );
        cx.notify();
    }

    pub fn authorize_proposal(&mut self, proposal_id: &str, cx: &mut Context<Self>) {
        if let Some(pos) = self.pending_proposals.iter().position(|p| p.id == proposal_id) {
            let prop = self.pending_proposals.remove(pos);
            ahead_viewmodel::push_agent_message(
                &mut self.session,
                ahead_viewmodel::AgentMessage {
                    sender: "AHEAD Agent".into(),
                    text: format!("Proposal {} authorized and applied.", prop.id),
                    is_challenge: false,
                },
            );
        }
        cx.notify();
    }

    pub fn reject_proposal(&mut self, proposal_id: &str, cx: &mut Context<Self>) {
        if let Some(pos) = self.pending_proposals.iter().position(|p| p.id == proposal_id) {
            let prop = self.pending_proposals.remove(pos);
            ahead_viewmodel::push_agent_message(
                &mut self.session,
                ahead_viewmodel::AgentMessage {
                    sender: "AHEAD Agent".into(),
                    text: format!("Proposal {} rejected.", prop.id),
                    is_challenge: false,
                },
            );
        }
        cx.notify();
    }
}

impl BasePanel for SessionPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_agent"
    }
}

impl Panel for SessionPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        "helo"
    }
    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}

impl EventEmitter<PanelEvent> for SessionPanel {}

impl Focusable for SessionPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SessionPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border_color = cx.theme().border;
        let text_color = cx.theme().sidebar_foreground;
        let is_dark = cx.theme().mode.is_dark();
        let bg_color = if is_dark { gpui_kit::rgb(0x18181B) } else { gpui_kit::rgb(0xFFFFFF) };
        let bubble_user_bg = if is_dark { gpui_kit::rgb(0x27272A) } else { gpui_kit::rgb(0xF1F5F9) };
        let card_bg = if is_dark { gpui_kit::rgb(0x202023) } else { gpui_kit::rgb(0xF8FAFC) };

        v_flex()
            .size_full()
            .bg(bg_color)
            .track_focus(&self.focus)
            // Top Thread Title Header
            .child(
                h_flex()
                    .h(px(38.))
                    .items_center()
                    .justify_between()
                    .px_3()
                    .border_b_1()
                    .border_color(border_color)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(13.))
                                    .text_color(text_color)
                                    .child("helo")
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(gpui_kit::rgb(0x71717A))
                                    .child("· Claude 3.5 Sonnet")
                            )
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("mode_pill")
                                    .icon(if self.mode_assist { IconName::Code } else { IconName::Shield })
                                    .label(if self.mode_assist { "Assist" } else { "Learn" })
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| this.toggle_mode(cx)))
                            )
                    )
            )
            // Scrollable Conversation Message Stream
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .gap_3()
                    .overflow_y_scrollbar()
                    // Conversation messages
                    .children(
                        self.session.chat.iter().map(|msg| {
                            let is_human = msg.sender == "Human";
                            let is_challenge = msg.is_challenge;

                            if is_human {
                                // User prompt bubble: clean, right-aligned, matching Zed
                                h_flex()
                                    .justify_end()
                                    .child(
                                        div()
                                            .max_w(px(400.))
                                            .p_2()
                                            .bg(bubble_user_bg)
                                            .text_size(px(13.))
                                            .text_color(text_color)
                                            .child(msg.text.clone())
                                    )
                            } else {
                                // Agent reply block: clean typography, avatar
                                v_flex()
                                    .gap_1()
                                    .p_1()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                if is_challenge { IconName::ShieldAlert } else { IconName::Bot }
                                            )
                                            .child(
                                                div()
                                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                                    .text_size(px(12.))
                                                    .text_color(if is_challenge { gpui_kit::rgb(0xFBBF24) } else { gpui_kit::rgb(0x34D399) })
                                                    .child(msg.sender.clone())
                                            )
                                    )
                                    .child(
                                        div()
                                            .pl_5()
                                            .text_size(px(13.))
                                            .line_height(px(20.))
                                            .text_color(text_color)
                                            .child(msg.text.clone())
                                    )
                            }
                        })
                    )
                    // Inline Plan / Work Items card inside the conversation
                    .child(
                        v_flex()
                            .p_3()
                            .gap_2()
                            .bg(card_bg)
                            .border_1()
                            .border_color(border_color)
                            .child(
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(IconName::ListTodo)
                                            .child(
                                                div()
                                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                                    .text_size(px(12.))
                                                    .text_color(text_color)
                                                    .child(format!("Plan: {} · Phase {}", self.active_work_title, self.phase_id))
                                            )
                                    )
                                    .child(
                                        Button::new("advance_btn")
                                            .icon(IconName::ArrowRight)
                                            .label("Advance")
                                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.advance_phase(cx)))
                                    )
                            )
                            .children(
                                self.work_items.iter().map(|item| {
                                    let is_done = matches!(item.status, ahead_rpc::ahead::WorkItemStatus::Done);
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(if is_done { IconName::CircleCheck } else { IconName::Circle })
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(if is_done { gpui_kit::rgb(0x71717A).into() } else { text_color })
                                                .child(item.title.clone())
                                        )
                                })
                            )
                    )
                    // Inline Code Proposal cards inside the conversation
                    .children(
                        self.pending_proposals.iter().map(|prop| {
                            let pid = prop.id.clone();
                            let pid_rej = prop.id.clone();
                            v_flex()
                                .p_3()
                                .gap_2()
                                .bg(card_bg)
                                .border_1()
                                .border_color(border_color)
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(IconName::FileCode)
                                        .child(
                                            div()
                                                .font_weight(gpui_kit::FontWeight::BOLD)
                                                .text_size(px(12.))
                                                .text_color(text_color)
                                                .child(format!("Proposal: {}", prop.path))
                                        )
                                )
                                .child(
                                    div()
                                        .p_2()
                                        .bg(if is_dark { gpui_kit::rgb(0x0C0E14) } else { gpui_kit::rgb(0xF1F5F9) })
                                        .text_size(px(11.))
                                        .font_family("Menlo")
                                        .text_color(gpui_kit::rgb(0x86EFAC))
                                        .child(prop.patch.clone())
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            Button::new(SharedString::from(format!("auth_{}", pid)))
                                                .primary()
                                                .icon(IconName::Check)
                                                .label("Authorize & Apply")
                                                .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                    this.authorize_proposal(&pid, cx);
                                                }))
                                        )
                                        .child(
                                            Button::new(SharedString::from(format!("rej_{}", pid_rej)))
                                                .icon(IconName::X)
                                                .label("Reject")
                                                .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                    this.reject_proposal(&pid_rej, cx);
                                                }))
                                        )
                                )
                        })
                    )
            )
            // Zed-Style Bottom Chat Composer
            .child(
                v_flex()
                    .m_3()
                    .p_2()
                    .gap_2()
                    .bg(card_bg)
                    .border_1()
                    .border_color(border_color)
                    .child(
                        Input::new(&self.chat_input)
                            .aria_label("Message the AHEAD Agent, @ to include context, / for commands")
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Button::new("add_context_btn")
                                            .icon(IconName::Plus)
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(gpui_kit::rgb(0x71717A))
                                            .child("Claude 3.5 Sonnet")
                                    )
                            )
                            .child(
                                Button::new("send_btn")
                                    .primary()
                                    .icon(IconName::Send)
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| this.send_chat(window, cx)))
                            )
                    )
            )
    }
}
