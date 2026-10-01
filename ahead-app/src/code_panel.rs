//! AHEAD Code Editor Panel - proxy-backed GPUI implementation.
//!
//! - Tree-sitter editor with Cmd+S save, no save button.
//! - Proxy-backed LSP completions (Ctrl+Space), hover, go-to-definition.
//! - Proxy-backed FIM inline ghost text (Tab accepts).
//! - Proxy-cached live-buffer Git gutter markers (green add, amber modify,
//!   red deletion boundary).
//!   A second rail shows attribution: who authored each uncommitted region.
//! - Breakpoint gutter rail synced to `ProxyClient::DebugState`.
//! - Diagnostics squiggles from proxy `PublishDiagnostics`.

use std::{
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent, PanelId, TabGroup,
};
use gpui_kit::component::empty::{Empty, EmptyDescription, EmptyHeader, EmptyTitle};
use gpui_kit::component::input::{
    Copy as CopyAction, Cut as CutAction, Editor, EditorState, Enter, Escape,
    IndentInline, InputEvent, MoveDown, MoveUp, Paste as PasteAction, Rope, RopeExt,
    SelectAll, TabSize, TextDecoration, TextDecorationCollection,
};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit_assets::IconName;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as FuzzyConfig, Matcher as FuzzyMatcher, Utf32Str};
use sha2::Digest;

use crate::app::{GoToDefinition, SaveFile};
use crate::proxy_client::{LspCompletion, ProxyClient};
use ahead_rpc::source_control::{BlameHunk, DiffHunkKind, GitFileState};

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompletionContext {
    generation: u64,
    request_id: u64,
    path: std::path::PathBuf,
    position: lsp_types::Position,
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
    CopyRelativePath,
    CopyAbsolutePath,
    AddToGitignore,
    RevealInFinder,
    DuplicateFile,
    TrashFile,
    ViewHistory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GitStateKey {
    path: std::path::PathBuf,
    revision: usize,
    repository_generation: u64,
}

#[derive(Clone, Debug)]
struct ActivePresentation {
    cue: ahead_rpc::ahead::PresentationCue,
    quote: String,
    pointer_quote: String,
}

#[derive(Debug)]
enum FileOpenError {
    UnsupportedText,
    Read(std::io::Error),
}

impl std::fmt::Display for FileOpenError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedText => formatter.write_str(
                "This file contains binary data or uses an unsupported text encoding.",
            ),
            Self::Read(error) => write!(formatter, "Could not read this file: {error}"),
        }
    }
}

fn read_editor_file(path: &str) -> Result<String, FileOpenError> {
    if path.is_empty() {
        return Ok(String::new());
    }
    let bytes = std::fs::read(path).map_err(FileOpenError::Read)?;
    if bytes.contains(&0) || ahead_core::search::is_binary_content(&bytes) {
        return Err(FileOpenError::UnsupportedText);
    }
    String::from_utf8(bytes).map_err(|_| FileOpenError::UnsupportedText)
}

pub struct CodePanel {
    pub focus: FocusHandle,
    pub editor: Entity<EditorState>,
    presentation_decorations: TextDecorationCollection,
    presentation: Option<ActivePresentation>,
    pointer_overlay_position: Option<(f32, f32)>,
    pub file_path: String,
    file_error: Option<FileOpenError>,
    pub saved_rev: usize,
    pub dirty_rev: usize,
    saved_text: Rope,
    save_request_id: u64,
    saved_request_id: u64,
    recovery_id: String,
    recovery_revision: u64,
    recovery_key: Option<(u64, u64)>,
    recovery_reply: Option<(Instant, async_channel::Receiver<Result<bool, String>>)>,
    recovery_conflict: bool,
    saved_sha256: Option<String>,
    pub dirty: bool,
    pub status: SharedString,
    pub active_line: u32,
    pub completions: Vec<LspCompletion>,
    completion_context: Option<CompletionContext>,
    pub selected_completion: usize,
    pub show_completions: bool,
    pub ghost_text: Option<String>,
    /// Byte offset in the editor text where the current ghost was
    /// requested. Lets edits narrow the ghost (Zed-style interpolation)
    /// instead of always dismissing it.
    pub ghost_offset: Option<usize>,
    pub diagnostics: Vec<DiagnosticItem>,
    pub proxy: Option<Arc<ProxyClient>>,
    diagnostic_task: Option<Task<()>>,
    pub workspace: String,
    pub hover_text: Option<String>,
    hover_chord_deadline: Option<Instant>,
    pub is_preview: bool,
    show_markdown_preview: bool,
    git_state: GitFileState,
    git_key: Option<GitStateKey>,
    git_task: Option<Task<()>>,
    hovered_breakpoint_line: Option<u32>,
    expanded_hunk_start: Option<u32>,
    /// Invalidates asynchronous completion/FIM results after edits or file switches.
    pub request_generation: u64,
    completion_request_id: u64,
    suppress_completion_for_next_edit: bool,
    loading: bool,
    close_handler: Option<Rc<dyn Fn(PanelId, &mut Window, &mut App)>>,
    tab_handler: Option<Rc<dyn Fn(PanelId, CodeTabAction, &mut Window, &mut App)>>,
    speech_interruption_handler: Option<Rc<dyn Fn()>>,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
}

impl CodePanel {
    pub fn new(path: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let disk_text = read_editor_file(path);
        let saved_sha256 = disk_text
            .as_ref()
            .ok()
            .map(|text| format!("{:x}", sha2::Sha256::digest(text.as_bytes())));
        let (text, file_error) = match disk_text {
            Ok(text) => (text, None),
            Err(error) => (String::new(), Some(error)),
        };

        let editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .language(editor_language(path))
                .line_number(true)
                .folding(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                .default_value(text);
            editor.set_disabled(file_error.is_some(), cx);
            editor
        });

        let editor_for_observer = editor.clone();
        cx.observe(&editor_for_observer, |_, _, cx| cx.notify())
            .detach();

        cx.subscribe(
            &editor,
            |this: &mut Self, _state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) && this.is_text_available() {
                    let suppress_completion =
                        std::mem::take(&mut this.suppress_completion_for_next_edit);
                    if !this.loading {
                        this.interrupt_speech();
                        this.refresh_presentation_after_edit(cx);
                    }
                    this.dirty_rev += 1;
                    // ponytail: same-length edits scan the rope; use edit-history dirtiness if large-file typing slows.
                    this.dirty = this.editor.read(cx).text() != &this.saved_text;
                    if !this.loading {
                        this.is_preview = false;
                    }
                    this.request_generation =
                        this.request_generation.wrapping_add(1);
                    this.show_completions = false;
                    if !this.loading {
                        if let Some(proxy) = this.proxy.as_ref() {
                            let text = this.editor.read(cx).value().to_string();
                            proxy.sync_editor_snapshot(
                                this.proxy_path(),
                                text.clone(),
                            );
                            let position = this.cursor_position(cx);
                            let should_complete = caret_byte_offset(&text, position)
                                .and_then(|offset| {
                                    text[..offset].chars().next_back()
                                })
                                .is_some_and(|character| {
                                    character.is_alphanumeric()
                                        || matches!(character, '_' | ':' | '.')
                                });
                            if should_complete && !suppress_completion {
                                let generation = this.request_generation;
                                cx.spawn(async move |this, cx| {
                                    cx.background_executor()
                                        .timer(Duration::from_millis(180))
                                        .await;
                                    let _ = this.update(cx, |this, cx| {
                                        if this.request_generation == generation {
                                            this.request_proxy_completions(cx);
                                        }
                                    });
                                })
                                .detach();
                            }
                        }
                    }
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

        let presentation_decorations = editor.update(cx, |state, cx| {
            state.create_decorations_collection(Vec::new(), cx)
        });
        let saved_text = editor.read(cx).text().clone();

        Self {
            focus: cx.focus_handle(),
            editor,
            presentation_decorations,
            presentation: None,
            pointer_overlay_position: None,
            file_path: path.to_string(),
            file_error,
            saved_rev: 0,
            dirty_rev: 0,
            saved_text,
            save_request_id: 0,
            saved_request_id: 0,
            recovery_id: uuid::Uuid::new_v4().to_string(),
            recovery_revision: 0,
            recovery_key: None,
            recovery_reply: None,
            recovery_conflict: false,
            saved_sha256,
            dirty: false,
            status: "".into(),
            active_line: 1,
            completions: Vec::new(),
            completion_context: None,
            selected_completion: 0,
            show_completions: false,
            ghost_text: None,
            ghost_offset: None,
            diagnostics: Vec::new(),
            proxy: None,
            diagnostic_task: None,
            workspace: String::new(),
            hover_text: None,
            hover_chord_deadline: None,
            is_preview: false,
            show_markdown_preview: is_markdown_path(path),
            git_state: GitFileState::default(),
            git_key: None,
            git_task: None,
            hovered_breakpoint_line: None,
            expanded_hunk_start: None,
            request_generation: 0,
            completion_request_id: 0,
            suppress_completion_for_next_edit: false,
            loading: false,
            close_handler: None,
            tab_handler: None,
            speech_interruption_handler: None,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
        }
    }

