//! AHEAD Code Editor Panel - GPUI Implementation
//!
//! Modern editor matching Zed and VS Code:
//! - NO save button: Cmd+S / Ctrl+S saves directly.
//! - NO intrusive alert banners: agent points directly with clean status.
//! - File tab bar with icon, dirty indicator (`•`), and close button.
//! - Path breadcrumb navigation.
//! - Real Tree-sitter highlighted code editor with line numbers and folding.

use gpui_kit::*;
use gpui_kit::prelude::*;
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
    pub active_line: u32,
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
            status: "Ready".into(),
            active_line: 1,
        }
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value().to_string();
        match std::fs::write(&self.file_path, text) {
            Ok(()) => {
                self.saved_rev = self.dirty_rev;
                self.status = format!("Saved {}", self.file_path).into();
            }
            Err(e) => {
                self.status = format!("Save failed: {e}").into();
            }
        }
        cx.notify();
    }

    pub fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let cmd = event.keystroke.modifiers.platform || event.keystroke.modifiers.control;

        if cmd && key == "s" {
            self.save(cx);
        }
    }
}

impl BasePanel for CodePanel {
    fn panel_name(&self) -> &'static str {
        "ahead_code"
    }
}

impl Panel for CodePanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let file_name = std::path::Path::new(&self.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled");
        let dirty = self.dirty_rev != self.saved_rev;
        let suffix = if dirty { " •" } else { "" };
        SharedString::from(format!("{}{}", file_name, suffix))
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
        let border_color = cx.theme().border;
        let text_color = cx.theme().sidebar_foreground;
        let is_dark = cx.theme().mode.is_dark();
        let bar_bg = if is_dark { gpui_kit::rgb(0x121214) } else { gpui_kit::rgb(0xF8FAFC) };
        let editor_bg = if is_dark { gpui_kit::rgb(0x18181B) } else { gpui_kit::rgb(0xFFFFFF) };

        let file_name = std::path::Path::new(&self.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled");

        v_flex()
            .size_full()
            .bg(editor_bg)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this: &mut Self, event: &KeyDownEvent, _, cx| {
                this.handle_key(event, cx);
            }))
            // Breadcrumb path navigation bar (clean, matching Zed / VS Code)
            .child(
                h_flex()
                    .h(px(28.))
                    .items_center()
                    .justify_between()
                    .px_3()
                    .bg(bar_bg)
                    .border_b_1()
                    .border_color(border_color)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(IconName::FileCode)
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(text_color)
                                    .child(self.file_path.clone())
                            )
                            .when(dirty, |el| {
                                el.child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(gpui_kit::rgb(0xF59E0B))
                                        .child("• modified (Cmd+S to save)")
                                )
                            })
                    )
            )
            // Real GPUI Code Editor
            .child(
                div()
                    .flex_1()
                    .bg(editor_bg)
                    .child(
                        Editor::new(&self.editor)
                            .aria_label("AHEAD Code Editor")
                            .h_full()
                    )
            )
            // Footer status strip
            .child(
                h_flex()
                    .h(px(22.))
                    .items_center()
                    .justify_between()
                    .px_3()
                    .bg(bar_bg)
                    .border_t_1()
                    .border_color(border_color)
                    .text_size(px(11.))
                    .text_color(text_color)
                    .child(format!("{} · Ln {}, Col 1", file_name, self.active_line))
                    .child(self.status.clone())
            )
    }
}
