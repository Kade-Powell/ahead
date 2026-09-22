//! AHEAD Code Editor Panel - proxy-backed GPUI implementation.
//!
//! - Tree-sitter editor with Cmd+S save, no save button.
//! - Proxy-backed LSP completions (Ctrl+Space), hover, go-to-definition.
//! - Proxy-backed FIM inline ghost text (Tab accepts).
//! - Native Git gutter markers from `git diff --unified=0` (green add, amber modify,
//!   red deletion boundary).
//!   A second rail shows attribution: who authored each uncommitted region.
//! - Breakpoint gutter rail synced to `ProxyClient::DebugState`.
//! - Diagnostics squiggles from proxy `PublishDiagnostics`.

use std::{rc::Rc, sync::Arc};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent, PanelId, TabGroup,
};
use gpui_kit::component::input::{
    Editor, EditorState, InputEvent, RopeExt, TabSize,
};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit_assets::IconName;

use crate::proxy_client::{DiffHunkKind, ProxyClient};

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: String,
    pub detail: String,
    pub insert_text: String,
    pub icon: IconName,
}

#[derive(Clone, Debug)]
pub struct DiagnosticItem {
    pub line: u32,
    pub message: String,
    pub is_error: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeTabAction {
    Promote,
    Close,
    CloseOthers,
    CloseAll,
    CloseLeft,
    CloseRight,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GitLensLine {
    author: String,
    date: String,
    subject: String,
    commit: String,
}

pub struct CodePanel {
    pub focus: FocusHandle,
    pub editor: Entity<EditorState>,
    pub file_path: String,
    pub saved_rev: usize,
    pub dirty_rev: usize,
    pub status: SharedString,
    pub active_line: u32,
    pub completions: Vec<CompletionItem>,
    pub selected_completion: usize,
    pub show_completions: bool,
    pub ghost_text: Option<String>,
    /// Byte offset in the editor text where the current ghost was
    /// requested. Lets edits narrow the ghost (Zed-style interpolation)
    /// instead of always dismissing it.
    pub ghost_offset: Option<usize>,
    pub diagnostics: Vec<DiagnosticItem>,
    pub proxy: Option<Arc<ProxyClient>>,
    pub workspace: String,
    pub hover_text: Option<String>,
    pub is_preview: bool,
    git_lens: Option<GitLensLine>,
    git_lens_key: Option<(String, u32, usize)>,
    /// Invalidates asynchronous completion/FIM results after edits or file switches.
    pub request_generation: u64,
    loading: bool,
    close_handler: Option<Rc<dyn Fn(PanelId, &mut Window, &mut App)>>,
    tab_handler: Option<Rc<dyn Fn(PanelId, CodeTabAction, &mut Window, &mut App)>>,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
}

impl CodePanel {
    pub fn new(path: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = std::fs::read_to_string(path).unwrap_or_default();

        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("rust")
                .line_number(true)
                .folding(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                .default_value(text)
        });

        let editor_for_observer = editor.clone();
        cx.observe(&editor_for_observer, |_, _, cx| cx.notify())
            .detach();

        cx.subscribe(
            &editor,
            |this: &mut Self, _state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.dirty_rev += 1;
                    if !this.loading {
                        this.is_preview = false;
                    }
                    this.request_generation =
                        this.request_generation.wrapping_add(1);
                    // Zed-style interpolation: when the user types a prefix of
                    // the ghost, keep the remainder; any divergence dismisses it.
                    let ghost = this.ghost_text.take();
                    let start = this.ghost_offset.take();
                    if let (Some(ghost), Some(start)) = (ghost, start) {
                        let text = this.editor.read(cx).value().to_string();
                        let caret =
                            caret_byte_offset(&text, this.cursor_position(cx));
                        let rest = caret.and_then(|caret| {
                            if caret < start {
                                return None;
                            }
                            text.get(start..caret).and_then(|typed| {
                                interpolate_insertion(&ghost, typed)
                            })
                        });
                        this.ghost_text = rest;
                        if this.ghost_text.is_some() {
                            this.ghost_offset = Some(start);
                        }
                    }
                    cx.notify();
                }
            },
        )
        .detach();

