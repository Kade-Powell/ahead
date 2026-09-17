//! AHEAD Code Editor Panel - GPUI Implementation
//!
//! Uses gpui-kit components, Lucide icons, and dynamic Dark/Light theming.
//! Grounded in Sections 3.1, 4.4, and 4.5 of `ahead-editor-mvp.md`.

use gpui_kit::*;
use gpui_kit::prelude::*;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent,
};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, TabSize};
use gpui_kit::component::{v_flex, h_flex, ActiveTheme};
use gpui_kit_assets::IconName;

pub struct CodePanel {
    pub focus: FocusHandle,
    pub editor: Entity<EditorState>,
    pub file_path: String,
    pub saved_rev: usize,
    pub dirty_rev: usize,
    pub status: SharedString,
    pub cue_info: Option<(String, u32)>,
}

impl CodePanel {
    pub fn new(path: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = std::fs::read_to_string(path).unwrap_or_else(|_| {
            "//! Welcome to AHEAD - The AI-Native Native Editor\n\nfn main() {\n    println!(\"AHEAD is ready for human-led engineering.\");\n}\n".to_string()
        });

        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("rust")
                .line_number(true)
                .folding(true)
                .tab_size(TabSize { tab_size: 4, hard_tabs: false })
                .default_value(text)
        });

        cx.subscribe(&editor, |this: &mut Self, _state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.dirty_rev += 1;
                cx.notify();
            }
        })
        .detach();

        Self {
            focus: cx.focus_handle(),
            editor,
            file_path: path.to_string(),
            saved_rev: 0,
            dirty_rev: 0,
            status: "File loaded into editor".into(),
            cue_info: Some(("Focus on retry invariants".to_string(), 3)),
        }
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value().to_string();
        match std::fs::write(&self.file_path, text) {
            Ok(()) => {
                self.saved_rev = self.dirty_rev;
                self.status = format!("Saved to {}", self.file_path).into();
            }
            Err(e) => {
                self.status = format!("Save failed: {e}").into();
            }
        }
        cx.notify();
    }

    pub fn set_presentation_cue(&mut self, label: String, line: u32, cx: &mut Context<Self>) {
        self.cue_info = Some((label, line));
        self.status = format!("Agent cue target set at line {line}").into();
        cx.notify();
    }

    pub fn dismiss_cue(&mut self, cx: &mut Context<Self>) {
        self.cue_info = None;
        cx.notify();
    }
}

impl BasePanel for CodePanel {
    fn panel_name(&self) -> &'static str {
        "ahead_code"
    }
}

impl Panel for CodePanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let name = std::path::Path::new(&self.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Code");
        SharedString::from(name.to_string())
    }

    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}

impl EventEmitter<PanelEvent> for CodePanel {}

impl Focusable for CodePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for CodePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dirty = self.dirty_rev != self.saved_rev;
        let cue_info = self.cue_info.clone();
        let bar_bg = cx.theme().sidebar;
        let border_color = cx.theme().border;
        let text_color = cx.theme().sidebar_foreground;
        let is_dark = cx.theme().mode.is_dark();

        v_flex()
            .size_full()
            .track_focus(&self.focus)
            // Top Toolbar: Path, Clean/Dirty Status, Save Button
            .child(
                h_flex()
                    .h(px(36.))
                    .items_center()
                    .justify_between()
                    .px_3()
                    .gap_2()
                    .bg(bar_bg)
                    .border_b_1()
                    .border_color(border_color)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(12.))
                                    .text_color(text_color)
                                    .child(self.file_path.clone())
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(if dirty {
                                        if is_dark { gpui_kit::rgb(0xF59E0B) } else { gpui_kit::rgb(0xD97706) }
                                    } else {
                                        if is_dark { gpui_kit::rgb(0x10B981) } else { gpui_kit::rgb(0x059669) }
                                    })
                                    .child(if dirty { "[modified • unsaved]" } else { "[clean]" })
                            )
                    )
                    .child(
                        Button::new("save_btn")
                            .primary()
                            .icon(IconName::Save)
                            .label("Save")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.save(cx)))
                    )
            )
            // Presentation Cue Banner (if active)
            .when_some(cue_info, |this, (label, line)| {
                this.child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .px_3()
                        .py_2()
                        .bg(if is_dark { gpui_kit::rgb(0x133E2F) } else { gpui_kit::rgb(0xECFDF5) })
                        .border_b_1()
                        .border_color(if is_dark { gpui_kit::rgb(0x059669) } else { gpui_kit::rgb(0xA7F3D0) })
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(IconName::Target)
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(if is_dark { gpui_kit::rgb(0x34D399) } else { gpui_kit::rgb(0x065F46) })
                                        .child(format!("Agent Pointer: {label} (pointing to line {line})"))
                                )
                        )
                        .child(
                            Button::new("dismiss_cue_btn")
                                .icon(IconName::X)
                                .label("Dismiss")
                                .on_click(cx.listener(|this: &mut Self, _, _, cx| this.dismiss_cue(cx)))
                        )
                )
            })
            // Real GPUI Code Editor
            .child(
                div()
                    .flex_1()
                    .child(
                        Editor::new(&self.editor)
                            .aria_label("AHEAD Code Editor")
                            .h_full()
                    )
            )
            // Footer status
            .child(
                h_flex()
                    .h(px(24.))
                    .items_center()
                    .px_3()
                    .bg(bar_bg)
                    .border_t_1()
                    .border_color(border_color)
                    .text_size(px(11.))
                    .text_color(text_color)
                    .child(self.status.clone())
            )
    }
}
