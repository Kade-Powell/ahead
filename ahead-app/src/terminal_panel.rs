//! AHEAD bottom terminal panel - streams `ahead-proxy` output.

use gpui_kit::*;
use gpui_kit::component::button::Button;
use gpui_kit::component::dock::{BasePanel, Panel, PanelControl, PanelEvent};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme};
use gpui_kit_assets::IconName;

pub struct TerminalPanel {
    pub focus: FocusHandle,
    pub lines: Vec<String>,
    pub status: SharedString,
}

impl TerminalPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            lines: vec!["AHEAD terminal ready. Proxy output streams here.".to_string()],
            status: "idle".into(),
        }
    }

    pub fn append(&mut self, line: String, cx: &mut Context<Self>) {
        self.lines.push(line);
        if self.lines.len() > 400 {
            let excess = self.lines.len() - 400;
            self.lines.drain(0..excess);
        }
        cx.notify();
    }
}

impl BasePanel for TerminalPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_terminal"
    }
}

impl Panel for TerminalPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        "Terminal"
    }
    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}

impl EventEmitter<PanelEvent> for TerminalPanel {}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let text = cx.theme().sidebar_foreground;
        let group = cx.theme().group_box;
        v_flex()
            .size_full()
            .p_2()
            .gap_1()
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(IconName::Terminal)
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_size(px(11.))
                            .text_color(text)
                            .child("TERMINAL"),
                    )
                    .child(
                        Button::new("terminal_clear")
                            .icon(IconName::Trash)
                            .label("Clear")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.lines.clear();
                                this.status = "cleared".into();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap_0()
                    .p_2()
                    .rounded_lg()
                    .bg(group)
                    .border_1()
                    .border_color(border)
                    .children(self.lines.iter().map(|line| {
                        div().text_size(px(11.)).text_color(text).child(line.clone())
                    })),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(text)
                    .child(self.status.clone()),
            )
    }
}
