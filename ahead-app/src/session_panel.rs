//! AHEAD AI Pairing Agent Panel - GPUI Implementation
//!
//! Fully scrollable agent panel matching the Zed AI Agent aesthetic.
//! Uses gpui-kit components, Lucide icons, and dynamic theming.

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
    pub work_item_input: Entity<InputState>,
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
        let work_item_input = cx.new(|cx| InputState::new(window, cx));
        let mut session = ahead_viewmodel::SessionSnapshot::default();

        ahead_viewmodel::push_human_message(&mut session, "helo");
        ahead_viewmodel::push_agent_message(
            &mut session,
            ahead_viewmodel::AgentMessage {
                sender: "AHEAD Agent".into(),
                text: "Session started. Current phase: Plan. State your next reasoning step or ask for an edge-case challenge.".into(),
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
            work_item_input,
            session,
            voice: ahead_viewmodel::VoiceIntent::new(),
            mode_assist: true,
            phase_id: "plan",
            active_work_title: "Implement resilient retry logic".to_string(),
            active_work_kind: ahead_rpc::ahead::WorkKind::ProductChange,
            status: "AHEAD Agent ready".into(),
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
                        text: format!("Analyzed reasoning: '{}'. Consider state invariants before proceeding to code edits.", text),
                        is_challenge: false,
                    },
                );
            }

            self.status = format!("Chat updated ({} messages)", self.session.chat.len()).into();
        }
        cx.notify();
    }

    pub fn send_prompt(&mut self, prompt: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.chat_input.update(cx, |input, cx| input.set_value(prompt, window, cx));
        self.send_chat(window, cx);
    }

    pub fn toggle_mode(&mut self, cx: &mut Context<Self>) {
        self.mode_assist = !self.mode_assist;
        let mode = if self.mode_assist {
            ahead_rpc::ahead::AssistanceMode::Assist
        } else {
            ahead_rpc::ahead::AssistanceMode::Learn
        };
        let policy = ahead_viewmodel::approval_policy_for_mode(mode);
        self.status = format!(
            "Mode switched to {} (Policy: {})",
            if self.mode_assist { "Assist" } else { "Learn" },
            policy.codex_wire_value()
        )
        .into();
        cx.notify();
    }

    pub fn advance_phase(&mut self, cx: &mut Context<Self>) {
        let (next_id, next_title) = ahead_viewmodel::next_phase(self.phase_id);
        self.phase_id = next_id;
        ahead_viewmodel::push_agent_message(
            &mut self.session,
            ahead_viewmodel::AgentMessage {
                sender: "AHEAD Agent".into(),
                text: format!("Advanced to phase: {next_title}. Invariants check active."),
                is_challenge: false,
            },
        );
        self.status = format!("Phase advanced to {next_title}").into();
        cx.notify();
    }

    pub fn toggle_voice(&mut self, cx: &mut Context<Self>) {
        ahead_viewmodel::voice::toggle(&mut self.voice);
        self.status = format!(
            "Voice: {} (generation {})",
            if self.voice.active { "LIVE (listening)" } else { "OFF" },
            self.voice.generation
        )
        .into();
        cx.notify();
    }

    pub fn barge_in(&mut self, cx: &mut Context<Self>) {
        ahead_viewmodel::voice::barge_in(&mut self.voice);
        self.status = format!("Barge-in triggered (gen {})", self.voice.generation).into();
        cx.notify();
    }

    pub fn toggle_mic_mute(&mut self, cx: &mut Context<Self>) {
        self.voice.mic_muted = !self.voice.mic_muted;
        self.status = format!(
            "Microphone {}",
            if self.voice.mic_muted { "MUTED" } else { "ACTIVE" }
        )
        .into();
        cx.notify();
    }

    pub fn add_work_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub fn cycle_work_item_status(&mut self, item_id: &str, cx: &mut Context<Self>) {
        if let Some(item) = self.work_items.iter_mut().find(|i| i.id == item_id) {
            item.status = match item.status {
                ahead_rpc::ahead::WorkItemStatus::Open => ahead_rpc::ahead::WorkItemStatus::InProgress,
                ahead_rpc::ahead::WorkItemStatus::InProgress => ahead_rpc::ahead::WorkItemStatus::Done,
                ahead_rpc::ahead::WorkItemStatus::Done => ahead_rpc::ahead::WorkItemStatus::Dropped,
                ahead_rpc::ahead::WorkItemStatus::Dropped => ahead_rpc::ahead::WorkItemStatus::Open,
            };
            self.status = format!("Work item status: {:?}", item.status).into();
        }
        cx.notify();
    }

    pub fn authorize_proposal(&mut self, proposal_id: &str, cx: &mut Context<Self>) {
        if let Some(pos) = self.pending_proposals.iter().position(|p| p.id == proposal_id) {
            let prop = self.pending_proposals.remove(pos);
            ahead_viewmodel::push_agent_message(
                &mut self.session,
                ahead_viewmodel::AgentMessage {
                    sender: "AHEAD Agent".into(),
                    text: format!("Proposal {} authorized and applied by human engineer.", prop.id),
                    is_challenge: false,
                },
            );
            self.status = format!("Authorized proposal {}", prop.id).into();
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
            self.status = format!("Rejected proposal {}", prop.id).into();
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
        "AHEAD AI Agent"
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
        let chat_count = self.session.chat.len();
        let phase_name = match self.phase_id {
            "plan" => "Plan",
            "invariants" => "Invariants",
            "implement" => "Implement",
            "verify" => "Verify",
            "review" => "Review",
            "complete" => "Completed",
            _ => "Plan",
        };

        let phase_goal = match self.phase_id {
            "plan" => "Define scope, map affected crates, author problem framing.",
            "invariants" => "Document state preconditions, postconditions, and idempotency.",
            "implement" => "Author minimal, strictly-bounded code changes adhering to invariants.",
            "verify" => "Run cargo test --workspace, inspect edge cases, zero regressions.",
            "review" => "Peer inspection, human verification signoff, and tracker sync.",
            _ => "Milestone complete.",
        };

        let panel_bg = cx.theme().sidebar;
        let border_color = cx.theme().border;
        let text_color = cx.theme().sidebar_foreground;
        let group_box = cx.theme().group_box;
        let is_dark = cx.theme().mode.is_dark();

        v_flex()
            .size_full()
            .min_h_0()
            .overflow_y_scrollbar()
            .gap_2()
            .p_3()
            .track_focus(&self.focus)
            // Header Section: active work kind and mode badge
            .child(
                v_flex()
                    .p_3()
                    .gap_1()
                    .rounded_lg()
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
                                                    .text_color(if is_dark { gpui_kit::rgb(0xF59E0B) } else { gpui_kit::rgb(0xD97706) })
                                                    .child("AHEAD AI AGENT")
                                            )
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(text_color)
                                            .child(format!("{} · {}", self.active_work_kind.display_name(), self.active_work_title))
                                    )
                            )
                            .child(
                                Button::new("mode_toggle")
                                    .primary()
                                    .icon(if self.mode_assist { IconName::Code } else { IconName::Shield })
                                    .label(if self.mode_assist { "Assist" } else { "Learn" })
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| this.toggle_mode(cx)))
                            )
                    )
            )
            // Workflow Pipeline Section
            .child(
                v_flex()
                    .p_3()
                    .gap_1()
                    .rounded_lg()
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
                                Button::new("advance_btn")
                                    .icon(IconName::ArrowRight)
                                    .label("Advance")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| this.advance_phase(cx)))
                            )
                    )
                    .child(
                        div()
                            .p_1()
                            .text_size(px(11.))
                            .text_color(text_color)
                            .child(phase_goal)
                    )
            )
            // Voice Controls
            .child(
                h_flex()
                    .p_2()
                    .gap_2()
                    .items_center()
                    .rounded_lg()
                    .bg(panel_bg)
                    .border_1()
                    .border_color(border_color)
                    .child(
                        Button::new("voice_toggle")
                            .icon(if self.voice.active { IconName::Mic } else { IconName::MicOff })
                            .label(if self.voice.active { "Voice: Live" } else { "Voice: Off" })
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.toggle_voice(cx)))
                    )
                    .child(
                        Button::new("barge_in")
                            .icon(IconName::Square)
                            .label("Barge In")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.barge_in(cx)))
                    )
                    .child(
                        Button::new("mute_mic")
                            .icon(if self.voice.mic_muted { IconName::MicOff } else { IconName::Mic })
                            .label(if self.voice.mic_muted { "Unmute" } else { "Mute" })
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.toggle_mic_mute(cx)))
                    )
            )
            // Agent Pairing Feed Section (chat stream)
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded_lg()
                    .bg(panel_bg)
                    .border_1()
                    .border_color(border_color)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(IconName::MessageSquare)
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(11.))
                                    .text_color(text_color)
                                    .child(format!("Conversation ({chat_count} messages)"))
                            )
                    )
                    .children(
                        self.session.chat.iter().map(|msg| {
                            let is_human = msg.sender == "Human";
                            let is_challenge = msg.is_challenge;
                            let sender_color = if is_human {
                                gpui_kit::rgb(0x60A5FA)
                            } else if is_challenge {
                                gpui_kit::rgb(0xFBBF24)
                            } else {
                                gpui_kit::rgb(0x34D399)
                            };

                            v_flex()
                                .p_2()
                                .gap_1()
                                .rounded_md()
                                .bg(if is_human {
                                    group_box
                                } else {
                                    panel_bg
                                })
                                .border_1()
                                .border_color(border_color)
                                .child(
                                    div()
                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                        .text_size(px(11.))
                                        .text_color(sender_color)
                                        .child(msg.sender.clone())
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(text_color)
                                        .child(msg.text.clone())
                                )
                        })
                    )
            )
            // Quick Action Prompt Chips
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("prompt_edge")
                            .icon(IconName::ShieldAlert)
                            .label("Edge Cases")
                            .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                this.send_prompt("Challenge my edge cases for this phase", window, cx);
                            }))
                    )
                    .child(
                        Button::new("prompt_inv")
                            .icon(IconName::ShieldCheck)
                            .label("Invariants")
                            .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                this.send_prompt("What invariants must hold here?", window, cx);
                            }))
                    )
                    .child(
                        Button::new("prompt_scaffold")
                            .icon(IconName::Code)
                            .label("Scaffold")
                            .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                this.send_prompt("Scaffold boilerplate for the active task", window, cx);
                            }))
                    )
            )
            // Zed-Style Bottom Chat Composer
            .child(
                v_flex()
                    .p_2()
                    .gap_2()
                    .rounded_lg()
                    .bg(group_box)
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
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(text_color)
                                            .child("AHEAD Model")
                                    )
                            )
                            .child(
                                Button::new("send_btn")
                                    .primary()
                                    .icon(IconName::Send)
                                    .label("Send")
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| this.send_chat(window, cx)))
                            )
                    )
            )
            // Work Items Checklist Section
            .child(
                v_flex()
                    .gap_1()
                    .p_3()
                    .rounded_lg()
                    .bg(panel_bg)
                    .border_1()
                    .border_color(border_color)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(IconName::ListTodo)
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(11.))
                                    .text_color(text_color)
                                    .child(format!("Work Items ({} items)", self.work_items.len()))
                            )
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Input::new(&self.work_item_input).aria_label("Work item title").flex_1())
                            .child(
                                Button::new("add_wi_btn")
                                    .icon(IconName::Plus)
                                    .label("Add")
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| this.add_work_item(window, cx)))
                            )
                    )
                    .children(
                        self.work_items.iter().map(|item| {
                            let item_id = item.id.clone();
                            let (status_icon, status_label) = match item.status {
                                ahead_rpc::ahead::WorkItemStatus::Open => (IconName::Circle, "Open"),
                                ahead_rpc::ahead::WorkItemStatus::InProgress => (IconName::CircleDashed, "Doing"),
                                ahead_rpc::ahead::WorkItemStatus::Done => (IconName::CircleCheck, "Done"),
                                ahead_rpc::ahead::WorkItemStatus::Dropped => (IconName::CircleX, "Dropped"),
                            };

                            h_flex()
                                .items_center()
                                .justify_between()
                                .p_2()
                                .rounded_md()
                                .bg(group_box)
                                .border_1()
                                .border_color(border_color)
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(text_color)
                                        .child(item.title.clone())
                                )
                                .child(
                                    Button::new(SharedString::from(format!("wi_status_{}", item_id)))
                                        .icon(status_icon)
                                        .label(status_label)
                                        .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                            this.cycle_work_item_status(&item_id, cx);
                                        }))
                                )
                        })
                    )
            )
            // Mechanical Proposals Gate Section
            .child(
                v_flex()
                    .gap_1()
                    .p_3()
                    .rounded_lg()
                    .bg(panel_bg)
                    .border_1()
                    .border_color(border_color)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(IconName::FileCode)
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(11.))
                                    .text_color(if is_dark { gpui_kit::rgb(0x60A5FA) } else { gpui_kit::rgb(0x2563EB) })
                                    .child(format!("Code Proposals ({} pending)", self.pending_proposals.len()))
                            )
                    )
                    .children(
                        self.pending_proposals.iter().map(|prop| {
                            let prop_id = prop.id.clone();
                            let prop_id_reject = prop.id.clone();
                            v_flex()
                                .p_2()
                                .gap_1()
                                .rounded_md()
                                .bg(group_box)
                                .border_1()
                                .border_color(border_color)
                                .child(
                                    div()
                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                        .text_size(px(11.))
                                        .text_color(text_color)
                                        .child(format!("File: {}", prop.path))
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(text_color)
                                        .child(prop.description.clone())
                                )
                                .child(
                                    div()
                                        .p_2()
                                        .rounded_md()
                                        .bg(if is_dark { gpui_kit::rgb(0x020617) } else { gpui_kit::rgb(0x0F172A) })
                                        .text_size(px(11.))
                                        .text_color(gpui_kit::rgb(0x86EFAC))
                                        .child(prop.patch.clone())
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            Button::new(SharedString::from(format!("auth_{}", prop_id)))
                                                .primary()
                                                .icon(IconName::Check)
                                                .label("Authorize & Apply")
                                                .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                    this.authorize_proposal(&prop_id, cx);
                                                }))
                                        )
                                        .child(
                                            Button::new(SharedString::from(format!("rej_{}", prop_id_reject)))
                                                .icon(IconName::X)
                                                .label("Reject")
                                                .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                    this.reject_proposal(&prop_id_reject, cx);
                                                }))
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