    pub fn with_proxy(
        mut self,
        proxy: Arc<ProxyClient>,
        workspace: &str,
        cx: &mut Context<Self>,
    ) -> Self {
        let receiver = proxy.subscribe_diagnostics();
        self.diagnostic_task = Some(cx.spawn(async move |this, cx| {
            while receiver.recv().await.is_ok() {
                if this
                    .update(cx, |this, cx| {
                        this.sync_proxy_diagnostics(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        self.proxy = Some(proxy);
        self.workspace = workspace.to_string();
        if !self.file_path.is_empty()
            && self.is_text_available()
            && let Some(proxy) = self.proxy.as_ref()
        {
            proxy.sync_editor_snapshot(
                self.proxy_path(),
                self.editor.read(cx).value().to_string(),
            );
            proxy.refresh_anchors(&[self.proxy_path()]);
        }
        self
    }

    pub fn release_buffer(&mut self) {
        if let Some(proxy) = self.proxy.take() {
            proxy.close_editor_buffer(self.proxy_path());
        }
        self.diagnostic_task = None;
        self.git_task = None;
        self.git_key = None;
        self.git_state = GitFileState::default();
        self.hovered_breakpoint_line = None;
        self.expanded_hunk_start = None;
        self.request_generation = self.request_generation.wrapping_add(1);
        self.show_completions = false;
        self.completion_context = None;
    }

    pub(crate) fn recovery_id(&self) -> &str {
        &self.recovery_id
    }

    pub(crate) fn is_text_available(&self) -> bool {
        self.file_error.is_none()
    }

    pub(crate) fn refresh_recovery(&mut self, cx: &mut Context<Self>) {
        self.recovery_reply = self
            .queue_recovery(false, true, cx)
            .map(|receiver| (Instant::now(), receiver));
    }

    pub(crate) fn poll_recovery(&mut self, cx: &mut Context<Self>) {
        if let Some((started, receiver)) = &self.recovery_reply {
            match receiver.try_recv() {
                Ok(Ok(true)) => self.recovery_reply = None,
                Ok(result) => {
                    self.status = format!(
                        "Unsaved recovery failed: {}",
                        result
                            .err()
                            .unwrap_or_else(|| "snapshot was rejected".into())
                    )
                    .into();
                    self.recovery_reply = None;
                    cx.notify();
                }
                Err(async_channel::TryRecvError::Empty)
                    if started.elapsed() < Duration::from_secs(30) =>
                {
                    return;
                }
                Err(async_channel::TryRecvError::Empty) => {
                    self.status =
                        "Unsaved recovery timed out; save this file before exiting"
                            .into();
                    self.recovery_reply = None;
                    cx.notify();
                }
                Err(async_channel::TryRecvError::Closed) => {
                    self.status = "Unsaved recovery was not confirmed; save this file before exiting".into();
                    self.recovery_reply = None;
                    cx.notify();
                }
            }
        }
        self.recovery_reply = self
            .queue_recovery(false, false, cx)
            .map(|receiver| (Instant::now(), receiver));
    }

    pub(crate) fn queue_recovery(
        &mut self,
        discard: bool,
        force: bool,
        cx: &mut Context<Self>,
    ) -> Option<async_channel::Receiver<Result<bool, String>>> {
        let proxy = self.proxy.as_ref()?;
        if !proxy.editor_recovery_available() || self.file_path.is_empty() || !self.is_text_available() {
            return None;
        }
        if self.recovery_revision == 0 && !self.dirty {
            return None;
        }
        let key = (self.request_generation, self.saved_request_id);
        if !force && self.recovery_key == Some(key) {
            return None;
        }
        let Ok(path) =
            std::path::Path::new(&self.file_path).strip_prefix(&self.workspace)
        else {
            self.status =
                "Unsaved recovery unavailable for a file outside this workspace"
                    .into();
            cx.notify();
            return None;
        };
        self.recovery_revision = self.recovery_revision.saturating_add(1);
        self.recovery_key = Some(key);
        #[cfg(windows)]
        let path =
            std::path::PathBuf::from(path.to_string_lossy().replace('\\', "/"));
        Some(
            proxy.write_editor_recovery(ahead_rpc::file::EditorRecoverySnapshot {
                buffer_id: self.recovery_id.clone(),
                revision: self.recovery_revision,
                path: path.to_owned(),
                contents: (self.dirty && !discard)
                    .then(|| self.editor.read(cx).text().to_string()),
                saved_sha256: self.saved_sha256.clone(),
            }),
        )
    }

    pub(crate) fn restore_recovery(
        &mut self,
        snapshot: ahead_rpc::file::EditorRecoverySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.dirty {
            return Err("Current unsaved edits were preserved; recovery remains in the database".into());
        }
        if let Some(error) = &self.file_error
            && !matches!(error, FileOpenError::Read(error) if error.kind() == std::io::ErrorKind::NotFound)
        {
            return Err(format!("{error} Unsaved recovery remains in the database."));
        }
        let content = snapshot.contents.ok_or("Recovery was already cleared")?;
        if self.recovery_id != snapshot.buffer_id {
            drop(self.queue_recovery(true, true, cx));
        }
        self.recovery_conflict = snapshot.saved_sha256 != self.saved_sha256;
        self.saved_sha256 = snapshot.saved_sha256;
        self.recovery_id = snapshot.buffer_id;
        self.recovery_revision = snapshot.revision;
        self.recovery_key = None;
        self.recovery_reply = None;
        self.file_error = None;
        self.suppress_completion_for_next_edit = true;
        self.editor
            .update(cx, |editor, cx| {
                editor.set_disabled(false, cx);
                editor.replace_all(content, window, cx);
            });
        self.is_preview = false;
        self.status = if self.recovery_conflict {
            "Recovered unsaved edits. The disk file changed; review before overwriting it."
        } else {
            "Recovered unsaved edits; the disk file was not changed"
        }.into();
        cx.notify();
        Ok(())
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

    pub fn set_speech_interruption_handler(&mut self, handler: impl Fn() + 'static) {
        self.speech_interruption_handler = Some(Rc::new(handler));
    }

    fn interrupt_speech(&self) {
        if let Some(handler) = self.speech_interruption_handler.as_ref() {
            handler();
        }
    }

    pub fn has_presentation_for_session(
        &self,
        session_id: &str,
        cue_id: &str,
    ) -> bool {
        self.presentation.as_ref().is_some_and(|presentation| {
            presentation.cue.anchor.session_id == session_id
                && presentation.cue.cue_id == cue_id
        })
    }

    fn refresh_presentation_after_edit(&mut self, cx: &mut Context<Self>) {
        let Some(presentation) = self.presentation.as_ref() else {
            return;
        };
        let quote = presentation.quote.clone();
        let pointer_quote = presentation.pointer_quote.clone();
        self.pointer_overlay_position = None;
        let Some(range) = self
            .presentation_decorations
            .get_ranges(cx)
            .first()
            .cloned()
        else {
            self.clear_presentation(None, cx);
            return;
        };
        let text = self.editor.read(cx).text().to_string();
        if text.get(range.clone()) != Some(quote.as_str()) {
            self.clear_presentation(None, cx);
            return;
        }
        let editor_text = self.editor.read(cx).text();
        let start = editor_text.offset_to_position(range.start);
        let end = editor_text.offset_to_position(range.end);
        let start = ahead_rpc::ahead::DisplayPosition {
            line: start.line,
            col: start.character,
        };
        let end = ahead_rpc::ahead::DisplayPosition {
            line: end.line,
            col: end.character,
        };
        let pointer_target =
            unique_quote_range(&text, &pointer_quote)
                .ok()
                .map(|(start, _)| {
                    let position = editor_text.offset_to_position(start);
                    ahead_rpc::ahead::DisplayPosition {
                        line: position.line,
                        col: position.character,
                    }
                });
        if let Some(presentation) = self.presentation.as_mut() {
            presentation.cue.anchor.range =
                ahead_rpc::ahead::DisplayRange { start, end };
            presentation.cue.pointer_target = pointer_target;
        }
    }

    pub fn promote_preview(&mut self, cx: &mut Context<Self>) {
        if self.is_preview {
            self.is_preview = false;
            cx.notify();
        }
    }

    fn toggle_markdown_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !is_markdown_path(&self.file_path) {
            return;
        }
        self.show_markdown_preview = !self.show_markdown_preview;
        if !self.show_markdown_preview {
            let focus = self.editor.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn proxy_path(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(&self.file_path)
    }

    pub fn turn_context(&self, cx: &App) -> Option<ahead_rpc::ahead::TurnEditorContext> {
        if !self.is_text_available() {
            return None;
        }
        let editor = self.editor.read(cx);
        let file_content = editor.value().to_string();
        let display_position = |position: gpui_kit::base::input::Position| {
            ahead_rpc::ahead::DisplayPosition {
                line: position.line,
                col: file_content
                    .split('\n')
                    .nth(position.line as usize)
                    .unwrap_or_default()
                    .chars()
                    .take(position.character as usize)
                    .map(|character| character.len_utf16() as u32)
                    .sum(),
            }
        };
        let position = editor.cursor_position();
        let selected_range = editor.selected_range();
        let selection = if selected_range.start != selected_range.end {
            let start = editor.text().offset_to_position(selected_range.start);
            let end = editor.text().offset_to_position(selected_range.end);
            Some(ahead_rpc::ahead::DisplayRange {
                start: display_position(start),
                end: display_position(end),
            })
        } else {
            None
        };
        Some(ahead_rpc::ahead::TurnEditorContext {
            active_path: std::path::Path::new(&self.file_path)
                .strip_prefix(&self.workspace)
                .unwrap_or_else(|_| std::path::Path::new(&self.file_path))
                .to_string_lossy()
                .to_string(),
            caret: display_position(position),
            selection,
            file_content,
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
            attached_memories: Vec::new(),
        })
    }

    pub fn open_file(
        &mut self,
        path: &str,
        preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dirty {
            self.status = "Keep this unsaved buffer open or save/discard it before replacing it".into();
            cx.notify();
            return;
        }
        let disk_text = read_editor_file(path);
        if !self.file_path.is_empty()
            && self.is_text_available()
            && let Some(proxy) = &self.proxy
        {
            proxy.close_editor_buffer(self.proxy_path());
        }
        if self.file_path != path {
            drop(self.queue_recovery(true, true, cx));
            self.recovery_id = uuid::Uuid::new_v4().to_string();
            self.recovery_revision = 0;
            self.recovery_key = None;
            self.recovery_reply = None;
            self.recovery_conflict = false;
        }
        self.saved_sha256 = disk_text.as_ref().ok()
            .map(|text| format!("{:x}", sha2::Sha256::digest(text.as_bytes())));
        let text = match disk_text {
            Ok(text) => {
                self.file_error = None;
                text
            }
            Err(error) => {
                self.file_error = Some(error);
                String::new()
            }
        };
        self.presentation = None;
        self.presentation_decorations.clear(cx);
        self.loading = true;
        self.editor.update(cx, |ed, cx| {
            ed.set_disabled(self.file_error.is_some(), cx);
            ed.set_highlighter(editor_language(path), cx);
            ed.set_value(text, window, cx);
        });
        self.loading = false;
        self.file_path = path.to_string();
        self.saved_rev = 0;
        self.dirty_rev = 0;
        self.saved_text = self.editor.read(cx).text().clone();
        self.dirty = false;
        self.is_preview = preview;
        self.show_markdown_preview = is_markdown_path(path);
        self.active_line = 1;
        self.request_generation = self.request_generation.wrapping_add(1);
        self.ghost_text = None;
        self.hover_text = None;
        self.git_task = None;
        self.git_key = None;
        self.git_state = GitFileState::default();
        self.show_completions = false;
        if self.is_text_available() && let Some(proxy) = self.proxy.as_ref() {
            proxy.sync_editor_snapshot(
                self.proxy_path(),
                self.editor.read(cx).value().to_string(),
            );
            proxy.refresh_anchors(&[self.proxy_path()]);
        }
        self.status = self.file_error.as_ref().map_or_else(
            || format!("Opened {path}"),
            |error| error.to_string(),
        ).into();
        cx.notify();
    }

    pub fn reveal_location(
        &mut self,
        location: crate::ross::OpenLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_text_available() {
            window.focus(&self.focus, cx);
            return;
        }
        self.editor.update(cx, |editor, cx| {
            let text = editor.text().to_string();
            let (start, _) =
                location_byte_position(&text, location.line, location.column);
            let (end, _) = location_byte_position(
                &text,
                location.end_line,
                location.end_column,
            );
            editor.set_selected_range(start..end.max(start), cx);
            editor.focus(window, cx);
        });
    }

    pub fn present_quote(
        &mut self,
        session_id: String,
        cue_id: String,
        path: String,
        quote: &str,
        label: String,
        note: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let text = self.editor.read(cx).text().to_string();
        let (start, end) =
            unique_quote_range(&text, quote).map_err(str::to_string)?;
        let prefix = &text[..start];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
        let editor_text = self.editor.read(cx).text();
        let start_position = editor_text.offset_to_position(start);
        let end_position = editor_text.offset_to_position(end);
        let display_start = ahead_rpc::ahead::DisplayPosition {
            line: start_position.line,
            col: start_position.character,
        };
        let display_end = ahead_rpc::ahead::DisplayPosition {
            line: end_position.line,
            col: end_position.character,
        };
        let quote_hash = format!("{:x}", sha2::Sha256::digest(quote.as_bytes()));
        self.presentation_decorations.set(
            vec![TextDecoration::new(
                start..start + quote.len(),
                gpui_kit::HighlightStyle {
                    background_color: Some(cx.theme().primary.opacity(0.25)),
                    font_weight: Some(gpui_kit::FontWeight::BOLD),
                    ..Default::default()
                },
            )],
            cx,
        );
        self.presentation = Some(ActivePresentation {
            cue: ahead_rpc::ahead::PresentationCue {
                cue_id: cue_id.clone(),
                anchor: ahead_rpc::ahead::CodeAnchor {
                    id: cue_id.clone(),
                    session_id,
                    actor_id: ahead_rpc::ahead::AHEAD_ACTOR_ID.to_string(),
                    path,
                    range: ahead_rpc::ahead::DisplayRange {
                        start: display_start,
                        end: display_end,
                    },
                    quote_hash,
                    surrounding_context: None,
                },
                label,
                pointer_target: Some(display_start),
                author_id: ahead_rpc::ahead::AHEAD_ACTOR_ID.to_string(),
                display_duration_ms: None,
                note: Some(note),
            },
            quote: quote.to_string(),
            pointer_quote: quote.to_string(),
        });
        self.pointer_overlay_position = None;
        self.scroll_to_source_line(line, cx);
        cx.notify();
        Ok(())
    }

    pub fn move_pointer(
        &mut self,
        session_id: &str,
        cue_id: &str,
        quote: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(presentation) = self.presentation.as_ref() else {
            return Err("There is no active code presentation".to_string());
        };
        if presentation.cue.anchor.session_id != session_id
            || presentation.cue.cue_id != cue_id
        {
            return Err(
                "The requested presentation cue is no longer active".to_string()
            );
        }
        let text = self.editor.read(cx).text().to_string();
        let (start, _) = unique_quote_range(&text, quote).map_err(str::to_string)?;
        let position = self.editor.read(cx).text().offset_to_position(start);
        let pointer_target = ahead_rpc::ahead::DisplayPosition {
            line: position.line,
            col: position.character,
        };
        if let Some(presentation) = self.presentation.as_mut() {
            presentation.pointer_quote = quote.to_string();
            presentation.cue.pointer_target = Some(pointer_target);
        }
        self.pointer_overlay_position = None;
        self.scroll_to_source_line(position.line as usize, cx);
        cx.notify();
        Ok(())
    }

    fn scroll_to_source_line(&self, line: usize, cx: &mut Context<Self>) {
        self.editor.update(cx, |state, cx| {
            let line_height = state.line_height().unwrap_or(px(16.));
            let offset = state.scroll_offset();
            let first_visible_line = line.saturating_sub(2) as f32;
            state.set_scroll_offset(
                gpui_kit::point(
                    offset.x,
                    px(-(first_visible_line * line_height.as_f32())),
                ),
                cx,
            );
        });
    }

    pub fn clear_presentation(
        &mut self,
        cue_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        if cue_id.is_some_and(|cue_id| {
            self.presentation
                .as_ref()
                .is_none_or(|active| active.cue.cue_id != cue_id)
        }) {
            return false;
        }
        if self.presentation.take().is_none() {
            return false;
        }
        self.pointer_overlay_position = None;
        self.presentation_decorations.clear(cx);
        cx.notify();
        true
    }

    pub fn clear_presentation_for_session(
        &mut self,
        session_id: &str,
        cue_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.presentation.as_ref().is_some_and(|presentation| {
            presentation.cue.anchor.session_id == session_id
                && cue_id.is_none_or(|cue_id| presentation.cue.cue_id == cue_id)
        }) {
            return false;
        }
        self.clear_presentation(cue_id, cx)
    }

    fn cursor_position(&self, cx: &App) -> lsp_types::Position {
        let editor = self.editor.read(cx);
        let position = editor.cursor_position();
        let text = editor.value().to_string();
        lsp_types::Position {
            line: position.line,
            character: text
                .split('\n')
                .nth(position.line as usize)
                .unwrap_or_default()
                .chars()
                .take(position.character as usize)
                .map(|character| character.len_utf16() as u32)
                .sum(),
        }
    }

    fn sync_proxy_diagnostics(&mut self, _cx: &App) {
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        self.diagnostics = proxy
            .diagnostics_for(&self.proxy_path())
            .unwrap_or_default()
            .into_iter()
            .map(|d| DiagnosticItem {
                line: d.range.start.line + 1,
                message: if d.message.is_empty() {
                    "Diagnostic".to_string()
                } else {
                    d.message
                },
                is_error: matches!(
                    d.severity,
                    Some(lsp_types::DiagnosticSeverity::ERROR)
                ),
            })
            .collect();
    }

    pub fn refresh_attribution(&mut self) {
        if let Some(proxy) = self.proxy.as_ref() {
            proxy.refresh_anchors(&[self.proxy_path()]);
        }
    }

    fn current_git_key(&self) -> Option<GitStateKey> {
        if self.file_path.is_empty() {
            return None;
        }
        Some(GitStateKey {
            path: self.proxy_path(),
            revision: self.dirty_rev,
            repository_generation: self.proxy.as_ref()?.git_generation(),
        })
    }

    fn refresh_git_metadata(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.current_git_key() else {
            return;
        };
        if self.git_key.as_ref() == Some(&key) {
            return;
        }
        if self.git_task.is_some() {
            return;
        }
        self.git_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let Ok(Some((key, receiver))) = this.update(cx, |this, cx| {
                let key = this.current_git_key()?;
                let receiver = this.proxy.as_ref()?.git_file_state(
                    key.path.clone(),
                    this.editor.read(cx).value().to_string(),
                );
                this.git_key = Some(key.clone());
                Some((key, receiver))
            }) else {
                return;
            };
            let result = receiver
                .recv()
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            if let Err(error) = this.update(cx, |this, cx| {
                this.finish_git_refresh(key, result, cx);
            }) {
                eprintln!("Git metadata view closed: {error}");
            }
        }));
    }

    fn finish_git_refresh(
        &mut self,
        key: GitStateKey,
        result: Result<GitFileState, String>,
        cx: &mut Context<Self>,
    ) {
        self.git_task = None;
        if self.git_key.as_ref() != Some(&key)
            || self.current_git_key().as_ref() != Some(&key)
        {
            self.refresh_git_metadata(cx);
            return;
        }
        match result {
            Ok(state) => {
                self.git_state = state;
                if self.status.starts_with("Git metadata:") {
                    self.status = "".into();
                }
            }
            Err(error) => {
                self.git_state = GitFileState::default();
                self.status = format!("Git metadata: {error}").into();
            }
        }
        cx.notify();
    }

    fn request_proxy_completions(&mut self, cx: &mut Context<Self>) {
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        let path = self.proxy_path();
        let pos = self.cursor_position(cx);
        let generation = self.request_generation;
        self.completion_request_id = self.completion_request_id.wrapping_add(1);
        let request_id = self.completion_request_id;
        let request_path = path.clone();
        let text = self.editor.read(cx).value().to_string();
        let prefix = text
            .lines()
            .nth(pos.line as usize)
            .unwrap_or("")
            .to_string();
        let rx = proxy.request_completions(path, pos, prefix);
        let query = completion_prefix_range(&text, pos)
            .and_then(|range| text.get(range))
            .unwrap_or_default()
            .to_owned();
        cx.spawn(async move |this, cx| {
            let items = cx
                .background_spawn(async move {
                    rank_completions(rx.recv().unwrap_or_default(), &query)
                })
                .await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                if this.request_generation != generation
                    || this.completion_request_id != request_id
                    || this.proxy_path() != request_path
                    || this.cursor_position(cx) != pos
                {
                    return;
                }
                if !items.is_empty() {
                    this.completions = items;
                    this.completion_context =
                        Some(this.current_completion_context(cx));
                    this.selected_completion = 0;
                    this.show_completions = true;
                    this.status = "Completions from proxy".into();
                } else {
                    this.completions.clear();
                    this.show_completions = false;
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

    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_after_recovery_review(window, cx).detach();
    }

    fn save_after_recovery_review(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        if !self.recovery_conflict {
            return self.save_and_wait(cx);
        }
        let generation = self.request_generation;
        let answer = window.prompt(
            PromptLevel::Warning, "Overwrite the changed disk file?",
            Some(&format!("{}\nThis recovered buffer was based on a different disk version. Overwriting replaces the current disk contents.", self.file_path)),
            &["Overwrite", "Cancel"], cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await.ok() != Some(0) {
                return false;
            }
            let save = this.update(cx, |this, cx| {
                if this.request_generation != generation {
                    return Task::ready(false);
                }
                this.save_and_wait(cx)
            });
            match save {
                Ok(save) => save.await,
                Err(_) => false,
            }
        })
    }

    fn save_and_wait(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        if let Some(error) = &self.file_error {
            self.status = error.to_string().into();
            cx.notify();
            return Task::ready(false);
        }
        let Some(proxy) = self.proxy.clone() else {
            self.status = "Save failed: workspace service is unavailable".into();
            cx.notify();
            return Task::ready(false);
        };
        let path = self.file_path.clone();
        let text = self.editor.read(cx).text().clone();
        let revision = self.dirty_rev;
        let receiver = proxy.save_editor_buffer(self.proxy_path(), text.to_string());
        self.save_request_id = self.save_request_id.wrapping_add(1);
        let request_id = self.save_request_id;
        self.status = format!("Saving {path}").into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut reply = std::pin::pin!(receiver.recv());
            let mut deadline = std::pin::pin!(cx.background_executor().timer(Duration::from_secs(30)));
            let result = std::future::poll_fn(|cx| {
                use std::task::Poll;
                if let Poll::Ready(result) = reply.as_mut().poll(cx) {
                    return Poll::Ready(result.unwrap_or_else(|error| Err(ahead_rpc::RpcError {
                        code: 0, message: format!("The workspace did not confirm the save: {error}"),
                    })));
                }
                if deadline.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Err(ahead_rpc::RpcError {
                        code: 0, message: "Save acknowledgement timed out. The file may have been written; keep this buffer open and verify before retrying.".into(),
                    }));
                }
                Poll::Pending
            }).await;
            let succeeded = result.is_ok();
            match this.update(cx, |this, cx| {
                this.finish_save(request_id, &path, revision, text, result, cx);
                succeeded && this.file_path == path && !this.dirty
            }) {
                Ok(saved) => saved,
                Err(error) => {
                    eprintln!("Updating editor after save: {error}");
                    false
                }
            }
        })
    }

    pub fn prepare_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Option<u64>> {
        let generation = self.request_generation;
        if !self.dirty {
            return Task::ready(Some(generation));
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "Save changes before closing?",
            Some(&self.file_path),
            &["Save", "Discard Changes", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            match answer.await {
                Ok(0) => {
                    let save = this
                        .update_in(cx, |this, window, cx| {
                            if this.request_generation == generation {
                                this.save_after_recovery_review(window, cx)
                            } else {
                                Task::ready(false)
                            }
                        })
                        .ok()?;
                    if !save.await {
                        return None;
                    }
                }
                Ok(1) => {}
                _ => return None,
            }
            this.update(cx, |this, _| {
                (this.request_generation == generation).then_some(generation)
            })
            .ok()
            .flatten()
        })
    }

    fn finish_save(
        &mut self,
        request_id: u64,
        path: &str,
        revision: usize,
        text: Rope,
        result: Result<(), ahead_rpc::RpcError>,
        cx: &mut Context<Self>,
    ) {
        if self.file_path == path {
            match result {
                Ok(()) if request_id >= self.saved_request_id => {
                    self.saved_request_id = request_id;
                    self.saved_rev = revision;
                    self.saved_sha256 = Some(format!(
                        "{:x}",
                        sha2::Sha256::digest(text.to_string().as_bytes())
                    ));
                    self.recovery_conflict = false;
                    self.saved_text = text;
                    self.dirty = self.editor.read(cx).text() != &self.saved_text;
                    self.git_key = None;
                    self.refresh_attribution();
                    if request_id == self.save_request_id {
                        self.status = format!("Saved {path}").into();
                    }
                }
                Err(error) if request_id == self.save_request_id => {
                    self.status = format!("Save failed: {}", error.message).into()
                }
                _ => {}
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
            self.suppress_completion_for_next_edit = true;
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
        let Some(completion) =
            self.completions.get(self.selected_completion).cloned()
        else {
            return;
        };
        let Some(context) = self.completion_context.take() else {
            return;
        };
        self.show_completions = false;
        cx.notify();
        if context != self.current_completion_context(cx) {
            self.status = "Completion is stale; request it again".into();
            return;
        }
        if let Some(proxy) = &self.proxy {
            let receiver = proxy.resolve_completion(completion);
            self.status = "Resolving completion…".into();
            cx.spawn_in(window, async move |this, cx| {
                let result = cx
                    .background_spawn(async move {
                        receiver
                            .recv()
                            .map_err(|error| error.to_string())?
                            .map_err(|error| error.message)
                    })
                    .await;
                this.update_in(cx, |this, window, cx| {
                    this.finish_completion(&context, result, window, cx);
                })
            })
            .detach_and_log_err(cx);
        } else {
            self.finish_completion(&context, Ok(completion.item), window, cx);
        }
    }

    fn current_completion_context(&self, cx: &App) -> CompletionContext {
        CompletionContext {
            generation: self.request_generation,
            request_id: self.completion_request_id,
            path: self.proxy_path(),
            position: self.cursor_position(cx),
        }
    }

    fn finish_completion(
        &mut self,
        context: &CompletionContext,
        result: Result<lsp_types::CompletionItem, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if context != &self.current_completion_context(cx) {
            return;
        }
        let applied = result.and_then(|item| {
            let text = self.editor.read(cx).value().to_string();
            let (range, replacement, caret) =
                completion_edit(&text, context.position, &item)?;
            self.suppress_completion_for_next_edit = true;
            self.editor.update(cx, |editor, cx| {
                editor.set_selected_range(range, cx);
                editor.replace(replacement, window, cx);
                editor.set_selected_range(caret..caret, cx);
            });
            self.request_generation = self.request_generation.wrapping_add(1);
            Ok(item.label)
        });
        self.status = match applied {
            Ok(label) => format!("Completed {label}"),
            Err(error) => format!("Completion failed: {error}"),
        }
        .into();
        cx.notify();
    }

    pub fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_text_available() {
            return;
        }
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        let primary = modifiers.platform
            && (!cfg!(target_os = "macos") || !modifiers.control)
            && !modifiers.alt;
        let ctrl = modifiers.control;
        let alt = modifiers.alt;

        if self
            .hover_chord_deadline
            .take()
            .is_some_and(|deadline| Instant::now() <= deadline)
            && primary
            && !modifiers.shift
            && key == "i"
        {
            self.request_proxy_hover(cx);
            return;
        }
        if primary && !modifiers.shift && key == "k" {
            self.hover_chord_deadline =
                Some(Instant::now() + Duration::from_secs(2));
            return;
        }

        if primary && !modifiers.shift && key == "s" {
            self.save(window, cx);
            return;
        }

        if primary
            && modifiers.shift
            && key == "v"
            && is_markdown_path(&self.file_path)
        {
            self.toggle_markdown_view(window, cx);
            return;
        }

        // Ctrl+Space: proxy-backed completions (falls back to local list).
        if ctrl && matches!(key, " " | "space") {
            self.request_proxy_completions(cx);
            return;
        }

        // Alt+G: accept next-word style inline prediction from proxy.
        if alt && key == "g" {
            self.request_proxy_inline(cx);
            return;
        }

        // F12: go to definition via proxy.
        if key == "f12" && !primary && !ctrl && !alt && !modifiers.shift {
            self.go_to_definition(cx);
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

    fn go_to_definition(&mut self, cx: &mut Context<Self>) {
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        let path = self.proxy_path();
        let pos = self.cursor_position(cx);
        let generation = self.request_generation;
        let rx = proxy.request_definition(path.clone(), pos);
        cx.spawn(async move |this, cx| {
            let locs = cx
                .background_spawn(async move { rx.recv().unwrap_or_default() })
                .await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                if this.request_generation != generation
                    || this.proxy_path() != path
                    || this.cursor_position(cx) != pos
                {
                    return;
                }
                if let Some(loc) = locs.first() {
                    match definition_open_request(loc) {
                        Ok(request) => {
                            this.status = if locs.len() > 1 {
                                format!(
                                    "Opening first of {} definitions",
                                    locs.len()
                                )
                                .into()
                            } else {
                                "Opening definition".into()
                            };
                            cx.emit(request);
                        }
                        Err(error) => this.status = error.into(),
                    }
                } else {
                    this.status = "No definition from proxy".into();
                }
                cx.notify();
            });
        })
        .detach();
    }

}

fn completion_edit(
    text: &str,
    position: lsp_types::Position,
    item: &lsp_types::CompletionItem,
) -> Result<(std::ops::Range<usize>, String, usize), String> {
    if item.insert_text_format == Some(lsp_types::InsertTextFormat::SNIPPET) {
        return Err("The server returned an unsupported snippet".into());
    }
    let byte_range = |range: lsp_types::Range| {
        let start = caret_byte_offset(text, range.start);
        let end = caret_byte_offset(text, range.end);
        match (start, end) {
            (Some(start), Some(end)) if start <= end => Ok(start..end),
            _ => Err("The server returned an invalid edit range".to_owned()),
        }
    };
    let (range, new_text) = match &item.text_edit {
        Some(lsp_types::CompletionTextEdit::Edit(edit)) => {
            (byte_range(edit.range)?, edit.new_text.as_str())
        }
        Some(lsp_types::CompletionTextEdit::InsertAndReplace(edit)) => {
            (byte_range(edit.replace)?, edit.new_text.as_str())
        }
        None => (
            completion_prefix_range(text, position)
                .ok_or("The completion position is stale")?,
            item.insert_text.as_deref().unwrap_or(&item.label),
        ),
    };
    let mut edits = vec![(range, new_text, true)];
    for edit in item.additional_text_edits.iter().flatten() {
        edits.push((byte_range(edit.range)?, edit.new_text.as_str(), false));
    }
    edits.sort_by_key(|(range, _, _)| (range.start, range.end));
    for pair in edits.windows(2) {
        let previous = &pair[0].0;
        let next = &pair[1].0;
        if previous.end > next.start
            || (previous.is_empty()
                && next.is_empty()
                && previous.start == next.start)
        {
            return Err("The server returned overlapping completion edits".into());
        }
    }
    let start = edits.first().ok_or("Missing completion edit")?.0.start;
    let end = edits.last().ok_or("Missing completion edit")?.0.end;
    let mut replacement = String::new();
    let mut offset = start;
    let mut caret = start;
    for (range, new_text, primary) in edits {
        replacement.push_str(&text[offset..range.start]);
        replacement.push_str(new_text);
        if primary {
            caret = start + replacement.len();
        }
        offset = range.end;
    }
    // gpui-kit's public edit API has no transaction grouping. One bounded
    // replacement keeps the symbol and its imports in a single Undo operation.
    Ok((start..end, replacement, caret))
}

fn completion_prefix_range(
    text: &str,
    position: lsp_types::Position,
) -> Option<std::ops::Range<usize>> {
    let end = caret_byte_offset(text, position)?;
    let start = text[..end]
        .char_indices()
        .rev()
        .take_while(|(_, character)| {
            character.is_alphanumeric() || matches!(character, '_' | '$')
        })
        .last()
        .map_or(end, |(index, _)| index);
    Some(start..end)
}

fn rank_completions(items: Vec<LspCompletion>, query: &str) -> Vec<LspCompletion> {
    let pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut matcher = FuzzyMatcher::new(FuzzyConfig::DEFAULT);
    let mut candidate_chars = Vec::new();
    let mut ranked: Vec<_> = items
        .into_iter()
        .filter_map(|completion| {
            let item = &completion.item;
            let filter = item.filter_text.as_deref().unwrap_or(&item.label);
            let score = pattern
                .score(Utf32Str::new(filter, &mut candidate_chars), &mut matcher)?;
            Some((score, completion))
        })
        .collect();
    ranked.sort_by(|(left_score, left), (right_score, right)| {
        let (left, right) = (&left.item, &right.item);
        right_score
            .cmp(left_score)
            .then_with(|| {
                right
                    .preselect
                    .unwrap_or(false)
                    .cmp(&left.preselect.unwrap_or(false))
            })
            .then_with(|| {
                left.sort_text
                    .as_deref()
                    .unwrap_or(&left.label)
                    .cmp(right.sort_text.as_deref().unwrap_or(&right.label))
            })
    });
    ranked.into_iter().take(12).map(|(_, item)| item).collect()
}

fn editor_language(path: &str) -> SharedString {
    let path = std::path::Path::new(path);
    let suffix = path
        .extension()
        .or_else(|| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = match suffix.as_str() {
        "js" | "mjs" | "cjs" | "jsx" => return "javascript".into(),
        "ts" | "mts" | "cts" => return "typescript".into(),
        "tsx" => return "tsx".into(),
        "py" | "pyi" => return "python".into(),
        name => name,
    };
    gpui_kit::component::highlighter::LanguageRegistry::singleton()
        .language(name)
        .map(|language| language.name)
        .unwrap_or_else(|| "text".into())
}

fn definition_open_request(
    location: &lsp_types::Location,
) -> Result<crate::ross::OpenRequest, String> {
    use crate::ross::{OpenColumn, OpenLocation, OpenRequest};
    let path = location
        .uri
        .to_file_path()
        .map_err(|_| format!("Cannot open definition URI: {}", location.uri))?;
    Ok(OpenRequest {
        path: path.to_string_lossy().into_owned(),
        permanent: false,
        location: Some(OpenLocation {
            line: location.range.start.line as usize,
            column: OpenColumn::Utf16(location.range.start.character as usize),
            end_line: location.range.end.line as usize,
            end_column: OpenColumn::Utf16(location.range.end.character as usize),
        }),
    })
}

fn unique_quote_range(
    text: &str,
    quote: &str,
) -> Result<(usize, usize), &'static str> {
    if quote.is_empty() {
        return Err("The requested code quote is empty");
    }
    let Some(start) = text.find(quote) else {
        return Err("The quoted code is no longer present in this editor buffer");
    };
    if text[start + quote.len()..].contains(quote) {
        return Err(
            "The quoted code appears more than once; use a more specific quote",
        );
    }
    Ok((start, start + quote.len()))
}

fn location_byte_position(
    text: &str,
    line: usize,
    column: crate::ross::OpenColumn,
) -> (usize, usize) {
    let mut line_start = 0;
    for _ in 0..line {
        let Some(line_break) = text[line_start..].find('\n') else {
            return (text.len(), 0);
        };
        line_start += line_break + 1;
    }
    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |line_break| line_start + line_break);
    let line_text = &text[line_start..line_end];
    let mut byte_column = match column {
        crate::ross::OpenColumn::Utf8Byte(byte_column) => {
            byte_column.min(line_text.len())
        }
        crate::ross::OpenColumn::Character(character_column) => line_text
            .char_indices()
            .nth(character_column)
            .map_or(line_text.len(), |(byte_column, _)| byte_column),
        crate::ross::OpenColumn::Utf16(column) => {
            ahead_core::encoding::offset_utf16_to_utf8_str(line_text, column)
        }
    };
    while byte_column > 0 && !line_text.is_char_boundary(byte_column) {
        byte_column -= 1;
    }
    (line_start + byte_column, byte_column)
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
        let dirty = self.dirty;
        let group = self.tab_group.clone();
        let panel_id = self.panel_id;
        let close_handler = self.close_handler.clone();
        let tab_handler = self.tab_handler.clone();
        let double_tab_handler = tab_handler.clone();
        let button_tab_handler = tab_handler.clone();
        let file_tab_handler = tab_handler.clone();
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
                    let tab_handler = file_tab_handler.clone();
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
                    .separator()
                    .item(item(
                        "Copy Relative Path",
                        CodeTabAction::CopyRelativePath,
                    ))
                    .item(item(
                        "Copy Absolute Path",
                        CodeTabAction::CopyAbsolutePath,
                    ))
                    .item(item("Add to .gitignore", CodeTabAction::AddToGitignore))
                    .item(item(
                        "Reveal in File Manager",
                        CodeTabAction::RevealInFinder,
                    ))
                    .item(item("Duplicate File", CodeTabAction::DuplicateFile))
                    .item(item("Move to Trash", CodeTabAction::TrashFile))
                    .separator()
                    .item(item("View History", CodeTabAction::ViewHistory))
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
impl EventEmitter<crate::ross::OpenRequest> for CodePanel {}

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
        if let Some(error) = &self.file_error {
            return v_flex()
                .id("file-open-error")
                .debug_selector(|| "file-open-error".into())
                .size_full()
                .bg(cx.theme().background)
                .track_focus(&self.focus)
                .child(Empty::new().header(
                    EmptyHeader::new()
                        .title(EmptyTitle::new().child("File cannot be opened as text"))
                        .description(EmptyDescription::new().child(error.to_string())),
                ))
                .into_any_element();
        }
        self.sync_proxy_diagnostics(cx);
        self.refresh_git_metadata(cx);
        let dirty = self.dirty;
        let border_color = cx.theme().border;
        let text_color = cx.theme().sidebar_foreground;
        let bar_bg = cx.theme().tab_bar;
        let editor_bg = cx.theme().background;
        let breakpoint_style =
            ButtonCustomVariant::new(cx).hover(cx.theme().danger.opacity(0.22));
        let popup_bg = cx.theme().popover;
        let active_sel_bg = popup_bg.blend(cx.theme().list_active);
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
        let tab_handler = self.tab_handler.clone();
        let panel_id = self.panel_id;

        let hunks = &self.git_state.hunks;
        let hunk_lines: std::collections::HashMap<u32, (u32, DiffHunkKind)> = hunks
            .iter()
            .filter(|h| !matches!(h.kind, DiffHunkKind::Deleted))
            .flat_map(|h| (h.start..h.start + h.len).map(|l| (l, (h.start, h.kind))))
            .collect();
        let actor_lines = self
            .proxy
            .as_ref()
            .map(|proxy| agent_anchor_lines(&proxy.anchors_for(&self.proxy_path())))
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
        let editor_text = editor_state.text().to_string();
        let presentation_line = self
            .presentation_decorations
            .get_ranges(cx)
            .first()
            .and_then(|range| editor_text.get(..range.start))
            .map(|prefix| prefix.bytes().filter(|byte| *byte == b'\n').count() + 1);
        let pointer_line = presentation_gutter_line(self.presentation.as_ref());
        let pointer_range = self
            .presentation
            .as_ref()
            .and_then(|presentation| {
                unique_quote_range(&editor_text, &presentation.pointer_quote).ok()
            })
            .map(|(start, _)| start..start);
        let pointer_editor = self.editor.clone();
        let pointer_panel = cx.entity();
        let presentation_overlay = self.presentation.clone().map(|presentation| {
            let line = presentation_line.unwrap_or(1) as f32;
            let top = scroll_offset.y + line_height * line + px(4.);
            (presentation, top)
        });
        let pointer_overlay = self
            .pointer_overlay_position
            .map(|(left, top)| (px(left), px(top)));
        let gutter_top = scroll_offset.y + px(8.);
        let expanded_hunk = self
            .expanded_hunk_start
            .and_then(|start| hunks.iter().find(|hunk| hunk.start == start))
            .cloned();
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
        let git_lens = self
            .git_state
            .blame
            .iter()
            .find(|hunk| {
                (hunk.start..hunk.start.saturating_add(hunk.len))
                    .contains(&(self.active_line as usize))
            })
            .map(blame_label);
        let hover_text = self.hover_text.clone();
        v_flex()
            .size_full()
            .bg(editor_bg)
            .track_focus(&self.focus)
            .capture_action(cx.listener(|this: &mut Self, action: &Enter, window, cx| {
                if this.show_completions && !action.secondary && !action.shift {
                    this.accept_completion(window, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &IndentInline, window, cx| {
                if this.show_completions {
                    this.accept_completion(window, cx);
                    cx.stop_propagation();
                } else if this.ghost_text.is_some() {
                    this.accept_ghost_text(window, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &MoveDown, _, cx| {
                if this.show_completions {
                    this.selected_completion = (this.selected_completion + 1)
                        .min(this.completions.len().saturating_sub(1));
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &MoveUp, _, cx| {
                if this.show_completions {
                    this.selected_completion = this.selected_completion.saturating_sub(1);
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &Escape, _, cx| {
                if this.show_completions || this.ghost_text.is_some() {
                    this.show_completions = false;
                    this.ghost_text = None;
                    this.ghost_offset = None;
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &SaveFile, window, cx| {
                this.save(window, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &GoToDefinition, _, cx| {
                this.go_to_definition(cx);
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(|this: &mut Self, event: &KeyDownEvent, window, cx| {
                this.interrupt_speech();
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
                                        .text_color(cx.theme().warning)
                                        .child(format!("• modified ({})", crate::app::shortcut_hint("⌘S", "Ctrl+S")))
                                )
                            })
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .when(self.show_markdown_preview || is_markdown_path(&self.file_path), |el| {
                                el.child(
                                    Button::new("toggle_markdown_view")
                                        .icon(if self.show_markdown_preview {
                                            IconName::Code
                                        } else {
                                            IconName::FileCode
                                        })
                                        .ghost()
                                        .tooltip(format!(
                                            "{} ({})",
                                            if self.show_markdown_preview { "Edit Markdown" } else { "Preview Markdown" },
                                            crate::app::shortcut_hint("⌘⇧V", "Ctrl+Shift+V")
                                        ))
                                        .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                            this.toggle_markdown_view(window, cx);
                                        })),
                                )
                            })
                            .when(has_diag, |el| {
                                el.child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        .child(IconName::ShieldAlert)
                                        .child(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(cx.theme().warning)
                                                .child(diag_text.clone())
                                        )
                                )
                            })
                    )
            )
            // Editor row: native gutter rail (diff + breakpoints) beside editor
            .when(!self.show_markdown_preview, |root| root.child(
                h_flex()
                    .flex_1()
                    .h_full()
                    .min_h_0()
                    .child(
                        div()
                            .w(px(32.))
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
                                        let show_breakpoint_preview = self.hovered_breakpoint_line == Some(ln) && !is_bp;
                                        let is_agent = actor_lines.contains(&ln);
                                        let is_pointer = pointer_line == Some(ln);
                                        h_flex()
                                            .id(("gutter", ln as usize))
                                            .h(line_height)
                                            .items_center()
                                            .justify_center()
                                            .gap(px(0.))
                                            .child(
                                                div()
                                                    .id(("git-change", ln as usize))
                                                    .debug_selector(move || format!("git-change-{ln}").into())
                                                    .w(px(8.))
                                                    .h(line_height)
                                                    .flex()
                                                    .items_center()
                                                    .child(
                                                        div()
                                                            .w(px(3.))
                                                            .h(if is_deleted && hunk.is_none() { px(3.) } else { line_height })
                                                            .bg(match hunk.map(|(_, kind)| kind) {
                                                                Some(DiffHunkKind::Added) => cx.theme().success,
                                                                Some(DiffHunkKind::Modified) => cx.theme().warning,
                                                                _ if is_deleted => cx.theme().danger,
                                                                _ => gpui_kit::Hsla::transparent_black(),
                                                            })
                                                    )
                                                    .when(hunk.is_some() || is_deleted, |el| {
                                                        el.cursor(CursorStyle::PointingHand)
                                                    })
                                                    .when_some(hunk.map(|(start, _)| start).or_else(|| is_deleted.then_some(ln)), |el, start| {
                                                        el.on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                            this.expanded_hunk_start = (this.expanded_hunk_start != Some(start)).then_some(start);
                                                            cx.notify();
                                                        }))
                                                    })
                                            )
                                            .child(
                                                div()
                                                    .w(px(2.))
                                                    .h(px(12.))
                                                    .bg(if is_agent {
                                                        cx.theme().magenta
                                                    } else {
                                                        gpui_kit::Hsla::transparent_black()
                                                    })
                                            )
                                            .child(
                                                div()
                                                    .id(("breakpoint-hover", ln as usize))
                                                    .w(px(16.))
                                                    .h(line_height)
                                                    .on_hover({
                                                        let panel = code_panel.clone();
                                                        move |hovered, _, cx| {
                                                            panel.update(cx, |panel, cx| {
                                                                let next = if *hovered { Some(ln) } else { None };
                                                                if panel.hovered_breakpoint_line != next {
                                                                    panel.hovered_breakpoint_line = next;
                                                                    cx.notify();
                                                                }
                                                            });
                                                        }
                                                    })
                                                    .child(Button::new(("breakpoint", ln as usize))
                                                    .debug_selector(move || format!("breakpoint-target-{ln}").into())
                                                    .with_size(px(16.))
                                                    .w(px(16.))
                                                    .h(line_height)
                                                    .p_0()
                                                    .rounded(px(3.))
                                                    .cursor(CursorStyle::PointingHand)
                                                    .custom(breakpoint_style)
                                                    .accessibility_label(if is_bp {
                                                        format!("Remove breakpoint at line {ln}")
                                                    } else {
                                                        format!("Add breakpoint at line {ln}")
                                                    })
                                                    .tooltip(if is_bp {
                                                        format!("Remove breakpoint (F9) · line {ln}")
                                                    } else {
                                                        format!("Add breakpoint (F9) · line {ln}")
                                                    })
                                                    .child(
                                                        div()
                                                            .w(px(6.))
                                                            .h(px(6.))
                                                            .rounded_full()
                                                            .bg(if is_bp || show_breakpoint_preview {
                                                                cx.theme().danger
                                                            } else {
                                                                gpui_kit::Hsla::transparent_black()
                                                            }),
                                                    )
                                                    .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                        this.interrupt_speech();
                                                        if let Some(proxy) = this.proxy.clone() {
                                                            proxy.toggle_breakpoint(
                                                                std::path::Path::new(&this.file_path),
                                                                ln,
                                                            );
                                                        }
                                                        this.active_line = ln;
                                                        cx.notify();
                                                    }))),
                                            )
                                            .child(
                                                div()
                                                    .w(px(4.))
                                                    .h(px(4.))
                                                    .bg(if is_pointer {
                                                        cx.theme().primary
                                                    } else {
                                                        gpui_kit::Hsla::transparent_black()
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
                                let editor_focus = code_panel
                                    .read(_cx)
                                    .editor
                                    .read(_cx)
                                    .focus_handle(_cx);
                                let file_item =
                                    |label: &'static str, action: CodeTabAction| {
                                        let tab_handler = tab_handler.clone();
                                        PopupMenuItem::new(label).on_click(
                                            move |_, window, cx| {
                                                if let Some(handler) =
                                                    tab_handler.as_ref()
                                                {
                                                    handler(
                                                        panel_id, action, window, cx,
                                                    );
                                                }
                                            },
                                        )
                                    };
                                menu.item(PopupMenuItem::new("Cut").on_click({
                                    let editor_focus = editor_focus.clone();
                                    move |_, window, cx| {
                                        editor_focus.dispatch_action(&CutAction, window, cx);
                                    }
                                }))
                                .item(PopupMenuItem::new("Copy").on_click({
                                    let editor_focus = editor_focus.clone();
                                    move |_, window, cx| {
                                        editor_focus.dispatch_action(&CopyAction, window, cx);
                                    }
                                }))
                                .item(PopupMenuItem::new("Paste").on_click({
                                    let editor_focus = editor_focus.clone();
                                    move |_, window, cx| {
                                        editor_focus.dispatch_action(&PasteAction, window, cx);
                                    }
                                }))
                                .item(PopupMenuItem::new("Select All").on_click(
                                    move |_, window, cx| {
                                        editor_focus.dispatch_action(&SelectAll, window, cx);
                                    },
                                ))
                                .separator()
                                .item(file_item(
                                    "Copy Relative Path",
                                    CodeTabAction::CopyRelativePath,
                                ))
                                .item(file_item(
                                    "Copy Absolute Path",
                                    CodeTabAction::CopyAbsolutePath,
                                ))
                                .item(file_item(
                                    "Add to .gitignore",
                                    CodeTabAction::AddToGitignore,
                                ))
                                .item(file_item(
                                    "Reveal in File Manager",
                                    CodeTabAction::RevealInFinder,
                                ))
                                .item(file_item(
                                    "Duplicate File",
                                    CodeTabAction::DuplicateFile,
                                ))
                                .item(file_item(
                                    "Move to Trash",
                                    CodeTabAction::TrashFile,
                                ))
                                .separator()
                                .item(file_item(
                                    "View History",
                                    CodeTabAction::ViewHistory,
                                ))
                                .separator()
                                .item(
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
                            .when_some(expanded_hunk, |el, hunk| {
                                let top = gutter_top + line_height * (hunk.start + hunk.len).saturating_sub(1) as f32;
                                let new_lines: Vec<_> = editor_text.lines()
                                    .skip(hunk.start.saturating_sub(1) as usize)
                                    .take(hunk.len as usize)
                                    .map(str::to_owned)
                                    .collect();
                                el.child(
                                    v_flex()
                                        .id("expanded_git_hunk")
                                        .debug_selector(|| "expanded-git-hunk".into())
                                        .absolute()
                                        .top(top)
                                        .left(px(8.))
                                        .right(px(8.))
                                        .max_w(px(720.))
                                        .max_h(px(320.))
                                        .overflow_y_scroll()
                                        .bg(popup_bg)
                                        .border_1()
                                        .border_color(border_color)
                                        .rounded_md()
                                        .shadow_md()
                                        .child(
                                            h_flex()
                                                .justify_between()
                                                .items_center()
                                                .p_1()
                                                .child(format!("Uncommitted change · line {}", hunk.start))
                                                .child(
                                                    Button::new("close_git_hunk")
                                                        .icon(IconName::X)
                                                        .ghost()
                                                        .tooltip("Close change preview")
                                                        .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                                            this.expanded_hunk_start = None;
                                                            cx.notify();
                                                        }))
                                                )
                                        )
                                        .children(hunk.old_lines.into_iter().map(|line| {
                                            div()
                                                .px_2()
                                                .font_family("Menlo")
                                                .text_size(px(11.))
                                                .bg(cx.theme().danger.opacity(0.16))
                                                .child(format!("- {line}"))
                                        }))
                                        .children(new_lines.into_iter().map(|line| {
                                            div()
                                                .px_2()
                                                .font_family("Menlo")
                                                .text_size(px(11.))
                                                .bg(cx.theme().success.opacity(0.16))
                                                .child(format!("+ {line}"))
                                        })),
                                )
                            })
                            .when_some(pointer_range, |el, range| {
                                let pointer_editor = pointer_editor.clone();
                                let pointer_panel = pointer_panel.clone();
                                el.child(
                                    canvas(
                                        move |bounds, _, cx| {
                                            pointer_editor
                                                .read(cx)
                                                .range_to_bounds(&range)
                                                .map(|target| {
                                                    let origin = target.origin - bounds.origin;
                                                    (origin.x.as_f32(), origin.y.as_f32())
                                                })
                                        },
                                        move |_, position, _, cx| {
                                            pointer_panel.update(cx, |panel, cx| {
                                                if panel.pointer_overlay_position != position {
                                                    panel.pointer_overlay_position = position;
                                                    cx.notify();
                                                }
                                            });
                                        },
                                    )
                                    .absolute()
                                    .inset_0(),
                                )
                            })
                            .when_some(presentation_overlay, |el, (presentation, top)| {
                                el.child(
                                    div()
                                        .absolute()
                                        .top(top)
                                        .left(px(8.))
                                        .max_w(px(420.))
                                        .p_2()
                                        .bg(popup_bg)
                                        .border_1()
                                        .border_color(cx.theme().primary)
                                        .shadow_lg()
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .items_center()
                                                .child(IconName::MessageSquare)
                                                .child(
                                                    div()
                                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                                        .text_size(px(11.))
                                                        .text_color(cx.theme().primary)
                                                        .child(presentation.cue.label),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .mt_1()
                                                .text_size(px(11.))
                                                .text_color(text_color)
                                                .child(presentation.cue.note.unwrap_or_default()),
                                        ),
                                )
                            })
                            .when_some(pointer_overlay, |el, (left, top)| {
                                el.child(
                                    div()
                                        .absolute()
                                        .top(top)
                                        .left(left)
                                        .p_1()
                                        .rounded_md()
                                        .bg(popup_bg)
                                        .border_1()
                                        .border_color(cx.theme().primary)
                                        .text_color(cx.theme().primary)
                                        .child(IconName::ArrowRight),
                                )
                            })
                            .when(has_diag, |el| {
                                el.child(
                                    div()
                                        .absolute()
                                        .top(px(6.))
                                        .right(px(8.))
                                        .max_w(px(520.))
                                        .text_size(px(10.))
                                        .text_color(cx.theme().warning)
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
                                .bg(cx.theme().accent)
                                .border_1()
                                .border_color(cx.theme().primary)
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .font_family("Menlo")
                                                .text_color(cx.theme().accent_foreground)
                                                .child(ghost)
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(cx.theme().muted_foreground)
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
                                                    .bg(if is_sel { active_sel_bg } else { popup_bg })
                                                    .child(
                                                        h_flex()
                                                            .gap_2()
                                                            .items_center()
                                                            .child(IconName::Code)
                                                            .child(
                                                                div()
                                                                    .font_weight(if is_sel { gpui_kit::FontWeight::BOLD } else { gpui_kit::FontWeight::NORMAL })
                                                                    .text_size(px(12.))
                                                                    .text_color(text_color)
                                                                    .child(item.item.label.clone())
                                                            )
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(10.))
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(item.item.detail.clone().unwrap_or_default())
                                                    )
                                            })
                                        )
                                )
                        )
                    })
                    )
            ))
            .when(self.show_markdown_preview, |root| {
                let markdown = self.editor.read(cx).value().to_string();
                root.child(
                    div()
                        .flex_1()
                        .h_full()
                        .min_w_0()
                        .bg(editor_bg)
                        .p_4()
                        .child(
                            TextView::markdown(
                                SharedString::from(format!(
                                    "markdown-preview-{}",
                                    self.panel_id.as_u64()
                                )),
                                SharedString::from(markdown),
                            )
                            .scrollable(true),
                        ),
                )
            })
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
                            .when_some(git_lens, |el, label| {
                                el.child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(label),
                                )
                            })
                    )
            )
            .into_any_element()
    }
}