        Self {
            focus: cx.focus_handle(),
            editor,
            file_path: path.to_string(),
            saved_rev: 0,
            dirty_rev: 0,
            status: "".into(),
            active_line: 1,
            completions: Vec::new(),
            selected_completion: 0,
            show_completions: false,
            ghost_text: None,
            ghost_offset: None,
            diagnostics: Vec::new(),
            proxy: None,
            workspace: String::new(),
            hover_text: None,
            is_preview: false,
            git_lens: None,
            git_lens_key: None,
            request_generation: 0,
            loading: false,
            close_handler: None,
            tab_handler: None,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
        }
    }

    pub fn with_proxy(mut self, proxy: Arc<ProxyClient>, workspace: &str) -> Self {
        self.proxy = Some(proxy);
        self.workspace = workspace.to_string();
        if let Some(proxy) = self.proxy.as_ref() {
            proxy.refresh_anchors(&[self.proxy_path()]);
        }
        self
    }

    pub fn set_close_handler<F>(&mut self, handler: F)
    where
        F: Fn(PanelId, &mut Window, &mut App) + 'static,
    {
        self.close_handler = Some(Rc::new(handler));
    }

    pub fn set_tab_handler<F>(&mut self, handler: F)
    where
        F: Fn(PanelId, CodeTabAction, &mut Window, &mut App) + 'static,
    {
        self.tab_handler = Some(Rc::new(handler));
    }

    pub fn promote_preview(&mut self, cx: &mut Context<Self>) {
        if self.is_preview {
            self.is_preview = false;
            cx.notify();
        }
    }

    fn proxy_path(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(&self.file_path)
    }

    pub fn turn_context(&self, cx: &App) -> ahead_rpc::ahead::TurnEditorContext {
        let editor = self.editor.read(cx);
        let position = editor.cursor_position();
        let selected_range = editor.selected_range();
        let selection = if selected_range.start != selected_range.end {
            let start = editor.text().offset_to_position(selected_range.start);
            let end = editor.text().offset_to_position(selected_range.end);
            Some(ahead_rpc::ahead::DisplayRange {
                start: ahead_rpc::ahead::DisplayPosition {
                    line: start.line,
                    col: start.character,
                },
                end: ahead_rpc::ahead::DisplayPosition {
                    line: end.line,
                    col: end.character,
                },
            })
        } else {
            None
        };
        ahead_rpc::ahead::TurnEditorContext {
            active_path: std::path::Path::new(&self.file_path)
                .strip_prefix(&self.workspace)
                .unwrap_or_else(|_| std::path::Path::new(&self.file_path))
                .to_string_lossy()
                .to_string(),
            caret: ahead_rpc::ahead::DisplayPosition {
                line: position.line,
                col: position.character,
            },
            selection,
            file_content: editor.value().to_string(),
            visible_end: None,
            attached_anchor_ids: self
                .proxy
                .as_ref()
                .map(|proxy| {
                    proxy
                        .anchors_for(&self.proxy_path())
                        .into_iter()
                        .map(|anchor| anchor.id)
                        .collect()
                })
                .unwrap_or_default(),
            attached_files: Vec::new(),
        }
    }

    pub fn open_file(
        &mut self,
        path: &str,
        preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if text.is_empty() && std::fs::metadata(path).is_err() {
            self.status = format!("Cannot open {}", path).into();
            cx.notify();
            return;
        }
        self.loading = true;
        self.editor.update(cx, |ed, cx| {
            ed.set_value(text, window, cx);
        });
        self.loading = false;
        self.file_path = path.to_string();
        self.saved_rev = 0;
        self.dirty_rev = 0;
        self.is_preview = preview;
        self.active_line = 1;
        self.request_generation = self.request_generation.wrapping_add(1);
        self.ghost_text = None;
        self.hover_text = None;
        self.git_lens = None;
        self.git_lens_key = None;
        self.show_completions = false;
        if let Some(proxy) = self.proxy.as_ref() {
            proxy.refresh_anchors(&[self.proxy_path()]);
        }
        self.status = format!("Opened {}", path).into();
        cx.notify();
    }

    fn cursor_position(&self, cx: &App) -> lsp_types::Position {
        let position = self.editor.read(cx).cursor_position();
        lsp_types::Position {
            line: position.line,
            character: position.character,
        }
    }

    fn sync_proxy_diagnostics(&mut self, _cx: &App) {
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        if let Some(items) = proxy.diagnostics_for(&self.proxy_path()) {
            self.diagnostics = items
                .into_iter()
                .map(|d| DiagnosticItem {
                    line: d.range.start.line + 1,
                    message: match &d.message {
                        s if s.is_empty() => "Diagnostic".to_string(),
                        s => s.clone(),
                    },
                    is_error: matches!(
                        d.severity,
                        Some(lsp_types::DiagnosticSeverity::ERROR)
                    ),
                })
                .collect();
        }
    }

    pub fn refresh_attribution(&mut self) {
        if let Some(proxy) = self.proxy.as_ref() {
            proxy.refresh_anchors(&[self.proxy_path()]);
        }
    }

    fn request_proxy_completions(&mut self, cx: &mut Context<Self>) {
        let Some(proxy) = self.proxy.clone() else {
            self.show_completions = !self.show_completions;
            cx.notify();
            return;
        };
        let path = self.proxy_path();
        let pos = self.cursor_position(cx);
        self.request_generation = self.request_generation.wrapping_add(1);
        let generation = self.request_generation;
        let request_path = path.clone();
        let text = self.editor.read(cx).value().to_string();
        let prefix = text
            .lines()
            .nth(pos.line as usize)
            .unwrap_or("")
            .to_string();
        let rx = proxy.request_completions(path, pos, prefix);
        cx.spawn(async move |this, cx| {
            let items = cx.background_spawn(async move { rx.recv().unwrap_or_default() }).await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                if this.request_generation != generation
                    || this.proxy_path() != request_path
                    || this.cursor_position(cx) != pos
                {
                    return;
                }
                if !items.is_empty() {
                    this.completions = items
                        .into_iter()
                        .take(12)
                        .map(|c| CompletionItem {
                            label: c.label.clone(),
                            detail: c.detail.clone().unwrap_or_default(),
                            insert_text: c.insert_text.or(c.text_edit.as_ref().and_then(|e| match e {
                                lsp_types::CompletionTextEdit::Edit(e) => Some(e.new_text.clone()),
                                lsp_types::CompletionTextEdit::InsertAndReplace(e) => Some(e.new_text.clone()),
                            })).unwrap_or(c.label.clone()),
                            icon: IconName::Code,
                        })
                        .collect();
                    this.selected_completion = 0;
                    this.show_completions = true;
                    this.status = "Completions from proxy".into();
                } else {
                    this.show_completions = !this.show_completions;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn request_proxy_inline(&mut self, cx: &mut Context<Self>) {
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        let path = self.proxy_path();
        let pos = self.cursor_position(cx);
        self.request_generation = self.request_generation.wrapping_add(1);
        let generation = self.request_generation;
        let request_path = path.clone();
        let rx = proxy.request_inline(path, pos);
        cx.spawn(async move |this, cx| {
            let text = cx
                .background_spawn(async move {
                    rx.recv().unwrap_or_else(|_| String::new())
                })
                .await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                if this.request_generation != generation
                    || this.proxy_path() != request_path
                    || this.cursor_position(cx) != pos
                {
                    return;
                }
                if !text.trim().is_empty() {
                    this.ghost_offset = caret_byte_offset(
                        &this.editor.read(cx).value().to_string(),
                        pos,
                    );
                    this.ghost_text = Some(text);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn request_proxy_hover(&mut self, cx: &mut Context<Self>) {
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        let path = self.proxy_path();
        let pos = self.cursor_position(cx);
        let rx = proxy.request_hover(path, pos);
        cx.spawn(async move |this, cx| {
            let hover = cx.background_spawn(async move { rx.recv().ok() }).await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                if let Some(hover) = hover {
                    let text = match hover.contents {
                        lsp_types::HoverContents::Scalar(m) => match m {
                            lsp_types::MarkedString::String(s) => s,
                            lsp_types::MarkedString::LanguageString(l) => l.value,
                        },
                        lsp_types::HoverContents::Array(arr) => arr
                            .into_iter()
                            .map(|m| match m {
                                lsp_types::MarkedString::String(s) => s,
                                lsp_types::MarkedString::LanguageString(l) => {
                                    l.value
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                        lsp_types::HoverContents::Markup(m) => m.value,
                    };
                    this.hover_text = Some(text);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value().to_string();
        match std::fs::write(&self.file_path, text) {
            Ok(()) => {
                self.saved_rev = self.dirty_rev;
                self.git_lens_key = None;
                if let Some(proxy) = self.proxy.as_ref() {
                    proxy.refresh_anchors(&[self.proxy_path()]);
                }
                self.status = format!("Saved {}", self.file_path).into();
            }
            Err(e) => {
                self.status = format!("Save failed: {e}").into();
            }
        }
        cx.notify();
    }

    pub fn accept_ghost_text(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ghost) = self.ghost_text.take() {
            self.ghost_offset = None;
            self.editor.update(cx, |ed, cx| {
                ed.insert(ghost, window, cx);
            });
            self.status = "Accepted inline FIM completion".into();
            cx.notify();
        }
    }

    pub fn accept_completion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(item) = self.completions.get(self.selected_completion) {
            let insert = item.insert_text.clone();
            self.editor.update(cx, |ed, cx| {
                ed.replace(insert, window, cx);
            });
            self.show_completions = false;
            self.status = format!("Completed {}", item.label).into();
            cx.notify();
        }
    }

    pub fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let cmd =
            event.keystroke.modifiers.platform || event.keystroke.modifiers.control;
        let ctrl = event.keystroke.modifiers.control;
        let alt = event.keystroke.modifiers.alt;

        if cmd && key == "s" {
            self.save(cx);
            return;
        }

        // Ctrl+Space: proxy-backed completions (falls back to local list).
        if ctrl && key == " " {
            self.request_proxy_completions(cx);
            return;
        }

        // Alt+G: accept next-word style inline prediction from proxy.
        if alt && key == "g" {
            self.request_proxy_inline(cx);
            return;
        }

        // Ctrl+K Ctrl+I style hover: Ctrl+H shows proxy hover for cursor.
        if ctrl && key == "h" {
            self.request_proxy_hover(cx);
            return;
        }

        // Ctrl+]: go to definition via proxy.
        if ctrl && key == "]" {
            if let Some(proxy) = self.proxy.clone() {
                let path = self.proxy_path();
                let pos = self.cursor_position(cx);
                let rx = proxy.request_definition(path, pos);
                cx.spawn(async move |this, cx| {
                    let locs = cx
                        .background_spawn(
                            async move { rx.recv().unwrap_or_default() },
                        )
                        .await;
                    let _ = this.update(cx, |this: &mut Self, cx| {
                        if let Some(loc) = locs.first() {
                            this.status = format!(
                                "Definition: {}:{}",
                                loc.uri
                                    .to_file_path()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_default(),
                                loc.range.start.line + 1
                            )
                            .into();
                        } else {
                            this.status = "No definition from proxy".into();
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            return;
        }

        if self.show_completions {
            match key {
                "down" => {
                    if self.selected_completion + 1 < self.completions.len() {
                        self.selected_completion += 1;
                        cx.notify();
                    }
                    return;
                }
                "up" => {
                    if self.selected_completion > 0 {
                        self.selected_completion -= 1;
                        cx.notify();
                    }
                    return;
                }
                "enter" | "tab" => {
                    self.accept_completion(window, cx);
                    return;
                }
                "escape" => {
                    self.show_completions = false;
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }

        // Tab accepts FIM ghost text
        if key == "tab" && self.ghost_text.is_some() {
            self.accept_ghost_text(window, cx);
            return;
        }

        if key == "escape" && self.ghost_text.is_some() {
            self.ghost_text = None;
            self.ghost_offset = None;
            cx.notify();
        }
    }
}

impl BasePanel for CodePanel {
    fn panel_name(&self) -> &'static str {
        "ahead_code"
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

impl Panel for CodePanel {
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let file_name = std::path::Path::new(&self.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled");
        let dirty = self.dirty_rev != self.saved_rev;
        let group = self.tab_group.clone();
        let panel_id = self.panel_id;
        let close_handler = self.close_handler.clone();
        let tab_handler = self.tab_handler.clone();
        let double_tab_handler = tab_handler.clone();
        let button_tab_handler = tab_handler.clone();
        let title = SharedString::from(file_name.to_string());
        let tab_label = div()
            .when(self.is_preview, |this| this.italic())
            .child(title)
            .when(dirty, |this| {
                this.child(div().text_color(cx.theme().warning).child("•"))
            });
        h_flex()
            .id(("code-tab", panel_id.as_u64()))
            .items_center()
            .gap_1()
            .on_click({
                let tab_handler = double_tab_handler;
                move |event, window, cx| {
                    if event.click_count() > 1 {
                        if let Some(handler) = tab_handler.as_ref() {
                            handler(panel_id, CodeTabAction::Promote, window, cx);
                        }
                    }
                }
            })
            .context_menu(move |menu, _window, _cx| {
                let item = |label: &'static str, action: CodeTabAction| {
                    let tab_handler = tab_handler.clone();
                    PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        if let Some(handler) = tab_handler.as_ref() {
                            handler(panel_id, action, window, cx);
                        }
                    })
                };
                menu.item(item("Close", CodeTabAction::Close))
                    .item(item("Close Others", CodeTabAction::CloseOthers))
                    .separator()
                    .item(item("Close Left", CodeTabAction::CloseLeft))
                    .item(item("Close Right", CodeTabAction::CloseRight))
                    .separator()
                    .item(item("Close All", CodeTabAction::CloseAll))
            })
            .child(tab_label)
            .child(
                Button::new(SharedString::from(format!(
                    "close_file_{}",
                    panel_id.as_u64()
                )))
                .icon(IconName::X)
                .ghost()
                .tooltip(format!("Close {file_name}"))
                .on_click(move |_, window, cx| {
                    if let Some(handler) = button_tab_handler.as_ref() {
                        handler(panel_id, CodeTabAction::Close, window, cx);
                        return;
                    }
                    let can_close = group.as_ref().is_some_and(|group| {
                        group
                            .read_with(cx, |group, cx| {
                                group.context(cx).is_draggable()
                            })
                            .unwrap_or(false)
                    });
                    if can_close {
                        if let Some(group) = group.as_ref() {
                            _ = group.update(cx, |group, cx| {
                                group.close_panel(panel_id, cx)
                            });
                        }
                    } else if let Some(handler) = close_handler.as_ref() {
                        handler(panel_id, window, cx);
                    } else if let Some(group) = group.as_ref() {
                        _ = group
                            .update(cx, |group, cx| group.close_panel(panel_id, cx));
                    }
                }),
            )
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
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.sync_proxy_diagnostics(cx);
        let dirty = self.dirty_rev != self.saved_rev;
        let border_color = cx.theme().border;
        let text_color = cx.theme().sidebar_foreground;
        let is_dark = cx.theme().mode.is_dark();
        let bar_bg = if is_dark {
            gpui_kit::rgb(0x121214)
        } else {
            gpui_kit::rgb(0xF8FAFC)
        };
        let editor_bg = if is_dark {
            gpui_kit::rgb(0x18181B)
        } else {
            gpui_kit::rgb(0xFFFFFF)
        };
        let popup_bg = if is_dark {
            gpui_kit::rgb(0x202023)
        } else {
            gpui_kit::rgb(0xF1F5F9)
        };
        let active_sel_bg = if is_dark {
            gpui_kit::rgb(0x2E3035)
        } else {
            gpui_kit::rgb(0xE2E8F0)
        };
        let file_name = std::path::Path::new(&self.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled");

        let has_diag = !self.diagnostics.is_empty();
        let diag_text = self
            .diagnostics
            .first()
            .map(|d| d.message.clone())
            .unwrap_or_default();
        let selected_idx = self.selected_completion;
        let code_panel = cx.entity();

        // Native Git gutter: real hunks for this file. Green = added, amber = modified.
        let workspace = if self.workspace.is_empty() {
            std::path::Path::new(&self.file_path)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or(".")
                .to_string()
        } else {
            self.workspace.clone()
        };
        let hunks = if let Some(proxy) = self.proxy.clone() {
            proxy.hunks_for(&self.proxy_path())
        } else {
            crate::proxy_client::parse_unified_hunks(&git_diff_text(
                &workspace,
                &self.file_path,
            ))
        };
        let hunk_lines: std::collections::HashMap<u32, DiffHunkKind> = hunks
            .iter()
            .filter(|h| !matches!(h.kind, DiffHunkKind::Deleted))
            .flat_map(|h| (h.start..h.start + h.len).map(|l| (l, h.kind)))
            .collect();
        let actor_lines: std::collections::HashSet<u32> = self
            .proxy
            .as_ref()
            .map(|proxy| {
                proxy
                    .anchors_for(&self.proxy_path())
                    .into_iter()
                    .filter(|anchor| {
                        anchor.actor_id == ahead_rpc::ahead::AHEAD_ACTOR_ID
                    })
                    .flat_map(|anchor| {
                        anchor.range.start.line..=anchor.range.end.line
                    })
                    .collect()
            })
            .unwrap_or_default();
        let editor_state = self.editor.read(cx);
        let line_count =
            editor_state.value().to_string().lines().count().max(1) as u32;
        let deleted_lines: std::collections::HashSet<u32> = hunks
            .iter()
            .filter(|h| matches!(h.kind, DiffHunkKind::Deleted))
            .map(|h| h.start.min(line_count))
            .collect();
        let line_height = editor_state.line_height().unwrap_or(px(16.));
        let scroll_offset = editor_state.scroll_offset();
        // ponytail: keep one source-row coordinate system until the editor exposes
        // its display-row/content origin; the viewport clips this layer.
        let gutter_top = scroll_offset.y + px(8.);
        let breakpoints = self
            .proxy
            .as_ref()
            .map(|proxy| {
                proxy.breakpoints_for(std::path::Path::new(&self.file_path))
            })
            .unwrap_or_default();
        let cursor_position = self.cursor_position(cx);
        self.active_line = cursor_position.line + 1;
        let active_column = cursor_position.character + 1;
        // ponytail: use the editor's monospace advance until EditorState exposes
        // the caret bounds directly.
        let completion_top = scroll_offset.y
            + line_height * (cursor_position.line as f32 + 1.0)
            + px(8.);
        let completion_left = px(42. + cursor_position.character as f32 * 7.2);
        let git_lens_key =
            (self.file_path.clone(), self.active_line, self.dirty_rev);
        if self.git_lens_key.as_ref() != Some(&git_lens_key) {
            self.git_lens =
                git_lens_for_line(&workspace, &self.file_path, self.active_line);
            self.git_lens_key = Some(git_lens_key);
        }
        let git_lens = self.git_lens.clone();
        let hover_text = self.hover_text.clone();
        v_flex()
            .size_full()
            .bg(editor_bg)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this: &mut Self, event: &KeyDownEvent, window, cx| {
                this.handle_key(event, window, cx);
            }))
            // Breadcrumb path navigation bar (matching Zed / VS Code)
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
                                        .child("• modified (⌘S)")
                                )
                            })
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .when(has_diag, |el| {
                                el.child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        .child(IconName::ShieldAlert)
                                        .child(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(gpui_kit::rgb(0xF59E0B))
                                                .child(diag_text.clone())
                                        )
                                )
                            })
                    )
            )
            // Editor row: native gutter rail (diff + breakpoints) beside editor
            .child(
                h_flex()
                    .flex_1()
                    .h_full()
                    .min_h_0()
                    .child(
                        div()
                            .w(px(30.))
                            .h_full()
                            .relative()
                            .overflow_hidden()
                            .bg(bar_bg)
                            .border_r_1()
                            .border_color(border_color)
                            .child(
                                v_flex()
                                    .absolute()
                                    .left(px(0.))
                                    .right(px(0.))
                                    .top(gutter_top)
                                    .children((1..=line_count).map(|ln| {
                                        let hunk = hunk_lines.get(&ln).copied();
                                        let is_deleted = deleted_lines.contains(&ln);
                                        let is_bp = breakpoints.contains(&ln);
                                        let is_agent = actor_lines.contains(&ln);
                                        h_flex()
                                            .id(("gutter", ln as usize))
                                            .h(line_height)
                                            .items_center()
                                            .justify_center()
                                            .gap(px(2.))
                                            .cursor(CursorStyle::PointingHand)
                                            .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                if let Some(proxy) = this.proxy.clone() {
                                                    proxy.toggle_breakpoint(
                                                        std::path::Path::new(&this.file_path),
                                                        ln,
                                                    );
                                                }
                                                this.active_line = ln;
                                                cx.notify();
                                            }))
                                            .child(
                                                div()
                                                    .w(px(3.))
                                                    .h(px(12.))
                                                    .rounded_sm()
                                                    .bg(match hunk {
                                                        Some(DiffHunkKind::Added) => gpui_kit::rgb(0x22C55E),
                                                        Some(DiffHunkKind::Modified) => gpui_kit::rgb(0xF59E0B),
                                                        Some(DiffHunkKind::Deleted) => gpui_kit::rgb(0x00000000),
                                                        None => gpui_kit::rgb(0x00000000),
                                                    })
                                            )
                                            .child(
                                                div()
                                                    .w(px(5.))
                                                    .h(px(8.))
                                                    .rounded_full()
                                                    .bg(if is_deleted {
                                                        gpui_kit::rgb(0xEF4444)
                                                    } else {
                                                        gpui_kit::rgb(0x00000000)
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .w(px(2.))
                                                    .h(px(12.))
                                                    .rounded_sm()
                                                    .bg(if is_agent {
                                                        gpui_kit::rgb(0x8B5CF6)
                                                    } else {
                                                        gpui_kit::rgb(0x00000000)
                                                    })
                                            )
                                            .child(
                                                div()
                                                    .w(px(5.))
                                                    .h(px(5.))
                                                    .rounded_full()
                                                    .bg(if is_bp {
                                                        gpui_kit::rgb(0xEF4444)
                                                    } else {
                                                        gpui_kit::rgb(0x00000000)
                                                    }),
                                            )
                                    }))
                            )
                    )
                    // Code Editor & Floating Overlays Container
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                        .min_w_0()
                        .relative()
                        .bg(editor_bg)
                            .context_menu(move |menu, window, _cx| {
                                menu.item(
                                    PopupMenuItem::new("Refresh diagnostics").on_click(
                                        window.listener_for(
                                            &code_panel,
                                            |this, _, _, cx| {
                                                this.sync_proxy_diagnostics(cx);
                                                cx.notify();
                                            },
                                        ),
                                    ),
                                )
                            })
                            .child(
                                Editor::new(&self.editor)
                                    .aria_label("AHEAD Code Editor")
                                    .h_full()
                            )
                            .when(has_diag, |el| {
                                el.child(
                                    div()
                                        .absolute()
                                        .top(px(6.))
                                        .right(px(8.))
                                        .max_w(px(520.))
                                        .text_size(px(10.))
                                        .text_color(gpui_kit::rgb(0xF59E0B))
                                        .child(diag_text.clone()),
                                )
                            })
                            .when_some(hover_text.clone(), |el, hover| {
                                el.child(
                                    div()
                                        .absolute()
                                        .top(px(28.))
                                        .left(px(8.))
                                        .p_2()
                                        .rounded_md()
                                        .bg(popup_bg)
                                        .border_1()
                                        .border_color(border_color)
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .font_family("Menlo")
                                                .text_color(text_color)
                                                .child(hover),
                                        ),
                                )
                            })
                    // FIM Ghost Text Banner Overlay (dim inline prediction)
                    .when_some(self.ghost_text.clone(), |el, ghost| {
                        el.child(
                            div()
                                .absolute()
                                .top(completion_top)
                                .left(completion_left)
                                .max_w(px(520.))
                                .p_2()
                                .rounded_md()
                                .bg(if is_dark { gpui_kit::rgb(0x133E2F) } else { gpui_kit::rgb(0xECFDF5) })
                                .border_1()
                                .border_color(if is_dark { gpui_kit::rgb(0x059669) } else { gpui_kit::rgb(0xA7F3D0) })
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .font_family("Menlo")
                                                .text_color(if is_dark { gpui_kit::rgb(0x6EE7B7) } else { gpui_kit::rgb(0x065F46) })
                                                .child(ghost)
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(gpui_kit::rgb(0x94A3B8))
                                                .child("Tab to accept · Esc to dismiss")
                                        )
                                )
                        )
                    })
                    // Floating LSP Completion Popup
                    .when(self.show_completions, |el| {
                        el.child(
                            div()
                                .absolute()
                                .top(completion_top)
                                .left(completion_left)
                                .w(px(320.))
                                .rounded_lg()
                                .bg(popup_bg)
                                .border_1()
                                .border_color(border_color)
                                .shadow_lg()
                                .p_1()
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .children(
                                            self.completions.iter().enumerate().map(|(idx, item)| {
                                                let is_sel = idx == selected_idx;
                                                h_flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .p_1()
                                                    .rounded_md()
                                                    .bg(if is_sel { active_sel_bg } else { popup_bg })
                                                    .child(
                                                        h_flex()
                                                            .gap_2()
                                                            .items_center()
                                                            .child(item.icon)
                                                            .child(
                                                                div()
                                                                    .font_weight(if is_sel { gpui_kit::FontWeight::BOLD } else { gpui_kit::FontWeight::NORMAL })
                                                                    .text_size(px(12.))
                                                                    .text_color(text_color)
                                                                    .child(item.label.clone())
                                                            )
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(10.))
                                                            .text_color(gpui_kit::rgb(0x71717A))
                                                            .child(item.detail.clone())
                                                    )
                                            })
                                        )
                                )
                        )
                    })
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
                    .child(format!(
                        "{} · Ln {}, Col {}",
                        file_name, self.active_line, active_column
                    ))
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .when_some(git_lens, |el, blame| {
                                let label = if blame.commit.is_empty() {
                                    format!(
                                        "{} · {} · {}",
                                        blame.author, blame.date, blame.subject
                                    )
                                } else {
                                    format!(
                                        "{} · {} · {} ({}){}",
                                        blame.author,
                                        blame.date,
                                        blame.subject,
                                        blame.commit,
                                        if dirty { " · save to refresh" } else { "" }
                                    )
                                };
                                el.child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(gpui_kit::rgb(0x94A3B8))
                                        .child(label),
                                )
                            })
                    )
            )
    }
}

fn git_lens_for_line(workspace: &str, path: &str, line: u32) -> Option<GitLensLine> {
    if line == 0 {
        return None;
    }
    let relative_path = std::path::Path::new(path)
        .strip_prefix(workspace)
        .ok()?
        .to_string_lossy()
        .to_string();
    let line_range = format!("{line},{line}");
    let output = std::process::Command::new("git")
        .args([
            "blame",
            "--line-porcelain",
            "-L",
            &line_range,
            "--",
            &relative_path,
        ])
        .current_dir(workspace)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let parsed = parse_git_blame(&String::from_utf8_lossy(&output.stdout))?;
    if parsed.author == "Not Committed Yet" {
        return Some(GitLensLine {
            author: "Uncommitted changes".to_string(),
            date: "working tree".to_string(),
            subject: "Save and commit to attribute".to_string(),
            commit: String::new(),
        });
    }
    let date = std::process::Command::new("git")
        .args(["show", "-s", "--format=%ar", &parsed.commit])
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|show| show.status.success())
        .map(|show| String::from_utf8_lossy(&show.stdout).trim().to_string())
        .filter(|date| !date.is_empty())
        .unwrap_or_else(|| "unknown time".to_string());
    Some(GitLensLine { date, ..parsed })
}

fn parse_git_blame(text: &str) -> Option<GitLensLine> {
    let commit = text.lines().next()?.split_whitespace().next()?.to_string();
    let mut author = None;
    let mut subject = None;
    for line in text.lines().skip(1) {
        if let Some(value) = line.strip_prefix("author ") {
            author = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("summary ") {
            subject = Some(value.to_string());
        }
    }
    Some(GitLensLine {
        author: author?,
        date: String::new(),
        subject: subject.unwrap_or_else(|| "No commit subject".to_string()),
        commit: commit.chars().take(7).collect(),
    })
}

fn git_diff_text(workspace: &str, path: &str) -> String {
    let rel = std::path::Path::new(path)
        .strip_prefix(workspace)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string());
    std::process::Command::new("git")
        .args(["diff", "--unified=0", "--", &rel])
        .current_dir(workspace)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

/// Converts an LSP position to a byte offset in `text`, counting the
/// position character as Unicode scalar values. Returns `None` when the
/// position is out of range; callers dismiss the ghost on doubt.
fn caret_byte_offset(text: &str, pos: lsp_types::Position) -> Option<usize> {
    let mut offset = 0;
    let mut lines = text.split_inclusive('\n');
    for _ in 0..pos.line {
        offset += lines.next()?.len();
    }
    let line = lines.next().unwrap_or("");
    let content = line.strip_suffix('\n').unwrap_or(line);
    let mut taken = 0;
    let mut count: usize = 0;
    for ch in content.chars() {
        if count >= pos.character as usize {
            break;
        }
        taken += ch.len_utf8();
        count += 1;
    }
    if count < pos.character as usize {
        return None;
    }
    Some(offset + taken)
}

/// Narrows a ghost insertion as the user types, mirroring Zed's
/// `interpolate_edits` (`crates/edit_prediction_types`) for the
/// single-insertion case: a typed prefix of the prediction leaves the
/// remainder, anything else (or a fully typed prediction) discards it.
/// Independent reimplementation for the clean-room record (decision 0006).
fn interpolate_insertion(predicted: &str, typed: &str) -> Option<String> {
    if typed.is_empty() {
        return Some(predicted.to_string());
    }
    predicted
        .strip_prefix(typed)
        .filter(|rest| !rest.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::{caret_byte_offset, interpolate_insertion, parse_git_blame};

    #[test]
    fn parses_git_lens_blame_metadata() {
        let blame = "abc1234567890123456789012345678901234567 3 3 1\nauthor Ada Lovelace\nauthor-time 1700000000\nsummary Add the engine\n\tlet engine = true;\n";
        assert_eq!(
            parse_git_blame(blame),
            Some(super::GitLensLine {
                author: "Ada Lovelace".to_string(),
                date: String::new(),
                subject: "Add the engine".to_string(),
                commit: "abc1234".to_string(),
            })
        );
    }

    #[test]
    fn caret_byte_offset_maps_lines_and_columns() {
        let text = "ab\ncde\nf";
        assert_eq!(
            caret_byte_offset(
                text,
                lsp_types::Position {
                    line: 0,
                    character: 1
                }
            ),
            Some(1)
        );
        assert_eq!(
            caret_byte_offset(
                text,
                lsp_types::Position {
                    line: 1,
                    character: 2
                }
            ),
            Some(5)
        );
        assert_eq!(
            caret_byte_offset(
                text,
                lsp_types::Position {
                    line: 9,
                    character: 0
                }
            ),
            None
        );
        assert_eq!(
            caret_byte_offset(
                text,
                lsp_types::Position {
                    line: 0,
                    character: 9
                }
            ),
            None
        );
    }

    #[test]
    fn interpolate_insertion_shrinks_or_discards() {
        assert_eq!(
            interpolate_insertion("retry_with_backoff()", "retry_"),
            Some("with_backoff()".to_string())
        );
        assert_eq!(interpolate_insertion("ab", "ab"), None);
        assert_eq!(interpolate_insertion("ab", "ac"), None);
        assert_eq!(interpolate_insertion("ab", ""), Some("ab".to_string()));
    }
}