fn is_markdown_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md")
                || extension.eq_ignore_ascii_case("markdown")
        })
}

fn blame_label(hunk: &BlameHunk) -> String {
    let Some(commit) = &hunk.commit else {
        return "Uncommitted changes".to_string();
    };
    let date = chrono::DateTime::from_timestamp(commit.timestamp, 0)
        .map(|date| date.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown date".to_string());
    let short_id: String = commit.id.chars().take(7).collect();
    format!(
        "{} · {} · {} ({})",
        commit.author, date, commit.subject, short_id
    )
}

/// Converts an LSP UTF-16 position to a byte offset in `text`.
fn caret_byte_offset(text: &str, pos: lsp_types::Position) -> Option<usize> {
    let mut offset = 0;
    let mut lines = text.split_inclusive('\n');
    for _ in 0..pos.line {
        offset += lines.next()?.len();
    }
    let line = match lines.next() {
        Some(line) => line,
        None if pos.line == 0 || text.ends_with('\n') => "",
        None => return None,
    };
    let content = line.strip_suffix('\n').unwrap_or(line);
    let content = content.strip_suffix('\r').unwrap_or(content);
    let mut taken = 0;
    let mut count: u32 = 0;
    for ch in content.chars() {
        if count >= pos.character {
            break;
        }
        taken += ch.len_utf8();
        count += ch.len_utf16() as u32;
    }
    if count != pos.character {
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

fn agent_anchor_lines(
    anchors: &[ahead_rpc::ahead::CodeAnchor],
) -> std::collections::HashSet<u32> {
    anchors
        .iter()
        .filter(|anchor| anchor.actor_id == ahead_rpc::ahead::AHEAD_ACTOR_ID)
        .flat_map(|anchor| anchor.range.start.line..=anchor.range.end.line)
        .collect()
}

fn presentation_gutter_line(
    presentation: Option<&ActivePresentation>,
) -> Option<u32> {
    presentation
        .and_then(|presentation| presentation.cue.pointer_target)
        .map(|target| target.line.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::{
        CodePanel, agent_anchor_lines, blame_label, caret_byte_offset,
        completion_edit, completion_prefix_range, definition_open_request,
        editor_language, interpolate_insertion, is_markdown_path,
        location_byte_position, presentation_gutter_line, rank_completions,
    };
    use crate::proxy_client::LspCompletion;
    use crate::ross::{OpenColumn, OpenLocation};
    use ahead_rpc::plugin::PluginId;
    use gpui_kit::Focusable;
    use gpui_kit::component::input::Undo;

    #[test]
    fn file_open_accepts_utf8_and_preserves_read_failures() {
        use super::{FileOpenError, read_editor_file};
        let directory = tempfile::tempdir().expect("workspace");
        let path = directory.path().join("file");
        let path = path.to_str().expect("path");
        for text in ["", "hello\n", "α🙂\r\n", "\u{feff}UTF-8 BOM\n"] {
            std::fs::write(path, text).expect("text");
            assert_eq!(read_editor_file(path).expect("UTF-8"), text);
        }
        for bytes in [
            &b"SQLite format 3\0\0\x01\xff"[..],
            &b"%PDF-1.7\n"[..],
            &b"GIF89a"[..],
            &b"text\0binary"[..],
            &b"\xff\xfeh\0i\0"[..],
            &b"h\0i\0"[..],
            &b"caf\xe9"[..],
            &b"truncated \xf0\x9f"[..],
        ] {
            std::fs::write(path, bytes).expect("unsupported data");
            assert!(matches!(read_editor_file(path), Err(FileOpenError::UnsupportedText)));
            assert_eq!(std::fs::read(path).expect("unchanged file"), bytes);
        }
        std::fs::remove_file(path).expect("remove fixture");
        assert!(matches!(read_editor_file(path), Err(FileOpenError::Read(error)) if error.kind() == std::io::ErrorKind::NotFound));
        let denied = FileOpenError::Read(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert!(denied.to_string().contains("permission denied"));
    }

    #[gpui_kit::test]
    fn file_open_errors_block_edit_save_and_proxy_snapshots(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::proxy::{ProxyNotification, ProxyRequest, ProxyRpc};
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let binary = directory.path().join("session.db");
        let bytes = b"SQLite format 3\0\x01\xff";
        std::fs::write(&binary, bytes).expect("binary fixture");
        let text = directory.path().join("empty.txt");
        std::fs::write(&text, "").expect("empty text");
        let missing = directory.path().join("missing.txt");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(directory.path().to_owned());
        let rpc = proxy.rpc_for_test();
        proxy.enable_editor_recovery();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(binary.to_str().expect("path"), window, cx)
                .with_proxy(proxy.clone(), directory.path().to_str().expect("workspace"), cx)
        });
        panel.update_in(cx, |panel, window, cx| {
            assert!(!panel.is_text_available());
            assert!(panel.turn_context(cx).is_none());
            panel.save(window, cx);
            assert!(panel.queue_recovery(false, true, cx).is_none());
        });
        assert!(rpc.rx().try_recv().is_err(), "blocked tab must not reach the proxy");

        panel.update_in(cx, |panel, window, cx| {
            panel.open_file(text.to_str().expect("path"), true, window, cx);
            assert!(panel.is_text_available());
            assert!(panel.editor.read(cx).value().is_empty());
        });
        assert!(rpc.rx().try_iter().any(|message| matches!(message,
            ProxyRpc::Notification(ProxyNotification::EditorSnapshot { path, content }) if path == text && content.is_empty()
        )), "a real empty file is a valid text buffer");

        for path in [&binary, &missing] {
            panel.update_in(cx, |panel, window, cx| {
                panel.open_file(path.to_str().expect("path"), true, window, cx);
                assert_eq!(panel.file_path, path.to_str().expect("path"));
                assert!(!panel.is_text_available());
                assert!(!panel.dirty);
                assert!(panel.turn_context(cx).is_none());
                panel.save(window, cx);
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert!(cx.debug_bounds("file-open-error").is_some());
            cx.simulate_keystrokes("x cmd-v cmd-s");
            assert!(!panel.read_with(cx, |panel, _| panel.dirty));
            assert!(!rpc.rx().try_iter().any(|message| matches!(message,
                ProxyRpc::Notification(ProxyNotification::EditorSnapshot { .. }) |
                ProxyRpc::Request(_, ProxyRequest::SaveEditorBuffer { .. })
            )), "blocked tabs cannot publish or save empty buffers");
        }
        assert_eq!(std::fs::read(&binary).expect("binary unchanged"), bytes);
        assert!(!missing.exists());
        panel.update_in(cx, |panel, window, cx| {
            panel.open_file(text.to_str().expect("path"), true, window, cx);
            panel.editor.update(cx, |editor, cx| editor.replace_all("unsaved", window, cx));
        });
        panel.update_in(cx, |panel, window, cx| {
            assert!(panel.dirty);
            panel.open_file(binary.to_str().expect("path"), true, window, cx);
            assert_eq!(panel.file_path, text.to_str().expect("path"));
            assert_eq!(panel.editor.read(cx).value().as_ref(), "unsaved");
        });
    }

    #[test]
    fn completions_filter_before_the_limit_and_honor_server_sort_keys() {
        let mut items: Vec<_> = (0..30)
            .map(|index| lsp_types::CompletionItem {
                label: format!("unrelated_{index}"),
                ..Default::default()
            })
            .collect();
        items.extend([
            lsp_types::CompletionItem {
                label: "second label".into(),
                filter_text: Some("doubled".into()),
                sort_text: Some("2".into()),
                ..Default::default()
            },
            lsp_types::CompletionItem {
                label: "first label".into(),
                filter_text: Some("doubled".into()),
                sort_text: Some("1".into()),
                ..Default::default()
            },
        ]);
        let matches = rank_completions(
            items
                .into_iter()
                .map(|item| LspCompletion {
                    plugin_id: PluginId(7),
                    item,
                })
                .collect(),
            "dou",
        );
        assert_eq!(
            matches
                .iter()
                .map(|item| item.item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["first label", "second label"]
        );
        assert!(rank_completions(matches, "not_a_match").is_empty());
        let text = "α🙂 object.$dou";
        let position =
            lsp_types::Position::new(0, text.encode_utf16().count() as u32);
        assert_eq!(
            text.get(completion_prefix_range(text, position).expect("prefix")),
            Some("$dou")
        );
    }

    #[test]
    fn completion_edits_validate_ranges_and_preserve_unicode() {
        use lsp_types::{
            CompletionItem, CompletionTextEdit, Position, Range, TextEdit,
        };
        let text = "// α🙂\r\ncalc\r\n";
        let position = Position::new(1, 4);
        let mut item = CompletionItem {
            label: "calculate".into(),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: Range::new(Position::new(1, 0), position),
                new_text: "calculate".into(),
            })),
            additional_text_edits: Some(vec![TextEdit {
                range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                new_text: "import { calculate } from './math';\r\n".into(),
            }]),
            ..Default::default()
        };
        let (range, replacement, caret) =
            completion_edit(text, position, &item).expect("valid edits");
        let mut changed = text.to_owned();
        changed.replace_range(range, &replacement);
        assert_eq!(
            changed,
            "import { calculate } from './math';\r\n// α🙂\r\ncalculate\r\n"
        );
        assert!(changed[..caret].ends_with("calculate"));
        item.additional_text_edits = Some(vec![TextEdit {
            range: Range::new(Position::new(1, 2), Position::new(1, 3)),
            new_text: "overlap".into(),
        }]);
        assert!(
            completion_edit(text, position, &item)
                .expect_err("overlap")
                .contains("overlapping")
        );
        item.additional_text_edits = Some(vec![TextEdit {
            range: Range::new(Position::new(0, 5), Position::new(0, 5)),
            new_text: "inside a surrogate pair".into(),
        }]);
        assert!(completion_edit(text, position, &item).is_err());
        item.additional_text_edits = None;
        item.insert_text_format = Some(lsp_types::InsertTextFormat::SNIPPET);
        assert!(
            completion_edit(text, position, &item)
                .expect_err("unadvertised snippet")
                .contains("snippet")
        );
        assert_eq!(caret_byte_offset("abc", Position::new(1, 0)), None);
        assert_eq!(caret_byte_offset("abc\n", Position::new(1, 0)), Some(4));
        assert_eq!(caret_byte_offset("abc\r\n", Position::new(0, 4)), None);

        let boundary = CompletionItem {
            label: "calculate".into(),
            additional_text_edits: Some(vec![TextEdit {
                range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                new_text: "import { calculate } from './math';\n".into(),
            }]),
            ..Default::default()
        };
        let (_, replacement, caret) =
            completion_edit("calc", Position::new(0, 4), &boundary)
                .expect("boundary insertion");
        assert_eq!(
            replacement,
            "import { calculate } from './math';\ncalculate"
        );
        assert_eq!(caret, replacement.len());
    }

    #[gpui_kit::test]
    fn completion_auto_import_is_one_undo_and_late_results_are_ignored(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use lsp_types::{CompletionItem, Position, Range, TextEdit};
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("disposable workspace");
        let path = directory.path().join("main.ts");
        let original = "// α🙂\ncalc\n";
        std::fs::write(&path, original).expect("source");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        let item = CompletionItem {
            label: "calculate".into(),
            additional_text_edits: Some(vec![TextEdit {
                range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                new_text: "import { calculate } from './math';\n".into(),
            }]),
            ..Default::default()
        };
        let context = panel.update(cx, |panel, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(original.len() - 1..original.len() - 1, cx)
            });
            panel.current_completion_context(cx)
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.finish_completion(&context, Ok(item.clone()), window, cx)
        });
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).value().to_string()),
            "import { calculate } from './math';\n// α🙂\ncalculate\n"
        );
        assert_eq!(
            panel.update(cx, |panel, cx| panel.cursor_position(cx)),
            Position::new(2, 9)
        );
        assert!(panel.update(cx, |panel, _| panel.dirty));
        let editor = panel.update(cx, |panel, _| panel.editor.clone());
        let focus = editor.update(cx, |editor, cx| editor.focus_handle(cx));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.focus(&focus, cx);
            focus.dispatch_action(&Undo, window, cx);
        });
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).value().to_string()),
            original
        );
        assert!(!panel.update(cx, |panel, _| panel.dirty));

        let current =
            panel.update(cx, |panel, cx| panel.current_completion_context(cx));
        for stale in [
            super::CompletionContext {
                generation: current.generation.wrapping_add(1),
                ..current.clone()
            },
            super::CompletionContext {
                request_id: current.request_id.wrapping_add(1),
                ..current.clone()
            },
            super::CompletionContext {
                path: directory.path().join("other.ts"),
                ..current.clone()
            },
            super::CompletionContext {
                position: Position::new(
                    current.position.line,
                    current.position.character + 1,
                ),
                ..current.clone()
            },
        ] {
            panel.update_in(cx, |panel, window, cx| {
                panel.finish_completion(&stale, Ok(item.clone()), window, cx)
            });
            assert_eq!(
                panel.update(cx, |panel, cx| panel
                    .editor
                    .read(cx)
                    .value()
                    .to_string()),
                original
            );
        }
        panel.update_in(cx, |panel, window, cx| {
            panel.finish_completion(
                &current,
                Err("server stopped".into()),
                window,
                cx,
            )
        });
        assert!(
            panel.update(cx, |panel, _| panel.status.contains("server stopped"))
        );
        assert_eq!(
            std::fs::read_to_string(path).expect("unchanged disk"),
            original
        );
    }

    #[gpui_kit::test]
    fn preview_switches_update_the_editor_language(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("test workspace");
        let original = directory.path().join("main.rs");
        std::fs::write(&original, "fn main() {}\n").expect("write Rust source");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(original.to_str().expect("source path"), window, cx)
        });
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).language_name()),
            "rust"
        );
        for (name, language, source) in [
            ("main.ts", "typescript", "const count: number = 1;\n"),
            ("main.mts", "typescript", "export const count = 1;\n"),
            ("view.tsx", "tsx", "export const view = <div />;\n"),
            ("main.mjs", "javascript", "export const count = 1;\n"),
            ("main.cjs", "javascript", "module.exports = 1;\n"),
            ("view.jsx", "javascript", "export const view = <div />;\n"),
            (
                "main.py",
                "python",
                "def double(value):\n    return value * 2\n",
            ),
            ("main.pyi", "python", "def double(value: int) -> int: ...\n"),
            ("settings.json", "json", "{\"count\": 1}\n"),
            ("notes.unknown", "text", "ordinary text\n"),
            (
                "other.rs",
                "rust",
                "fn double(value: i32) -> i32 { value * 2 }\n",
            ),
        ] {
            let path = directory.path().join(name);
            std::fs::write(&path, source).expect("write source");
            let path = path.to_str().expect("source path");
            assert_eq!(editor_language(path), language);
            if matches!(language, "rust" | "json") {
                assert!(
                    gpui_kit::component::highlighter::LanguageRegistry::singleton()
                        .language(language)
                        .expect("registered language")
                        .has_grammar()
                );
            }
            panel.update_in(cx, |panel, window, cx| {
                panel.open_file(path, true, window, cx);
                assert_eq!(panel.editor.read(cx).language_name(), language);
                assert_eq!(panel.editor.read(cx).value(), source);
                assert!(!panel.dirty);
            });
        }
    }

    #[gpui_kit::test]
    fn definition_navigation_selects_the_utf16_target(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("test workspace");
        let path = directory.path().join("target.ts");
        std::fs::write(&path, "// before\nα🙂target\n").expect("write target");
        let uri = lsp_types::Url::from_file_path(&path).expect("file URI");
        let target = lsp_types::Location::new(
            uri,
            lsp_types::Range::new(
                lsp_types::Position::new(1, 3),
                lsp_types::Position::new(1, 9),
            ),
        );
        let request = definition_open_request(&target).expect("open definition");
        assert_eq!(request.path, path.to_string_lossy());
        let (panel, cx) = cx
            .add_window_view(|window, cx| CodePanel::new(&request.path, window, cx));
        panel.update_in(cx, |panel, window, cx| {
            panel.reveal_location(
                request.location.expect("target range"),
                window,
                cx,
            );
            let editor = panel.editor.read(cx);
            let text = editor.value().to_string();
            assert_eq!(text.get(editor.selected_range()), Some("target"));
        });
        let non_file = lsp_types::Location::new(
            lsp_types::Url::parse("https://example.invalid/target.ts").expect("URI"),
            target.range,
        );
        assert!(
            definition_open_request(&non_file)
                .expect_err("non-file URI")
                .contains("Cannot open definition URI")
        );
    }

    #[gpui_kit::test]
    fn enter_accepts_lsp_completion_over_unsaved_prefix(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let path = std::env::temp_dir()
            .join(format!("ahead-lsp-completion-{}.rs", uuid::Uuid::new_v4()));
        std::fs::write(&path, "🦀 math::dou\n").expect("write editor source");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_string_lossy().as_ref(), window, cx)
        });
        panel.update(cx, |panel, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(14..14, cx);
            });
            panel.completions.push(LspCompletion {
                plugin_id: PluginId(7),
                item: lsp_types::CompletionItem {
                    label: "double".into(),
                    text_edit: Some(lsp_types::CompletionTextEdit::Edit(
                        lsp_types::TextEdit {
                            new_text: "double".into(),
                            range: lsp_types::Range {
                                start: lsp_types::Position {
                                    line: 0,
                                    character: 9,
                                },
                                end: lsp_types::Position {
                                    line: 0,
                                    character: 12,
                                },
                            },
                        },
                    )),
                    ..Default::default()
                },
            });
            panel.completion_context = Some(panel.current_completion_context(cx));
            panel.show_completions = true;
        });
        let editor_focus =
            panel.update(cx, |panel, cx| panel.editor.read(cx).focus_handle(cx));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.focus(&editor_focus, cx);
        });
        cx.simulate_keystrokes("enter");
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).value().to_string()),
            "🦀 math::double\n"
        );
        assert!(!panel.update(cx, |panel, _| panel.show_completions));
        assert!(
            !panel.update(cx, |panel, _| panel.suppress_completion_for_next_edit)
        );
        std::fs::remove_file(path).expect("remove test source");
    }

    #[gpui_kit::test]
    async fn close_requires_save_or_explicit_unchanged_discard(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("disposable project");
        let path = directory.path().join("main.ts");
        std::fs::write(&path, "const saved = true;\n").expect("source");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        let clean =
            panel.update_in(cx, |panel, window, cx| panel.prepare_close(window, cx));
        assert!(clean.await.is_some());
        assert!(!cx.has_pending_prompt());
        let editor = panel.update(cx, |panel, _| panel.editor.clone());
        editor.update_in(cx, |editor, window, cx| {
            editor.insert("// keep my edit\n", window, cx)
        });
        let changed = editor.update(cx, |editor, _| editor.value().to_string());
        for answer in ["Cancel", "Save"] {
            let close = panel
                .update_in(cx, |panel, window, cx| panel.prepare_close(window, cx));
            assert!(cx.has_pending_prompt());
            cx.simulate_prompt_answer(answer);
            assert!(close.await.is_none());
            assert_eq!(
                editor.update(cx, |editor, _| editor.value().to_string()),
                changed
            );
            assert!(panel.update(cx, |panel, _| panel.dirty));
        }
        assert!(panel.update(cx, |panel, _| panel.status.contains("Save failed")));

        let stale =
            panel.update_in(cx, |panel, window, cx| panel.prepare_close(window, cx));
        editor.update_in(cx, |editor, window, cx| {
            editor.insert("// typed while prompt was open\n", window, cx)
        });
        cx.simulate_prompt_answer("Discard Changes");
        assert!(
            stale.await.is_none(),
            "a stale discard cannot authorize newer edits"
        );
        let confirmed =
            panel.update_in(cx, |panel, window, cx| panel.prepare_close(window, cx));
        cx.simulate_prompt_answer("Discard Changes");
        assert_eq!(
            confirmed.await,
            Some(panel.update(cx, |panel, _| panel.request_generation))
        );
        assert_eq!(
            std::fs::read_to_string(path).expect("disk"),
            "const saved = true;\n"
        );
    }

    #[gpui_kit::test]
    fn save_confirmation_preserves_newer_edits_and_failed_save_state(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("test project");
        let path = directory.path().join("main.rs");
        std::fs::write(&path, "fn main() {}\n").expect("write editor source");
        let path = path.to_string_lossy().into_owned();
        let (panel, cx) =
            cx.add_window_view(|window, cx| CodePanel::new(&path, window, cx));
        let editor = panel.update(cx, |panel, _| panel.editor.clone());
        editor.update_in(cx, |editor, window, cx| {
            editor.insert("// save this\n", window, cx)
        });
        let snapshot = editor.update(cx, |editor, _| editor.text().clone());
        let revision = panel.update(cx, |panel, _| panel.dirty_rev);
        editor.update_in(cx, |editor, window, cx| {
            editor.insert("// newer edit\n", window, cx)
        });
        panel.update(cx, |panel, cx| {
            panel.save_request_id = 1;
            panel.finish_save(1, &path, revision, snapshot.clone(), Ok(()), cx);
            assert!(panel.dirty);
            assert_eq!(panel.saved_text, snapshot);
            let current = panel.editor.read(cx).text().clone();
            panel.save_request_id = 2;
            panel.finish_save(
                2,
                &path,
                panel.dirty_rev,
                current.clone(),
                Err(ahead_rpc::RpcError {
                    code: 0,
                    message: "disk is full".into(),
                }),
                cx,
            );
            assert!(panel.dirty);
            assert_eq!(panel.saved_text, snapshot);
            assert!(panel.status.contains("disk is full"));
            panel.save_request_id = 3;
            panel.finish_save(
                3,
                &path,
                panel.dirty_rev,
                current.clone(),
                Ok(()),
                cx,
            );
            panel.finish_save(1, &path, revision, snapshot, Ok(()), cx);
            assert_eq!(panel.saved_text, current);
            assert!(!panel.dirty);
        });
    }

    #[gpui_kit::test]
    fn undo_to_saved_content_clears_dirty_state(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::component::init);
        let path = std::env::temp_dir()
            .join(format!("ahead-dirty-{}.rs", uuid::Uuid::new_v4()));
        std::fs::write(&path, "fn main() {}\n").expect("write editor source");
        let view_path = path.to_string_lossy().into_owned();
        let (panel, cx) =
            cx.add_window_view(|window, cx| CodePanel::new(&view_path, window, cx));

        let editor = panel.update(cx, |panel, _| panel.editor.clone());
        editor.update_in(cx, |editor, window, cx| {
            editor.insert("// temporary\n", window, cx);
        });
        assert!(panel.update(cx, |panel, _| panel.dirty));

        let focus = editor.update(cx, |editor, cx| editor.focus_handle(cx));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.focus(&focus, cx);
            focus.dispatch_action(&Undo, window, cx);
        });
        assert_eq!(
            editor.update(cx, |editor, _| editor.value().to_string()),
            "fn main() {}\n"
        );
        assert!(!panel.update(cx, |panel, _| panel.dirty));
        std::fs::remove_file(path).expect("remove editor source");
    }

    #[gpui_kit::test]
    fn presenting_a_quote_preserves_the_current_selection(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let path = std::env::temp_dir()
            .join(format!("ahead-presentation-{}.rs", uuid::Uuid::new_v4()));
        std::fs::write(&path, "first line\nlet target = true;\nlast line")
            .expect("write temporary editor source");
        let view_path = path.to_string_lossy().into_owned();
        let (panel, cx) =
            cx.add_window_view(|window, cx| CodePanel::new(&view_path, window, cx));

        panel.update(cx, |panel, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(0..5, cx);
            });
            let selection_before = panel.editor.read(cx).selected_range();
            panel
                .present_quote(
                    "session".to_string(),
                    "cue".to_string(),
                    "src/main.rs".to_string(),
                    "let target = true;",
                    "Target assignment".to_string(),
                    "This is the value being explained.".to_string(),
                    cx,
                )
                .expect("present exact quote");

            assert_eq!(panel.editor.read(cx).selected_range(), selection_before);
            let cue = &panel.presentation.as_ref().expect("active cue").cue;
            assert_eq!(cue.anchor.range.start.line, 1);
            assert_eq!(cue.pointer_target, Some(cue.anchor.range.start));
            assert_eq!(
                presentation_gutter_line(panel.presentation.as_ref()),
                Some(2)
            );
            assert_eq!(cue.label, "Target assignment");
            assert_eq!(
                cue.note.as_deref(),
                Some("This is the value being explained.")
            );
            assert!(!panel.clear_presentation_for_session(
                "different-session",
                None,
                cx,
            ));
            assert!(
                panel
                    .move_pointer("different-session", "cue", "last line", cx)
                    .is_err()
            );
            assert!(
                panel
                    .move_pointer("session", "cue", "missing quote", cx)
                    .is_err()
            );
            assert!(panel.move_pointer("session", "cue", "line", cx).is_err());
            panel
                .move_pointer("session", "cue", "last line", cx)
                .expect("move the separate agent pointer");
            let cue = &panel.presentation.as_ref().expect("active cue").cue;
            assert_eq!(cue.anchor.range.start.line, 1);
            assert_eq!(cue.pointer_target.map(|target| target.line), Some(2));
            assert_eq!(
                presentation_gutter_line(panel.presentation.as_ref()),
                Some(3)
            );
            assert_eq!(cue.label, "Target assignment");
            assert_eq!(
                cue.note.as_deref(),
                Some("This is the value being explained.")
            );
            assert_eq!(panel.editor.read(cx).selected_range(), selection_before);

            assert!(
                panel
                    .present_quote(
                        "session".to_string(),
                        "stale-cue".to_string(),
                        "src/main.rs".to_string(),
                        "no longer in the buffer",
                        "Stale quote".to_string(),
                        "This must not replace the active cue.".to_string(),
                        cx,
                    )
                    .is_err()
            );
            assert_eq!(
                panel
                    .presentation
                    .as_ref()
                    .expect("verified cue remains active")
                    .cue
                    .cue_id,
                "cue"
            );

            panel
                .present_quote(
                    "session".to_string(),
                    "cue-2".to_string(),
                    "src/main.rs".to_string(),
                    "last line",
                    "Return value".to_string(),
                    "This is the returned value.".to_string(),
                    cx,
                )
                .expect("move presentation to another exact quote");
            let cue = &panel.presentation.as_ref().expect("moved cue").cue;
            assert_eq!(cue.cue_id, "cue-2");
            assert_eq!(cue.anchor.range.start.line, 2);
            assert_eq!(cue.pointer_target, Some(cue.anchor.range.start));
            assert_eq!(cue.label, "Return value");
            assert_eq!(panel.editor.read(cx).selected_range(), selection_before);
        });
        cx.update(|window, cx| {
            for _ in 0..3 {
                window.draw(cx).clear(cx);
            }
        });
        assert!(
            panel
                .update(cx, |panel, _| panel.pointer_overlay_position)
                .is_some()
        );
        std::fs::remove_file(path).expect("remove temporary editor source");
    }

    #[gpui_kit::test]
    fn presentation_note_tracks_its_quote_when_text_before_it_changes(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let path = std::env::temp_dir().join(format!(
            "ahead-presentation-edit-{}.rs",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "first line\nlet target = true;\nlast line")
            .expect("write temporary editor source");
        let view_path = path.to_string_lossy().into_owned();
        let (panel, cx) =
            cx.add_window_view(|window, cx| CodePanel::new(&view_path, window, cx));

        panel.update(cx, |panel, cx| {
            panel
                .present_quote(
                    "session".to_string(),
                    "cue".to_string(),
                    "src/main.rs".to_string(),
                    "let target = true;",
                    "Target assignment".to_string(),
                    "This is the value being explained.".to_string(),
                    cx,
                )
                .expect("present exact quote");
        });
        let editor = panel.update(cx, |panel, _| panel.editor.clone());
        editor.update_in(cx, |editor, window, cx| {
            editor.set_selected_range(0..0, cx);
            editor.insert("learner note\n", window, cx);
        });

        let (text, range, cue) = panel.update(cx, |panel, cx| {
            let range = panel
                .presentation_decorations
                .get_ranges(cx)
                .first()
                .cloned()
                .expect("presentation range remains active");
            let cue = panel
                .presentation
                .as_ref()
                .expect("presentation cue remains active")
                .cue
                .clone();
            (panel.editor.read(cx).text().to_string(), range, cue)
        });
        assert_eq!(text.get(range), Some("let target = true;"));
        assert_eq!(cue.anchor.range.start.line, 2);
        assert_eq!(cue.label, "Target assignment");
        assert_eq!(
            cue.note.as_deref(),
            Some("This is the value being explained.")
        );

        cx.update(|window, cx| window.draw(cx).clear(cx));
        std::fs::remove_file(path).expect("remove temporary editor source");
    }

    #[test]
    fn external_character_columns_convert_to_utf8_byte_offsets() {
        let text = "α🙂target\n";
        assert_eq!(
            location_byte_position(text, 0, OpenColumn::Character(2)),
            (6, 6)
        );
        assert_eq!(
            location_byte_position(text, 0, OpenColumn::Character(99)),
            (12, 12)
        );
    }

    #[gpui_kit::test]
    fn selecting_search_match_highlights_multiline_unicode_range(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let path = std::env::temp_dir().join(format!(
            "ahead-search-selection-{}.rs",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "before\n  αtarget\nend")
            .expect("write temporary editor source");
        let view_path = path.to_string_lossy().into_owned();
        let (panel, cx) =
            cx.add_window_view(|window, cx| CodePanel::new(&view_path, window, cx));

        panel.update_in(cx, |panel, window, cx| {
            panel.reveal_location(
                OpenLocation {
                    line: 1,
                    column: OpenColumn::Utf8Byte(2),
                    end_line: 2,
                    end_column: OpenColumn::Utf8Byte(3),
                },
                window,
                cx,
            );
        });
        let (selection, cursor) = panel.update(cx, |panel, cx| {
            (
                panel.editor.read(cx).selected_range(),
                panel.editor.read(cx).cursor_position(),
            )
        });
        assert_eq!(selection, 9..21);
        assert_eq!(cursor, gpui_kit::base::input::Position::new(2, 3));

        cx.update(|window, cx| window.draw(cx).clear(cx));
        std::fs::remove_file(path).expect("remove temporary editor source");
    }

    #[gpui_kit::test]
    fn selected_turn_context_uses_utf16_columns(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::component::init);
        let path = std::env::temp_dir()
            .join(format!("ahead-chat-selection-{}.rs", uuid::Uuid::new_v4()));
        std::fs::write(&path, "a🦀b\n").expect("write temporary editor source");
        let view_path = path.to_string_lossy().into_owned();
        let (panel, cx) =
            cx.add_window_view(|window, cx| CodePanel::new(&view_path, window, cx));
        let selection = panel.update(cx, |panel, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(1..5, cx);
            });
            panel.turn_context(cx).expect("text context").selection.expect("editor selection")
        });
        assert_eq!(selection.start.col, 1);
        assert_eq!(selection.end.col, 3);
        std::fs::remove_file(path).expect("remove temporary editor source");
    }

    #[test]
    fn search_match_byte_offset_clamps_to_line_and_document() {
        let text = "before\n  αtarget\nend";
        assert_eq!(
            location_byte_position(text, 1, OpenColumn::Utf8Byte(2)).0,
            9
        );
        assert_eq!(
            location_byte_position(text, 2, OpenColumn::Utf8Byte(3)).0,
            21
        );
        assert_eq!(
            location_byte_position(text, 99, OpenColumn::Utf8Byte(99)).0,
            text.len()
        );
        assert_eq!(
            location_byte_position("αb", 0, OpenColumn::Utf8Byte(1)).0,
            0
        );
    }

    #[test]
    fn recognizes_markdown_extensions() {
        assert!(is_markdown_path("README.md"));
        assert!(is_markdown_path("docs/guide.MARKDOWN"));
        assert!(!is_markdown_path("src/main.rs"));
    }

    #[test]
    fn formats_cached_git_blame_metadata() {
        let mut hunk = ahead_rpc::source_control::BlameHunk {
            start: 3,
            len: 1,
            commit: Some(ahead_rpc::source_control::BlameCommit {
                author: "Ada Lovelace".into(),
                timestamp: 1700000000,
                subject: "Add the engine".into(),
                id: "abc1234567890123456789012345678901234567".into(),
            }),
        };
        assert_eq!(
            blame_label(&hunk),
            "Ada Lovelace · 2023-11-14 · Add the engine (abc1234)"
        );
        hunk.commit = None;
        assert_eq!(blame_label(&hunk), "Uncommitted changes");
    }

    #[gpui_kit::test(iterations = 10)]
    fn git_metadata_coalesces_requests_and_rejects_stale_or_closed_replies(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::proxy::{ProxyRequest, ProxyResponse, ProxyRpc};
        use ahead_rpc::source_control::{DiffHunk, DiffHunkKind, GitFileState};
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let path = directory.path().join("main.ts");
        std::fs::write(&path, "original\n").expect("source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        panel.update(cx, |panel, cx| {
            panel.proxy = Some(proxy.clone());
            panel.workspace = directory.path().to_string_lossy().into_owned();
            for _ in 0..5 {
                panel.refresh_git_metadata(cx);
            }
        });
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(150));
        cx.run_until_parked();
        let take_request = || {
            rpc.rx()
                .try_iter()
                .find_map(|message| {
                    if let ProxyRpc::Request(
                        id,
                        ProxyRequest::GitFileState { content, .. },
                    ) = message
                    {
                        Some((id, content))
                    } else {
                        None
                    }
                })
                .expect("one metadata request")
        };
        let (old, content) = take_request();
        assert_eq!(content, "original\n");
        assert!(rpc.rx().try_recv().is_err());
        let editor = panel.update(cx, |panel, _| panel.editor.clone());
        editor.update_in(cx, |editor, window, cx| {
            editor.replace_all("unsaved\n", window, cx);
        });
        assert!(panel.update(cx, |panel, _| panel.dirty));
        cx.run_until_parked();
        panel.update(cx, |panel, cx| panel.refresh_git_metadata(cx));
        let state = GitFileState {
            hunks: vec![DiffHunk {
                start: 1,
                len: 1,
                kind: DiffHunkKind::Modified,
                old_start: 1,
                old_lines: vec!["original".into()],
            }],
            blame: Vec::new(),
        };
        rpc.handle_response(
            old,
            Ok(ProxyResponse::GitFileState {
                state: state.clone(),
            }),
        );
        cx.run_until_parked();
        panel.update(cx, |panel, _| {
            assert!(
                panel.git_state.hunks.is_empty(),
                "edited buffer rejects old reply"
            )
        });
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(150));
        cx.run_until_parked();
        let (current, content) = take_request();
        assert_eq!(content, "unsaved\n");
        rpc.handle_response(
            current,
            Ok(ProxyResponse::GitFileState {
                state: state.clone(),
            }),
        );
        cx.run_until_parked();
        panel.update(cx, |panel, cx| {
            assert_eq!(panel.git_state, state);
            panel.active_line = 2;
            panel.refresh_git_metadata(cx);
            assert!(panel.git_task.is_none(), "caret motion uses cached blame");
        });
        assert!(rpc.rx().try_recv().is_err(), "no repaint/caret RPC");

        editor.update_in(cx, |editor, window, cx| {
            editor.replace_all("typing again\n", window, cx);
        });
        panel.update(cx, |panel, cx| {
            panel.refresh_git_metadata(cx);
            assert_eq!(
                panel.git_state, state,
                "markers stay visible while refreshing"
            );
        });

        proxy.route_core(ahead_rpc::core::CoreNotification::DiffInfo {
            diff: Default::default(),
        });
        panel.update(cx, |panel, cx| panel.refresh_git_metadata(cx));
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(150));
        cx.run_until_parked();
        let (external_change, _) = take_request();
        panel.update(cx, |panel, _| panel.release_buffer());
        rpc.handle_response(
            external_change,
            Ok(ProxyResponse::GitFileState { state }),
        );
        cx.run_until_parked();
        panel.update(cx, |panel, _| {
            assert!(panel.git_task.is_none());
            assert!(
                panel.git_state.hunks.is_empty(),
                "closed buffer rejects late Git reply"
            );
        });
    }

    #[test]
    fn agent_anchor_gutter_rows_include_the_full_range_and_ignore_other_actors() {
        let anchor =
            |actor_id: &str, start: u32, end: u32| ahead_rpc::ahead::CodeAnchor {
                id: format!("{actor_id}-{start}"),
                session_id: "session".into(),
                actor_id: actor_id.into(),
                path: "src/lib.rs".into(),
                range: ahead_rpc::ahead::DisplayRange {
                    start: ahead_rpc::ahead::DisplayPosition {
                        line: start,
                        col: 0,
                    },
                    end: ahead_rpc::ahead::DisplayPosition { line: end, col: 0 },
                },
                quote_hash: "hash".into(),
                surrounding_context: None,
            };
        let anchors = [anchor("ahead", 2, 4), anchor("human", 3, 5)];

        assert_eq!(
            agent_anchor_lines(&anchors),
            [2, 3, 4].into_iter().collect()
        );
    }

    #[gpui_kit::test]
    fn git_change_marker_does_not_toggle_breakpoint(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::source_control::{DiffHunk, DiffHunkKind};

        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let path = directory.path().join("main.py");
        std::fs::write(&path, "print(1)\n").expect("source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        panel.update(cx, |panel, cx| {
            panel.proxy = Some(proxy.clone());
            panel.git_state.hunks = vec![DiffHunk {
                start: 1,
                len: 1,
                kind: DiffHunkKind::Added,
                old_start: 0,
                old_lines: Vec::new(),
            }];
            panel.git_key = panel.current_git_key();
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let change = cx.debug_bounds("git-change-1").expect("change marker");
        let breakpoint = cx
            .debug_bounds("breakpoint-target-1")
            .expect("breakpoint target");
        assert!(change.origin.x + change.size.width <= breakpoint.origin.x);
        cx.simulate_click(change.center(), Default::default());
        assert!(proxy.breakpoints_for(&path).is_empty());
        assert_eq!(
            panel.update(cx, |panel, _| panel.expanded_hunk_start),
            Some(1)
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("expanded-git-hunk").is_some());
        cx.simulate_mouse_move(breakpoint.center(), None, Default::default());
        assert_eq!(
            panel.update(cx, |panel, _| panel.hovered_breakpoint_line),
            Some(1)
        );
        cx.simulate_mouse_move(
            gpui_kit::point(gpui_kit::px(100.), gpui_kit::px(100.)),
            None,
            Default::default(),
        );
        assert_eq!(
            panel.update(cx, |panel, _| panel.hovered_breakpoint_line),
            None
        );
        cx.simulate_click(breakpoint.center(), Default::default());
        assert_eq!(proxy.breakpoints_for(&path), [1].into_iter().collect());
        cx.simulate_click(change.center(), Default::default());
        assert!(proxy.breakpoints_for(&path).contains(&1));
        assert_eq!(panel.update(cx, |panel, _| panel.expanded_hunk_start), None);
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
                "🦀x",
                lsp_types::Position {
                    line: 0,
                    character: 2
                }
            ),
            Some(4)
        );
        assert_eq!(
            caret_byte_offset(
                "🦀x",
                lsp_types::Position {
                    line: 0,
                    character: 1
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
