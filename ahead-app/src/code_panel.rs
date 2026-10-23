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
    collections::HashSet,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent, PanelId, TabGroup,
};
use gpui_kit::component::empty::{Empty, EmptyDescription, EmptyHeader, EmptyTitle};
use gpui_kit::component::input::{
    Copy as CopyAction, Cut as CutAction, Editor, EditorState, Enter, Escape,
    IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp,
    Paste as PasteAction, Rope, RopeExt, SelectAll, TabSize, TextDecoration,
    TextDecorationCollection, Textarea, TextareaState,
};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme, Icon, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit_assets::IconName;
use gpui_util::ResultExt;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as FuzzyConfig, Matcher as FuzzyMatcher, Utf32Str};
use sha2::Digest;

use crate::app::{
    FindReferences, FormatDocument, GoToDefinition, GoToImplementation,
    NextSnippetTabstop, PreviousSnippetTabstop, RenameSymbol, SaveFile,
    ShowCodeActions, ShowCompletions, ShowSignatureHelp,
};
use crate::lsp_snippet::{
    self, Session as SnippetSession, Tabstop as SnippetTabstop,
};
use crate::proxy_client::{
    LspCodeAction, LspCompletion, ProxyClient, WorkspaceFileEvent,
};
use ahead_rpc::source_control::{BlameHunk, DiffHunk, DiffHunkKind, GitFileState};
use ahead_rpc::style::LineStyle;

const SHARED_DRAFT_PREFIX: &str = ".ahead/shared-drafts";
const GIT_METADATA_REFRESH_DEBOUNCE: Duration = Duration::from_millis(600);
const LSP_RESPONSE_TIMEOUT: Duration = Duration::from_millis(1500);
const NULL_COMPLETION_RETRY_DELAY: Duration = Duration::from_millis(200);
const EDITOR_GUTTER_WIDTH: f32 = 23.0;
const GIT_GUTTER_TRACK_WIDTH: f32 = 3.0;
const GUTTER_ACTION_WIDTH: f32 = 20.0;

#[derive(Clone, Debug, PartialEq, Eq)]
struct MergeConflict {
    range: std::ops::Range<usize>,
    ours: std::ops::Range<usize>,
    theirs: std::ops::Range<usize>,
    line: usize,
}

#[derive(Clone, Copy)]
enum MergeConflictChoice {
    Current,
    Incoming,
    Both,
}

#[derive(Default)]
struct PendingMergeConflict {
    start: usize,
    ours_start: usize,
    ours_end: Option<usize>,
    theirs_start: Option<usize>,
    line: usize,
    invalid: bool,
}

fn parse_merge_conflicts(text: &str) -> Vec<MergeConflict> {
    if !text.contains("<<<<<<< ") {
        return Vec::new();
    }
    let mut conflicts = Vec::new();
    let mut pending: Option<PendingMergeConflict> = None;
    let mut offset = 0;

    for (line_index, line_with_ending) in text.split_inclusive('\n').enumerate() {
        let line = line_with_ending
            .strip_suffix('\n')
            .unwrap_or(line_with_ending);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.starts_with("<<<<<<< ") {
            if let Some(pending) = pending.as_mut() {
                pending.invalid = true;
            } else {
                pending = Some(PendingMergeConflict {
                    start: offset,
                    ours_start: offset + line_with_ending.len(),
                    line: line_index + 1,
                    ..PendingMergeConflict::default()
                });
            }
        } else if line.starts_with("|||||||") {
            if let Some(pending) = pending.as_mut() {
                if pending.ours_end.replace(offset).is_some()
                    || pending.theirs_start.is_some()
                {
                    pending.invalid = true;
                }
            }
        } else if line == "=======" {
            if let Some(pending) = pending.as_mut() {
                if pending.theirs_start.is_some() {
                    pending.invalid = true;
                } else {
                    pending.ours_end.get_or_insert(offset);
                    pending.theirs_start = Some(offset + line_with_ending.len());
                }
            }
        } else if line.starts_with(">>>>>>> ") {
            if let Some(pending) = pending.take()
                && !pending.invalid
                && let (Some(ours_end), Some(theirs_start)) =
                    (pending.ours_end, pending.theirs_start)
                && pending.start <= pending.ours_start
                && pending.ours_start <= ours_end
                && ours_end <= theirs_start
                && theirs_start <= offset
            {
                conflicts.push(MergeConflict {
                    range: pending.start..offset + line_with_ending.len(),
                    ours: pending.ours_start..ours_end,
                    theirs: theirs_start..offset,
                    line: pending.line,
                });
            }
        }
        offset += line_with_ending.len();
    }

    conflicts
}

fn merge_conflict_choice_text(
    text: &str,
    conflict: &MergeConflict,
    choice: MergeConflictChoice,
) -> Option<String> {
    let ours = text.get(conflict.ours.clone())?;
    let theirs = text.get(conflict.theirs.clone())?;
    Some(match choice {
        MergeConflictChoice::Current => ours.to_string(),
        MergeConflictChoice::Incoming => theirs.to_string(),
        MergeConflictChoice::Both => format!("{ours}{theirs}"),
    })
}

pub(crate) fn shared_draft_path(
    session_id: &str,
    file_path: &str,
) -> Option<std::path::PathBuf> {
    uuid::Uuid::parse_str(session_id).ok()?;
    let path = std::path::Path::new(file_path);
    if file_path.is_empty()
        || file_path.contains('\\')
        || !path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(format!("{SHARED_DRAFT_PREFIX}/{session_id}/{file_path}").into())
}

pub(crate) fn shared_draft_file_path<'a>(
    path: &'a std::path::Path,
    session_id: &str,
) -> Option<&'a std::path::Path> {
    uuid::Uuid::parse_str(session_id).ok()?;
    let prefix = std::path::Path::new(SHARED_DRAFT_PREFIX).join(session_id);
    let file = path.strip_prefix(prefix).ok()?;
    (!file.as_os_str().is_empty()
        && file
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_))))
    .then_some(file)
}

pub(crate) fn is_shared_draft_path(path: &std::path::Path) -> bool {
    path.starts_with(SHARED_DRAFT_PREFIX)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompletionContext {
    generation: u64,
    request_id: u64,
    path: std::path::PathBuf,
    position: lsp_types::Position,
}

const SEMANTIC_TOKEN_DEBOUNCE: Duration = Duration::from_millis(100);
const MAX_COMPLETION_SUGGESTIONS: usize = 5;

fn semantic_token_scope(token_type: &str) -> Option<&'static str> {
    match token_type {
        "attribute" | "decorator" => Some("attribute"),
        "boolean" => Some("boolean"),
        "class" | "interface" | "lifetime" | "namespace" | "struct" | "type"
        | "typeParameter" => Some("type"),
        "comment" => Some("comment"),
        "constant" | "enumMember" | "event" => Some("constant"),
        "constructor" => Some("constructor"),
        "enum" => Some("enum"),
        "function" | "macro" | "method" => Some("function"),
        "keyword" | "modifier" => Some("keyword"),
        "label" => Some("label"),
        "number" => Some("number"),
        "operator" => Some("operator"),
        "parameter" | "variable" => Some("variable"),
        "property" => Some("property"),
        "regexp" => Some("string.regex"),
        "string" => Some("string"),
        "tag" => Some("tag"),
        "variant" => Some("variant"),
        _ => None,
    }
}

fn semantic_text_decorations(
    styles: &[LineStyle],
    text: &str,
    mut resolve_style: impl FnMut(&str) -> Option<HighlightStyle>,
) -> Vec<TextDecoration> {
    styles
        .iter()
        .filter_map(|line_style| {
            let start = line_style.start;
            let end = line_style.end;
            if start >= end
                || end > text.len()
                || !text.is_char_boundary(start)
                || !text.is_char_boundary(end)
                || text[start..end].contains('\n')
            {
                return None;
            }

            let token_type = line_style.style.fg_color.as_deref()?;
            let scope = semantic_token_scope(token_type)?;
            let style = resolve_style(scope)?;
            Some(TextDecoration::new(start..end, style))
        })
        .collect()
}

fn extension_syntax_decorations(
    captures: &[ahead_rpc::style::SyntaxCapture],
    text: &str,
    mut resolve_style: impl FnMut(&str) -> Option<HighlightStyle>,
) -> Vec<TextDecoration> {
    captures
        .iter()
        .filter_map(|capture| {
            if capture.start >= capture.end
                || capture.end > text.len()
                || !text.is_char_boundary(capture.start)
                || !text.is_char_boundary(capture.end)
            {
                return None;
            }
            let style = resolve_style(&capture.scope)?;
            Some(TextDecoration::new(capture.start..capture.end, style))
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct DiagnosticItem {
    pub line: u32,
    pub message: String,
    pub is_error: bool,
}

#[derive(Clone, Debug)]
struct NavigationResults {
    title: &'static str,
    locations: Vec<lsp_types::Location>,
}

#[derive(Clone, Debug)]
struct PendingRename {
    path: std::path::PathBuf,
    position: lsp_types::Position,
    generation: u64,
    request_id: u64,
    old_name: String,
}

#[derive(Clone, Debug)]
pub(crate) enum WorkspaceEditPurpose {
    Rename { old_name: String, new_name: String },
    CodeAction { title: String },
    ServerRequest { label: Option<String> },
}

#[derive(Clone)]
pub(crate) struct WorkspaceEditRequest {
    pub edit: lsp_types::WorkspaceEdit,
    pub source_path: std::path::PathBuf,
    pub purpose: WorkspaceEditPurpose,
    pub generation: u64,
    pub completion: Option<async_channel::Sender<Result<u64, String>>>,
}

impl WorkspaceEditRequest {
    pub(crate) fn complete(&self, result: Result<u64, String>) {
        if let Some(completion) = &self.completion
            && let Err(error) = completion.try_send(result)
        {
            eprintln!("Workspace edit result was not observed: {error:?}");
        }
    }
}

#[derive(Clone)]
pub(crate) struct WorkspaceBufferSnapshot {
    pub path: std::path::PathBuf,
    pub text: String,
    pub selection: std::ops::Range<usize>,
    pub editable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeTabAction {
    CommentOnSelection,
    Promote,
    MoveToOtherEditorGroup,
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

#[derive(Clone)]
pub struct SelectedCodeComment {
    pub path: String,
    pub range: ahead_rpc::ahead::DisplayRange,
    pub quote: String,
    pub source_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GitStateKey {
    path: std::path::PathBuf,
    revision: usize,
    repository_generation: u64,
}

struct GitDiffPreview {
    hunk: DiffHunk,
    source_text: String,
    editor: Entity<EditorState>,
    inserted_rows: usize,
}

#[derive(Default)]
struct GutterDiffMarkers {
    hunk_geometry: Vec<(u32, u32, DiffHunkKind)>,
    line_count: usize,
    hunk_lines: std::collections::HashMap<u32, (u32, DiffHunkKind)>,
    deleted_lines: std::collections::HashSet<u32>,
}

impl GutterDiffMarkers {
    fn update(&mut self, hunks: &[DiffHunk], line_count: usize) -> bool {
        let geometry = hunks
            .iter()
            .map(|hunk| (hunk.start, hunk.len, hunk.kind))
            .collect::<Vec<_>>();
        if self.line_count == line_count && self.hunk_geometry == geometry {
            return false;
        }

        self.hunk_geometry = geometry;
        self.line_count = line_count;
        self.rebuild_lines();
        true
    }

    fn update_line_count(&mut self, line_count: usize) -> bool {
        if self.line_count == line_count {
            return false;
        }
        self.line_count = line_count;
        self.rebuild_lines();
        true
    }

    fn rebuild_lines(&mut self) {
        self.hunk_lines = self
            .hunk_geometry
            .iter()
            .filter(|(_, _, kind)| !matches!(kind, DiffHunkKind::Deleted))
            .flat_map(|(start, len, kind)| {
                let end = start.saturating_add(*len);
                (*start..end).map(|line| (line, (*start, *kind)))
            })
            .collect();
        self.deleted_lines = self
            .hunk_geometry
            .iter()
            .filter(|(_, _, kind)| matches!(kind, DiffHunkKind::Deleted))
            .map(|(start, _, _)| {
                (*start).min(u32::try_from(self.line_count).unwrap_or(u32::MAX))
            })
            .collect();
    }
}

struct GitHunkProjection {
    text: String,
    deleted_ranges: Vec<std::ops::Range<usize>>,
    added_ranges: Vec<std::ops::Range<usize>>,
    inserted_rows: usize,
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
    let decoded = ahead_core::encoding::decode_text(&bytes)
        .ok_or(FileOpenError::UnsupportedText)?;
    if decoded.text.contains('\0')
        || (decoded.encoding == ahead_core::encoding::TextEncoding::Utf8
            && ahead_core::search::is_binary_content(&bytes))
    {
        return Err(FileOpenError::UnsupportedText);
    }
    Ok(decoded.text)
}

fn renamed_path_for_open_file(
    file_path: &std::path::Path,
    old_path: &std::path::Path,
    new_path: &std::path::Path,
) -> Option<std::path::PathBuf> {
    if file_path == old_path {
        return Some(new_path.to_path_buf());
    }
    file_path
        .strip_prefix(old_path)
        .ok()
        .map(|relative| new_path.join(relative))
}

fn read_renamed_workspace_file(
    workspace: &str,
    new_path: &std::path::Path,
) -> Result<(std::path::PathBuf, String), String> {
    let workspace = std::fs::canonicalize(workspace)
        .map_err(|error| format!("Could not resolve workspace root: {error}"))?;
    let new_path = std::fs::canonicalize(new_path)
        .map_err(|error| format!("Could not resolve renamed file: {error}"))?;
    if !new_path.starts_with(&workspace) {
        return Err("The renamed file is outside this workspace".into());
    }
    if !std::fs::metadata(&new_path)
        .map_err(|error| format!("Could not inspect renamed file: {error}"))?
        .is_file()
    {
        return Err("The renamed path is not a regular file".into());
    }
    let path = new_path
        .to_str()
        .ok_or_else(|| "The renamed file path is not valid UTF-8".to_string())?;
    let text = read_editor_file(path).map_err(|error| error.to_string())?;
    Ok((new_path, text))
}

fn rebase_selection(
    old_text: &str,
    new_text: &str,
    selection: std::ops::Range<usize>,
) -> std::ops::Range<usize> {
    let mut prefix = old_text
        .bytes()
        .zip(new_text.bytes())
        .take_while(|(old, new)| old == new)
        .count();
    while prefix > 0
        && (!old_text.is_char_boundary(prefix) || !new_text.is_char_boundary(prefix))
    {
        prefix -= 1;
    }
    let mut suffix = old_text
        .as_bytes()
        .iter()
        .rev()
        .zip(new_text.as_bytes().iter().rev())
        .take_while(|(old, new)| old == new)
        .count()
        .min(old_text.len().saturating_sub(prefix))
        .min(new_text.len().saturating_sub(prefix));
    while suffix > 0
        && (!old_text.is_char_boundary(old_text.len() - suffix)
            || !new_text.is_char_boundary(new_text.len() - suffix))
    {
        suffix -= 1;
    }

    let rebase = |offset: usize| {
        let offset = offset.min(old_text.len());
        let offset = if offset <= prefix {
            offset
        } else if offset >= old_text.len() - suffix {
            new_text.len() - (old_text.len() - offset)
        } else {
            prefix
        };
        let mut offset = offset.min(new_text.len());
        while !new_text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    };
    let start = rebase(selection.start);
    let end = rebase(selection.end);
    start.min(end)..start.max(end)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditorCommand {
    GoToDefinition,
    FindReferences,
    GoToImplementation,
    RenameSymbol,
    FormatDocument,
    ShowCompletions,
    ShowSignatureHelp,
    ShowCodeActions,
}

pub struct CodePanel {
    pub focus: FocusHandle,
    pub editor: Entity<EditorState>,
    comment_input: Entity<TextareaState>,
    rename_input: Entity<InputState>,
    pending_rename: Option<PendingRename>,
    rename_in_progress: bool,
    rename_request_id: u64,
    comment_session_id: Option<String>,
    comment_generation: u64,
    code_comments: Vec<ahead_rpc::ahead::CodeComment>,
    comment_popover_line: Option<u32>,
    comment_draft_target: Option<SelectedCodeComment>,
    comment_pending: bool,
    comment_error: Option<String>,
    session_panel: Option<WeakEntity<crate::session_panel::SessionPanel>>,
    presentation_decorations: TextDecorationCollection,
    extension_syntax_decorations: TextDecorationCollection,
    semantic_decorations: TextDecorationCollection,
    semantic_task: Option<Task<()>>,
    semantic_request_id: u64,
    extension_syntax_task: Option<Task<()>>,
    extension_syntax_request_id: u64,
    presentation: Option<ActivePresentation>,
    pointer_overlay_position: Option<(f32, f32)>,
    inline_blame_position: Option<(u32, f32, f32)>,
    inline_blame_enabled: bool,
    preferred_line_length: usize,
    pub file_path: String,
    file_error: Option<FileOpenError>,
    pub saved_rev: usize,
    pub dirty_rev: usize,
    saved_text: Rope,
    save_request_id: u64,
    saved_request_id: u64,
    recovery_id: String,
    recovery_revision: u64,
    recovery_key: Option<(u64, u64, bool)>,
    recovery_reply: Option<(Instant, async_channel::Receiver<Result<bool, String>>)>,
    recovery_conflict: bool,
    external_change_conflict: bool,
    external_file_missing: bool,
    saved_sha256: Option<String>,
    pub dirty: bool,
    pub status: SharedString,
    pub active_line: u32,
    pub completions: Vec<LspCompletion>,
    completion_context: Option<CompletionContext>,
    completion_details_requested: HashSet<usize>,
    snippet_session: Option<SnippetSession>,
    extension_comment_variables: Option<lsp_snippet::CommentVariables>,
    pub selected_completion: usize,
    pub show_completions: bool,
    completion_caret_top: Option<Pixels>,
    completion_caret_left: Option<Pixels>,
    completion_available_width: Option<Pixels>,
    pub ghost_text: Option<String>,
    ghost_context: Option<crate::proxy_client::InlinePredictionContext>,
    /// Byte offset in the editor text where the current ghost was
    /// requested. Lets edits narrow the ghost (Zed-style interpolation)
    /// instead of always dismissing it.
    pub ghost_offset: Option<usize>,
    pub diagnostics: Vec<DiagnosticItem>,
    pub proxy: Option<Arc<ProxyClient>>,
    diagnostic_task: Option<Task<()>>,
    prediction_context_task: Option<Task<()>>,
    inline_prediction_task: Option<Task<()>>,
    ghost_accepting: bool,
    pending_inline: Option<(
        std::ops::Range<usize>,
        crate::proxy_client::InlinePredictionContext,
    )>,
    workspace_file_task: Option<Task<()>>,
    open_file_task: Option<Task<()>>,
    pub(crate) shared_session_id: Option<String>,
    shared_revision: Option<u64>,
    shared_committed_content: Option<String>,
    shared_editable: bool,
    shared_edit_task: Option<Task<()>>,
    shared_poll_task: Option<Task<()>>,
    shared_update_task: Option<Task<()>>,
    shared_conflict: Option<ahead_rpc::ahead::SharedBufferSnapshot>,
    shared_conflict_session_id: Option<String>,
    shared_conflict_resolving: bool,
    host_shared_recovery_pending: bool,
    pub workspace: String,
    pub hover_text: Option<String>,
    signature_help: Option<lsp_types::SignatureHelp>,
    signature_help_position: Option<lsp_types::Position>,
    signature_request_id: u64,
    code_actions: Vec<LspCodeAction>,
    code_actions_position: Option<lsp_types::Position>,
    code_action_request_id: u64,
    code_actions_visible: bool,
    selected_code_action: usize,
    navigation_results: Option<NavigationResults>,
    hover_chord_deadline: Option<Instant>,
    pub is_preview: bool,
    show_markdown_preview: bool,
    git_state: GitFileState,
    git_gutter_markers: GutterDiffMarkers,
    merge_conflicts: Vec<MergeConflict>,
    git_key: Option<GitStateKey>,
    git_task_key: Option<GitStateKey>,
    git_task: Option<Task<()>>,
    hovered_breakpoint_line: Option<u32>,
    expanded_hunk_start: Option<u32>,
    git_diff_preview: Option<GitDiffPreview>,
    expand_first_git_hunk_after_refresh: bool,
    /// Invalidates asynchronous completion/FIM results after edits or file switches.
    pub request_generation: u64,
    completion_request_id: u64,
    format_request_id: u64,
    suppress_completion_for_next_edit: bool,
    preserve_code_action_request_for_next_edit: bool,
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
        let comment_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Comment on this code…")
                .auto_grow(2, 5)
        });
        let rename_input = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe(
            &rename_input,
            |this: &mut Self, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    if this.rename_in_progress {
                        this.rename_in_progress = false;
                        this.rename_request_id =
                            this.rename_request_id.wrapping_add(1);
                        if let Some(pending) = this.pending_rename.as_mut() {
                            pending.request_id = this.rename_request_id;
                        }
                        this.status = "".into();
                    }
                    cx.notify();
                }
            },
        )
        .detach();
        cx.subscribe(&comment_input, |_: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();

        let editor_for_observer = editor.clone();
        cx.observe(&editor_for_observer, |this, _, cx| {
            let cursor_position = this.cursor_position(cx);
            if this.pending_inline.as_ref().is_some_and(|(selection, _)| {
                selection != &this.editor.read(cx).selected_range()
            }) {
                this.cancel_pending_inline();
            }
            if this.ghost_offset.is_some_and(|offset| {
                !this.editor.read(cx).selected_range().is_empty()
                    || caret_byte_offset(
                        &this.editor.read(cx).value(),
                        cursor_position,
                    ) != Some(offset)
            }) {
                this.ghost_text = None;
                this.ghost_offset = None;
                this.ghost_context = None;
            }
            if this
                .signature_help_position
                .is_some_and(|position| cursor_position != position)
            {
                let text = this.editor.read(cx).value().to_string();
                if has_unclosed_parenthesis_before_cursor(&text, cursor_position) {
                    this.signature_help_position = Some(cursor_position);
                } else {
                    this.signature_help = None;
                    this.signature_help_position = None;
                }
            }
            if this
                .code_actions_position
                .is_some_and(|position| this.cursor_position(cx) != position)
            {
                this.code_actions.clear();
                this.code_actions_position = None;
                this.code_actions_visible = false;
            }
            cx.notify();
        })
        .detach();

        cx.subscribe(
            &editor,
            |this: &mut Self, _state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) && this.is_text_available() {
                    this.pending_rename = None;
                    this.rename_in_progress = false;
                    this.rename_request_id = this.rename_request_id.wrapping_add(1);
                    let suppress_completion =
                        std::mem::take(&mut this.suppress_completion_for_next_edit);
                    let preserve_code_action_request = std::mem::take(
                        &mut this.preserve_code_action_request_for_next_edit,
                    );
                    if !suppress_completion
                        && let Some(mut snippet) = this.snippet_session.take()
                    {
                        let (text, selection) = {
                            let editor = this.editor.read(cx);
                            (editor.value().to_string(), editor.selected_range())
                        };
                        if snippet.update_after_edit(text, selection) {
                            this.snippet_session = Some(snippet);
                        }
                    }
                    if !this.loading {
                        this.interrupt_speech();
                        this.refresh_presentation_after_edit(cx);
                        if let Some(session_panel) = &this.session_panel {
                            session_panel
                                .update(cx, |panel, cx| {
                                    panel.stop_following_on_edit(cx);
                                })
                                .log_err();
                        }
                    }
                    this.dirty_rev += 1;
                    this.merge_conflicts = parse_merge_conflicts(
                        &this.editor.read(cx).value().to_string(),
                    );
                    this.update_git_gutter_line_count(cx);
                    if !this.loading {
                        this.refresh_git_metadata(cx);
                    }
                    // ponytail: same-length edits scan the rope; use edit-history dirtiness if large-file typing slows.
                    this.dirty = this.editor.read(cx).text() != &this.saved_text;
                    if !this.loading {
                        this.is_preview = false;
                    }
                    this.invalidate_editor_requests();
                    this.show_completions = false;
                    this.signature_request_id =
                        this.signature_request_id.wrapping_add(1);
                    if !preserve_code_action_request {
                        this.code_action_request_id =
                            this.code_action_request_id.wrapping_add(1);
                    }
                    this.code_actions.clear();
                    this.code_actions_position = None;
                    this.code_actions_visible = false;
                    if !this.loading {
                        if this.shared_session_id.is_some() {
                            this.start_shared_edit_sync(cx);
                        }
                        if let Some(proxy) = this.proxy.as_ref()
                            && this.shared_conflict.is_none()
                            && this.shared_session_id.is_none()
                        {
                            let text = this.editor.read(cx).value().to_string();
                            proxy
                                .update_editor_text(this.proxy_path(), text.clone());
                            let position = this.cursor_position(cx);
                            let should_request_signature_help =
                                has_unclosed_parenthesis_before_cursor(
                                    &text, position,
                                );
                            let should_complete = caret_byte_offset(&text, position)
                                .and_then(|offset| {
                                    text[..offset].chars().next_back()
                                })
                                .is_some_and(|character| {
                                    character.is_alphanumeric()
                                        || matches!(character, '_' | ':' | '.')
                                });
                            let generation = this.request_generation;
                            if should_complete && !suppress_completion {
                                cx.spawn(async move |this, cx| {
                                    cx.background_executor()
                                        .timer(Duration::from_millis(180))
                                        .await;
                                    let _ = this.update(cx, |this, cx| {
                                        if this.request_generation == generation {
                                            this.request_proxy_completions(
                                                cx, false,
                                            );
                                        }
                                    });
                                })
                                .detach();
                            }
                            if should_request_signature_help {
                                cx.spawn(async move |this, cx| {
                                    cx.background_executor()
                                        .timer(Duration::from_millis(180))
                                        .await;
                                    let _ = this.update(cx, |this, cx| {
                                        if this.request_generation != generation {
                                            return;
                                        }
                                        let text =
                                            this.editor.read(cx).value().to_string();
                                        let position = this.cursor_position(cx);
                                        if has_unclosed_parenthesis_before_cursor(
                                            &text, position,
                                        ) {
                                            this.request_signature_help_after_content_change(cx);
                                        }
                                    });
                                })
                                .detach();
                            }
                        }
                        this.schedule_semantic_tokens(cx);
                        this.schedule_extension_syntax(cx);
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
                            this.ghost_offset = caret;
                        } else {
                            this.ghost_context = None;
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
        let extension_syntax_decorations = editor.update(cx, |state, cx| {
            state.create_decorations_collection(Vec::new(), cx)
        });
        let semantic_decorations = editor.update(cx, |state, cx| {
            state.create_decorations_collection(Vec::new(), cx)
        });
        let saved_text = editor.read(cx).text().clone();
        let merge_conflicts =
            parse_merge_conflicts(&editor.read(cx).value().to_string());

        Self {
            focus: cx.focus_handle(),
            editor,
            comment_input,
            rename_input,
            pending_rename: None,
            rename_in_progress: false,
            rename_request_id: 0,
            comment_session_id: None,
            comment_generation: 0,
            code_comments: Vec::new(),
            comment_popover_line: None,
            comment_draft_target: None,
            comment_pending: false,
            comment_error: None,
            session_panel: None,
            presentation_decorations,
            extension_syntax_decorations,
            semantic_decorations,
            semantic_task: None,
            semantic_request_id: 0,
            extension_syntax_task: None,
            extension_syntax_request_id: 0,
            presentation: None,
            pointer_overlay_position: None,
            inline_blame_position: None,
            inline_blame_enabled: true,
            preferred_line_length: 80,
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
            external_change_conflict: false,
            external_file_missing: false,
            saved_sha256,
            dirty: false,
            status: "".into(),
            active_line: 1,
            completions: Vec::new(),
            completion_context: None,
            completion_details_requested: HashSet::new(),
            snippet_session: None,
            extension_comment_variables: None,
            selected_completion: 0,
            show_completions: false,
            completion_caret_top: None,
            completion_caret_left: None,
            completion_available_width: None,
            ghost_text: None,
            ghost_context: None,
            ghost_offset: None,
            diagnostics: Vec::new(),
            proxy: None,
            diagnostic_task: None,
            prediction_context_task: None,
            inline_prediction_task: None,
            ghost_accepting: false,
            pending_inline: None,
            workspace_file_task: None,
            open_file_task: None,
            shared_session_id: None,
            shared_revision: None,
            shared_committed_content: None,
            shared_editable: false,
            shared_edit_task: None,
            shared_poll_task: None,
            shared_update_task: None,
            shared_conflict: None,
            shared_conflict_session_id: None,
            shared_conflict_resolving: false,
            host_shared_recovery_pending: false,
            workspace: String::new(),
            hover_text: None,
            signature_help: None,
            signature_help_position: None,
            signature_request_id: 0,
            code_actions: Vec::new(),
            code_actions_position: None,
            code_action_request_id: 0,
            code_actions_visible: false,
            selected_code_action: 0,
            navigation_results: None,
            hover_chord_deadline: None,
            is_preview: false,
            show_markdown_preview: is_markdown_path(path),
            git_state: GitFileState::default(),
            git_gutter_markers: GutterDiffMarkers::default(),
            merge_conflicts,
            git_key: None,
            git_task_key: None,
            git_task: None,
            hovered_breakpoint_line: None,
            expanded_hunk_start: None,
            git_diff_preview: None,
            expand_first_git_hunk_after_refresh: false,
            request_generation: 0,
            completion_request_id: 0,
            format_request_id: 0,
            suppress_completion_for_next_edit: false,
            preserve_code_action_request_for_next_edit: false,
            loading: false,
            close_handler: None,
            tab_handler: None,
            speech_interruption_handler: None,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
        }
    }

    pub(crate) fn new_shared(
        session_id: String,
        snapshot: ahead_rpc::ahead::SharedBufferSnapshot,
        editable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self::new("", window, cx);
        panel.file_path = snapshot.path.clone();
        panel.shared_session_id = Some(session_id);
        panel.shared_revision = Some(snapshot.revision);
        panel.shared_committed_content = Some(snapshot.content.clone());
        panel.shared_editable = editable;
        panel.loading = true;
        panel.editor.update(cx, |editor, cx| {
            editor.set_highlighter(editor_language(&snapshot.path), cx);
            editor.set_value(snapshot.content.clone(), window, cx);
            editor.set_disabled(!editable, cx);
        });
        panel.loading = false;
        panel.saved_text = panel.editor.read(cx).text().clone();
        panel.saved_sha256 = Some(format!(
            "{:x}",
            sha2::Sha256::digest(snapshot.content.as_bytes())
        ));
        panel.dirty = false;
        panel.status = if editable {
            "Shared file · live editing"
        } else {
            "Shared file · read only"
        }
        .into();
        panel
    }

    pub(crate) fn restore_shared_draft(
        &mut self,
        snapshot: ahead_rpc::file::EditorRecoverySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let session_id = self
            .shared_session_id
            .as_deref()
            .ok_or("Not a shared file")?;
        let draft_path = shared_draft_file_path(&snapshot.path, session_id)
            .ok_or("Invalid shared draft path")?;
        if draft_path.to_str() != Some(self.file_path.as_str()) {
            return Err("Shared draft belongs to a different file".into());
        }
        if self.dirty {
            return Err(
                "Local edits are already open; the draft remains recoverable".into(),
            );
        }
        let content = snapshot.contents.ok_or("Shared draft was cleared")?;
        let host_content = self
            .shared_committed_content
            .clone()
            .ok_or("Host file is unavailable")?;
        let host_snapshot = ahead_rpc::ahead::SharedBufferSnapshot {
            path: self.file_path.clone(),
            revision: self.shared_revision.ok_or("Host revision is unavailable")?,
            content: host_content,
        };
        self.recovery_id = snapshot.buffer_id;
        self.recovery_revision = snapshot.revision;
        self.recovery_key = None;
        self.recovery_reply = None;
        self.loading = true;
        self.editor.update(cx, |editor, cx| {
            editor.set_value(content, window, cx);
        });
        self.loading = false;
        self.dirty = self.editor.read(cx).text() != &self.saved_text;
        if self.dirty {
            self.shared_conflict = Some(host_snapshot);
            self.shared_conflict_session_id = Some(session_id.to_string());
            self.status = "Recovered shared draft · choose a version".into();
        } else {
            self.status = "Shared draft already matches host".into();
        }
        cx.notify();
        Ok(())
    }

    pub(crate) fn start_shared_poll(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shared_poll_task.is_some() {
            return;
        }
        let (Some(proxy), Some(session_id)) =
            (self.proxy.clone(), self.shared_session_id.clone())
        else {
            return;
        };
        let path = self.file_path.clone();
        self.shared_poll_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(750))
                    .await;
                let session_id_for_read = session_id.clone();
                let path_for_read = path.clone();
                let proxy = proxy.clone();
                let result = cx
                    .background_spawn(async move {
                        proxy.read_shared_buffer(session_id_for_read, path_for_read)
                    })
                    .await;
                let active = this.update_in(cx, |panel, window, cx| {
                    if panel.shared_session_id.as_deref()
                        != Some(session_id.as_str())
                        || panel.file_path != path
                    {
                        return false;
                    }
                    match result {
                        Ok(snapshot) => {
                            panel.apply_shared_snapshot(snapshot, window, cx)
                        }
                        Err(error) => {
                            panel.status = format!(
                                "Shared file unavailable: {}",
                                error.message
                            )
                            .into();
                            cx.notify();
                        }
                    }
                    true
                });
                if !matches!(active, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn start_shared_edit_sync(&mut self, cx: &mut Context<Self>) {
        if !self.shared_editable
            || self.shared_edit_task.is_some()
            || self.shared_conflict.is_some()
        {
            return;
        }
        let (Some(proxy), Some(session_id)) =
            (self.proxy.clone(), self.shared_session_id.clone())
        else {
            return;
        };
        let path = self.file_path.clone();
        self.shared_edit_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let pending = this
                    .update(cx, |panel, cx| {
                        if !panel.shared_editable
                            || panel.shared_session_id.as_deref()
                                != Some(session_id.as_str())
                            || panel.file_path != path
                            || panel.shared_conflict.is_some()
                        {
                            panel.shared_edit_task = None;
                            return None;
                        }
                        let content = panel.editor.read(cx).value().to_string();
                        if panel.shared_committed_content.as_deref()
                            == Some(content.as_str())
                        {
                            panel.shared_edit_task = None;
                            return None;
                        }
                        panel.shared_revision.map(|revision| (revision, content))
                    })
                    .ok()
                    .flatten();
                let Some((revision, content)) = pending else {
                    break;
                };
                let submitted_content = content.clone();
                let proxy = proxy.clone();
                let session = session_id.clone();
                let file = path.clone();
                let result = cx
                    .background_spawn(async move {
                        proxy.replace_shared_buffer(session, file, revision, content)
                    })
                    .await;
                let again = this
                    .update(cx, |panel, cx| {
                        if panel.shared_session_id.as_deref()
                            != Some(session_id.as_str())
                            || panel.file_path != path
                        {
                            panel.shared_edit_task = None;
                            return false;
                        }
                        match result {
                            Ok(edit) if edit.applied => {
                                if panel.shared_conflict.as_ref().is_some_and(
                                    |conflict| {
                                        conflict.revision <= edit.snapshot.revision
                                            && conflict.content
                                                == edit.snapshot.content
                                    },
                                ) {
                                    panel.shared_conflict = None;
                                    panel.shared_conflict_session_id = None;
                                }
                                panel.shared_revision = Some(edit.snapshot.revision);
                                panel.shared_committed_content =
                                    Some(edit.snapshot.content.clone());
                                panel.saved_text = Rope::from(edit.snapshot.content);
                                panel.dirty = panel.editor.read(cx).text()
                                    != &panel.saved_text;
                                panel.recovery_key = None;
                                if panel.shared_conflict.is_some() {
                                    panel.shared_edit_task = None;
                                    cx.notify();
                                    return false;
                                }
                                panel.status = if panel.dirty {
                                    "Syncing shared edits…"
                                } else {
                                    "Shared edits synced"
                                }
                                .into();
                                if !panel.dirty {
                                    panel.shared_edit_task = None;
                                }
                                cx.notify();
                                panel.dirty
                                    && panel.editor.read(cx).value()
                                        != submitted_content
                            }
                            Ok(edit) => {
                                panel.shared_conflict = Some(edit.snapshot);
                                panel.shared_conflict_session_id =
                                    Some(session_id.clone());
                                panel.status =
                                    "Shared edit conflict · choose a version".into();
                                panel.shared_edit_task = None;
                                cx.notify();
                                false
                            }
                            Err(error) => {
                                panel.status =
                                    format!("Shared edit failed: {}", error.message)
                                        .into();
                                panel.shared_edit_task = None;
                                cx.notify();
                                false
                            }
                        }
                    })
                    .unwrap_or(false);
                if !again {
                    break;
                }
            }
        }));
    }

    pub(crate) fn stop_shared_poll(&mut self, cx: &mut Context<Self>) {
        self.shared_poll_task = None;
        self.shared_edit_task = None;
        self.shared_editable = false;
        self.dirty = self.editor.read(cx).text() != &self.saved_text;
        self.editor
            .update(cx, |editor, cx| editor.set_disabled(true, cx));
        if self.shared_session_id.is_some() {
            self.status = "Shared session left · read-only snapshot".into();
            cx.notify();
        }
    }

    pub(crate) fn set_shared_editable(
        &mut self,
        editable: bool,
        cx: &mut Context<Self>,
    ) {
        if self.shared_session_id.is_none() || self.shared_editable == editable {
            return;
        }
        self.dirty = self.editor.read(cx).text() != &self.saved_text;
        self.shared_editable = editable;
        if !editable {
            self.shared_edit_task = None;
            self.status = "Shared file · read only; local changes retained".into();
        } else if self.status.as_ref().starts_with("Shared file · read only") {
            self.status = "Shared file · live editing".into();
        }
        self.editor
            .update(cx, |editor, cx| editor.set_disabled(!editable, cx));
        if editable {
            self.start_shared_edit_sync(cx);
        }
        cx.notify();
    }

    pub(crate) fn current_line(&self, cx: &App) -> u32 {
        self.cursor_position(cx).line
    }

    pub(crate) fn apply_shared_snapshot(
        &mut self,
        snapshot: ahead_rpc::ahead::SharedBufferSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shared_revision == Some(snapshot.revision) {
            if self.shared_editable && self.dirty {
                self.start_shared_edit_sync(cx);
            }
            return;
        }
        let current = self.editor.read(cx).value().to_string();
        if self.shared_session_id.is_some()
            && self.shared_committed_content.as_deref() != Some(current.as_str())
            && current != snapshot.content
        {
            self.dirty = self.editor.read(cx).text() != &self.saved_text;
            if self.shared_conflict.as_ref() != Some(&snapshot) {
                self.shared_conflict = Some(snapshot);
                self.shared_conflict_session_id = self.shared_session_id.clone();
                self.status = "Shared edit conflict · choose a version".into();
                cx.notify();
            }
            return;
        }
        self.shared_revision = Some(snapshot.revision);
        self.shared_committed_content = Some(snapshot.content.clone());
        self.loading = true;
        if current != snapshot.content {
            self.editor.update(cx, |editor, cx| {
                editor.set_value(snapshot.content.clone(), window, cx);
            });
        }
        self.loading = false;
        self.saved_text = self.editor.read(cx).text().clone();
        self.saved_sha256 = Some(format!(
            "{:x}",
            sha2::Sha256::digest(snapshot.content.as_bytes())
        ));
        self.dirty = false;
        self.shared_conflict = None;
        self.shared_conflict_session_id = None;
        self.status = if self.shared_editable {
            "Shared file · live editing"
        } else {
            "Shared file · read only"
        }
        .into();
        cx.notify();
    }

    fn apply_cached_host_shared_change(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shared_session_id.is_some()
            || !self.is_text_available()
            || self.file_path.is_empty()
        {
            return;
        }
        let Some(proxy) = self.proxy.as_ref() else {
            return;
        };
        let Some((session_id, previous, snapshot)) =
            proxy.shared_buffer_change_for_path(&self.workspace, &self.file_path)
        else {
            return;
        };
        self.host_shared_recovery_pending = true;
        let local = self.editor.read(cx).value().to_string();
        if local == snapshot.content || local == previous {
            if local != snapshot.content {
                self.loading = true;
                self.editor.update(cx, |editor, cx| {
                    editor.set_value(snapshot.content.clone(), window, cx);
                });
                self.loading = false;
            }
            self.dirty = self.editor.read(cx).text() != &self.saved_text;
            self.shared_conflict = None;
            self.shared_conflict_session_id = None;
            proxy.acknowledge_shared_buffer(&session_id, &snapshot);
            self.status = "Applied collaborator edit".into();
        } else {
            self.shared_conflict = Some(snapshot);
            self.shared_conflict_session_id = Some(session_id);
            self.status =
                "Shared edit conflict · choose latest or keep local".into();
        }
        cx.notify();
    }

    fn resolve_shared_conflict(
        &mut self,
        keep_local: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shared_conflict_resolving {
            return;
        }
        let (Some(snapshot), Some(session_id), Some(proxy)) = (
            self.shared_conflict.clone(),
            self.shared_conflict_session_id.clone(),
            self.proxy.clone(),
        ) else {
            return;
        };
        if self.shared_session_id.is_some() {
            self.shared_edit_task = None;
            self.shared_revision = Some(snapshot.revision);
            self.shared_committed_content = Some(snapshot.content.clone());
            self.saved_text = Rope::from(snapshot.content.clone());
            self.shared_conflict = None;
            self.shared_conflict_session_id = None;
            if keep_local {
                self.dirty = self.editor.read(cx).text() != &self.saved_text;
                self.status = "Applying local version to host…".into();
                self.start_shared_edit_sync(cx);
            } else {
                self.loading = true;
                self.editor.update(cx, |editor, cx| {
                    editor.set_value(snapshot.content.clone(), window, cx);
                });
                self.loading = false;
                self.dirty = false;
                self.status = "Used latest host version".into();
            }
            cx.notify();
            return;
        }
        if !keep_local {
            self.loading = true;
            self.editor.update(cx, |editor, cx| {
                editor.set_value(snapshot.content.clone(), window, cx);
            });
            self.loading = false;
            self.dirty = self.editor.read(cx).text() != &self.saved_text;
            self.shared_conflict = None;
            self.shared_conflict_session_id = None;
            proxy.acknowledge_shared_buffer(&session_id, &snapshot);
            self.status = "Used latest collaborator edit".into();
            cx.notify();
            return;
        }
        let local = self.editor.read(cx).value().to_string();
        let submitted_local = local.clone();
        let path = snapshot.path.clone();
        let acknowledgement_session_id = session_id.clone();
        self.shared_conflict_resolving = true;
        self.status = "Applying local version…".into();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    proxy.replace_shared_buffer(
                        session_id,
                        path,
                        snapshot.revision,
                        local,
                    )
                })
                .await;
            this.update_in(cx, |panel, _, cx| {
                panel.shared_conflict_resolving = false;
                match result {
                    Ok(edit) if edit.applied => {
                        if panel.editor.read(cx).value() == submitted_local {
                            if let Some(proxy) = &panel.proxy {
                                proxy.acknowledge_shared_buffer(
                                    &acknowledgement_session_id,
                                    &edit.snapshot,
                                );
                            }
                            panel.shared_conflict = None;
                            panel.shared_conflict_session_id = None;
                            panel.status = "Kept local version".into();
                        } else {
                            panel.shared_conflict = Some(edit.snapshot);
                            panel.status =
                                "Local text changed during resolution; choose again"
                                    .into();
                        }
                    }
                    Ok(edit) => {
                        panel.shared_conflict = Some(edit.snapshot);
                        panel.status =
                            "Shared file changed again; choose a version".into();
                    }
                    Err(error) => {
                        panel.status =
                            format!("Shared conflict: {}", error.message).into()
                    }
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
        cx.notify();
    }

    pub fn with_proxy(
        mut self,
        proxy: Arc<ProxyClient>,
        workspace: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let receiver = proxy.subscribe_diagnostics();
        let metadata_proxy = proxy.clone();
        let mut git_generation = metadata_proxy.git_generation();
        let mut extension_syntax_generation =
            metadata_proxy.extension_syntax_generation();
        self.diagnostic_task = Some(cx.spawn(async move |this, cx| {
            while receiver.recv().await.is_ok() {
                let current_git_generation = metadata_proxy.git_generation();
                let git_metadata_changed = current_git_generation != git_generation;
                git_generation = current_git_generation;
                let current_extension_syntax_generation =
                    metadata_proxy.extension_syntax_generation();
                let extension_syntax_changed = current_extension_syntax_generation
                    != extension_syntax_generation;
                extension_syntax_generation = current_extension_syntax_generation;
                if this
                    .update(cx, |this, cx| {
                        this.sync_proxy_diagnostics(cx);
                        if git_metadata_changed {
                            this.refresh_git_metadata(cx);
                        }
                        if extension_syntax_changed {
                            this.schedule_extension_syntax(cx);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        self.proxy = Some(proxy.clone());
        self.workspace = workspace.to_string();
        if !self.file_path.is_empty() {
            if let Err(error) =
                self.apply_editor_language_settings(&self.file_path.clone(), cx)
            {
                self.status =
                    format!("Editor indentation settings were ignored: {error}")
                        .into();
            }
        }
        self.watch_prediction_context(&proxy, cx);
        self.watch_workspace_file_changes(&proxy, window, cx);
        self.watch_open_file_changes(&proxy, window, cx);
        self.watch_shared_buffer_changes(&proxy, window, cx);
        self.set_inline_blame_enabled(
            crate::settings_panel::inline_blame_enabled(std::path::Path::new(
                workspace,
            )),
            cx,
        );
        if self.shared_session_id.is_none()
            && !self.file_path.is_empty()
            && self.is_text_available()
            && let Some(proxy) = self.proxy.as_ref()
        {
            proxy.sync_editor_snapshot(
                self.proxy_path(),
                self.editor.read(cx).value().to_string(),
            );
            proxy.refresh_anchors(&[self.proxy_path()]);
        }
        self.schedule_semantic_tokens(cx);
        self.schedule_extension_syntax(cx);
        self.refresh_git_metadata(cx);
        self
    }

    fn watch_prediction_context(
        &mut self,
        proxy: &Arc<ProxyClient>,
        cx: &mut Context<Self>,
    ) {
        let receiver = proxy.subscribe_prediction_changes();
        self.prediction_context_task = Some(cx.spawn(async move |this, cx| {
            while receiver.recv().await.is_ok() {
                if this
                    .update(cx, |this, cx| {
                        if this.pending_inline.as_ref().is_some_and(
                            |(_, context)| {
                                !this.proxy.as_ref().is_some_and(|proxy| {
                                    proxy.inline_prediction_route_is_current(context)
                                })
                            },
                        ) {
                            this.cancel_pending_inline();
                        }
                        if this.ghost_text.is_some()
                            && !this.ghost_context_is_current(cx)
                        {
                            this.ghost_text = None;
                            this.ghost_offset = None;
                            this.ghost_context = None;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    pub(crate) fn watch_shared_buffer_changes(
        &mut self,
        proxy: &Arc<ProxyClient>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let receiver = proxy.subscribe_shared_buffers();
        self.shared_update_task =
            Some(cx.spawn_in(window, async move |this, cx| {
                while receiver.recv().await.is_ok() {
                    if this
                        .update_in(cx, |panel, window, cx| {
                            panel.apply_cached_host_shared_change(window, cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        self.apply_cached_host_shared_change(window, cx);
    }

    fn watch_workspace_file_changes(
        &mut self,
        proxy: &Arc<ProxyClient>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let receiver = proxy.subscribe_workspace_file_events();
        self.workspace_file_task =
            Some(cx.spawn_in(window, async move |this, cx| {
                while let Ok(event) = receiver.recv().await {
                    match event {
                        WorkspaceFileEvent::Changed(paths) => {
                            let path = this
                                .update_in(cx, |panel, _, cx| {
                                    if (paths.is_empty()
                                        || crate::settings_panel::SettingsPanel::is_provider_settings_change(
                                            std::path::Path::new(&panel.workspace),
                                            &paths,
                                        ))
                                        && !panel.file_path.is_empty()
                                        && let Err(error) = panel
                                            .apply_editor_language_settings(
                                                &panel.file_path.clone(),
                                                cx,
                                            )
                                    {
                                        panel.status = format!(
                                            "Editor indentation settings were ignored: {error}"
                                        )
                                        .into();
                                    }
                                    if panel.shared_session_id.is_some()
                                        || panel.file_path.is_empty()
                                        || (!paths.is_empty()
                                            && !paths.iter().any(|changed| {
                                                changed
                                                    == std::path::Path::new(
                                                        &panel.file_path,
                                                    )
                                            }))
                                    {
                                        None
                                    } else {
                                        Some(panel.file_path.clone())
                                    }
                                })
                                .ok()
                                .flatten();
                            let Some(path) = path else {
                                continue;
                            };
                            let read_path = path.clone();
                            let disk_text = cx
                                .background_spawn(async move {
                                    read_editor_file(&read_path)
                                })
                                .await;
                            this.update_in(cx, |panel, window, cx| {
                                panel.apply_workspace_file_change(
                                    &path, disk_text, window, cx,
                                );
                            })
                            .log_err();
                        }
                        WorkspaceFileEvent::Renamed { old_path, new_path } => {
                            let rename = this
                                .update_in(cx, |panel, _, _| {
                                    if panel.shared_session_id.is_some()
                                        || panel.file_path.is_empty()
                                    {
                                        return None;
                                    }
                                    let source_path = panel.file_path.clone();
                                    let destination = renamed_path_for_open_file(
                                        std::path::Path::new(&source_path),
                                        &old_path,
                                        &new_path,
                                    )?;
                                    Some((
                                        source_path,
                                        panel.workspace.clone(),
                                        destination,
                                    ))
                                })
                                .ok()
                                .flatten();
                            let Some((source_path, workspace, destination)) = rename
                            else {
                                continue;
                            };
                            let renamed_file = cx
                                .background_spawn(async move {
                                    read_renamed_workspace_file(
                                        &workspace,
                                        &destination,
                                    )
                                })
                                .await;
                            this.update_in(cx, |panel, window, cx| {
                                panel.adopt_workspace_file_rename(
                                    &source_path,
                                    renamed_file,
                                    window,
                                    cx,
                                );
                            })
                            .log_err();
                        }
                    }
                }
            }));
    }

    fn watch_open_file_changes(
        &mut self,
        proxy: &Arc<ProxyClient>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let receiver = proxy.subscribe_open_file_changes();
        self.open_file_task = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(changed_path) = receiver.recv().await {
                let path = this
                    .update_in(cx, |panel, _, _| {
                        (panel.shared_session_id.is_none()
                            && std::path::Path::new(&panel.file_path)
                                == changed_path)
                            .then(|| panel.file_path.clone())
                    })
                    .ok()
                    .flatten();
                let Some(path) = path else {
                    continue;
                };
                let read_path = path.clone();
                let disk_text = cx
                    .background_spawn(async move { read_editor_file(&read_path) })
                    .await;
                this.update_in(cx, |panel, window, cx| {
                    panel.apply_workspace_file_change(&path, disk_text, window, cx);
                })
                .log_err();
            }
        }));
    }

    fn apply_workspace_file_change(
        &mut self,
        path: &str,
        disk_text: Result<String, FileOpenError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_path != path || self.shared_session_id.is_some() {
            return;
        }
        let current_text = self.editor.read(cx).value().to_string();
        self.dirty = current_text != self.saved_text.to_string();
        match disk_text {
            Ok(disk_text) => {
                let disk_sha256 =
                    format!("{:x}", sha2::Sha256::digest(disk_text.as_bytes()));
                let saved_sha256 = self.saved_sha256.clone().unwrap_or_else(|| {
                    format!(
                        "{:x}",
                        sha2::Sha256::digest(self.saved_text.to_string().as_bytes())
                    )
                });
                if disk_sha256 == saved_sha256 {
                    if self.external_change_conflict {
                        self.external_change_conflict = false;
                        self.external_file_missing = false;
                        self.invalidate_editor_requests();
                        cx.notify();
                    }
                    return;
                }
                if self.dirty {
                    self.external_change_conflict = true;
                    self.external_file_missing = false;
                    self.invalidate_editor_requests();
                    self.git_key = None;
                    self.git_task_key = None;
                    self.refresh_git_metadata(cx);
                    self.status =
                        "File changed on disk; unsaved edits kept. Save to review overwrite.".into();
                    cx.notify();
                    return;
                }

                let selection = rebase_selection(
                    &current_text,
                    &disk_text,
                    self.editor.read(cx).selected_range(),
                );
                self.loading = true;
                self.editor.update(cx, |editor, cx| {
                    editor.set_value(disk_text.clone(), window, cx);
                    editor.set_selected_range(selection, cx);
                });
                self.loading = false;
                self.saved_text = self.editor.read(cx).text().clone();
                self.saved_sha256 = Some(disk_sha256);
                self.dirty = false;
                self.saved_rev = self.dirty_rev;
                self.external_change_conflict = false;
                self.external_file_missing = false;
                self.recovery_conflict = false;
                self.invalidate_editor_requests();
                self.completion_request_id =
                    self.completion_request_id.wrapping_add(1);
                self.signature_request_id =
                    self.signature_request_id.wrapping_add(1);
                self.signature_help = None;
                self.signature_help_position = None;
                self.code_action_request_id =
                    self.code_action_request_id.wrapping_add(1);
                self.code_actions.clear();
                self.code_actions_position = None;
                self.code_actions_visible = false;
                self.completions.clear();
                self.completion_context = None;
                self.completion_details_requested.clear();
                self.show_completions = false;
                self.ghost_text = None;
                self.ghost_offset = None;
                self.hover_text = None;
                self.diagnostics.clear();
                self.presentation = None;
                self.comment_popover_line = None;
                self.comment_draft_target = None;
                self.presentation_decorations.clear(cx);
                self.git_state = GitFileState::default();
                self.refresh_git_gutter_markers(cx);
                self.git_key = None;
                self.git_task = None;
                self.git_task_key = None;
                self.status = format!("Reloaded {path} from disk").into();
                if let Some(proxy) = self.proxy.as_ref() {
                    proxy.sync_editor_snapshot(
                        self.proxy_path(),
                        self.editor.read(cx).value().to_string(),
                    );
                    proxy.refresh_anchors(&[self.proxy_path()]);
                }
                drop(self.queue_recovery(true, true, cx));
                self.refresh_git_metadata(cx);
                cx.notify();
            }
            Err(error) => {
                self.external_change_conflict = true;
                self.external_file_missing = matches!(
                    &error,
                    FileOpenError::Read(error)
                        if error.kind() == std::io::ErrorKind::NotFound
                );
                self.invalidate_editor_requests();
                self.status = if self.external_file_missing {
                    format!(
                        "{path} was deleted or renamed on disk; the open buffer was kept. Save will recreate the old path."
                    )
                    .into()
                } else {
                    format!(
                        "Could not reload {path}; the open buffer was kept: {error}"
                    )
                    .into()
                };
                cx.notify();
            }
        }
    }

    fn adopt_workspace_file_rename(
        &mut self,
        source_path: &str,
        renamed_file: Result<(std::path::PathBuf, String), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_path != source_path || self.shared_session_id.is_some() {
            return;
        }
        let (new_path, disk_text) = match renamed_file {
            Ok(renamed_file) => renamed_file,
            Err(error) => {
                self.status =
                    format!("Could not follow external file rename: {error}").into();
                cx.notify();
                return;
            }
        };
        let old_path = self.proxy_path();
        let new_path = new_path.to_string_lossy().into_owned();
        let disk_hash = format!("{:x}", sha2::Sha256::digest(disk_text.as_bytes()));
        let disk_matches_saved =
            self.saved_sha256.as_deref() == Some(disk_hash.as_str());
        let was_markdown_preview = self.show_markdown_preview;

        self.file_path = new_path.clone();
        self.file_error = None;
        self.external_file_missing = false;
        self.show_markdown_preview =
            was_markdown_preview && is_markdown_path(&new_path);
        self.invalidate_editor_requests();
        self.completion_request_id = self.completion_request_id.wrapping_add(1);
        self.signature_request_id = self.signature_request_id.wrapping_add(1);
        self.code_action_request_id = self.code_action_request_id.wrapping_add(1);
        self.semantic_request_id = self.semantic_request_id.wrapping_add(1);
        self.extension_syntax_request_id =
            self.extension_syntax_request_id.wrapping_add(1);
        self.semantic_task = None;
        self.extension_syntax_task = None;
        self.completions.clear();
        self.completion_context = None;
        self.completion_details_requested.clear();
        self.show_completions = false;
        self.completion_caret_top = None;
        self.completion_caret_left = None;
        self.completion_available_width = None;
        self.ghost_text = None;
        self.ghost_context = None;
        self.ghost_offset = None;
        self.hover_text = None;
        self.diagnostics.clear();
        self.signature_help = None;
        self.signature_help_position = None;
        self.code_actions.clear();
        self.code_actions_position = None;
        self.code_actions_visible = false;
        self.semantic_decorations.clear(cx);
        self.extension_comment_variables = None;
        self.extension_syntax_decorations.clear(cx);
        self.presentation = None;
        self.comment_popover_line = None;
        self.comment_draft_target = None;
        self.presentation_decorations.clear(cx);
        self.editor.update(cx, |editor, cx| {
            editor.set_disabled(false, cx);
            editor.set_highlighter(editor_language(&new_path), cx);
        });
        if let Err(error) = self.apply_editor_language_settings(&new_path, cx) {
            self.status =
                format!("Editor indentation settings were ignored: {error}").into();
        }
        self.git_state = GitFileState::default();
        self.git_gutter_markers = GutterDiffMarkers::default();
        self.git_key = None;
        self.git_task = None;
        self.git_task_key = None;
        self.git_diff_preview = None;
        self.expanded_hunk_start = None;
        self.hovered_breakpoint_line = None;

        let proxy = self.proxy.clone();
        if let Some(proxy) = &proxy {
            proxy.close_editor_buffer(old_path);
        }
        self.apply_workspace_file_change(&new_path, Ok(disk_text), window, cx);
        if let Some(proxy) = proxy {
            proxy.sync_editor_snapshot(
                std::path::PathBuf::from(&new_path),
                self.editor.read(cx).value().to_string(),
            );
            proxy.refresh_anchors(&[std::path::PathBuf::from(&new_path)]);
        }
        if disk_matches_saved && !self.external_change_conflict {
            self.status = if self.dirty {
                "File renamed; unsaved edits were kept at the new path".into()
            } else {
                format!("File renamed to {new_path}").into()
            };
        }
        drop(self.queue_recovery(true, true, cx));
        self.refresh_git_metadata(cx);
        cx.notify();
    }

    pub(crate) fn set_inline_blame_enabled(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if self.inline_blame_enabled != enabled {
            self.inline_blame_enabled = enabled;
            cx.notify();
        }
    }

    pub fn release_buffer(&mut self) {
        self.pending_rename = None;
        self.rename_in_progress = false;
        self.rename_request_id = self.rename_request_id.wrapping_add(1);
        if let Some(proxy) = self.proxy.take() {
            if self.shared_session_id.is_none() {
                proxy.close_editor_buffer(self.proxy_path());
            }
        }
        self.shared_poll_task = None;
        self.shared_edit_task = None;
        self.shared_update_task = None;
        self.diagnostic_task = None;
        self.workspace_file_task = None;
        self.open_file_task = None;
        self.git_task = None;
        self.git_task_key = None;
        self.git_key = None;
        self.git_state = GitFileState::default();
        self.git_gutter_markers = GutterDiffMarkers::default();
        self.hovered_breakpoint_line = None;
        self.expanded_hunk_start = None;
        self.git_diff_preview = None;
        self.expand_first_git_hunk_after_refresh = false;
        self.invalidate_editor_requests();
        self.show_completions = false;
        self.completion_context = None;
        self.completion_details_requested.clear();
    }

    pub(crate) fn recovery_id(&self) -> &str {
        &self.recovery_id
    }

    pub(crate) fn is_text_available(&self) -> bool {
        self.file_error.is_none()
    }

    pub(crate) fn navigation_request(
        &self,
        cx: &App,
    ) -> Option<crate::ross::OpenRequest> {
        if self.file_path.is_empty() {
            return None;
        }
        let position = self.cursor_position(cx);
        let column = crate::ross::OpenColumn::Utf16(position.character as usize);
        Some(crate::ross::OpenRequest {
            path: self.file_path.clone(),
            permanent: true,
            location: Some(crate::ross::OpenLocation {
                line: position.line as usize,
                column,
                end_line: position.line as usize,
                end_column: column,
            }),
        })
    }

    pub(crate) fn run_command(
        &mut self,
        command: EditorCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            EditorCommand::GoToDefinition => self.go_to_definition(cx),
            EditorCommand::FindReferences => self.find_references(cx),
            EditorCommand::GoToImplementation => self.go_to_implementation(cx),
            EditorCommand::RenameSymbol => self.prepare_symbol_rename(window, cx),
            EditorCommand::FormatDocument => self.format_document(window, cx),
            EditorCommand::ShowCompletions => {
                self.request_proxy_completions(cx, false)
            }
            EditorCommand::ShowSignatureHelp => self.request_signature_help(cx),
            EditorCommand::ShowCodeActions => self.request_code_actions(cx),
        }
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
        if !proxy.editor_recovery_available()
            || self.file_path.is_empty()
            || !self.is_text_available()
        {
            return None;
        }
        if self.recovery_revision == 0 && !self.dirty {
            return None;
        }
        let key = (self.request_generation, self.saved_request_id, self.dirty);
        if !force && self.recovery_key == Some(key) {
            return None;
        }
        let path = if let Some(session_id) = &self.shared_session_id {
            shared_draft_path(session_id, &self.file_path)
        } else {
            std::path::Path::new(&self.file_path)
                .strip_prefix(&self.workspace)
                .ok()
                .map(std::path::Path::to_path_buf)
        };
        let Some(path) = path else {
            self.status = "Unsaved recovery unavailable for this file".into();
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
                path,
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
            return Err(format!(
                "{error} Unsaved recovery remains in the database."
            ));
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
        self.editor.update(cx, |editor, cx| {
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

    pub(crate) fn workspace_edit_snapshot(
        &self,
        cx: &App,
    ) -> Result<WorkspaceBufferSnapshot, String> {
        if !self.is_text_available() {
            return Err(
                "A workspace edit targets a file that is not open as text".into()
            );
        }
        Ok(WorkspaceBufferSnapshot {
            path: self.proxy_path(),
            text: self.editor.read(cx).value().to_string(),
            selection: self.editor.read(cx).selected_range(),
            editable: self.shared_session_id.is_none() || self.shared_editable,
        })
    }

    pub(crate) fn buffer_generation(&self) -> u64 {
        self.request_generation
    }

    pub(crate) fn apply_workspace_text_edits(
        &mut self,
        expected_text: &str,
        edits: &[lsp_types::TextEdit],
        preserve_code_action_request: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.shared_session_id.is_some() && !self.shared_editable {
            return Err("A workspace edit targets a read-only shared file".into());
        }
        let text = self.editor.read(cx).value().to_string();
        if text != expected_text {
            return Err(
                "A file changed while the workspace edit was being prepared".into(),
            );
        }
        let selection = self.editor.read(cx).selected_range();
        let Some((range, replacement, selection)) =
            formatting_edit(&text, edits, selection)?
        else {
            return Ok(());
        };
        self.suppress_completion_for_next_edit = true;
        self.preserve_code_action_request_for_next_edit =
            preserve_code_action_request;
        self.snippet_session = None;
        self.ghost_text = None;
        self.ghost_offset = None;
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range(range, cx);
            editor.replace(replacement, window, cx);
            editor.set_selected_range(selection, cx);
        });
        self.status = "Applied workspace edit".into();
        cx.notify();
        Ok(())
    }

    fn resolve_merge_conflict(
        &mut self,
        range: std::ops::Range<usize>,
        choice: MergeConflictChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shared_session_id.is_some() && !self.shared_editable {
            self.status = "This shared file is read-only".into();
            cx.notify();
            return;
        }

        let text = self.editor.read(cx).value().to_string();
        let conflict = parse_merge_conflicts(&text)
            .into_iter()
            .find(|conflict| conflict.range == range);
        let Some(conflict) = conflict else {
            self.status =
                "Conflict changed; select its markers and try again".into();
            cx.notify();
            return;
        };
        let Some(replacement) = merge_conflict_choice_text(&text, &conflict, choice)
        else {
            self.status = "Conflict text is no longer available".into();
            cx.notify();
            return;
        };

        let caret = conflict.range.start + replacement.len();
        self.snippet_session = None;
        self.ghost_text = None;
        self.ghost_offset = None;
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range(conflict.range, cx);
            editor.replace(replacement, window, cx);
            editor.set_selected_range(caret..caret, cx);
        });
        self.status =
            "Conflict resolved in the buffer; review and stage the file".into();
        cx.notify();
    }

    pub fn turn_context(
        &self,
        cx: &App,
    ) -> Option<ahead_rpc::ahead::TurnEditorContext> {
        if !self.is_text_available() {
            return None;
        }
        let editor = self.editor.read(cx);
        let file_content = editor.value().to_string();
        let position = editor.cursor_position();
        let selected_range = editor.selected_range();
        let selection = if selected_range.start != selected_range.end {
            let start = editor.text().offset_to_position(selected_range.start);
            let end = editor.text().offset_to_position(selected_range.end);
            Some(ahead_rpc::ahead::DisplayRange {
                start: display_position_utf16(&file_content, start),
                end: display_position_utf16(&file_content, end),
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
            caret: display_position_utf16(&file_content, position),
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

    pub fn selected_code_comment(&self, cx: &App) -> Option<SelectedCodeComment> {
        let context = self.turn_context(cx)?;
        let range = context.selection?;
        let selection = self.editor.read(cx).selected_range();
        let quote = context.file_content.get(selection)?.to_string();
        Some(SelectedCodeComment {
            path: context.active_path,
            range,
            quote,
            source_sha256: format!(
                "{:x}",
                sha2::Sha256::digest(context.file_content.as_bytes())
            ),
        })
    }

    pub(crate) fn set_session_panel(
        &mut self,
        panel: WeakEntity<crate::session_panel::SessionPanel>,
    ) {
        self.session_panel = Some(panel);
    }

    pub(crate) fn show_code_comment_line(
        &mut self,
        line: u32,
        cx: &mut Context<Self>,
    ) {
        if self.comment_session_id.is_none() {
            return;
        }
        self.comment_draft_target = None;
        self.comment_popover_line = Some(line);
        cx.notify();
    }

    pub(crate) fn set_code_comments(
        &mut self,
        session_id: Option<String>,
        comments: Vec<ahead_rpc::ahead::CodeComment>,
        cx: &mut Context<Self>,
    ) {
        let session_changed = self.comment_session_id != session_id;
        if session_changed {
            self.comment_generation = self.comment_generation.wrapping_add(1);
            self.invalidate_editor_requests();
            self.ghost_text = None;
            self.ghost_offset = None;
            self.ghost_context = None;
            self.comment_pending = false;
            self.comment_error = None;
        }
        if session_changed || session_id.is_none() {
            self.comment_popover_line = None;
            self.comment_draft_target = None;
        }
        let comments = comments
            .into_iter()
            .filter(|comment| {
                session_id.as_deref() == Some(comment.session_id.as_str())
            })
            .collect();
        self.comment_session_id = session_id;
        self.code_comments = comments;
        cx.notify();
    }

    pub(crate) fn begin_code_comment(
        &mut self,
        target: SelectedCodeComment,
        session_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.comment_session_id.is_none() {
            self.status = "Start an AHEAD session before commenting".into();
            cx.notify();
            return;
        }
        if session_id.as_deref() != self.comment_session_id.as_deref() {
            self.status = "The active AHEAD session changed; try again".into();
            cx.notify();
            return;
        }
        if self.proxy.is_none() {
            self.status = "Reconnect the AHEAD session before commenting".into();
            cx.notify();
            return;
        }
        self.comment_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.comment_popover_line = Some(target.range.start.line + 1);
        self.comment_draft_target = Some(target);
        self.comment_error = None;
        self.comment_input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn save_code_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(proxy), Some(session_id), Some(target)) = (
            self.proxy.clone(),
            self.comment_session_id.clone(),
            self.comment_draft_target.clone(),
        ) else {
            return;
        };
        let body = self.comment_input.read(cx).value().trim().to_string();
        if body.is_empty() || self.comment_pending {
            return;
        }
        self.comment_pending = true;
        self.comment_error = None;
        let request_generation = self.comment_generation;
        cx.spawn_in(window, async move |this, cx| {
            let request_session_id = session_id.clone();
            let request_generation = request_generation;
            let result = cx
                .background_spawn(async move {
                    proxy.create_code_comment(
                        &session_id,
                        target.path,
                        target.range,
                        target.quote,
                        target.source_sha256,
                        body,
                    )
                })
                .await;
            this.update_in(cx, |code, window, cx| {
                if code.comment_session_id.as_deref() != Some(&request_session_id)
                    || code.comment_generation != request_generation
                {
                    return;
                }
                code.comment_pending = false;
                match result {
                    Ok(comment) => {
                        code.code_comments.push(comment.clone());
                        code.comment_draft_target = None;
                        code.comment_input
                            .update(cx, |input, cx| input.set_value("", window, cx));
                        if let Some(panel) = &code.session_panel {
                            panel
                                .update(cx, |panel, cx| {
                                    panel
                                        .upsert_code_comment_from_editor(comment, cx)
                                })
                                .log_err();
                        }
                        code.status = "Code comment saved to session".into();
                    }
                    Err(error) => code.comment_error = Some(error.message),
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
        cx.notify();
    }

    fn resolve_code_comment(
        &mut self,
        comment_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(proxy), Some(session_id)) =
            (self.proxy.clone(), self.comment_session_id.clone())
        else {
            return;
        };
        if self.comment_pending {
            return;
        }
        self.comment_pending = true;
        self.comment_error = None;
        let request_generation = self.comment_generation;
        cx.spawn_in(window, async move |this, cx| {
            let request_session_id = session_id.clone();
            let request_generation = request_generation;
            let result = cx
                .background_spawn(async move {
                    proxy.resolve_code_comment(&session_id, &comment_id)
                })
                .await;
            this.update(cx, |code, cx| {
                if code.comment_session_id.as_deref() != Some(&request_session_id)
                    || code.comment_generation != request_generation
                {
                    return;
                }
                code.comment_pending = false;
                match result {
                    Ok(comment) => {
                        if let Some(existing) = code
                            .code_comments
                            .iter_mut()
                            .find(|existing| existing.id == comment.id)
                        {
                            *existing = comment.clone();
                        }
                        if let Some(panel) = &code.session_panel {
                            panel
                                .update(cx, |panel, cx| {
                                    panel
                                        .upsert_code_comment_from_editor(comment, cx)
                                })
                                .log_err();
                        }
                        code.comment_popover_line = None;
                    }
                    Err(error) => code.comment_error = Some(error.message),
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
        cx.notify();
    }

    fn reference_code_comment(
        &mut self,
        comment: &ahead_rpc::ahead::CodeComment,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(panel) = &self.session_panel {
            if let Err(error) = panel.update_in(cx, |panel, window, cx| {
                panel.reference_code_comment(comment, window, cx)
            }) {
                self.status =
                    format!("Could not attach comment to chat: {error}").into();
            }
        }
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.dirty
    }

    fn apply_editor_language_settings(
        &mut self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let settings =
            match configured_editor_language_settings(&self.workspace, path) {
                Ok(settings) => settings,
                Err(error) => {
                    self.preferred_line_length =
                        ahead_core::config::LanguageEditorSettings::default()
                            .preferred_line_length as usize;
                    self.editor.update(cx, |editor, cx| {
                        editor.set_tab_size(default_editor_tab_size(), cx);
                    });
                    return Err(error);
                }
            };
        self.preferred_line_length = settings.preferred_line_length as usize;
        let tab_size = TabSize {
            tab_size: settings.tab_size as usize,
            hard_tabs: settings.hard_tabs,
        };
        self.editor.update(cx, |editor, cx| {
            editor.set_tab_size(tab_size, cx);
        });
        Ok(())
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
        if self.shared_session_id.is_some() && self.recovery_revision > 0 {
            drop(self.queue_recovery(true, true, cx));
        }
        let was_shared = self.shared_session_id.take().is_some();
        self.shared_poll_task = None;
        self.shared_edit_task = None;
        self.shared_committed_content = None;
        self.shared_editable = false;
        self.shared_conflict = None;
        self.shared_conflict_session_id = None;
        self.shared_revision = None;
        self.host_shared_recovery_pending = false;
        let disk_text = read_editor_file(path);
        if !was_shared
            && !self.file_path.is_empty()
            && self.is_text_available()
            && let Some(proxy) = &self.proxy
        {
            proxy.close_editor_buffer(self.proxy_path());
        }
        if self.file_path != path {
            if !was_shared {
                drop(self.queue_recovery(true, true, cx));
            }
            self.recovery_id = uuid::Uuid::new_v4().to_string();
            self.recovery_revision = 0;
            self.recovery_key = None;
            self.recovery_reply = None;
            self.recovery_conflict = false;
        }
        self.saved_sha256 = disk_text
            .as_ref()
            .ok()
            .map(|text| format!("{:x}", sha2::Sha256::digest(text.as_bytes())));
        self.external_change_conflict = false;
        self.external_file_missing = false;
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
        let tab_size_error = self.apply_editor_language_settings(path, cx).err();
        self.presentation = None;
        self.comment_popover_line = None;
        self.comment_draft_target = None;
        self.presentation_decorations.clear(cx);
        self.extension_comment_variables = None;
        self.extension_syntax_decorations.clear(cx);
        self.semantic_decorations.clear(cx);
        self.loading = true;
        self.snippet_session = None;
        self.pending_rename = None;
        self.rename_in_progress = false;
        self.rename_request_id = self.rename_request_id.wrapping_add(1);
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
        self.invalidate_editor_requests();
        self.ghost_text = None;
        self.hover_text = None;
        self.signature_request_id = self.signature_request_id.wrapping_add(1);
        self.signature_help = None;
        self.signature_help_position = None;
        self.code_action_request_id = self.code_action_request_id.wrapping_add(1);
        self.code_actions.clear();
        self.code_actions_position = None;
        self.code_actions_visible = false;
        self.git_task = None;
        self.git_task_key = None;
        self.git_key = None;
        self.git_state = GitFileState::default();
        self.refresh_git_gutter_markers(cx);
        self.expanded_hunk_start = None;
        self.git_diff_preview = None;
        self.expand_first_git_hunk_after_refresh = false;
        self.show_completions = false;
        if self.is_text_available()
            && let Some(proxy) = self.proxy.as_ref()
        {
            proxy.sync_editor_snapshot(
                self.proxy_path(),
                self.editor.read(cx).value().to_string(),
            );
            proxy.refresh_anchors(&[self.proxy_path()]);
        }
        self.schedule_semantic_tokens(cx);
        self.schedule_extension_syntax(cx);
        self.status = match (self.file_error.as_ref(), tab_size_error) {
            (Some(error), _) => error.to_string(),
            (None, Some(error)) => format!(
                "Opened {path}; editor indentation settings were ignored: {error}"
            ),
            (None, None) => format!("Opened {path}"),
        }
        .into();
        self.apply_cached_host_shared_change(window, cx);
        self.refresh_git_metadata(cx);
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
        if self.shared_session_id.is_some() {
            self.diagnostics.clear();
            return;
        }
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
        if self.shared_session_id.is_some() {
            return;
        }
        if let Some(proxy) = self.proxy.as_ref() {
            proxy.refresh_anchors(&[self.proxy_path()]);
        }
    }

    fn current_git_key(&self) -> Option<GitStateKey> {
        if self.file_path.is_empty() || self.shared_session_id.is_some() {
            return None;
        }
        Some(GitStateKey {
            path: self.proxy_path(),
            revision: self.dirty_rev,
            repository_generation: self.proxy.as_ref()?.git_generation(),
        })
    }

    fn ensure_git_diff_preview(
        &mut self,
        hunk: &DiffHunk,
        source_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_diff_preview.as_ref().is_some_and(|preview| {
            preview.hunk == *hunk && preview.source_text == source_text
        }) {
            return;
        }

        let projection = git_hunk_projection(hunk, source_text);
        let source_offset = self.editor.read(cx).scroll_offset();
        let line_height = self.editor.read(cx).line_height().unwrap_or(px(16.));
        let insertion_y = line_height * hunk.start.saturating_sub(1) as f32;
        let inserted_height = line_height * projection.inserted_rows as f32;
        let preview_offset = if source_offset.y < -insertion_y {
            point(source_offset.x, source_offset.y - inserted_height)
        } else {
            source_offset
        };
        let removed_background = cx.theme().danger.opacity(0.18);
        let added_background = cx.theme().success.opacity(0.16);
        let mut decorations = projection
            .deleted_ranges
            .iter()
            .cloned()
            .map(|range| {
                TextDecoration::new(
                    range,
                    HighlightStyle {
                        background_color: Some(removed_background),
                        ..Default::default()
                    },
                )
            })
            .collect::<Vec<_>>();
        decorations.extend(projection.added_ranges.iter().cloned().map(|range| {
            TextDecoration::new(
                range,
                HighlightStyle {
                    background_color: Some(added_background),
                    ..Default::default()
                },
            )
        }));
        let language = editor_language(&self.file_path);
        let tab_size = TabSize {
            tab_size: 4,
            hard_tabs: false,
        };
        let projected_text = projection.text;
        let inserted_rows = projection.inserted_rows;
        let editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .language(language)
                .line_number(true)
                .folding(true)
                .tab_size(tab_size)
                .default_value(projected_text);
            editor.set_readonly(true, cx);
            editor.set_scroll_offset(preview_offset, cx);
            editor.create_decorations_collection(decorations, cx);
            editor
        });

        self.git_diff_preview = Some(GitDiffPreview {
            hunk: hunk.clone(),
            source_text: source_text.to_owned(),
            editor,
            inserted_rows,
        });
    }

    fn close_git_diff_preview(&mut self, cx: &mut Context<Self>) {
        self.expanded_hunk_start = None;
        if let Some(preview) = self.git_diff_preview.take() {
            let projected_offset = preview.editor.read(cx).scroll_offset();
            let line_height =
                preview.editor.read(cx).line_height().unwrap_or(px(16.));
            let insertion_y =
                line_height * preview.hunk.start.saturating_sub(1) as f32;
            let inserted_height = line_height * preview.inserted_rows as f32;
            let source_y = if projected_offset.y < -insertion_y {
                projected_offset.y + inserted_height
            } else {
                projected_offset.y
            };
            self.editor.update(cx, |editor, cx| {
                editor.set_scroll_offset(point(projected_offset.x, source_y), cx);
            });
        }
        cx.notify();
    }

    pub(crate) fn reveal_first_git_hunk(&mut self, cx: &mut Context<Self>) {
        if let Some(hunk) = self.git_state.hunks.first() {
            self.expanded_hunk_start = Some(hunk.start);
            self.git_diff_preview = None;
            self.expand_first_git_hunk_after_refresh = false;
            self.scroll_to_source_line(hunk.start.saturating_sub(1) as usize, cx);
        } else if self.current_git_key().as_ref() == self.git_key.as_ref()
            && self.git_task.is_none()
            && self.git_task_key.is_none()
        {
            self.expanded_hunk_start = None;
            self.expand_first_git_hunk_after_refresh = false;
        } else {
            self.expand_first_git_hunk_after_refresh = true;
            self.refresh_git_metadata(cx);
        }
        cx.notify();
    }

    fn refresh_git_metadata(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.current_git_key() else {
            return;
        };
        if self.git_task_key.as_ref() == Some(&key) {
            return;
        }
        if self.git_key.as_ref() == Some(&key) {
            self.git_task = None;
            self.git_task_key = None;
            return;
        }
        self.git_task = None;
        self.git_task_key = Some(key.clone());
        self.git_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(GIT_METADATA_REFRESH_DEBOUNCE)
                .await;
            let Ok(Some((key, receiver))) = this.update(cx, |this, cx| {
                if this.current_git_key().as_ref() != Some(&key) {
                    return None;
                }
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

    fn refresh_git_gutter_markers(&mut self, cx: &App) {
        let line_count = self.editor.read(cx).text().lines_len().max(1);
        self.git_gutter_markers
            .update(&self.git_state.hunks, line_count);
    }

    fn update_git_gutter_line_count(&mut self, cx: &App) {
        let line_count = self.editor.read(cx).text().lines_len().max(1);
        self.git_gutter_markers.update_line_count(line_count);
    }

    fn finish_git_refresh(
        &mut self,
        key: GitStateKey,
        result: Result<GitFileState, String>,
        cx: &mut Context<Self>,
    ) {
        self.git_task = None;
        self.git_task_key = None;
        if self.git_key.as_ref() != Some(&key)
            || self.current_git_key().as_ref() != Some(&key)
        {
            self.refresh_git_metadata(cx);
            return;
        }
        let mut changed = false;
        match result {
            Ok(state) => {
                if self.git_state != state {
                    self.git_state = state;
                    self.refresh_git_gutter_markers(cx);
                    changed = true;
                }
                if self.expand_first_git_hunk_after_refresh {
                    self.expanded_hunk_start =
                        self.git_state.hunks.first().map(|hunk| hunk.start);
                    if let Some(hunk) = self.git_state.hunks.first() {
                        self.scroll_to_source_line(
                            hunk.start.saturating_sub(1) as usize,
                            cx,
                        );
                    }
                    self.expand_first_git_hunk_after_refresh = false;
                    changed = true;
                }
                if self.status.starts_with("Git metadata:") {
                    self.status = "".into();
                    changed = true;
                }
            }
            Err(error) => {
                self.expand_first_git_hunk_after_refresh = false;
                self.status = format!("Git metadata: {error}").into();
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn schedule_semantic_tokens(&mut self, cx: &mut Context<Self>) {
        if self.shared_session_id.is_some()
            || self.shared_conflict.is_some()
            || self.proxy.is_none()
            || !self.is_text_available()
        {
            self.semantic_request_id = self.semantic_request_id.wrapping_add(1);
            self.semantic_task = None;
            self.semantic_decorations.clear(cx);
            return;
        }

        self.semantic_request_id = self.semantic_request_id.wrapping_add(1);
        let request_id = self.semantic_request_id;
        let generation = self.request_generation;
        self.semantic_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(SEMANTIC_TOKEN_DEBOUNCE)
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.semantic_request_id == request_id
                    && this.request_generation == generation
                {
                    this.request_semantic_tokens(request_id, generation, cx);
                }
            });
        }));
    }

    fn schedule_extension_syntax(&mut self, cx: &mut Context<Self>) {
        if self.proxy.is_none() || !self.is_text_available() {
            self.extension_syntax_request_id =
                self.extension_syntax_request_id.wrapping_add(1);
            self.extension_syntax_task = None;
            self.extension_comment_variables = None;
            self.extension_syntax_decorations.clear(cx);
            return;
        }

        self.extension_syntax_request_id =
            self.extension_syntax_request_id.wrapping_add(1);
        let request_id = self.extension_syntax_request_id;
        let generation = self.request_generation;
        self.extension_syntax_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(SEMANTIC_TOKEN_DEBOUNCE)
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.extension_syntax_request_id == request_id
                    && this.request_generation == generation
                {
                    this.request_extension_syntax(request_id, generation, cx);
                }
            });
        }));
    }

    fn request_extension_syntax(
        &mut self,
        request_id: u64,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        let path = self.proxy_path();
        let text_len = self.editor.read(cx).value().len();
        let receiver = proxy.extension_syntax(path.clone());
        cx.spawn(async move |this, cx| {
            let response = cx
                .background_spawn(async move { receiver.recv().await })
                .await;
            let syntax = match response {
                Ok(Ok(syntax)) => Some(syntax),
                Ok(Err(_)) | Err(_) => None,
            };
            let _ = this.update(cx, |this, cx| {
                if this.extension_syntax_request_id != request_id
                    || this.request_generation != generation
                    || this.proxy_path() != path
                {
                    return;
                }

                let text = this.editor.read(cx).value().to_string();
                this.extension_comment_variables = syntax
                    .as_ref()
                    .filter(|syntax| syntax.path == path)
                    .and_then(|syntax| syntax.comments.as_ref())
                    .map(|comments| lsp_snippet::CommentVariables {
                        line_comment: comments.line_comment.clone(),
                        block_comment: comments.block_comment.clone(),
                    });
                let decorations = syntax
                    .filter(|syntax| {
                        syntax.path == path
                            && syntax.len == text_len
                            && text.len() == text_len
                    })
                    .map(|syntax| {
                        let theme = cx.theme().highlight_theme.clone();
                        extension_syntax_decorations(
                            &syntax.captures,
                            &text,
                            |scope| theme.style.syntax.style(scope),
                        )
                    })
                    .unwrap_or_default();
                this.extension_syntax_decorations.set(decorations, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn request_semantic_tokens(
        &mut self,
        request_id: u64,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        let path = self.proxy_path();
        let text_len = self.editor.read(cx).value().len();
        let receiver = proxy.semantic_tokens(path.clone());
        cx.spawn(async move |this, cx| {
            let response = cx
                .background_spawn(async move { receiver.recv().await })
                .await;
            let styles = match response {
                Ok(Ok(styles)) => Some(styles),
                Ok(Err(_)) | Err(_) => None,
            };
            let _ = this.update(cx, |this, cx| {
                if this.semantic_request_id != request_id
                    || this.request_generation != generation
                    || this.proxy_path() != path
                    || this.shared_session_id.is_some()
                    || this.shared_conflict.is_some()
                {
                    return;
                }

                let text = this.editor.read(cx).value().to_string();
                let decorations = styles
                    .filter(|styles| {
                        styles.path == path
                            && styles.len == text_len
                            && text.len() == text_len
                    })
                    .map(|styles| {
                        let theme = cx.theme().highlight_theme.clone();
                        semantic_text_decorations(&styles.styles, &text, |scope| {
                            theme.style.syntax.style(scope)
                        })
                    })
                    .unwrap_or_default();
                this.semantic_decorations.set(decorations, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn request_proxy_completions(
        &mut self,
        cx: &mut Context<Self>,
        retried_after_null: bool,
    ) {
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
        let (proxy_request_id, rx) = proxy.request_completions(path, pos, prefix);
        let query = completion_prefix_range(&text, pos)
            .and_then(|range| text.get(range))
            .unwrap_or_default()
            .to_owned();
        cx.spawn(async move |this, cx| {
            let mut reply = std::pin::pin!(rx.recv());
            let mut deadline =
                std::pin::pin!(cx.background_executor().timer(LSP_RESPONSE_TIMEOUT));
            let response = std::future::poll_fn(|cx| {
                use std::task::Poll;
                if let Poll::Ready(response) = reply.as_mut().poll(cx) {
                    return Poll::Ready(Some(response));
                }
                if deadline.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(None);
                }
                Poll::Pending
            })
            .await;
            let response = match response {
                Some(Ok(response)) => response,
                Some(Err(error)) => {
                    eprintln!("Receiving LSP completions: {error}");
                    proxy.cancel_completion_request(proxy_request_id);
                    Some(Vec::new())
                }
                None => {
                    proxy.cancel_completion_request(proxy_request_id);
                    Some(Vec::new())
                }
            };
            if response.is_none() && !retried_after_null {
                cx.background_executor()
                    .timer(NULL_COMPLETION_RETRY_DELAY)
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.request_generation == generation
                        && this.completion_request_id == request_id
                        && this.proxy_path() == request_path
                        && this.cursor_position(cx) == pos
                    {
                        this.request_proxy_completions(cx, true);
                    }
                });
                return;
            }
            let items = rank_completions(response.unwrap_or_default(), &query);
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
                    this.completion_details_requested.clear();
                    this.show_completions = true;
                    this.request_visible_completion_details(cx);
                    this.status = "Completions from proxy".into();
                } else {
                    this.completions.clear();
                    this.completion_details_requested.clear();
                    this.show_completions = false;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn request_completion_detail(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.show_completions
            || self.completion_details_requested.contains(&index)
        {
            return;
        }
        let (Some(proxy), Some(context), Some(completion)) = (
            self.proxy.clone(),
            self.completion_context.clone(),
            self.completions.get(index).cloned(),
        ) else {
            return;
        };
        if completion
            .item
            .detail
            .as_deref()
            .is_some_and(|detail| !detail.trim().is_empty())
        {
            return;
        }
        self.completion_details_requested.insert(index);
        let receiver = proxy.resolve_completion(completion);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    receiver
                        .recv()
                        .map_err(|error| error.to_string())?
                        .map_err(|error| error.message)
                })
                .await;
            if let Err(error) = this.update(cx, |this, cx| {
                if !this.show_completions
                    || this.completion_context.as_ref() != Some(&context)
                {
                    return;
                }
                match result {
                    Ok(item) => {
                        if let Some(completion) = this.completions.get_mut(index) {
                            completion.item = item;
                            cx.notify();
                        }
                    }
                    Err(error) => {
                        eprintln!("Resolving LSP completion detail: {error}");
                    }
                }
            }) {
                eprintln!("Applying resolved LSP completion detail: {error}");
            }
        })
        .detach();
    }

    fn request_visible_completion_details(&mut self, cx: &mut Context<Self>) {
        for index in 0..self.completions.len().min(MAX_COMPLETION_SUGGESTIONS) {
            self.request_completion_detail(index, cx);
        }
    }

    fn select_completion(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(last_index) = self.completions.len().checked_sub(1) else {
            return;
        };
        let index = index.min(last_index);
        if self.selected_completion == index {
            return;
        }
        self.selected_completion = index;
        self.request_completion_detail(index, cx);
        cx.notify();
    }

    fn invalidate_editor_requests(&mut self) {
        self.cancel_pending_inline();
        self.request_generation = self.request_generation.wrapping_add(1);
    }

    fn cancel_pending_inline(&mut self) {
        self.inline_prediction_task = None;
        self.pending_inline = None;
        self.ghost_accepting = false;
    }

    pub(crate) fn invalidate_prediction_for_model_change(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.invalidate_editor_requests();
        self.ghost_text = None;
        self.ghost_offset = None;
        self.ghost_context = None;
        cx.notify();
    }

    fn request_proxy_inline(&mut self, cx: &mut Context<Self>) {
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        let path = self.proxy_path();
        let pos = self.cursor_position(cx);
        self.invalidate_editor_requests();
        let generation = self.request_generation;
        let request_path = path.clone();
        let session_id = self.comment_session_id.clone();
        let model_context = self.session_panel.as_ref().and_then(|panel| {
            panel
                .read_with(cx, |panel, _| panel.prediction_model_context())
                .ok()
                .flatten()
        });
        let rx = proxy.request_inline(
            path,
            pos,
            session_id.clone(),
            model_context.clone(),
        );
        self.pending_inline =
            Some((self.editor.read(cx).selected_range(), rx.context.clone()));
        self.inline_prediction_task = Some(cx.spawn(async move |this, cx| {
            let prediction = rx.recv().await;
            this.update(cx, |this: &mut Self, cx| {
                if this.request_generation == generation {
                    this.pending_inline = None;
                }
                let Ok(prediction) = prediction else {
                    return;
                };
                if this.request_generation != generation
                    || this.proxy_path() != request_path
                    || this.cursor_position(cx) != pos
                    || this.comment_session_id != session_id
                    || this.session_panel.as_ref().and_then(|panel| {
                        panel
                            .read_with(cx, |panel, _| {
                                panel.prediction_model_context()
                            })
                            .ok()
                            .flatten()
                    }) != model_context
                    || !this.proxy.as_ref().is_some_and(|proxy| {
                        proxy.inline_prediction_is_current(&prediction.context)
                    })
                {
                    return;
                }
                if !prediction.text.trim().is_empty() {
                    this.ghost_offset =
                        caret_byte_offset(&this.editor.read(cx).value(), pos);
                    this.ghost_text = Some(prediction.text);
                    this.ghost_context = Some(prediction.context);
                    cx.notify();
                }
            })
            .log_err();
        }));
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
        if !self.recovery_conflict && !self.external_change_conflict {
            return self.save_and_wait(cx);
        }
        let generation = self.request_generation;
        let description = if self.external_change_conflict {
            if self.external_file_missing {
                "The file was deleted or renamed on disk. Saving recreates it at this path."
            } else {
                "The file changed on disk since this buffer was loaded. Overwriting replaces the current disk contents."
            }
        } else {
            "This recovered buffer was based on a different disk version. Overwriting replaces the current disk contents."
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            "Overwrite the changed disk file?",
            Some(&format!("{}\n{description}", self.file_path)),
            &["Overwrite", "Cancel"],
            cx,
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
        if self.shared_session_id.is_some() {
            self.dirty = self.editor.read(cx).text() != &self.saved_text;
            if !self.shared_editable {
                self.status = if self.dirty {
                    "Local shared edits cannot sync; reconnect or copy them before closing"
                } else {
                    "Shared file is read-only"
                }.into();
                cx.notify();
                return Task::ready(!self.dirty);
            }
            if self.shared_conflict.is_some() {
                self.status =
                    "Resolve the shared edit conflict before saving".into();
                cx.notify();
                return Task::ready(false);
            }
            self.start_shared_edit_sync(cx);
            self.status = if self.dirty {
                "Syncing shared edits…"
            } else {
                "Shared edits synced"
            }
            .into();
            cx.notify();
            return Task::ready(!self.dirty);
        }
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
        self.dirty = self.editor.read(cx).text() != &self.saved_text;
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
                Ok(1) => {
                    let discard = this
                        .read_with(cx, |panel, _| {
                            (panel.host_shared_recovery_pending
                                && panel.shared_session_id.is_none()
                                && panel.request_generation == generation)
                                .then(|| {
                                    panel.proxy.as_ref().map(|proxy| {
                                        (proxy.clone(), panel.proxy_path())
                                    })
                                })
                                .flatten()
                        })
                        .ok()?;
                    if let Some((proxy, path)) = discard {
                        let requested_path = path.clone();
                        let result = cx
                            .background_spawn(async move {
                                proxy.discard_shared_remote_recovery(path)
                            })
                            .await;
                        if let Err(error) = result {
                            this.update(cx, |panel, cx| {
                                panel.status = format!(
                                    "Could not discard shared edit: {}",
                                    error.message
                                )
                                .into();
                                cx.notify();
                            })
                            .log_err();
                            return None;
                        }
                        this.update(cx, |panel, _| {
                            if panel.request_generation == generation
                                && panel.proxy_path() == requested_path
                            {
                                panel.host_shared_recovery_pending = false;
                            }
                        })
                        .ok()?;
                    }
                }
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
                    self.external_change_conflict = false;
                    self.external_file_missing = false;
                    self.host_shared_recovery_pending = false;
                    self.saved_text = text;
                    self.dirty = self.editor.read(cx).text() != &self.saved_text;
                    self.git_key = None;
                    self.refresh_git_metadata(cx);
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

    fn ghost_context_is_current(&self, cx: &App) -> bool {
        self.ghost_context.as_ref().is_none_or(|context| {
            context.session_id == self.comment_session_id
                && context.model_context
                    == self.session_panel.as_ref().and_then(|panel| {
                        panel
                            .read_with(cx, |panel, _| {
                                panel.prediction_model_context()
                            })
                            .ok()
                            .flatten()
                    })
                && self
                    .proxy
                    .as_ref()
                    .is_some_and(|proxy| proxy.inline_prediction_is_current(context))
        })
    }

    fn ghost_is_current_at_caret(&self, cx: &App) -> bool {
        self.ghost_context_is_current(cx)
            && self.editor.read(cx).selected_range().is_empty()
            && self.ghost_offset.is_none_or(|offset| {
                caret_byte_offset(
                    &self.editor.read(cx).value(),
                    self.cursor_position(cx),
                ) == Some(offset)
            })
    }

    pub fn accept_ghost_text(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.ghost_accepting {
            return;
        }
        if !self.ghost_is_current_at_caret(cx) {
            self.ghost_text = None;
            self.ghost_offset = None;
            self.ghost_context = None;
            cx.notify();
            return;
        }
        if let Some(binding) = self
            .ghost_context
            .as_ref()
            .and_then(|context| context.binding.clone())
        {
            let Some(proxy) = self.proxy.clone() else {
                return;
            };
            self.cancel_pending_inline();
            let path = self.proxy_path();
            let position = self.cursor_position(cx);
            let generation = self.request_generation;
            let Some(ghost) = self.ghost_text.clone() else {
                return;
            };
            let content_sha256 = format!(
                "{:x}",
                sha2::Sha256::digest(self.editor.read(cx).value().as_bytes())
            );
            let receiver = proxy.validate_inline_prediction(
                path.clone(),
                position,
                content_sha256,
                ghost.clone(),
                binding,
            );
            self.ghost_accepting = true;
            self.pending_inline = self
                .ghost_context
                .clone()
                .map(|context| (self.editor.read(cx).selected_range(), context));
            self.inline_prediction_task =
                Some(cx.spawn_in(window, async move |this, cx| {
                    let result = receiver.recv().await;
                    this.update_in(cx, |this, window, cx| {
                        if this.request_generation != generation
                            || this.proxy_path() != path
                        {
                            return;
                        }
                        this.ghost_accepting = false;
                        this.pending_inline = None;
                        if this.ghost_text.as_ref() != Some(&ghost)
                            || !this.ghost_is_current_at_caret(cx)
                        {
                            return;
                        }
                        let status = match result {
                            Ok(Ok(true)) => {
                                this.insert_ghost_text(window, cx);
                                return;
                            }
                            Ok(Ok(false)) => {
                                "Suggestion is stale; request another".into()
                            }
                            Ok(Err(error)) => format!(
                                "Could not validate suggestion: {}",
                                error.message
                            ),
                            Err(error) => {
                                format!("Could not validate suggestion: {error}")
                            }
                        };
                        this.ghost_text = None;
                        this.ghost_offset = None;
                        this.ghost_context = None;
                        this.status = status.into();
                        cx.notify();
                    })
                    .log_err();
                }));
            return;
        }
        self.insert_ghost_text(window, cx);
    }

    fn insert_ghost_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ghost) = self.ghost_text.take() {
            let prediction_context = self.ghost_context.take();
            let acceptance_context = prediction_context.and_then(|context| {
                Some((context.session_id?, context.binding?, context.model_context))
            });
            let start = self.cursor_position(cx);
            self.ghost_offset = None;
            self.suppress_completion_for_next_edit = true;
            self.editor.update(cx, |ed, cx| {
                ed.insert(ghost.clone(), window, cx);
            });
            let end = self.cursor_position(cx);
            if let (Some(proxy), Some((session_id, binding, model_context))) =
                (self.proxy.as_ref(), acceptance_context)
                && let Some(path) = std::path::Path::new(&self.file_path)
                    .strip_prefix(&self.workspace)
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned())
                && (start.line, start.character) < (end.line, end.character)
            {
                let record_session_id = session_id.clone();
                let record_path = self.file_path.clone();
                let generation = self.request_generation;
                let receiver = proxy.record_prediction_acceptance(
                    session_id,
                    path,
                    ahead_rpc::ahead::DisplayRange {
                        start: ahead_rpc::ahead::DisplayPosition {
                            line: start.line,
                            col: start.character,
                        },
                        end: ahead_rpc::ahead::DisplayPosition {
                            line: end.line,
                            col: end.character,
                        },
                    },
                    binding,
                    model_context,
                    format!("{:x}", sha2::Sha256::digest(ghost.as_bytes())),
                );
                cx.spawn(async move |this, cx| {
                    let error = match receiver.recv().await {
                        Ok(Ok(())) => return,
                        Ok(Err(error)) => error.message,
                        Err(error) => error.to_string(),
                    };
                    this.update(cx, |this, cx| {
                        if this.request_generation == generation
                            && this.file_path == record_path
                            && this.comment_session_id.as_deref()
                                == Some(record_session_id.as_str())
                        {
                            this.status = format!(
                                "Accepted inline completion, but its session record failed: {error}"
                            )
                            .into();
                            cx.notify();
                        }
                    })
                    .log_err();
                })
                .detach();
            }
            self.status = "Accepted inline completion".into();
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
            let cursor = caret_byte_offset(&text, context.position)
                .ok_or("The completion position is stale")?;
            let selected_range = self.editor.read(cx).selected_range();
            let selected_text = text.get(selected_range).unwrap_or_default();
            let insertion_text = match &item.text_edit {
                Some(lsp_types::CompletionTextEdit::Edit(edit)) => {
                    edit.new_text.as_str()
                }
                Some(lsp_types::CompletionTextEdit::InsertAndReplace(edit)) => {
                    edit.new_text.as_str()
                }
                None => item.insert_text.as_deref().unwrap_or(&item.label),
            };
            let clipboard = if item.insert_text_format
                == Some(lsp_types::InsertTextFormat::SNIPPET)
                && insertion_text.contains("$CLIPBOARD")
            {
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            let variables = lsp_snippet::Variables::for_completion(
                &text,
                cursor,
                context.position.line as usize,
                std::path::Path::new(&self.file_path),
                std::path::Path::new(&self.workspace),
                selected_text,
                &clipboard,
                self.extension_comment_variables.as_ref(),
            );
            let (edit_range, replacement, caret, mut tabstops) =
                completion_edit_with_variables(
                    &text,
                    context.position,
                    &item,
                    &variables,
                )?;
            let selection = tabstops
                .as_ref()
                .and_then(|tabstops| tabstops.first())
                .and_then(|tabstop| tabstop.ranges.first())
                .map_or(caret..caret, |range| {
                    edit_range.start + range.start..edit_range.start + range.end
                });
            let snippet_session = tabstops.take().and_then(|mut tabstops| {
                for tabstop in &mut tabstops {
                    for range in &mut tabstop.ranges {
                        range.start += edit_range.start;
                        range.end += edit_range.start;
                    }
                }
                let updated_text = format!(
                    "{}{}{}",
                    &text[..edit_range.start],
                    replacement,
                    &text[edit_range.end..]
                );
                SnippetSession::new(updated_text, tabstops)
            });
            self.suppress_completion_for_next_edit = true;
            self.editor.update(cx, |editor, cx| {
                editor.set_selected_range(edit_range, cx);
                editor.replace(replacement, window, cx);
                editor.set_selected_range(selection, cx);
            });
            self.snippet_session = snippet_session;
            self.invalidate_editor_requests();
            Ok(item.label)
        });
        self.status = match applied {
            Ok(label) => format!("Completed {label}"),
            Err(error) => format!("Completion failed: {error}"),
        }
        .into();
        cx.notify();
    }

    fn move_snippet_tabstop(
        &mut self,
        forward: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mut session) = self.snippet_session.take() else {
            return false;
        };
        let selection = self.editor.read(cx).selected_range();
        if !session.active_contains(&selection) {
            self.snippet_session = Some(session);
            return false;
        }

        let active = session.active_index();
        if !forward && active == 0 {
            self.snippet_session = Some(session);
            return false;
        }
        let next = if forward { active + 1 } else { active - 1 };
        if !session.can_move_to(next) {
            self.snippet_session = Some(session);
            return false;
        }
        let Some(range) = session.move_to(next) else {
            self.snippet_session = Some(session);
            return false;
        };
        let is_final = session.active_is_final();
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range(range, cx);
        });
        self.snippet_session = (!is_final).then_some(session);
        cx.notify();
        true
    }

    fn cycle_snippet_choice(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mut session) = self.snippet_session.take() else {
            return false;
        };
        let selection = self.editor.read(cx).selected_range();
        if !session.active_contains(&selection) || session.active_choices().len() < 2
        {
            self.snippet_session = Some(session);
            return false;
        }
        let Some((old_range, new_range, replacement)) =
            session.cycle_choice(forward)
        else {
            self.snippet_session = Some(session);
            return false;
        };
        if self.editor.read(cx).value().get(old_range.clone())
            == Some(replacement.as_str())
        {
            self.snippet_session = Some(session);
            cx.notify();
            return true;
        }
        self.suppress_completion_for_next_edit = true;
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range(old_range, cx);
            editor.replace(replacement, window, cx);
            editor.set_selected_range(new_range, cx);
        });
        self.snippet_session = Some(session);
        cx.notify();
        true
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

        if key == "escape" && self.navigation_results.take().is_some() {
            cx.notify();
            return;
        }

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

        if key == "f12" && modifiers.shift && !primary && !ctrl && !alt {
            self.find_references(cx);
            return;
        }

        if key == "f12" && (primary || ctrl) && !modifiers.shift && !alt {
            self.go_to_implementation(cx);
            return;
        }

        if self.show_completions {
            match key {
                "down" => {
                    if self.selected_completion + 1 < self.completions.len() {
                        self.select_completion(self.selected_completion + 1, cx);
                    }
                    return;
                }
                "up" => {
                    if self.selected_completion > 0 {
                        self.select_completion(self.selected_completion - 1, cx);
                    }
                    return;
                }
                "enter" => {
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

        if key == "escape"
            && (self.ghost_text.is_some() || self.pending_inline.is_some())
        {
            self.cancel_pending_inline();
            self.ghost_text = None;
            self.ghost_offset = None;
            self.ghost_context = None;
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

    fn find_references(&mut self, cx: &mut Context<Self>) {
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        let path = self.proxy_path();
        let position = self.cursor_position(cx);
        let receiver = proxy.request_references(path.clone(), position);
        self.request_navigation("References", path, position, receiver, cx);
    }

    fn go_to_implementation(&mut self, cx: &mut Context<Self>) {
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        let path = self.proxy_path();
        let position = self.cursor_position(cx);
        let receiver = proxy.request_implementations(path.clone(), position);
        self.request_navigation("Implementations", path, position, receiver, cx);
    }

    fn prepare_symbol_rename(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_text_available() {
            return;
        }
        if self.shared_session_id.is_some() && !self.shared_editable {
            self.status = "This shared file is read-only".into();
            cx.notify();
            return;
        }
        let Some(proxy) = self.proxy.clone() else {
            self.status = "Language services are unavailable".into();
            cx.notify();
            return;
        };
        let path = self.proxy_path();
        let position = self.cursor_position(cx);
        let text = self.editor.read(cx).value().to_string();
        let generation = self.request_generation;
        self.rename_request_id = self.rename_request_id.wrapping_add(1);
        let request_id = self.rename_request_id;
        self.pending_rename = None;
        self.rename_in_progress = false;
        self.status = "Preparing symbol rename…".into();
        let receiver = proxy.request_prepare_rename(path.clone(), position);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    receiver.recv_timeout(Duration::from_secs(5)).map_err(
                        |error| format!("Waiting for rename preparation: {error}"),
                    )?
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.rename_request_id != request_id
                    || this.request_generation != generation
                    || this.proxy_path() != path
                    || this.cursor_position(cx) != position
                {
                    return;
                }
                match result.and_then(|response| {
                    rename_placeholder(&text, position, response)
                }) {
                    Ok(old_name) => {
                        this.rename_input.update(cx, |input, cx| {
                            input.set_value(old_name.clone(), window, cx);
                        });
                        this.pending_rename = Some(PendingRename {
                            path,
                            position,
                            generation,
                            request_id,
                            old_name,
                        });
                        this.status = "".into();
                        this.rename_input.focus_handle(cx).focus(window, cx);
                    }
                    Err(error) => {
                        this.status = format!("Rename unavailable: {error}").into();
                    }
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    fn confirm_symbol_rename(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pending_rename.clone() else {
            return;
        };
        if self.rename_in_progress {
            return;
        }
        if pending.generation != self.request_generation
            || pending.path != self.proxy_path()
        {
            self.cancel_symbol_rename(cx);
            return;
        }
        let new_name = self.rename_input.read(cx).value().to_string();
        if new_name.trim().is_empty() {
            self.status = "Rename failed: enter a symbol name".into();
            cx.notify();
            return;
        }
        if new_name == pending.old_name {
            self.cancel_symbol_rename(cx);
            return;
        }
        let Some(proxy) = self.proxy.clone() else {
            self.status = "Rename failed: language services are unavailable".into();
            cx.notify();
            return;
        };
        self.rename_in_progress = true;
        self.status = "Renaming symbol…".into();
        let receiver = proxy.request_rename(
            pending.path.clone(),
            pending.position,
            new_name.clone(),
        );
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    receiver
                        .recv_timeout(Duration::from_secs(10))
                        .map_err(|error| format!("Waiting for rename: {error}"))?
                })
                .await;
            this.update_in(cx, |this, _, cx| {
                let still_current =
                    this.pending_rename.as_ref().is_some_and(|current| {
                        current.request_id == pending.request_id
                            && current.generation == this.request_generation
                            && current.path == this.proxy_path()
                    });
                if !still_current {
                    return;
                }
                match result.and_then(|edit| {
                    workspace_edit_text_edits(&edit)?;
                    Ok(edit)
                }) {
                    Ok(edit) => {
                        this.pending_rename = None;
                        this.rename_in_progress = false;
                        this.status = "Applying workspace rename…".into();
                        cx.emit(WorkspaceEditRequest {
                            edit,
                            source_path: pending.path,
                            purpose: WorkspaceEditPurpose::Rename {
                                old_name: pending.old_name,
                                new_name,
                            },
                            generation: pending.generation,
                            completion: None,
                        });
                    }
                    Err(error) => {
                        this.rename_in_progress = false;
                        this.status = format!("Rename failed: {error}").into();
                    }
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    fn cancel_symbol_rename(&mut self, cx: &mut Context<Self>) {
        self.pending_rename = None;
        self.rename_in_progress = false;
        self.rename_request_id = self.rename_request_id.wrapping_add(1);
        self.status = "".into();
        cx.notify();
    }

    fn format_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_text_available() {
            return;
        }
        if self.shared_session_id.is_some() && !self.shared_editable {
            self.status = "This shared file is read-only".into();
            cx.notify();
            return;
        }
        let Some(proxy) = self.proxy.clone() else {
            self.status = "Language services are unavailable".into();
            cx.notify();
            return;
        };
        let path = self.proxy_path();
        let generation = self.request_generation;
        self.format_request_id = self.format_request_id.wrapping_add(1);
        let request_id = self.format_request_id;
        let text = self.editor.read(cx).value().to_string();
        let selection = self.editor.read(cx).selected_range();
        let receiver = proxy.request_document_formatting(path.clone());
        self.status = "Formatting document…".into();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    receiver.recv().map_err(|error| error.to_string())?
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.format_request_id != request_id {
                    return;
                }
                if this.request_generation != generation || this.proxy_path() != path
                {
                    if this.status.as_ref() == "Formatting document…" {
                        this.status = "".into();
                        cx.notify();
                    }
                    return;
                }
                match result
                    .and_then(|edits| formatting_edit(&text, &edits, selection))
                {
                    Ok(None) => this.status = "Already formatted".into(),
                    Ok(Some((range, replacement, selection))) => {
                        this.suppress_completion_for_next_edit = true;
                        this.snippet_session = None;
                        this.ghost_text = None;
                        this.ghost_offset = None;
                        this.editor.update(cx, |editor, cx| {
                            editor.set_selected_range(range, cx);
                            editor.replace(replacement, window, cx);
                            editor.set_selected_range(selection, cx);
                        });
                        this.status = "Formatted document".into();
                    }
                    Err(error) => {
                        this.status = format!("Formatting failed: {error}").into();
                    }
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    fn request_signature_help(&mut self, cx: &mut Context<Self>) {
        self.request_signature_help_with_trigger(
            lsp_types::SignatureHelpTriggerKind::INVOKED,
            cx,
        );
    }

    fn request_signature_help_after_content_change(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.request_signature_help_with_trigger(
            lsp_types::SignatureHelpTriggerKind::CONTENT_CHANGE,
            cx,
        );
    }

    fn request_signature_help_with_trigger(
        &mut self,
        trigger_kind: lsp_types::SignatureHelpTriggerKind,
        cx: &mut Context<Self>,
    ) {
        if !self.is_text_available() {
            return;
        }
        let Some(proxy) = self.proxy.clone() else {
            self.status = "Language services are unavailable".into();
            cx.notify();
            return;
        };
        let path = self.proxy_path();
        let position = self.cursor_position(cx);
        let generation = self.request_generation;
        self.signature_request_id = self.signature_request_id.wrapping_add(1);
        let request_id = self.signature_request_id;
        let active_signature_help = self.signature_help.clone();
        let context = lsp_types::SignatureHelpContext {
            trigger_kind,
            trigger_character: None,
            is_retrigger: active_signature_help.is_some(),
            active_signature_help,
        };
        self.signature_help_position =
            self.signature_help.as_ref().map(|_| position);
        self.hover_text = None;
        let (proxy_request_id, receiver) =
            proxy.request_signature_help(path.clone(), position, context);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut reply = std::pin::pin!(receiver.recv());
            let mut deadline =
                std::pin::pin!(cx.background_executor().timer(LSP_RESPONSE_TIMEOUT));
            let response = std::future::poll_fn(|cx| {
                use std::task::Poll;
                if let Poll::Ready(response) = reply.as_mut().poll(cx) {
                    return Poll::Ready(Some(response));
                }
                if deadline.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(None);
                }
                Poll::Pending
            })
            .await;
            let help = match response {
                Some(Ok(help)) => help,
                Some(Err(error)) => {
                    eprintln!("Receiving signature help: {error}");
                    proxy.cancel_signature_help_request(proxy_request_id);
                    None
                }
                None => {
                    proxy.cancel_signature_help_request(proxy_request_id);
                    None
                }
            };
            let _ = this.update(cx, |this, cx| {
                if this.signature_request_id != request_id
                    || this.request_generation != generation
                    || this.proxy_path() != path
                    || this.cursor_position(cx) != position
                {
                    return;
                }
                this.signature_help_position = help.as_ref().map(|_| position);
                this.signature_help = help;
                cx.notify();
            });
        })
        .detach();
    }

    fn cycle_signature_help(
        &mut self,
        forward: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(help) = self.signature_help.as_mut() else {
            return false;
        };
        let count = help.signatures.len();
        if count < 2 {
            return false;
        }
        let current = (help.active_signature.unwrap_or(0) as usize).min(count - 1);
        let next = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        let Ok(next) = u32::try_from(next) else {
            return false;
        };
        help.active_signature = Some(next);
        cx.notify();
        true
    }

    fn request_code_actions(&mut self, cx: &mut Context<Self>) {
        if !self.is_text_available() {
            return;
        }
        if self.shared_session_id.is_some() && !self.shared_editable {
            self.status = "This shared file is read-only".into();
            cx.notify();
            return;
        }
        let Some(proxy) = self.proxy.clone() else {
            self.status = "Language services are unavailable".into();
            cx.notify();
            return;
        };

        let path = self.proxy_path();
        let position = self.cursor_position(cx);
        let generation = self.request_generation;
        self.code_action_request_id = self.code_action_request_id.wrapping_add(1);
        let request_id = self.code_action_request_id;
        let diagnostics = proxy.diagnostics_for(&path).unwrap_or_default();
        let receiver =
            proxy.request_code_actions(path.clone(), position, diagnostics);
        self.code_actions.clear();
        self.code_actions_position = None;
        self.code_actions_visible = false;
        self.show_completions = false;
        self.signature_help = None;
        self.signature_help_position = None;
        self.ghost_text = None;
        self.ghost_offset = None;
        self.hover_text = None;
        self.status = "Looking for code actions…".into();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    receiver.recv_timeout(Duration::from_secs(5)).map_err(
                        |error| format!("Waiting for code actions: {error}"),
                    )?
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.code_action_request_id != request_id
                    || this.request_generation != generation
                    || this.proxy_path() != path
                    || this.cursor_position(cx) != position
                {
                    return;
                }
                match result {
                    Ok(actions) if !actions.is_empty() => {
                        this.code_actions = actions;
                        this.code_actions_position = Some(position);
                        this.selected_code_action = 0;
                        this.code_actions_visible = true;
                        this.status = "".into();
                    }
                    Ok(_) => {
                        this.status = "No code actions available".into();
                    }
                    Err(error) => {
                        this.status =
                            format!("Code actions unavailable: {error}").into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_selected_code_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected) =
            self.code_actions.get(self.selected_code_action).cloned()
        else {
            return;
        };
        if self.shared_session_id.is_some() && !self.shared_editable {
            self.code_actions_visible = false;
            self.status = "This shared file is read-only".into();
            cx.notify();
            return;
        }
        let LspCodeAction { plugin_id, action } = selected;
        let (action, standalone_command) = match action {
            lsp_types::CodeActionOrCommand::CodeAction(action) => {
                (Some(action), None)
            }
            lsp_types::CodeActionOrCommand::Command(command) => {
                (None, Some(command))
            }
        };
        if let Some(disabled) =
            action.as_ref().and_then(|action| action.disabled.as_ref())
        {
            self.code_actions_visible = false;
            self.status = disabled.reason.clone().into();
            cx.notify();
            return;
        }
        let Some(proxy) = self.proxy.clone() else {
            self.code_actions_visible = false;
            self.status = "Language services are unavailable".into();
            cx.notify();
            return;
        };

        let generation = self.request_generation;
        let source_buffer_generation = self.buffer_generation();
        let request_id = self.code_action_request_id;
        let path = self.proxy_path();
        let position = self
            .code_actions_position
            .unwrap_or_else(|| self.cursor_position(cx));
        let source = self.editor.read(cx).value().to_string();
        let receiver = action
            .as_ref()
            .filter(|action| action.edit.is_none() && action.data.is_some())
            .map(|action| proxy.resolve_code_action(action.clone(), plugin_id));
        let title = action
            .as_ref()
            .map(|action| action.title.clone())
            .or_else(|| {
                standalone_command
                    .as_ref()
                    .map(|command| command.title.clone())
            })
            .unwrap_or_default();
        self.code_actions_visible = false;
        self.status = format!("Applying code action: {title}…").into();
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let action_result = if let Some(receiver) = receiver {
                cx.background_spawn(async move {
                    receiver
                        .recv_timeout(Duration::from_secs(5))
                        .map_err(|error| format!("Resolving code action: {error}"))?
                        .map_err(|error| error.message)
                })
                .await
                .map(Some)
            } else {
                Ok(action)
            };
            let command_proxy = proxy.clone();
            let command_path = path.clone();
            let validation_path = path.clone();
            let action_result = this.update_in(cx, move |this, window, cx| {
                if this.code_action_request_id != request_id
                    || this.request_generation != generation
                    || this.proxy_path() != validation_path
                    || this.cursor_position(cx) != position
                {
                    return None;
                }
                if this.shared_session_id.is_some() && !this.shared_editable {
                    this.status = "This shared file became read-only".into();
                    cx.notify();
                    return None;
                }
                if this.editor.read(cx).value().as_ref() != source.as_str() {
                    this.status = "Code action cancelled because the buffer changed".into();
                    cx.notify();
                    return None;
                }
                let outcome = action_result.and_then(|action| {
                    let command = action
                        .as_ref()
                        .and_then(|action| action.command.clone())
                        .or(standalone_command);
                    let (replacement, workspace_edit) = if let Some(edit) =
                        action.and_then(|action| action.edit)
                    {
                        let documents = workspace_edit_text_edits(&edit)?;
                        if documents.len() == 1
                            && documents[0].path.as_path()
                                == validation_path.as_path()
                            && documents[0].version.is_none()
                        {
                            let text = this.editor.read(cx).value().to_string();
                            let selection = this.editor.read(cx).selected_range();
                            let replacement = formatting_edit(
                                &text,
                                &documents[0].edits,
                                selection,
                            )?
                            .ok_or_else(|| {
                                "The code action made no change".to_owned()
                            })?;
                            (Some(replacement), None)
                        } else {
                            (None, Some(edit))
                        }
                    } else {
                        (None, None)
                    };
                    if replacement.is_none()
                        && workspace_edit.is_none()
                        && command.is_none()
                    {
                        return Err("The code action contains no edit or command".into());
                    }
                    Ok((replacement, workspace_edit, command))
                });

                let result = match outcome {
                    Ok((replacement, workspace_edit, command)) => {
                        this.code_actions.clear();
                        this.code_actions_position = None;
                        match workspace_edit {
                            Some(edit) => {
                                let (completion, receiver) =
                                    async_channel::bounded(1);
                                this.status = format!(
                                    "Applying workspace code action: {title}…"
                                )
                                .into();
                                cx.emit(WorkspaceEditRequest {
                                    edit,
                                    source_path: validation_path,
                                    purpose: WorkspaceEditPurpose::CodeAction {
                                        title,
                                    },
                                    generation: source_buffer_generation,
                                    completion: Some(completion),
                                });
                                Some((Some(receiver), command, None))
                            }
                            None => {
                                if let Some((range, text, selection)) =
                                    replacement
                                {
                                    this.suppress_completion_for_next_edit =
                                        true;
                                    this.preserve_code_action_request_for_next_edit =
                                        true;
                                    this.snippet_session = None;
                                    this.ghost_text = None;
                                    this.ghost_offset = None;
                                    this.editor.update(cx, |editor, cx| {
                                        editor.set_selected_range(range, cx);
                                        editor.replace(text, window, cx);
                                        editor.set_selected_range(selection, cx);
                                    });
                                }
                                match command {
                                    Some(command) => {
                                        let command_title =
                                            if command.title.is_empty() {
                                                command.command.clone()
                                            } else {
                                                command.title.clone()
                                            };
                                        this.status = format!(
                                            "Running language-server command: {command_title}…"
                                        )
                                        .into();
                                        Some((
                                            None,
                                            Some(command),
                                            Some(
                                                this.editor
                                                    .read(cx)
                                                    .value()
                                                    .to_string(),
                                            ),
                                        ))
                                    }
                                    None => {
                                        this.status = "Applied code action".into();
                                        None
                                    }
                                }
                            }
                        }
                    }
                    Err(error) => {
                        this.code_actions.clear();
                        this.code_actions_position = None;
                        this.status = format!("Could not apply code action: {error}").into();
                        None
                    }
                };
                cx.notify();
                result
            });
            let Some((completion, command, expected_text)) =
                action_result.log_err().flatten()
            else {
                return;
            };

            let command_and_text = if let Some(completion) = completion {
                let result = cx
                    .background_spawn(async move {
                        completion
                            .recv()
                            .await
                            .map_err(|_| "The workspace edit was canceled".to_owned())?
                    })
                    .await;
                this.update_in(cx, move |this, _, cx| {
                    if this.proxy_path() != command_path {
                        return None;
                    }
                    let applied_generation = match result {
                        Ok(generation) => generation,
                        Err(error) => {
                            if this.request_generation != generation
                                || this.code_action_request_id != request_id
                            {
                                return None;
                            }
                            this.status = format!(
                                "Could not apply code action: {error}"
                            )
                            .into();
                            cx.notify();
                            return None;
                        }
                    };
                    if this.buffer_generation() != applied_generation {
                        this.status = "Code action canceled because the source buffer changed".into();
                        cx.notify();
                        return None;
                    }
                    match command {
                        Some(command) => {
                            let command_title = if command.title.is_empty() {
                                command.command.clone()
                            } else {
                                command.title.clone()
                            };
                            this.status = format!(
                                "Running language-server command: {command_title}…"
                            )
                            .into();
                            Some((
                                command,
                                this.editor.read(cx).value().to_string(),
                            ))
                        }
                        None => {
                            this.status = "Applied code action".into();
                            cx.notify();
                            None
                        }
                    }
                })
            } else {
                Ok(command.zip(expected_text))
            };
            let Some((command, expected_text)) =
                command_and_text.log_err().flatten()
            else {
                return;
            };
            let receiver = command_proxy
                .execute_code_action_command(command, plugin_id);
            let mut reply = std::pin::pin!(receiver.recv());
            let mut deadline = std::pin::pin!(
                cx.background_executor().timer(Duration::from_secs(5))
            );
            let result = std::future::poll_fn(|cx| {
                use std::task::Poll;
                if let Poll::Ready(result) = reply.as_mut().poll(cx) {
                    return Poll::Ready(match result {
                        Ok(Ok(())) => Ok(()),
                        Ok(Err(error)) => Err(error.message),
                        Err(error) => Err(format!(
                            "Waiting for language-server command: {error}"
                        )),
                    });
                }
                if deadline.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Err(
                        "Waiting for language-server command: timed out".into(),
                    ));
                }
                Poll::Pending
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                if this.code_action_request_id != request_id
                    || this.proxy_path() != path
                    || this.editor.read(cx).value().as_ref()
                        != expected_text.as_str()
                {
                    return;
                }
                this.status = match result {
                    Ok(()) => "Applied code action".into(),
                    Err(error) => format!("Code action applied, but its server command failed: {error}").into(),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn request_navigation(
        &mut self,
        title: &'static str,
        path: std::path::PathBuf,
        position: lsp_types::Position,
        receiver: crossbeam_channel::Receiver<Vec<lsp_types::Location>>,
        cx: &mut Context<Self>,
    ) {
        let generation = self.request_generation;
        cx.spawn(async move |this, cx| {
            let locations = cx
                .background_spawn(async move { receiver.recv().unwrap_or_default() })
                .await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                if this.request_generation != generation
                    || this.proxy_path() != path
                    || this.cursor_position(cx) != position
                {
                    return;
                }
                if locations.is_empty() {
                    this.status = format!("No {title} from proxy").into();
                } else {
                    this.status =
                        format!("Found {} {title}", locations.len()).into();
                    this.navigation_results =
                        Some(NavigationResults { title, locations });
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_navigation_location(
        &mut self,
        location: lsp_types::Location,
        cx: &mut Context<Self>,
    ) {
        self.navigation_results = None;
        match definition_open_request(&location) {
            Ok(request) => cx.emit(request),
            Err(error) => self.status = error.into(),
        }
        cx.notify();
    }
}

#[cfg(test)]
fn completion_edit(
    text: &str,
    position: lsp_types::Position,
    item: &lsp_types::CompletionItem,
) -> Result<
    (
        std::ops::Range<usize>,
        String,
        usize,
        Option<Vec<SnippetTabstop>>,
    ),
    String,
> {
    completion_edit_with_variables(
        text,
        position,
        item,
        &lsp_snippet::Variables::default(),
    )
}

fn completion_edit_with_variables(
    text: &str,
    position: lsp_types::Position,
    item: &lsp_types::CompletionItem,
    variables: &lsp_snippet::Variables,
) -> Result<
    (
        std::ops::Range<usize>,
        String,
        usize,
        Option<Vec<SnippetTabstop>>,
    ),
    String,
> {
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
    let snippet = (item.insert_text_format
        == Some(lsp_types::InsertTextFormat::SNIPPET))
    .then(|| lsp_snippet::Snippet::parse_with_variables(new_text, variables))
    .transpose()
    .map_err(|error| format!("The server returned an invalid snippet: {error}"))?;
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
    let mut snippet_tabstops = None;
    for (range, new_text, primary) in edits {
        replacement.push_str(&text[offset..range.start]);
        if primary {
            if let Some(snippet) = &snippet {
                let snippet_offset = replacement.len();
                let mut tabstops = snippet.tabstops.clone();
                for tabstop in &mut tabstops {
                    for range in &mut tabstop.ranges {
                        range.start += snippet_offset;
                        range.end += snippet_offset;
                    }
                }
                snippet_tabstops = Some(tabstops);
                replacement.push_str(&snippet.text);
            } else {
                replacement.push_str(new_text);
            }
            caret = start + replacement.len();
        } else {
            replacement.push_str(new_text);
        }
        offset = range.end;
    }
    if let Some(final_tabstop) = snippet_tabstops
        .as_ref()
        .and_then(|tabstops| tabstops.iter().find(|tabstop| tabstop.index == 0))
        .and_then(|tabstop| tabstop.ranges.first())
    {
        caret = start + final_tabstop.start;
    }
    // gpui-kit's public edit API has no transaction grouping. One bounded
    // replacement keeps the symbol and its imports in a single Undo operation.
    Ok((start..end, replacement, caret, snippet_tabstops))
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
    ranked
        .into_iter()
        .take(MAX_COMPLETION_SUGGESTIONS)
        .map(|(_, item)| item)
        .collect()
}

fn caret_popup_position(
    caret_top: Pixels,
    line_height: Pixels,
    preferred_max_height: Pixels,
) -> (Pixels, Pixels) {
    let gap = px(8.);
    let available_above = caret_top - gap;
    if available_above >= line_height {
        let max_height = if available_above < preferred_max_height {
            available_above
        } else {
            preferred_max_height
        };
        (caret_top - max_height - gap, max_height)
    } else {
        (caret_top + line_height + gap, preferred_max_height)
    }
}

fn caret_popup_horizontal_position(
    caret_left: Pixels,
    available_width: Pixels,
    preferred_width: Pixels,
) -> (Pixels, Pixels) {
    let available_width = available_width.as_f32().max(0.);
    let width = preferred_width.as_f32().max(0.).min(available_width);
    let left = caret_left
        .as_f32()
        .max(0.)
        .min((available_width - width).max(0.));
    (px(left), px(width))
}

fn completion_item_detail(item: &lsp_types::CompletionItem) -> Option<String> {
    let mut details = Vec::with_capacity(4);
    for detail in [
        item.detail.as_deref(),
        completion_documentation_signature(item),
        item.label_details
            .as_ref()
            .and_then(|details| details.detail.as_deref()),
        item.label_details
            .as_ref()
            .and_then(|details| details.description.as_deref()),
    ] {
        let Some(detail) = detail.map(str::trim).filter(|detail| !detail.is_empty())
        else {
            continue;
        };
        if !details.contains(&detail) {
            details.push(detail);
        }
    }
    (!details.is_empty()).then(|| details.join(" · "))
}

fn completion_documentation_signature(
    item: &lsp_types::CompletionItem,
) -> Option<&str> {
    let documentation = item.documentation.as_ref()?;
    let text = match documentation {
        lsp_types::Documentation::String(text) => text.as_str(),
        lsp_types::Documentation::MarkupContent(markup) => markup.value.as_str(),
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("```"))
        .find(|line| {
            line.contains('(')
                && line.contains(')')
                && (line.contains("->") || line.contains(':'))
        })
}

fn has_unclosed_parenthesis_before_cursor(
    text: &str,
    position: lsp_types::Position,
) -> bool {
    let Some(offset) = caret_byte_offset(text, position) else {
        return false;
    };
    let mut depth = 0usize;
    for character in text[..offset].chars() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth > 0
}

fn default_editor_tab_size() -> TabSize {
    TabSize {
        tab_size: 4,
        hard_tabs: false,
    }
}

fn configured_editor_language_settings(
    workspace_root: &str,
    path: &str,
) -> Result<ahead_core::config::LanguageEditorSettings, String> {
    if workspace_root.is_empty() {
        return Ok(ahead_core::config::LanguageEditorSettings::default());
    }
    let user_home = directories::BaseDirs::new()
        .map(|directories| directories.home_dir().to_path_buf());
    let language = editor_language(path).to_string();
    let settings = ahead_core::config::language_editor_settings(
        std::path::Path::new(workspace_root),
        user_home.as_deref(),
        Some(&language),
    )
    .map_err(|error| error.to_string())?;
    Ok(settings)
}

fn line_start_offset(text: &str, offset: usize) -> Option<usize> {
    let prefix = text.get(..offset)?;
    Some(prefix.rfind('\n').map_or(0, |newline| newline + 1))
}

fn monospace_probe_range(
    text: &str,
    line_start: usize,
) -> Option<std::ops::Range<usize>> {
    let line = text.get(line_start..)?.split('\n').next()?;
    line.char_indices()
        .find(|(_, character)| character.is_ascii_graphic() || *character == ' ')
        .map(|(offset, character)| {
            let start = line_start + offset;
            start..start + character.len_utf8()
        })
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

fn git_hunk_preview_text(hunk: &DiffHunk, new_lines: &[String]) -> String {
    let mut diff = String::new();
    for line in &hunk.old_lines {
        diff.push_str("- ");
        diff.push_str(line);
        diff.push('\n');
    }
    for line in new_lines {
        diff.push_str("+ ");
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

fn git_hunk_projection(hunk: &DiffHunk, source_text: &str) -> GitHunkProjection {
    let has_trailing_newline = source_text.ends_with('\n');
    let mut source_lines = source_text
        .split('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let source_line_count = source_lines
        .len()
        .saturating_sub(if has_trailing_newline { 1 } else { 0 });
    source_lines.truncate(source_line_count);

    let insertion_row = (hunk.start as usize)
        .saturating_sub(1)
        .min(source_line_count);
    let added_end = insertion_row
        .saturating_add(hunk.len as usize)
        .min(source_line_count);
    let mut rows = Vec::with_capacity(source_line_count + hunk.old_lines.len());
    for (index, line) in source_lines.into_iter().enumerate() {
        if index == insertion_row {
            rows.extend(hunk.old_lines.iter().map(|line| {
                (format!("- {line}"), Some(GitProjectionRowKind::Deleted))
            }));
        }
        let kind = (insertion_row..added_end)
            .contains(&index)
            .then_some(GitProjectionRowKind::Added);
        rows.push((line, kind));
    }
    if insertion_row == source_line_count {
        rows.extend(
            hunk.old_lines.iter().map(|line| {
                (format!("- {line}"), Some(GitProjectionRowKind::Deleted))
            }),
        );
    }

    let mut text = String::new();
    let mut deleted_ranges = Vec::new();
    let mut added_ranges = Vec::new();
    let row_count = rows.len();
    for (index, (line, kind)) in rows.into_iter().enumerate() {
        let start = text.len();
        text.push_str(&line);
        let has_newline = index + 1 < row_count || has_trailing_newline;
        if has_newline {
            text.push('\n');
        }
        let range = start..text.len();
        if range.start < range.end {
            match kind {
                Some(GitProjectionRowKind::Deleted) => deleted_ranges.push(range),
                Some(GitProjectionRowKind::Added) => added_ranges.push(range),
                None => {}
            }
        }
    }

    GitHunkProjection {
        text,
        deleted_ranges,
        added_ranges,
        inserted_rows: hunk.old_lines.len(),
    }
}

#[derive(Clone, Copy)]
enum GitProjectionRowKind {
    Deleted,
    Added,
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

fn navigation_location_label(location: &lsp_types::Location) -> String {
    let path = location
        .uri
        .to_file_path()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| location.uri.to_string());
    format!("{path}:{}", location.range.start.line + 1)
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

fn display_position_utf16(
    text: &str,
    position: gpui_kit::base::input::Position,
) -> ahead_rpc::ahead::DisplayPosition {
    ahead_rpc::ahead::DisplayPosition {
        line: position.line,
        col: text
            .split('\n')
            .nth(position.line as usize)
            .unwrap_or_default()
            .chars()
            .take(position.character as usize)
            .map(|character| character.len_utf16() as u32)
            .sum(),
    }
}

pub(crate) fn current_comment_range(
    comment: &ahead_rpc::ahead::CodeComment,
    text: &str,
) -> Option<ahead_rpc::ahead::DisplayRange> {
    use ahead_rpc::ahead::DisplayRange;
    if format!("{:x}", sha2::Sha256::digest(text.as_bytes()))
        == comment.source_sha256
    {
        return Some(comment.range);
    }
    let (start, end) = unique_quote_range(text, &comment.quote).ok()?;
    let rope = Rope::from(text);
    let start = rope.offset_to_position(start);
    let end = rope.offset_to_position(end);
    Some(DisplayRange {
        start: display_position_utf16(text, start),
        end: display_position_utf16(text, end),
    })
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
        cx: &mut Context<Self>,
    ) {
        let group_id = group.entity_id().as_u64();
        self.tab_group = Some(group);
        cx.emit(CodePanelGroupChanged { group_id });
    }

    fn set_active(&mut self, active: bool, _: &mut Window, cx: &mut Context<Self>) {
        if active {
            cx.emit(CodePanelActivated);
        }
    }
}

impl Panel for CodePanel {
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let file_name = std::path::Path::new(&self.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled")
            .to_owned();
        let dirty = self.dirty;
        let group = self.tab_group.clone();
        let panel_id = self.panel_id;
        let close_handler = self.close_handler.clone();
        let tab_handler = self.tab_handler.clone();
        let double_tab_handler = tab_handler.clone();
        let button_tab_handler = tab_handler.clone();
        let file_tab_handler = tab_handler.clone();
        let title = SharedString::from(file_name.to_string());
        let tab_label = h_flex()
            .min_w_0()
            .flex_nowrap()
            .items_center()
            .gap_1()
            .when(self.is_preview, |this| this.italic())
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .debug_selector(|| "code-tab-filename".into())
                    .child(title),
            )
            .when(dirty, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .debug_selector(|| "code-tab-dirty-marker".into())
                        .text_color(cx.theme().warning)
                        .child("•"),
                )
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
                        "Move to Other Editor Group",
                        CodeTabAction::MoveToOtherEditorGroup,
                    ))
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
impl EventEmitter<CodePanelActivated> for CodePanel {}
impl EventEmitter<CodePanelGroupChanged> for CodePanel {}
impl EventEmitter<crate::ross::OpenRequest> for CodePanel {}
impl EventEmitter<WorkspaceEditRequest> for CodePanel {}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CodePanelActivated;

#[derive(Clone, Copy, Debug)]
pub(crate) struct CodePanelGroupChanged {
    pub group_id: u64,
}

impl Focusable for CodePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for CodePanel {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if let Some(error) = &self.file_error {
            return v_flex()
                .id("file-open-error")
                .debug_selector(|| "file-open-error".into())
                .size_full()
                .bg(cx.theme().background)
                .track_focus(&self.focus)
                .child(
                    Empty::new().header(
                        EmptyHeader::new()
                            .title(
                                EmptyTitle::new()
                                    .child("File cannot be opened as text"),
                            )
                            .description(
                                EmptyDescription::new().child(error.to_string()),
                            ),
                    ),
                )
                .into_any_element();
        }
        self.sync_proxy_diagnostics(cx);
        let dirty = self.dirty;
        let border_color = cx.theme().border;
        let text_color = cx.theme().sidebar_foreground;
        let bar_bg = cx.theme().tab_bar;
        let editor_bg = cx.theme().background;
        let breakpoint_style =
            ButtonCustomVariant::new(cx).hover(cx.theme().danger.opacity(0.22));
        let comment_accent = cx.theme().magenta;
        let comment_marker_style = ButtonCustomVariant::new(cx)
            .color(comment_accent)
            .foreground(cx.theme().sidebar_foreground)
            .hover(comment_accent)
            .active(comment_accent.opacity(0.82));
        let popup_bg = cx.theme().popover;
        let active_sel_bg = popup_bg.blend(cx.theme().list_active);
        let file_name = std::path::Path::new(&self.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled")
            .to_owned();

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
        let actor_lines = self
            .proxy
            .as_ref()
            .map(|proxy| agent_anchor_lines(&proxy.anchors_for(&self.proxy_path())))
            .unwrap_or_default();
        let (
            line_count,
            line_height,
            gutter_rows,
            scroll_offset,
            editor_text,
            selected_range,
        ) = {
            let editor_state = self.editor.read(cx);
            let line_count = editor_state.text().lines_len().max(1);
            let line_height = editor_state.line_height().unwrap_or(px(16.));
            let initial_visible_rows = (f32::from(window.bounds().size.height)
                / f32::from(line_height))
            .ceil()
            .max(1.) as usize;
            let gutter_rows = visible_gutter_rows(
                editor_state.visible_row_range(),
                line_count,
                initial_visible_rows,
            );
            (
                line_count,
                line_height,
                gutter_rows,
                editor_state.scroll_offset(),
                editor_state.text().to_string(),
                editor_state.selected_range(),
            )
        };
        let comment_path = std::path::Path::new(&self.file_path)
            .strip_prefix(&self.workspace)
            .unwrap_or_else(|_| std::path::Path::new(&self.file_path))
            .to_string_lossy();
        let mut comment_starts = std::collections::HashMap::<u32, usize>::new();
        let mut comment_ranges = std::collections::HashSet::<u32>::new();
        let mut comment_ends = std::collections::HashSet::<u32>::new();
        let located_comments: Vec<_> = self
            .code_comments
            .iter()
            .filter(|comment| {
                self.comment_session_id.as_deref()
                    == Some(comment.session_id.as_str())
                    && comment.resolved_at.is_none()
                    && comment.path == comment_path
            })
            .filter_map(|comment| {
                current_comment_range(comment, &editor_text)
                    .map(|range| (comment, range))
            })
            .collect();
        for (_, range) in &located_comments {
            let start = range.start.line.saturating_add(1);
            *comment_starts.entry(start).or_default() += 1;
            let end = range.end.line.saturating_add(
                if range.end.col == 0 && range.end.line > range.start.line {
                    0
                } else {
                    1
                },
            );
            for line in start..=end.min(line_count as u32) {
                comment_ranges.insert(line);
            }
            comment_ends.insert(end);
        }
        let selected_comment_target = self
            .expanded_hunk_start
            .is_none()
            .then(|| self.selected_code_comment(cx))
            .flatten();
        let visible_comments = self.comment_popover_line.map(|line| {
            let comments = located_comments
                .iter()
                .filter(|(_, range)| range.start.line.saturating_add(1) == line)
                .map(|(comment, range)| ((*comment).clone(), *range))
                .collect::<Vec<_>>();
            (line, comments)
        });
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
        let inline_blame_panel = cx.entity();
        let presentation_overlay = self.presentation.clone().map(|presentation| {
            let line = presentation_line.unwrap_or(1) as f32;
            let top = scroll_offset.y + line_height * line + px(4.);
            (presentation, top)
        });
        let pointer_overlay = self
            .pointer_overlay_position
            .map(|(left, top)| (px(left), px(top)));
        let gutter_top = scroll_offset.y + px(8.);
        let gutter_row_top = gutter_top + line_height * gutter_rows.start as f32;
        let expanded_hunk = self
            .expanded_hunk_start
            .and_then(|start| hunks.iter().find(|hunk| hunk.start == start))
            .cloned();
        if let Some(hunk) = expanded_hunk.as_ref() {
            self.ensure_git_diff_preview(hunk, &editor_text, window, cx);
        } else {
            self.git_diff_preview = None;
        }
        let hunk_lines = &self.git_gutter_markers.hunk_lines;
        let deleted_lines = &self.git_gutter_markers.deleted_lines;
        let rendered_editor = self
            .git_diff_preview
            .as_ref()
            .map(|preview| preview.editor.clone())
            .unwrap_or_else(|| self.editor.clone());
        let ruler_editor = rendered_editor.clone();
        let ruler_column = self.preferred_line_length;
        let ruler_color = cx.theme().muted_foreground.opacity(0.35);
        let ruler_fallback_advance = cx.theme().mono_font_size * 0.6;
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
        let snippet_has_active_selection = self
            .snippet_session
            .as_ref()
            .is_some_and(|session| session.active_contains(&selected_range));
        let snippet_choice_options =
            self.snippet_session.as_ref().and_then(|session| {
                if !session.active_contains(&selected_range) {
                    return None;
                }
                let choices = session.active_choices();
                (choices.len() > 1)
                    .then(|| (choices.to_vec(), session.active_choice_index()))
            });
        let signature_help = self
            .signature_help
            .as_ref()
            .filter(|_| self.signature_help_position == Some(cursor_position))
            .and_then(signature_help_view);
        let show_cursor_popup_anchor = self.show_completions
            || signature_help.is_some()
            || self.code_actions_visible
            || self.ghost_text.is_some()
            || snippet_choice_options.is_some();
        let caret_top = scroll_offset.y + line_height * cursor_position.line as f32;
        let completion_anchor_top = self.completion_caret_top.unwrap_or(caret_top);
        let completion_top = completion_anchor_top + line_height + px(8.);
        let (completion_menu_top, completion_menu_max_height) =
            caret_popup_position(completion_anchor_top, line_height, px(320.));
        let (signature_help_top, signature_help_max_height) =
            caret_popup_position(completion_anchor_top, line_height, px(150.));
        // The first frame uses an estimate; the canvas replaces it with the measured glyph x.
        let fallback_completion_left =
            px(42. + cursor_position.character as f32 * 7.2);
        let completion_anchor_left = self
            .completion_caret_left
            .unwrap_or(fallback_completion_left);
        let completion_available_width = self
            .completion_available_width
            .unwrap_or_else(|| px(completion_anchor_left.as_f32() + 480.));
        let (signature_help_left, signature_help_width) =
            caret_popup_horizontal_position(
                completion_anchor_left,
                completion_available_width,
                px(520.),
            );
        let (code_actions_left, code_actions_width) =
            caret_popup_horizontal_position(
                completion_anchor_left,
                completion_available_width,
                px(400.),
            );
        let (ghost_text_left, ghost_text_width) = caret_popup_horizontal_position(
            completion_anchor_left,
            completion_available_width,
            px(520.),
        );
        let (completion_left, completion_width) = caret_popup_horizontal_position(
            completion_anchor_left,
            completion_available_width,
            px(480.),
        );
        let (snippet_choices_left, snippet_choices_width) =
            caret_popup_horizontal_position(
                completion_anchor_left,
                completion_available_width,
                px(260.),
            );
        let completion_editor = self.editor.clone();
        let completion_panel = code_panel.clone();
        let inline_blame = self
            .inline_blame_enabled
            .then(|| {
                self.git_state.blame.iter().find(|hunk| {
                    (hunk.start..hunk.start.saturating_add(hunk.len))
                        .contains(&(self.active_line as usize))
                })
            })
            .flatten();
        let inline_blame_range = inline_blame
            .and_then(|_| line_end_offset(&editor_text, self.active_line))
            .map(|offset| offset..offset);
        let inline_blame_text = inline_blame.map(inline_blame_label);
        let inline_blame_details = inline_blame.map(blame_label);
        let tab_action_context = if snippet_has_active_selection
            || self.show_completions
            || self.ghost_text.is_some()
        {
            "AHEADCodeEditor tab_action=true"
        } else {
            "AHEADCodeEditor"
        };
        let inline_blame_position = self
            .inline_blame_position
            .filter(|(line, _, _)| *line == self.active_line);
        let inline_blame_top = (gutter_top
            + line_height * self.active_line.saturating_sub(1) as f32)
            .as_f32();
        let hover_text = self.hover_text.clone();
        let code_actions = self.code_actions.clone();
        let selected_code_action = self.selected_code_action;
        let caret_offset = selected_range.end;
        let active_merge_conflict = self
            .merge_conflicts
            .iter()
            .find(|conflict| {
                conflict.range.start <= caret_offset
                    && caret_offset <= conflict.range.end
            })
            .cloned()
            .filter(|_| self.shared_conflict.is_none());
        let merge_conflict_read_only =
            self.shared_session_id.is_some() && !self.shared_editable;
        let rename_label = self
            .pending_rename
            .as_ref()
            .map(|pending| format!("Rename {}", pending.old_name));
        let rename_in_progress = self.rename_in_progress;
        v_flex()
            .size_full()
            .bg(editor_bg)
            .track_focus(&self.focus)
            .capture_action(cx.listener(|this: &mut Self, action: &Enter, window, cx| {
                if this.pending_rename.is_some() && !action.secondary && !action.shift {
                    this.confirm_symbol_rename(window, cx);
                    cx.stop_propagation();
                } else if this.code_actions_visible && !action.secondary && !action.shift {
                    this.apply_selected_code_action(window, cx);
                    cx.stop_propagation();
                } else if this.show_completions && !action.secondary && !action.shift {
                    this.accept_completion(window, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &NextSnippetTabstop, window, cx| {
                if this.show_completions {
                    this.accept_completion(window, cx);
                } else if this.snippet_session.is_some() {
                    this.move_snippet_tabstop(true, cx);
                } else if this.ghost_text.is_some() {
                    this.accept_ghost_text(window, cx);
                }
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &PreviousSnippetTabstop, _, cx| {
                if this.snippet_session.is_some() {
                    this.move_snippet_tabstop(false, cx);
                }
                cx.stop_propagation();
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
            .capture_action(cx.listener(|this: &mut Self, _: &MoveDown, window, cx| {
                if this.code_actions_visible {
                    this.selected_code_action = (this.selected_code_action + 1)
                        .min(this.code_actions.len().saturating_sub(1));
                    cx.notify();
                    cx.stop_propagation();
                } else if this.show_completions {
                    this.select_completion(
                        this.selected_completion.saturating_add(1),
                        cx,
                    );
                    cx.stop_propagation();
                } else if this
                    .snippet_session
                    .as_ref()
                    .is_some_and(|session| session.active_choices().len() > 1)
                {
                    this.cycle_snippet_choice(true, window, cx);
                    cx.stop_propagation();
                } else if this.cycle_signature_help(true, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &MoveUp, window, cx| {
                if this.code_actions_visible {
                    this.selected_code_action =
                        this.selected_code_action.saturating_sub(1);
                    cx.notify();
                    cx.stop_propagation();
                } else if this.show_completions {
                    this.select_completion(
                        this.selected_completion.saturating_sub(1),
                        cx,
                    );
                    cx.stop_propagation();
                } else if this
                    .snippet_session
                    .as_ref()
                    .is_some_and(|session| session.active_choices().len() > 1)
                {
                    this.cycle_snippet_choice(false, window, cx);
                    cx.stop_propagation();
                } else if this.cycle_signature_help(false, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &Escape, _, cx| {
                if this.pending_rename.is_some() {
                    this.cancel_symbol_rename(cx);
                    cx.stop_propagation();
                } else if this.code_actions_visible
                    || this.show_completions
                    || this.ghost_text.is_some()
                    || this.signature_help.is_some()
                    || this.snippet_session.is_some()
                {
                    this.code_actions_visible = false;
                    this.show_completions = false;
                    this.ghost_text = None;
                    this.ghost_offset = None;
                    this.signature_help = None;
                    this.snippet_session = None;
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &SaveFile, window, cx| {
                this.save(window, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &FormatDocument, window, cx| {
                this.format_document(window, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &ShowCompletions, _, cx| {
                this.request_proxy_completions(cx, false);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &ShowSignatureHelp, _, cx| {
                this.request_signature_help(cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &ShowCodeActions, _, cx| {
                this.request_code_actions(cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &GoToDefinition, _, cx| {
                this.go_to_definition(cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &FindReferences, _, cx| {
                this.find_references(cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &GoToImplementation, _, cx| {
                this.go_to_implementation(cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this: &mut Self, _: &RenameSymbol, window, cx| {
                this.prepare_symbol_rename(window, cx);
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
                            .min_w_0()
                            .flex_1()
                            .child(IconName::FileCode)
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
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
                            .flex_shrink_0()
                        .when(
                            self.comment_session_id.is_some() && self.proxy.is_some(),
                            |actions| {
                                actions.child(
                                    Button::new("add-comment-selection")
                                        .debug_selector(|| {
                                            "add-comment-selection".into()
                                        })
                                        .primary()
                                        .icon(IconName::MessageSquarePlus)
                                        .label("Add comment")
                                        .accessibility_label(
                                            "Add comment to selected code",
                                        )
                                        .tooltip(if selected_comment_target.is_none() {
                                            "Select code to add a comment"
                                        } else {
                                            "Add comment to selected code"
                                        })
                                        .on_click(cx.listener({
                                            let target =
                                                selected_comment_target.clone();
                                            move |this: &mut Self, _, window, cx| {
                                                if let Some(target) = target.clone() {
                                                    let session_id = this
                                                        .comment_session_id
                                                        .clone();
                                                    this.begin_code_comment(
                                                        target,
                                                        session_id,
                                                        window,
                                                        cx,
                                                    );
                                                } else {
                                                    this.status =
                                                        "Select code to add a comment"
                                                            .into();
                                                    cx.notify();
                                                }
                                            }
                                        })),
                                )
                            },
                        )
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
            .when_some(active_merge_conflict, |root, conflict| {
                root.child(
                    h_flex()
                        .px_3()
                        .py_1()
                        .gap_2()
                        .items_center()
                        .border_b_1()
                        .border_color(border_color)
                        .bg(popup_bg)
                        .child(IconName::GitBranch)
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(11.))
                                .child(format!("Merge conflict · line {}", conflict.line)),
                        )
                        .child(
                            Button::new("resolve-merge-current")
                                .label("Use Current")
                                .disabled(merge_conflict_read_only)
                                .tooltip("Keep the current side and remove the conflict markers")
                                .on_click(cx.listener({
                                    let range = conflict.range.clone();
                                    move |this: &mut Self, _, window, cx| {
                                        this.resolve_merge_conflict(
                                            range.clone(),
                                            MergeConflictChoice::Current,
                                            window,
                                            cx,
                                        );
                                    }
                                })),
                        )
                        .child(
                            Button::new("resolve-merge-incoming")
                                .label("Use Incoming")
                                .disabled(merge_conflict_read_only)
                                .tooltip("Keep the incoming side and remove the conflict markers")
                                .on_click(cx.listener({
                                    let range = conflict.range.clone();
                                    move |this: &mut Self, _, window, cx| {
                                        this.resolve_merge_conflict(
                                            range.clone(),
                                            MergeConflictChoice::Incoming,
                                            window,
                                            cx,
                                        );
                                    }
                                })),
                        )
                        .child(
                            Button::new("resolve-merge-both")
                                .label("Use Both")
                                .disabled(merge_conflict_read_only)
                                .tooltip("Keep both sides in order and remove the conflict markers")
                                .on_click(cx.listener({
                                    let range = conflict.range.clone();
                                    move |this: &mut Self, _, window, cx| {
                                        this.resolve_merge_conflict(
                                            range.clone(),
                                            MergeConflictChoice::Both,
                                            window,
                                            cx,
                                        );
                                    }
                                })),
                        ),
                )
            })
            .when_some(rename_label, |root, label| root.child(
                v_flex()
                    .px_3()
                    .py_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(border_color)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_size(px(11.)).child(label))
                            .child(
                                Input::new(&self.rename_input)
                                    .aria_label("New symbol name")
                                    .flex_1(),
                            )
                            .child(
                                Button::new("confirm_symbol_rename")
                                    .label(if rename_in_progress { "Renaming…" } else { "Rename" })
                                    .tooltip("Apply workspace rename (Enter)")
                                    .disabled(rename_in_progress)
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        this.confirm_symbol_rename(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("cancel_symbol_rename")
                                    .label("Cancel")
                                    .tooltip("Cancel rename (Esc)")
                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                        this.cancel_symbol_rename(cx);
                                    })),
                            ),
                    )
                    .when(self.status.starts_with("Rename failed:"), |row| {
                        row.child(
                            div()
                                .text_size(px(10.))
                                .text_color(cx.theme().danger)
                                .child(self.status.clone()),
                        )
                    }),
            ))
            .when_some(self.shared_conflict.clone(), |root, snapshot| root.child(
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border_color)
                    .child(IconName::ShieldAlert)
                    .child(div().flex_1().text_size(px(11.)).child(format!(
                        "Collaborator revision {} differs from this editor. Choose a version.",
                        snapshot.revision
                    )))
                    .child(Button::new("use_shared_latest")
                        .label("Use latest")
                        .tooltip("Discard local edits and use the collaborator version")
                        .disabled(self.shared_conflict_resolving)
                        .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                            this.resolve_shared_conflict(false, window, cx);
                        })))
                    .child(Button::new("keep_shared_local")
                        .label("Keep mine")
                        .tooltip("Overwrite the collaborator version with local edits")
                        .disabled(self.shared_conflict_resolving || (self.shared_session_id.is_some() && !self.shared_editable))
                        .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                            this.resolve_shared_conflict(true, window, cx);
                        })))
            ))
            .when(self.shared_session_id.is_some() && self.dirty && self.shared_conflict.is_none()
                && (!self.shared_editable || self.status.starts_with("Shared edit failed:")), |root| root.child(
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border_color)
                    .child(IconName::ShieldAlert)
                    .child(div().text_size(px(11.)).child(self.status.clone()))
            ))
            .when_some(expanded_hunk.clone(), |root, hunk| {
                root.when(self.git_diff_preview.is_some(), |root| {
                    let new_lines = editor_text
                        .lines()
                        .skip(hunk.start.saturating_sub(1) as usize)
                        .take(hunk.len as usize)
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    let diff_text = git_hunk_preview_text(&hunk, &new_lines);
                    root.child(
                        h_flex()
                            .id("expanded_git_hunk")
                            .debug_selector(|| "expanded-git-hunk".into())
                            .justify_between()
                            .items_center()
                            .px_2()
                            .py_1()
                            .border_b_1()
                            .border_color(border_color)
                            .bg(popup_bg)
                            .child(format!("Uncommitted change · line {}", hunk.start))
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        Button::new("copy_git_hunk")
                                            .debug_selector(|| "copy-git-hunk".into())
                                            .icon(IconName::Copy)
                                            .ghost()
                                            .tooltip("Copy change diff")
                                            .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(diff_text.clone()));
                                                this.status = "Copied change diff".into();
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("close_git_hunk")
                                            .debug_selector(|| "close-git-hunk".into())
                                            .icon(IconName::X)
                                            .ghost()
                                            .tooltip("Close change preview")
                                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                                this.close_git_diff_preview(cx);
                                            })),
                                    ),
                            ),
                    )
                })
            })
            // Editor row: native gutter rail (diff + breakpoints) beside editor
            .when(!self.show_markdown_preview, |root| root.child(
                h_flex()
                    .flex_1()
                    .h_full()
                    .min_h_0()
                    .child(
                        div()
                            .debug_selector(|| "editor-gutter".into())
                            .w(px(EDITOR_GUTTER_WIDTH))
                            .h_full()
                            .relative()
                            .overflow_hidden()
                            .bg(bar_bg)
                            .border_r_1()
                            .border_color(border_color)
                            .when(self.git_diff_preview.is_some(), |gutter| {
                                gutter.w(px(0.)).border_r_0()
                            })
                            .child(
                                v_flex()
                                    .absolute()
                                    .left(px(0.))
                                    .right(px(0.))
                                    .top(gutter_row_top)
                                    .children(gutter_rows.map(|row| {
                                        let ln = row as u32 + 1;
                                        let hunk = hunk_lines.get(&ln).copied();
                                        let is_deleted = deleted_lines.contains(&ln);
                                        let is_bp = breakpoints.contains(&ln);
                                        let is_agent = actor_lines.contains(&ln);
                                        let is_pointer = pointer_line == Some(ln);
                                        let comment_count = comment_starts.get(&ln).copied().unwrap_or(0);
                                        let has_comment_range = comment_ranges.contains(&ln);
                                        let is_comment_end = comment_ends.contains(&ln);
                                        let has_comment_action = comment_count > 0;
                                        let add_comment_target = selected_comment_target
                                            .clone()
                                            .filter(|target| {
                                                target.range.start.line + 1 == ln
                                            })
                                            .filter(|_| {
                                                self.comment_session_id.is_some()
                                                    && self.proxy.is_some()
                                            });
                                        let has_add_comment_action =
                                            add_comment_target.is_some();
                                        let comment_session_id = self.comment_session_id.clone();
                                        let show_breakpoint_preview =
                                            self.hovered_breakpoint_line == Some(ln)
                                                && !is_bp
                                                && comment_count == 0;
                                        h_flex()
                                            .id(("gutter", ln as usize))
                                            .h(line_height)
                                            .w(px(EDITOR_GUTTER_WIDTH))
                                            .flex_shrink_0()
                                            .items_center()
                                            .gap(px(0.))
                                            .child(
                                                div()
                                                    .id(("git-change", ln as usize))
                                                    .debug_selector(move || format!("git-change-{ln}"))
                                                    .w(px(GIT_GUTTER_TRACK_WIDTH))
                                                    .flex_shrink_0()
                                                    .h(line_height)
                                                    .flex()
                                                    .items_center()
                                                    .relative()
                                                    .child(
                                                        div()
                                                            .w(px(GIT_GUTTER_TRACK_WIDTH))
                                                            .h(if is_deleted && hunk.is_none() { px(3.) } else { line_height })
                                                            .bg(match hunk.map(|(_, kind)| kind) {
                                                                Some(DiffHunkKind::Added) => cx.theme().success,
                                                                Some(DiffHunkKind::Modified) => cx.theme().warning,
                                                                _ if is_deleted => cx.theme().danger,
                                                                _ => gpui_kit::Hsla::transparent_black(),
                                                            })
                                                    )
                                                    .when(is_agent, |track| track.child(
                                                        div()
                                                            .absolute()
                                                            .top(px(0.))
                                                            .left(px(GIT_GUTTER_TRACK_WIDTH - 1.))
                                                            .w(px(1.))
                                                            .h(line_height)
                                                            .bg(cx.theme().magenta)
                                                    ))
                                                    .when(is_pointer, |track| track.child(
                                                        div()
                                                            .absolute()
                                                            .top(px(0.))
                                                            .left(px(0.))
                                                            .w(px(1.))
                                                            .h(line_height)
                                                            .bg(cx.theme().primary)
                                                    ))
                                                    .when(hunk.is_some() || is_deleted, |el| {
                                                        el.cursor(CursorStyle::PointingHand)
                                                    })
                                                    .when_some(hunk.map(|(start, _)| start).or_else(|| is_deleted.then_some(ln)), |el, start| {
                                                        el.on_click(cx.listener(move |this: &mut Self, _, window, cx| {
                                                            if this.expanded_hunk_start == Some(start) {
                                                                this.close_git_diff_preview(cx);
                                                            } else if let Some(hunk) = this.git_state.hunks.iter().find(|hunk| hunk.start == start).cloned() {
                                                                this.expanded_hunk_start = Some(start);
                                                                let source_text = this.editor.read(cx).text().to_string();
                                                                this.ensure_git_diff_preview(&hunk, &source_text, window, cx);
                                                                cx.notify();
                                                            }
                                                        }))
                                                    })
                                            )
                                            .child(
                                                div()
                                                    .id(("breakpoint-hover", ln as usize))
                                                    .debug_selector(move || format!("gutter-actions-{ln}"))
                                                    .w(px(GUTTER_ACTION_WIDTH))
                                                    .flex_shrink_0()
                                                    .h(line_height)
                                                    .relative()
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
                                                    .when(has_comment_range, |slot| slot.child(
                                                        div()
                                                            .absolute()
                                                            .left(px(0.))
                                                            .top(px(0.))
                                                            .w(px(2.))
                                                            .h(line_height)
                                                            .bg(comment_accent)
                                                    ))
                                                    .when(!has_comment_action && !has_add_comment_action, |slot| slot.child(Button::new(("breakpoint", ln as usize))
                                                    .debug_selector(move || format!("breakpoint-target-{ln}"))
                                                    .with_size(px(GUTTER_ACTION_WIDTH - 2.))
                                                    .w(px(GUTTER_ACTION_WIDTH - 2.))
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
                                                    }))))
                                                    .when_some(add_comment_target, |slot, target| slot.child(
                                                        Button::new(("add-comment-marker", ln as usize))
                                                            .debug_selector(move || format!("add-comment-marker-{ln}"))
                                                            .ghost()
                                                            .with_size(px(GUTTER_ACTION_WIDTH))
                                                            .w(px(GUTTER_ACTION_WIDTH))
                                                            .h(line_height)
                                                            .p_0()
                                                            .rounded(px(3.))
                                                            .cursor(CursorStyle::PointingHand)
                                                            .child(
                                                                div()
                                                                    .size(px(GUTTER_ACTION_WIDTH))
                                                                    .rounded(px(4.))
                                                                    .bg(comment_accent.opacity(0.9))
                                                                    .border_1()
                                                                    .border_color(comment_accent)
                                                                    .items_center()
                                                                    .justify_center()
                                                                    .text_color(cx.theme().sidebar_foreground)
                                                                    .child(Icon::new(IconName::MessageSquarePlus).with_size(px(14.))),
                                                            )
                                                            .accessibility_label("Add comment to selected code")
                                                            .tooltip("Add comment to selected code")
                                                            .on_click(cx.listener(move |this: &mut Self, _, window, cx| {
                                                                this.begin_code_comment(
                                                                    target.clone(),
                                                                    comment_session_id.clone(),
                                                                    window,
                                                                    cx,
                                                                );
                                                            }))
                                                    ))
                                                    .when(comment_count > 0 && !has_add_comment_action, |slot| slot.child(
                                                        Button::new(("comment-marker", ln as usize))
                                                            .debug_selector(move || format!("comment-marker-{ln}"))
                                                            .icon(IconName::MessageSquare)
                                                            .custom(comment_marker_style)
                                                            .with_size(px(16.))
                                                            .w(px(16.))
                                                            .h(line_height)
                                                            .p_0()
                                                            .rounded(px(3.))
                                                            .cursor(CursorStyle::PointingHand)
                                                            .accessibility_label(format!("Show {comment_count} code comments on line {ln}"))
                                                            .tooltip(format!("Show {comment_count} code comment(s) on line {ln}"))
                                                            .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                                this.comment_popover_line = (this.comment_popover_line != Some(ln)).then_some(ln);
                                                                this.comment_draft_target = None;
                                                                cx.notify();
                                                            }))
                                                    ))
                                                    .when(is_comment_end, |slot| slot.child(
                                                        div()
                                                            .absolute()
                                                            .bottom(px(0.))
                                                            .left(px(0.))
                                                            .w(px(6.))
                                                            .h(px(3.))
                                                            .bg(comment_accent)
                                                    ))
                                            )
                                    }))
                            )
                    )
                    // Code Editor & Floating Overlays Container
                    .child(
                        div()
                            .key_context(tab_action_context)
                            .flex_1()
                            .h_full()
                        .min_w_0()
                        .relative()
                        .bg(editor_bg)
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this: &mut Self, event: &MouseUpEvent, _, cx| {
                                    if this.git_diff_preview.is_none()
                                        && (event.modifiers.platform || event.modifiers.control)
                                    {
                                        this.go_to_definition(cx);
                                    }
                                }),
                            )
                        .context_menu(move |menu, window, _cx| {
                                let editor_focus = code_panel
                                    .read(_cx)
                                    .editor
                                    .read(_cx)
                                    .focus_handle(_cx);
                                let can_comment = code_panel.read(_cx).comment_session_id.is_some()
                                    && code_panel.read(_cx).git_diff_preview.is_none();
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
                                let menu = menu.item(PopupMenuItem::new("Cut").on_click({
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
                                ));
                                let menu = if can_comment {
                                    menu.separator().item(file_item(
                                        "Comment on selection",
                                        CodeTabAction::CommentOnSelection,
                                    ))
                                } else {
                                    menu
                                };
                                menu.separator().item(file_item(
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
                                div()
                                    .when(self.git_diff_preview.is_some(), |editor| {
                                        editor.debug_selector(|| "git-diff-projection-editor".into())
                                    })
                                    .flex_1()
                                    .h_full()
                                    .min_h_0()
                                    .child(
                                        Editor::new(&rendered_editor)
                                            .aria_label(if self.git_diff_preview.is_some() {
                                                "AHEAD Git diff preview"
                                            } else {
                                                "AHEAD Code Editor"
                                            })
                                            // The parent GPUI popup already provides these edit actions.
                                            .context_menu(|menu, _, _| menu)
                                            .h_full(),
                                    )
                                    .child(
                                        canvas(
                                            move |bounds, _, cx| {
                                                if ruler_column == 0 {
                                                    return None;
                                                }
                                                let editor = ruler_editor.read(cx);
                                                let text = editor.value().to_string();
                                                let caret = editor.cursor();
                                                let line_start =
                                                    line_start_offset(&text, caret)?;
                                                let line_origin = editor.range_to_bounds(
                                                    &(line_start..line_start),
                                                )?;
                                                let advance = monospace_probe_range(
                                                    &text,
                                                    line_start,
                                                )
                                                .and_then(|range| {
                                                    editor.range_to_bounds(&range)
                                                })
                                                .map(|glyph| glyph.size.width)
                                                .unwrap_or(ruler_fallback_advance);
                                                Some(
                                                    line_origin.origin.x
                                                        - bounds.origin.x
                                                        + advance * ruler_column as f32,
                                                )
                                            },
                                            move |bounds, ruler_x, window, _| {
                                                if let Some(ruler_x) = ruler_x {
                                                    window.paint_quad(gpui_kit::fill(
                                                        gpui_kit::bounds(
                                                            gpui_kit::point(
                                                                ruler_x,
                                                                bounds.origin.y,
                                                            ),
                                                            gpui_kit::size(
                                                                px(1.),
                                                                bounds.size.height,
                                                            ),
                                                        ),
                                                        ruler_color,
                                                    ));
                                                }
                                            },
                                        )
                                        .absolute()
                                        .inset_0(),
                                    ),
                            )
                            .when_some(
                                pointer_range.filter(|_| self.git_diff_preview.is_none()),
                                |el, range| {
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
                            .when_some(
                                self.navigation_results
                                    .clone()
                                    .filter(|_| self.git_diff_preview.is_none()),
                                |el, results| {
                                let locations = results.locations;
                                el.child(
                                    v_flex()
                                        .id("navigation-results")
                                        .absolute()
                                        .top(px(8.))
                                        .right(px(12.))
                                        .w(px(420.))
                                        .max_h(px(360.))
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
                                                .p_2()
                                                .child(results.title)
                                                .child(
                                                    Button::new("close-navigation-results")
                                                        .icon(IconName::X)
                                                        .ghost()
                                                        .tooltip("Close navigation results")
                                                        .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                                            this.navigation_results = None;
                                                            cx.notify();
                                                        })),
                                                ),
                                        )
                                        .children(locations.into_iter().enumerate().map(|(index, location)| {
                                            let label = navigation_location_label(&location);
                                            Button::new(("navigation-location", index))
                                                .ghost()
                                                .w_full()
                                                .justify_start()
                                                .px_2()
                                                .py_1()
                                                .child(label)
                                                .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                    this.open_navigation_location(location.clone(), cx);
                                                }))
                                        })),
                                )
                            })
                            .when_some(
                                inline_blame_range.filter(|_| self.git_diff_preview.is_none()),
                                |el, range| {
                                let editor = self.editor.clone();
                                let panel = inline_blame_panel.clone();
                                let line = self.active_line;
                                el.child(
                                    canvas(
                                        move |bounds, _, cx| {
                                            editor.read(cx).range_to_bounds(&range).map(|target| {
                                                let origin = target.origin - bounds.origin;
                                                (line, origin.x.as_f32(), origin.y.as_f32())
                                            })
                                        },
                                        move |_, position, _, cx| {
                                            panel.update(cx, |panel, cx| {
                                                if panel.inline_blame_position != position {
                                                    panel.inline_blame_position = position;
                                                    cx.notify();
                                                }
                                            });
                                        },
                                    )
                                    .absolute()
                                    .inset_0(),
                                )
                            })
                            .when_some(
                                inline_blame_position
                                    .zip(inline_blame_text)
                                    .filter(|_| self.git_diff_preview.is_none()),
                                |el, ((_, left, _), label)| {
                                el.child(
                                    h_flex()
                                        .absolute()
                                        .top(px(inline_blame_top))
                                        .left(px(left + 28.))
                                        .items_center()
                                        .gap_1()
                                        .text_size(px(11.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(IconName::GitBranch)
                                        .child(label),
                                )
                            })
                            .when_some(
                                presentation_overlay.filter(|_| self.git_diff_preview.is_none()),
                                |el, (presentation, top)| {
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
                            .when_some(
                                pointer_overlay.filter(|_| self.git_diff_preview.is_none()),
                                |el, (left, top)| {
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
                            .when_some(
                                hover_text
                                    .clone()
                                    .filter(|_| self.git_diff_preview.is_none()),
                                |el, hover| {
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
                    .when(show_cursor_popup_anchor && self.git_diff_preview.is_none(), |el| {
                        let editor = completion_editor.clone();
                        let panel = completion_panel.clone();
                        el.child(
                            canvas(
                                move |bounds, _, cx| {
                                    let editor = editor.read(cx);
                                    let offset = editor.cursor();
                                    editor.range_to_bounds(&(offset..offset)).map(|caret| {
                                        (
                                            (caret.origin.x - bounds.origin.x).as_f32(),
                                            (caret.origin.y - bounds.origin.y).as_f32(),
                                            bounds.size.width.as_f32(),
                                        )
                                    })
                                },
                                move |_, caret_anchor, _, cx| {
                                    panel.update(cx, |panel, cx| {
                                        let (caret_left, caret_top, available_width) = caret_anchor
                                            .map(|(left, top, width)| {
                                                (Some(px(left)), Some(px(top)), Some(px(width)))
                                            })
                                            .unwrap_or((None, None, None));
                                        if panel.completion_caret_left != caret_left
                                            || panel.completion_caret_top != caret_top
                                            || panel.completion_available_width != available_width
                                        {
                                            panel.completion_caret_left = caret_left;
                                            panel.completion_caret_top = caret_top;
                                            panel.completion_available_width = available_width;
                                            cx.notify();
                                        }
                                    });
                                },
                            )
                            .absolute()
                            .inset_0(),
                        )
                    })
                    .when_some(
                        signature_help.filter(|_| self.git_diff_preview.is_none()),
                        |el, signature| {
                        let (prefix, active, suffix) = signature
                            .active_parameter
                            .as_ref()
                            .map_or((signature.label.clone(), String::new(), String::new()), |range| {
                                (
                                    signature.label[..range.start].to_owned(),
                                    signature.label[range.clone()].to_owned(),
                                    signature.label[range.end..].to_owned(),
                                )
                            });
                        el.child(
                            v_flex()
                                .id("signature-help")
                                .debug_selector(|| "signature-help".into())
                                .absolute()
                                .top(signature_help_top)
                                .left(signature_help_left)
                                .max_w(signature_help_width)
                                .max_h(signature_help_max_height)
                                .overflow_y_scroll()
                                .p_2()
                                .bg(popup_bg)
                                .border_1()
                                .border_color(border_color)
                                .shadow_md()
                                .child(
                                    h_flex()
                                        .w_full()
                                        .min_w_0()
                                        .flex_wrap()
                                        .font_family("Menlo")
                                        .text_size(px(11.))
                                        .child(div().min_w_0().text_color(text_color).child(prefix))
                                        .when(!active.is_empty(), |el| {
                                            el.child(
                                                div()
                                                    .min_w_0()
                                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                                    .text_color(cx.theme().primary)
                                                    .child(active),
                                            )
                                        })
                                        .child(div().min_w_0().text_color(text_color).child(suffix)),
                                )
                                .when_some(
                                    signature.parameter_documentation,
                                    |el, documentation| {
                                        el.child(
                                            div()
                                                .debug_selector(|| {
                                                    "signature-help-parameter-documentation"
                                                        .into()
                                                })
                                                .mt_1()
                                                .text_size(px(10.))
                                                .text_color(
                                                    cx.theme().muted_foreground,
                                                )
                                                .child(documentation),
                                        )
                                    },
                                )
                                .when_some(signature.documentation, |el, documentation| {
                                    el.child(
                                        div()
                                            .mt_1()
                                            .text_size(px(10.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(documentation),
                                    )
                                })
                                .when(signature.total > 1, |el| {
                                    el.child(
                                        div()
                                            .mt_1()
                                            .text_size(px(10.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!(
                                                "{} / {} · ↑/↓ to switch · Esc to close",
                                                signature.index + 1,
                                                signature.total
                                            )),
                                    )
                                }),
                        )
                    })
                    .when(self.code_actions_visible, |el| {
                        el.child(
                            v_flex()
                                .id("code-actions")
                                .debug_selector(|| "code-actions".into())
                                .absolute()
                                .top(completion_top)
                                .left(code_actions_left)
                                .w(code_actions_width)
                                .max_h(px(320.))
                                .overflow_y_scroll()
                                .p_1()
                                .bg(popup_bg)
                                .border_1()
                                .border_color(border_color)
                                .shadow_lg()
                                .child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .text_size(px(10.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Code Actions"),
                                )
                                .children(code_actions.iter().enumerate().map(
                                    |(index, entry)| {
                                        let (title, detail) = match &entry.action {
                                            lsp_types::CodeActionOrCommand::CodeAction(
                                                action,
                                            ) => (
                                                action.title.clone(),
                                                action
                                                    .disabled
                                                    .as_ref()
                                                    .map(|disabled| disabled.reason.clone())
                                                    .or_else(|| {
                                                        action.command.as_ref().map(|command| {
                                                            format!("Runs server command: {}", command.command)
                                                        })
                                                    })
                                                    .unwrap_or_default(),
                                            ),
                                            lsp_types::CodeActionOrCommand::Command(
                                                command,
                                            ) => (
                                                command.title.clone(),
                                                format!(
                                                    "Runs server command: {}",
                                                    command.command
                                                ),
                                            ),
                                        };
                                        let selected = index == selected_code_action;
                                        Button::new(("code-action", index))
                                            .ghost()
                                            .w_full()
                                            .justify_start()
                                            .px_2()
                                            .py_1()
                                            .bg(if selected {
                                                active_sel_bg
                                            } else {
                                                popup_bg
                                            })
                                            .tooltip(if detail.is_empty() {
                                                title.clone()
                                            } else {
                                                format!("{title} — {detail}")
                                            })
                                            .child(
                                                h_flex()
                                                    .w_full()
                                                    .min_w_0()
                                                    .items_start()
                                                    .gap_2()
                                                    .child(IconName::Code)
                                                    .child(
                                                        v_flex()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .child(
                                                                div()
                                                                    .min_w_0()
                                                                    .text_size(px(12.))
                                                                    .text_color(text_color)
                                                                    .child(title),
                                                            )
                                                            .when(!detail.is_empty(), |el| {
                                                                el.child(
                                                                    div()
                                                                        .min_w_0()
                                                                        .text_size(px(10.))
                                                                        .text_color(cx.theme().muted_foreground)
                                                                        .child(detail),
                                                                )
                                                            }),
                                                    ),
                                            )
                                            .on_click(cx.listener(
                                                move |this, _, window, cx| {
                                                    this.selected_code_action = index;
                                                    this.apply_selected_code_action(
                                                        window, cx,
                                                    );
                                                },
                                            ))
                                    },
                                ))
                                .child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .text_size(px(10.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child("↑/↓ to select · Enter to apply · Esc to close"),
                                ),
                        )
                    })
                    // FIM Ghost Text Banner Overlay (dim inline prediction)
                    .when_some(
                        self.ghost_text
                            .clone()
                            .filter(|_| self.git_diff_preview.is_none()),
                        |el, ghost| {
                        el.child(
                            div()
                                .absolute()
                                .top(completion_top)
                                .left(ghost_text_left)
                                .max_w(ghost_text_width)
                                .overflow_x_hidden()
                                .p_2()
                                .bg(cx.theme().accent)
                                .border_1()
                                .border_color(cx.theme().primary)
                                .child(
                                    h_flex()
                                        .w_full()
                                        .min_w_0()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            div()
                                                .min_w_0()
                                                .flex_1()
                                                .truncate()
                                                .text_size(px(11.))
                                                .font_family("Menlo")
                                                .text_color(cx.theme().accent_foreground)
                                                .child(ghost)
                                        )
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_size(px(10.))
                                                .text_color(cx.theme().muted_foreground)
                                                .child("Tab to accept · Esc to dismiss")
                                        )
                                )
                        )
                    })
                    // Floating LSP Completion Popup
                    .when(self.show_completions && self.git_diff_preview.is_none(), |el| {
                        el.child(
                            v_flex()
                                .absolute()
                                .top(completion_menu_top)
                                .left(completion_left)
                                .w(completion_width)
                                .debug_selector(|| "lsp-completions".into())
                                .bg(popup_bg)
                                .border_1()
                                .border_color(border_color)
                                .shadow_lg()
                                .p_1()
                                .child(
                                    v_flex()
                                        .max_h(completion_menu_max_height)
                                        .gap_1()
                                        .overflow_y_scrollbar()
                                        .children(
                                            self.completions.iter().enumerate().map(|(idx, item)| {
                                                let is_sel = idx == selected_idx;
                                                let detail = completion_item_detail(&item.item);
                                                h_flex()
                                                    .w_full()
                                                    .min_w_0()
                                                    .items_center()
                                                    .gap_2()
                                                    .p_1()
                                                    .bg(if is_sel { active_sel_bg } else { popup_bg })
                                                    .child(IconName::Code)
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .flex_1()
                                                            .truncate()
                                                            .font_weight(if is_sel { gpui_kit::FontWeight::BOLD } else { gpui_kit::FontWeight::NORMAL })
                                                            .text_size(px(12.))
                                                            .text_color(text_color)
                                                            .child(item.item.label.clone())
                                                    )
                                                    .when_some(detail, |row, detail| {
                                                        row.child(
                                                            div()
                                                                .debug_selector(move || format!("lsp-completion-detail-{idx}"))
                                                                .w(completion_width * 0.58)
                                                                .min_w_0()
                                                                .text_size(px(11.))
                                                                .font_family("Menlo")
                                                                .text_color(cx.theme().muted_foreground)
                                                                .child(detail),
                                                        )
                                                    })
                                            })
                                        )
                                )
                        )
                    })
                    .when_some(
                        snippet_choice_options.filter(|_| {
                            !self.show_completions
                                && self.git_diff_preview.is_none()
                        }),
                        |el, (choices, selected_choice)| {
                            el.child(
                                v_flex()
                                    .absolute()
                                    .top(completion_menu_top)
                                    .left(snippet_choices_left)
                                    .w(snippet_choices_width)
                                    .debug_selector(|| "lsp-snippet-choices".into())
                                    .bg(popup_bg)
                                    .border_1()
                                    .border_color(border_color)
                                    .shadow_lg()
                                    .p_1()
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .text_size(px(10.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(
                                                "Snippet choice · ↑/↓ to choose · Tab next",
                                            ),
                                    )
                                    .children(choices.into_iter().enumerate().map(
                                        |(index, choice)| {
                                            h_flex()
                                                .w_full()
                                                .min_w_0()
                                                .p_1()
                                                .bg(if index == selected_choice {
                                                    active_sel_bg
                                                } else {
                                                    popup_bg
                                                })
                                                .text_size(px(12.))
                                                .text_color(text_color)
                                                .child(choice)
                                        },
                                    )),
                            )
                        },
                    )
                    )
                    .when_some(
                        visible_comments.filter(|_| self.git_diff_preview.is_none()),
                        |el, (line, comments)| {
                        el.child(
                            v_flex()
                                .id("inline_code_comments")
                                .debug_selector(|| "inline-code-comments".into())
                                .w(px(260.))
                                .min_w_0()
                                .h_full()
                                .overflow_x_hidden()
                                .overflow_y_scroll()
                                .p_2()
                                .gap_2()
                                .bg(popup_bg)
                                .border_l_1()
                                .border_color(border_color)
                                .child(
                                    h_flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .text_color(comment_accent)
                                                        .child(IconName::MessageSquare),
                                                )
                                                .child(format!("Code comments · line {line}")),
                                        )
                                        .child(
                                            Button::new("close_inline_comments")
                                                .icon(IconName::X)
                                                .tooltip("Close code comments")
                                                .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                                    this.comment_popover_line = None;
                                                    this.comment_draft_target = None;
                                                    cx.notify();
                                                }))
                                        )
                                )
                                .children(comments.into_iter().map(|(comment, current_range)| {
                                    let reference_comment = comment.clone();
                                    let resolve_id = comment.id.clone();
                                    let source_changed = format!("{:x}", sha2::Sha256::digest(editor_text.as_bytes())) != comment.source_sha256;
                                    v_flex()
                                        .gap_1()
                                        .p_2()
                                        .border_l_2()
                                        .border_color(comment_accent)
                                        .child(div().text_size(px(11.)).text_color(text_color).child(format!("{} · lines {}–{}", comment.actor_id, current_range.start.line + 1, current_range.end.line + 1)))
                                        .child(div().min_w_0().truncate().text_size(px(11.)).text_color(cx.theme().muted_foreground).child(comment.path.clone()))
                                        .when(source_changed, |card| card.child(div().text_size(px(10.)).text_color(cx.theme().warning).child("Source changed since this comment")))
                                        .child(div().text_size(px(12.)).child(comment.body.clone()))
                                        .child(h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new(SharedString::from(format!("attach-inline-{}", comment.id)))
                                                    .ghost()
                                                    .icon(IconName::MessageSquare)
                                                    .label("Attach to chat")
                                                    .tooltip("Attach this comment to the AHEAD chat composer")
                                                    .on_click(cx.listener(move |this: &mut Self, _, window, cx| {
                                                        this.reference_code_comment(&reference_comment, window, cx);
                                                    }))
                                            )
                                            .child(
                                                Button::new(SharedString::from(format!("resolve-inline-{}", comment.id)))
                                                    .ghost()
                                                    .icon(IconName::MessageSquareCheck)
                                                    .label("Resolve")
                                                    .tooltip("Resolve this code comment")
                                                    .disabled(self.comment_pending)
                                                    .on_click(cx.listener(move |this: &mut Self, _, window, cx| {
                                                        this.resolve_code_comment(resolve_id.clone(), window, cx);
                                                    }))
                                            )
                                        )
                                }))
                                .when_some(self.comment_draft_target.clone(), |card, target| card.child(
                                    v_flex()
                                        .gap_2()
                                        .p_2()
                                        .border_l_2()
                                        .border_color(comment_accent)
                                        .child(div().text_size(px(11.)).child(format!("Selected lines {}–{}", target.range.start.line + 1, target.range.end.line + 1)))
                                        .child(div().min_w_0().truncate().text_size(px(11.)).text_color(cx.theme().muted_foreground).child(target.path.clone()))
                                        .child(Textarea::new(&self.comment_input).aria_label("Code comment").w_full())
                                        .child(h_flex().gap_1()
                                            .child(
                                                Button::new("save_inline_code_comment")
                                                    .primary()
                                                    .icon(IconName::Send)
                                                    .label(if self.comment_pending { "Saving…" } else { "Save comment" })
                                                    .tooltip("Save comment in this AHEAD session")
                                                    .disabled(self.comment_pending || self.comment_input.read(cx).value().trim().is_empty())
                                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| this.save_code_comment(window, cx)))
                                            )
                                            .child(
                                                Button::new("cancel_inline_code_comment")
                                                    .ghost()
                                                    .label("Cancel")
                                                    .tooltip("Discard comment draft")
                                                    .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                                        this.comment_draft_target = None;
                                                        this.comment_popover_line = None;
                                                        cx.notify();
                                                    }))
                                            )
                                        )
                                ))
                                .when_some(self.comment_error.clone(), |card, error| card.child(div().text_size(px(11.)).text_color(cx.theme().danger).child(error)))
                        )
                    })
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
                    .when_some(
                        inline_blame_details.filter(|_| self.git_diff_preview.is_none()),
                        |el, label| {
                        el.child(
                            div()
                                .text_size(px(10.))
                                .text_color(cx.theme().muted_foreground)
                                .child(label),
                        )
                    })
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

fn line_end_offset(text: &str, line: u32) -> Option<usize> {
    let mut offset = 0;
    for (index, content) in text.split('\n').enumerate() {
        if index + 1 == line as usize {
            return Some(offset + content.len());
        }
        offset += content.len() + 1;
    }
    None
}

fn visible_gutter_rows(
    visible_rows: Option<std::ops::Range<usize>>,
    line_count: usize,
    initial_visible_rows: usize,
) -> std::ops::Range<usize> {
    let line_count = line_count.max(1);
    let rows = visible_rows
        .filter(|rows| !rows.is_empty())
        .unwrap_or_else(|| 0..initial_visible_rows.max(1));
    let start = rows.start.saturating_sub(1).min(line_count);
    let end = rows.end.saturating_add(1).min(line_count);
    start..end.max(start)
}

fn inline_blame_label(hunk: &BlameHunk) -> String {
    let Some(commit) = &hunk.commit else {
        return "Uncommitted changes".to_string();
    };
    format!(
        "{}, {}",
        commit.author,
        relative_blame_time(commit.timestamp, chrono::Utc::now().timestamp())
    )
}

fn relative_blame_time(timestamp: i64, now: i64) -> String {
    let seconds = now.saturating_sub(timestamp);
    let (count, unit) = if seconds < 60 {
        return "just now".to_string();
    } else if seconds < 3_600 {
        (seconds / 60, "minute")
    } else if seconds < 86_400 {
        (seconds / 3_600, "hour")
    } else if seconds < 2_592_000 {
        (seconds / 86_400, "day")
    } else if seconds < 31_536_000 {
        (seconds / 2_592_000, "month")
    } else {
        (seconds / 31_536_000, "year")
    };
    format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" })
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

fn rename_placeholder(
    text: &str,
    position: lsp_types::Position,
    response: lsp_types::PrepareRenameResponse,
) -> Result<String, String> {
    let (range, placeholder) = match response {
        lsp_types::PrepareRenameResponse::Range(range) => (range, None),
        lsp_types::PrepareRenameResponse::RangeWithPlaceholder {
            range,
            placeholder,
        } => (range, Some(placeholder)),
        lsp_types::PrepareRenameResponse::DefaultBehavior {
            default_behavior: true,
        } => {
            let cursor = caret_byte_offset(text, position).ok_or_else(|| {
                "The rename position is outside the buffer".to_owned()
            })?;
            let range = word_range_at(text, cursor).ok_or_else(|| {
                "No symbol is available to rename at the caret".to_owned()
            })?;
            return text.get(range).map(ToOwned::to_owned).ok_or_else(|| {
                "No symbol is available to rename at the caret".to_owned()
            });
        }
        lsp_types::PrepareRenameResponse::DefaultBehavior {
            default_behavior: false,
        } => return Err("The language server cannot rename this symbol".into()),
    };
    let start = caret_byte_offset(text, range.start).ok_or_else(|| {
        "The language server returned an invalid rename range".to_owned()
    })?;
    let end = caret_byte_offset(text, range.end).ok_or_else(|| {
        "The language server returned an invalid rename range".to_owned()
    })?;
    let old_name = text
        .get(start..end)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            "The language server returned an empty rename range".to_owned()
        })?;
    let placeholder = placeholder
        .filter(|placeholder| !placeholder.trim().is_empty())
        .unwrap_or_else(|| old_name.to_owned());
    Ok(placeholder)
}

fn word_range_at(text: &str, offset: usize) -> Option<std::ops::Range<usize>> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let is_word = |character: char| character.is_alphanumeric() || character == '_';
    let mut start = offset;
    while start > 0 {
        let character = text[..start].chars().next_back()?;
        if !is_word(character) {
            break;
        }
        start -= character.len_utf8();
    }
    let mut end = offset;
    while end < text.len() {
        let character = text[end..].chars().next()?;
        if !is_word(character) {
            break;
        }
        end += character.len_utf8();
    }
    (start < end).then_some(start..end)
}

#[derive(Clone)]
struct SignatureHelpView {
    label: String,
    active_parameter: Option<std::ops::Range<usize>>,
    parameter_documentation: Option<String>,
    documentation: Option<String>,
    index: usize,
    total: usize,
}

fn signature_help_view(
    help: &lsp_types::SignatureHelp,
) -> Option<SignatureHelpView> {
    let requested_index = help.active_signature.unwrap_or(0) as usize;
    let (index, signature) = help
        .signatures
        .get(requested_index)
        .map(|signature| (requested_index, signature))
        .or_else(|| help.signatures.first().map(|signature| (0, signature)))?;
    let active_parameter_index =
        signature.active_parameter.or(help.active_parameter);
    let active_parameter = active_parameter_index
        .and_then(|index| signature.parameters.as_ref()?.get(index as usize))
        .and_then(|parameter| {
            signature_parameter_range(&signature.label, parameter)
        });
    let parameter_documentation = active_parameter_index
        .and_then(|index| signature.parameters.as_ref()?.get(index as usize))
        .and_then(|parameter| parameter.documentation.as_ref())
        .map(lsp_documentation_text);
    let documentation = signature.documentation.as_ref().map(lsp_documentation_text);
    Some(SignatureHelpView {
        label: signature.label.clone(),
        active_parameter,
        parameter_documentation,
        documentation,
        index,
        total: help.signatures.len(),
    })
}

fn lsp_documentation_text(documentation: &lsp_types::Documentation) -> String {
    match documentation {
        lsp_types::Documentation::String(text) => text.clone(),
        lsp_types::Documentation::MarkupContent(markup) => markup.value.clone(),
    }
}

fn signature_parameter_range(
    label: &str,
    parameter: &lsp_types::ParameterInformation,
) -> Option<std::ops::Range<usize>> {
    match &parameter.label {
        lsp_types::ParameterLabel::Simple(parameter) => {
            let start = label.find(parameter)?;
            Some(start..start + parameter.len())
        }
        lsp_types::ParameterLabel::LabelOffsets([start, end]) if start < end => {
            Some(utf16_byte_offset(label, *start)?..utf16_byte_offset(label, *end)?)
        }
        lsp_types::ParameterLabel::LabelOffsets(_) => None,
    }
}

fn utf16_byte_offset(text: &str, target: u32) -> Option<usize> {
    let mut units = 0;
    for (offset, character) in text.char_indices() {
        if units == target {
            return Some(offset);
        }
        units += character.len_utf16() as u32;
        if units > target {
            return None;
        }
    }
    (units == target).then_some(text.len())
}

#[derive(Clone, Debug)]
pub(crate) struct WorkspaceTextEdits {
    pub(crate) path: std::path::PathBuf,
    pub(crate) version: Option<i32>,
    pub(crate) edits: Vec<lsp_types::TextEdit>,
}

pub(crate) fn workspace_edit_text_edits(
    edit: &lsp_types::WorkspaceEdit,
) -> Result<Vec<WorkspaceTextEdits>, String> {
    if edit.changes.is_some() && edit.document_changes.is_some() {
        return Err(
            "The workspace edit contains two conflicting change forms".into()
        );
    }
    let mut documents = std::collections::HashMap::new();
    if let Some(changes) = &edit.changes {
        for (uri, changes) in changes {
            let path = uri.to_file_path().map_err(|_| {
                "The workspace edit contains a non-file URI".to_owned()
            })?;
            append_workspace_text_edits(
                &mut documents,
                path,
                None,
                changes.iter().cloned(),
            )?;
        }
    }
    if let Some(changes) = &edit.document_changes {
        match changes {
            lsp_types::DocumentChanges::Edits(document_edits) => {
                for document in document_edits {
                    append_workspace_document_edits(document, &mut documents)?;
                }
            }
            lsp_types::DocumentChanges::Operations(operations) => {
                for operation in operations {
                    match operation {
                        lsp_types::DocumentChangeOperation::Edit(document) => {
                            append_workspace_document_edits(
                                document,
                                &mut documents,
                            )?;
                        }
                        lsp_types::DocumentChangeOperation::Op(_) => {
                            return Err(
                                "The workspace edit includes unsupported file operations".into(),
                            );
                        }
                    }
                }
            }
        }
    }
    let mut documents = documents
        .into_values()
        .filter(|document: &WorkspaceTextEdits| !document.edits.is_empty())
        .collect::<Vec<_>>();
    documents.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    if documents.is_empty() {
        return Err("The workspace edit contains no text edits".into());
    }
    Ok(documents)
}

#[cfg(test)]
fn active_file_workspace_edit_text_edits(
    path: &std::path::Path,
    edit: &lsp_types::WorkspaceEdit,
) -> Result<Vec<lsp_types::TextEdit>, String> {
    let documents = workspace_edit_text_edits(edit)?;
    if documents.len() != 1 || documents[0].path.as_path() != path {
        return Err("The code action edits another file".into());
    }
    documents
        .into_iter()
        .next()
        .map(|document| document.edits)
        .ok_or_else(|| "The code action contains no text edits".to_owned())
}

fn append_workspace_document_edits(
    document: &lsp_types::TextDocumentEdit,
    documents: &mut std::collections::HashMap<
        std::path::PathBuf,
        WorkspaceTextEdits,
    >,
) -> Result<(), String> {
    let path = document
        .text_document
        .uri
        .to_file_path()
        .map_err(|_| "The workspace edit contains a non-file URI".to_owned())?;
    append_workspace_text_edits(
        documents,
        path,
        document.text_document.version,
        document.edits.iter().map(|edit| match edit {
            lsp_types::OneOf::Left(edit) => edit.clone(),
            lsp_types::OneOf::Right(edit) => edit.text_edit.clone(),
        }),
    )
}

fn append_workspace_text_edits(
    documents: &mut std::collections::HashMap<
        std::path::PathBuf,
        WorkspaceTextEdits,
    >,
    path: std::path::PathBuf,
    version: Option<i32>,
    edits: impl IntoIterator<Item = lsp_types::TextEdit>,
) -> Result<(), String> {
    let document =
        documents
            .entry(path.clone())
            .or_insert_with(|| WorkspaceTextEdits {
                path,
                version,
                edits: Vec::new(),
            });
    if document.version != version {
        return Err(
            "The workspace edit assigns conflicting versions to a file".into()
        );
    }
    document.edits.extend(edits);
    Ok(())
}

pub(crate) fn formatting_edit(
    text: &str,
    edits: &[lsp_types::TextEdit],
    selection: std::ops::Range<usize>,
) -> Result<Option<(std::ops::Range<usize>, String, std::ops::Range<usize>)>, String>
{
    if selection.start > selection.end
        || selection.end > text.len()
        || !text.is_char_boundary(selection.start)
        || !text.is_char_boundary(selection.end)
    {
        return Err("The editor selection is invalid".into());
    }
    let mut edits = edits
        .iter()
        .map(|edit| {
            let start = caret_byte_offset(text, edit.range.start);
            let end = caret_byte_offset(text, edit.range.end);
            match (start, end) {
                (Some(start), Some(end)) if start <= end => {
                    Ok((start..end, edit.new_text.as_str()))
                }
                _ => Err("The server returned an invalid edit range".to_owned()),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    if edits.is_empty() {
        return Ok(None);
    }
    edits.sort_by_key(|(range, _)| (range.start, range.end));
    for pair in edits.windows(2) {
        let previous = &pair[0].0;
        let next = &pair[1].0;
        if previous.end > next.start
            || (previous.is_empty()
                && next.is_empty()
                && previous.start == next.start)
        {
            return Err("The server returned overlapping edits".into());
        }
    }

    let start = edits
        .first()
        .ok_or("The formatting response contained no edits")?
        .0
        .start;
    let end = edits
        .last()
        .ok_or("The formatting response contained no edits")?
        .0
        .end;
    let range = start..end;
    let mut replacement = String::new();
    let mut old_cursor = range.start;
    for (edit_range, new_text) in &edits {
        replacement.push_str(&text[old_cursor..edit_range.start]);
        replacement.push_str(new_text);
        old_cursor = edit_range.end;
    }
    replacement.push_str(&text[old_cursor..range.end]);
    if replacement == text[range.clone()] {
        return Ok(None);
    }

    let map_offset = |offset: usize| {
        let mut old_cursor = 0;
        let mut new_cursor = 0;
        for (edit_range, new_text) in &edits {
            if offset < edit_range.start {
                return new_cursor + offset - old_cursor;
            }
            new_cursor += edit_range.start - old_cursor;
            if edit_range.is_empty() && offset == edit_range.start {
                return new_cursor + new_text.len();
            }
            if offset < edit_range.end {
                return new_cursor
                    + map_replaced_offset(
                        &text[edit_range.clone()],
                        new_text,
                        offset - edit_range.start,
                    );
            }
            new_cursor += new_text.len();
            old_cursor = edit_range.end;
        }
        new_cursor + offset - old_cursor
    };
    Ok(Some((
        range,
        replacement,
        map_offset(selection.start)..map_offset(selection.end),
    )))
}

fn map_replaced_offset(old: &str, new: &str, offset: usize) -> usize {
    let diff = similar::TextDiff::from_chars(old, new);
    let (mut old_cursor, mut new_cursor) = (0, 0);
    for change in diff.iter_all_changes() {
        let length = change.value().len();
        match change.tag() {
            similar::ChangeTag::Equal => {
                if offset < old_cursor + length {
                    return new_cursor + offset - old_cursor;
                }
                old_cursor += length;
                new_cursor += length;
            }
            similar::ChangeTag::Delete => {
                if offset < old_cursor + length {
                    return new_cursor;
                }
                old_cursor += length;
            }
            similar::ChangeTag::Insert => new_cursor += length,
        }
    }
    new_cursor + offset.saturating_sub(old_cursor)
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
        CodePanel, EDITOR_GUTTER_WIDTH, GIT_GUTTER_TRACK_WIDTH,
        GIT_METADATA_REFRESH_DEBOUNCE, GUTTER_ACTION_WIDTH, MergeConflictChoice,
        NULL_COMPLETION_RETRY_DELAY, SEMANTIC_TOKEN_DEBOUNCE, agent_anchor_lines,
        blame_label, caret_byte_offset, caret_popup_horizontal_position,
        caret_popup_position, completion_edit, completion_edit_with_variables,
        completion_item_detail, completion_prefix_range, current_comment_range,
        definition_open_request, editor_language, extension_syntax_decorations,
        formatting_edit, git_hunk_preview_text, git_hunk_projection,
        has_unclosed_parenthesis_before_cursor, interpolate_insertion,
        is_markdown_path, is_shared_draft_path, line_start_offset,
        location_byte_position, lsp_snippet, merge_conflict_choice_text,
        monospace_probe_range, parse_merge_conflicts, presentation_gutter_line,
        rank_completions, read_renamed_workspace_file, rename_placeholder,
        renamed_path_for_open_file, semantic_text_decorations, semantic_token_scope,
        shared_draft_file_path, shared_draft_path, signature_help_view,
    };
    use crate::proxy_client::{LspCodeAction, LspCompletion};
    use crate::ross::{OpenColumn, OpenLocation};
    use ahead_rpc::plugin::PluginId;
    use ahead_rpc::source_control::{DiffHunk, DiffHunkKind};
    use ahead_rpc::style::{LineStyle, Style};
    use gpui_kit::component::input::Undo;
    use gpui_kit::{Focusable, VisualContext};
    use sha2::Digest;

    #[test]
    fn external_rename_maps_open_files_and_descendants() {
        let old_path = std::path::Path::new("/workspace/src");
        let new_path = std::path::Path::new("/workspace/lib");
        assert_eq!(
            renamed_path_for_open_file(
                std::path::Path::new("/workspace/src/main.rs"),
                old_path,
                new_path,
            ),
            Some(std::path::PathBuf::from("/workspace/lib/main.rs"))
        );
        assert_eq!(
            renamed_path_for_open_file(old_path, old_path, new_path,),
            Some(new_path.to_path_buf())
        );
        assert_eq!(
            renamed_path_for_open_file(
                std::path::Path::new("/workspace/src-old/main.rs"),
                old_path,
                new_path,
            ),
            None
        );
    }

    #[test]
    fn renamed_workspace_file_must_stay_inside_workspace() {
        let workspace = tempfile::tempdir().expect("workspace");
        let destination = workspace.path().join("src/main.rs");
        std::fs::create_dir_all(destination.parent().expect("parent"))
            .expect("create source directory");
        std::fs::write(&destination, "fn main() {}\n").expect("write source");

        let (canonical_path, text) = read_renamed_workspace_file(
            workspace.path().to_str().expect("workspace path"),
            &destination,
        )
        .expect("read renamed workspace file");
        assert_eq!(
            canonical_path,
            destination.canonicalize().expect("canonical path")
        );
        assert_eq!(text, "fn main() {}\n");

        let outside = tempfile::tempdir().expect("outside directory");
        let outside_file = outside.path().join("external.rs");
        std::fs::write(&outside_file, "not workspace content")
            .expect("write outside file");
        assert!(
            read_renamed_workspace_file(
                workspace.path().to_str().expect("workspace path"),
                &outside_file,
            )
            .is_err()
        );
    }

    #[test]
    fn line_length_ruler_uses_the_caret_line_and_an_ascii_glyph_probe() {
        let text = "\t# first\nsecond line";
        let line_start = line_start_offset(text, 16).expect("valid caret offset");
        assert_eq!(line_start, 9);
        assert_eq!(monospace_probe_range(text, line_start), Some(9..10));
        assert_eq!(line_start_offset("é\nx", 1), None);
        assert_eq!(monospace_probe_range("\t\n", 0), None);
    }

    #[test]
    fn caret_popups_prefer_space_above_and_fall_below_at_the_top() {
        let (top, height) = caret_popup_position(
            gpui_kit::px(220.),
            gpui_kit::px(18.),
            gpui_kit::px(320.),
        );
        assert_eq!(top.as_f32(), 0.);
        assert_eq!(height.as_f32(), 212.);

        let (top, height) = caret_popup_position(
            gpui_kit::px(600.),
            gpui_kit::px(18.),
            gpui_kit::px(320.),
        );
        assert_eq!(top.as_f32(), 272.);
        assert_eq!(height.as_f32(), 320.);

        let (top, height) = caret_popup_position(
            gpui_kit::px(16.),
            gpui_kit::px(18.),
            gpui_kit::px(320.),
        );
        assert_eq!(top.as_f32(), 42.);
        assert_eq!(height.as_f32(), 320.);

        let (top, height) = caret_popup_position(
            gpui_kit::px(220.),
            gpui_kit::px(18.),
            gpui_kit::px(150.),
        );
        assert_eq!(top.as_f32(), 62.);
        assert_eq!(height.as_f32(), 150.);
    }

    #[test]
    fn caret_popups_stay_inside_the_editor_width() {
        let (left, width) = caret_popup_horizontal_position(
            gpui_kit::px(700.),
            gpui_kit::px(800.),
            gpui_kit::px(480.),
        );
        assert_eq!(left.as_f32(), 320.);
        assert_eq!(width.as_f32(), 480.);

        let (left, width) = caret_popup_horizontal_position(
            gpui_kit::px(10.),
            gpui_kit::px(300.),
            gpui_kit::px(480.),
        );
        assert_eq!(left.as_f32(), 10.);
        assert_eq!(width.as_f32(), 300.);

        let (left, width) = caret_popup_horizontal_position(
            gpui_kit::px(500.),
            gpui_kit::px(200.),
            gpui_kit::px(480.),
        );
        assert_eq!(left.as_f32(), 0.);
        assert_eq!(width.as_f32(), 200.);

        for preferred_width in [260., 400., 480., 520.] {
            let (left, width) = caret_popup_horizontal_position(
                gpui_kit::px(175.),
                gpui_kit::px(200.),
                gpui_kit::px(preferred_width),
            );
            assert_eq!(width.as_f32(), preferred_width.min(200.));
            assert!(left.as_f32() >= 0.);
            assert!(left.as_f32() + width.as_f32() <= 200.);
        }
    }

    #[test]
    fn completion_detail_includes_signature_and_label_context() {
        let item = lsp_types::CompletionItem {
            label: "_config_traces_dir".into(),
            detail: Some("def _config_traces_dir() -> Path | None".into()),
            label_details: Some(lsp_types::CompletionItemLabelDetails {
                detail: Some("function".into()),
                description: Some("internal_utils.config".into()),
            }),
            ..Default::default()
        };

        assert_eq!(
            completion_item_detail(&item).as_deref(),
            Some(
                "def _config_traces_dir() -> Path | None · function · internal_utils.config"
            )
        );

        let python_completion = lsp_types::CompletionItem {
            label: "double".into(),
            documentation: Some(lsp_types::Documentation::MarkupContent(
                lsp_types::MarkupContent {
                    kind: lsp_types::MarkupKind::Markdown,
                    value: "```python\ndef double(value: int) -> int\n```\n\nDouble a value.".into(),
                },
            )),
            ..Default::default()
        };
        assert_eq!(
            completion_item_detail(&python_completion).as_deref(),
            Some("def double(value: int) -> int")
        );
    }

    #[test]
    fn signature_help_retriggers_inside_open_calls_only() {
        use lsp_types::Position;

        assert!(has_unclosed_parenthesis_before_cursor(
            "run(value",
            Position::new(0, 9)
        ));
        assert!(has_unclosed_parenthesis_before_cursor(
            "run(first, second",
            Position::new(0, 17)
        ));
        assert!(!has_unclosed_parenthesis_before_cursor(
            "run(value)",
            Position::new(0, 10)
        ));
        assert!(!has_unclosed_parenthesis_before_cursor(
            "value",
            Position::new(0, 5)
        ));
    }

    #[gpui_kit::test]
    fn lsp_completion_popup_renders_above_the_caret_when_there_is_room(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("disposable workspace");
        let path = directory.path().join("main.rs");
        let source = (0..30)
            .map(|line| {
                if line == 15 {
                    "\tlet café_界 = 15;".to_string()
                } else {
                    format!("let value_{line} = {line};")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, &source).expect("source");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        cx.run_until_parked();

        let caret_offset = source
            .lines()
            .take(15)
            .map(|line| line.len() + 1)
            .sum::<usize>()
            + "\tlet café_界".len();
        panel.update(cx, |panel, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(caret_offset - 4..caret_offset, cx);
            });
            panel.completions = (0..12)
                .map(|index| LspCompletion {
                    plugin_id: PluginId(1),
                    item: lsp_types::CompletionItem {
                        label: format!("completion_{index}"),
                        detail: (index == 0).then(|| {
                            "fn completion_0(value: Path) -> Option<File>".into()
                        }),
                        label_details: (index == 0).then_some(
                            lsp_types::CompletionItemLabelDetails {
                                detail: Some("function".into()),
                                description: Some("workspace::files".into()),
                            },
                        ),
                        ..Default::default()
                    },
                })
                .collect();
            panel.show_completions = true;
            cx.notify();
        });
        cx.run_until_parked();

        let popup = cx
            .debug_bounds("lsp-completions")
            .expect("completion popup");
        let detail = cx
            .debug_bounds("lsp-completion-detail-0")
            .expect("LSP signature detail is visible in the completion row");
        assert!(popup.size.width >= gpui_kit::px(480.));
        assert!(detail.size.width > gpui_kit::px(100.));
        assert!(
            detail.size.height > gpui_kit::px(14.),
            "long completion signatures should wrap instead of being truncated: {detail:?}"
        );
        let caret = panel.update(cx, |panel, cx| {
            let editor = panel.editor.read(cx);
            assert_eq!(editor.cursor(), caret_offset);
            editor
                .range_to_bounds(&(editor.cursor()..editor.cursor()))
                .expect("caret bounds")
        });
        let (measured_caret_left, measured_caret_top, available_width) = panel
            .update(cx, |panel, _| {
                (
                    panel.completion_caret_left,
                    panel.completion_caret_top,
                    panel.completion_available_width,
                )
            });
        assert!(
            popup.bottom() < caret.origin.y,
            "completion popup {popup:?} must end above caret {caret:?}; measured top {measured_caret_top:?}"
        );
        assert!(
            popup.size.width <= available_width.expect("editor width measured"),
            "completion popup {popup:?} must fit the editor width"
        );
        let caret_left = measured_caret_left.expect("caret x measured");
        let expected_popup_left = caret.origin.x - caret_left
            + caret_popup_horizontal_position(
                caret_left,
                available_width.expect("editor width measured"),
                gpui_kit::px(480.),
            )
            .0;
        assert!(
            (popup.origin.x.as_f32() - expected_popup_left.as_f32()).abs() < 0.5,
            "completion popup {popup:?} should anchor to the actual caret at {caret:?}"
        );
    }

    #[gpui_kit::test]
    fn visible_lsp_completions_prefetch_detail_and_fence_stale_replies(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::proxy::{ProxyRequest, ProxyResponse, ProxyRpc};

        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("disposable workspace");
        let path = directory.path().join("main.py");
        std::fs::write(&path, "print(double(21))\n").expect("source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        panel.update(cx, |panel, cx| {
            panel.proxy = Some(proxy.clone());
            panel.completions = vec![
                LspCompletion {
                    plugin_id: PluginId(1),
                    item: lsp_types::CompletionItem {
                        label: "double".into(),
                        data: Some(serde_json::json!({"resolve": "double"})),
                        ..Default::default()
                    },
                },
                LspCompletion {
                    plugin_id: PluginId(1),
                    item: lsp_types::CompletionItem {
                        label: "triple".into(),
                        data: Some(serde_json::json!({"resolve": "triple"})),
                        ..Default::default()
                    },
                },
            ];
            panel.completion_context = Some(panel.current_completion_context(cx));
            panel.show_completions = true;
            panel.request_visible_completion_details(cx);
        });

        let (first_request, first_item) = match rpc
            .rx()
            .try_recv()
            .expect("first visible completion resolve request")
        {
            ProxyRpc::Request(
                request_id,
                ProxyRequest::CompletionResolve {
                    plugin_id,
                    completion_item,
                },
            ) => {
                assert_eq!(plugin_id, PluginId(1));
                (request_id, *completion_item)
            }
            _ => panic!("unexpected proxy request kind"),
        };
        let (stale_request, mut stale_item) = match rpc
            .rx()
            .try_recv()
            .expect("next visible completion is prefetched")
        {
            ProxyRpc::Request(
                request_id,
                ProxyRequest::CompletionResolve {
                    plugin_id,
                    completion_item,
                },
            ) => {
                assert_eq!(plugin_id, PluginId(1));
                assert_eq!(completion_item.label, "triple");
                (request_id, *completion_item)
            }
            _ => panic!("unexpected proxy request kind"),
        };
        let mut resolved_item = first_item;
        resolved_item.detail = Some("def double(value: int) -> int".into());
        rpc.handle_response(
            first_request,
            Ok(ProxyResponse::CompletionResolveResponse {
                item: Box::new(resolved_item),
            }),
        );
        cx.run_until_parked();
        assert_eq!(
            panel.update(cx, |panel, _| {
                completion_item_detail(&panel.completions[0].item)
            }),
            Some("def double(value: int) -> int".into())
        );

        panel.update(cx, |panel, cx| panel.select_completion(1, cx));
        stale_item.detail = Some("def triple(value: int) -> int".into());
        panel.update(cx, |panel, cx| {
            panel.completion_request_id =
                panel.completion_request_id.wrapping_add(1);
            panel.completions = vec![LspCompletion {
                plugin_id: PluginId(1),
                item: lsp_types::CompletionItem {
                    label: "replacement".into(),
                    ..Default::default()
                },
            }];
            panel.completion_details_requested.clear();
            panel.completion_context = Some(panel.current_completion_context(cx));
        });
        rpc.handle_response(
            stale_request,
            Ok(ProxyResponse::CompletionResolveResponse {
                item: Box::new(stale_item),
            }),
        );
        cx.run_until_parked();
        assert_eq!(
            panel.update(cx, |panel, _| { panel.completions[0].item.label.clone() }),
            "replacement"
        );
        assert!(
            panel.update(cx, |panel, _| panel.completions[0].item.detail.is_none())
        );
    }

    #[gpui_kit::test]
    fn signature_help_popup_renders_above_caret_with_parameter_documentation(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use lsp_types::{
            Documentation, ParameterInformation, ParameterLabel, SignatureHelp,
            SignatureInformation,
        };

        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("disposable workspace");
        let path = directory.path().join("main.rs");
        let source = (0..30)
            .map(|line| format!("let value_{line} = {line};"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, &source).expect("source");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        cx.run_until_parked();

        let caret_offset = source
            .lines()
            .take(15)
            .map(|line| line.len() + 1)
            .sum::<usize>();
        panel.update(cx, |panel, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(caret_offset..caret_offset, cx);
            });
            let position = panel.cursor_position(cx);
            panel.signature_help_position = Some(position);
            panel.signature_help = Some(SignatureHelp {
                signatures: vec![SignatureInformation {
                    label: "open(path: Path) -> Option<File>".into(),
                    documentation: Some(Documentation::String(
                        "Open a file by path.".into(),
                    )),
                    parameters: Some(vec![ParameterInformation {
                        label: ParameterLabel::Simple("path: Path".into()),
                        documentation: Some(Documentation::String(
                            "File path to open.".into(),
                        )),
                    }]),
                    active_parameter: Some(0),
                }],
                active_signature: Some(0),
                active_parameter: Some(0),
            });
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let popup = cx.debug_bounds("signature-help").expect("signature popup");
        let parameter_documentation = cx
            .debug_bounds("signature-help-parameter-documentation")
            .expect("active parameter documentation");
        let caret = panel.update(cx, |panel, cx| {
            panel
                .editor
                .read(cx)
                .range_to_bounds(&(caret_offset..caret_offset))
                .expect("caret bounds")
        });
        let (caret_left, available_width) = panel.update(cx, |panel, _| {
            (
                panel.completion_caret_left.expect("measured caret x"),
                panel
                    .completion_available_width
                    .expect("measured editor width"),
            )
        });
        let (expected_offset, max_width) = caret_popup_horizontal_position(
            caret_left,
            available_width,
            gpui_kit::px(520.),
        );
        let expected_left = caret.origin.x - caret_left + expected_offset;
        assert!(
            popup.size.width <= max_width,
            "signature popup {popup:?} must fit the measured editor width {available_width:?}"
        );
        assert!(
            (popup.origin.x.as_f32() - expected_left.as_f32()).abs() < 0.5,
            "signature popup {popup:?} should use the measured caret anchor {caret:?}"
        );
        assert!(
            popup.bottom() < caret.origin.y,
            "signature popup {popup:?} must end above caret {caret:?}"
        );
        assert!(parameter_documentation.size.width > gpui_kit::px(0.));
    }

    #[test]
    fn semantic_lsp_token_types_map_to_gpui_syntax_scopes() {
        for (token_type, scope) in [
            ("class", "type"),
            ("enumMember", "constant"),
            ("method", "function"),
            ("parameter", "variable"),
            ("decorator", "attribute"),
            ("regexp", "string.regex"),
        ] {
            assert_eq!(semantic_token_scope(token_type), Some(scope));
        }
        assert_eq!(semantic_token_scope("customUnknownType"), None);
    }

    #[test]
    fn semantic_tokens_keep_utf8_ranges_and_skip_malformed_spans() {
        let text = "fn café\nlet x";
        let styles = [
            LineStyle {
                start: 3,
                end: 8,
                style: Style {
                    fg_color: Some("method".into()),
                },
            },
            LineStyle {
                start: 7,
                end: 8,
                style: Style {
                    fg_color: Some("variable".into()),
                },
            },
            LineStyle {
                start: 0,
                end: 9,
                style: Style {
                    fg_color: Some("function".into()),
                },
            },
            LineStyle {
                start: 0,
                end: 1,
                style: Style { fg_color: None },
            },
        ];
        let mut resolved_scopes = Vec::new();
        let decorations = semantic_text_decorations(&styles, text, |scope| {
            resolved_scopes.push(scope.to_string());
            Some(gpui_kit::HighlightStyle::default())
        });

        assert_eq!(
            decorations
                .iter()
                .map(|decoration| decoration.range.clone())
                .collect::<Vec<_>>(),
            vec![3..8]
        );
        assert_eq!(resolved_scopes, vec!["function"]);
    }

    #[test]
    fn extension_syntax_keeps_utf8_ranges_and_capture_scopes() {
        let text = "fn café\n";
        let captures = [
            ahead_rpc::style::SyntaxCapture {
                start: 3,
                end: 8,
                scope: "function".into(),
            },
            ahead_rpc::style::SyntaxCapture {
                start: 7,
                end: 8,
                scope: "variable".into(),
            },
            ahead_rpc::style::SyntaxCapture {
                start: 0,
                end: text.len() + 1,
                scope: "keyword".into(),
            },
        ];
        let mut resolved_scopes = Vec::new();
        let decorations = extension_syntax_decorations(&captures, text, |scope| {
            resolved_scopes.push(scope.to_string());
            Some(gpui_kit::HighlightStyle::default())
        });

        assert_eq!(
            decorations
                .iter()
                .map(|decoration| decoration.range.clone())
                .collect::<Vec<_>>(),
            vec![3..8]
        );
        assert_eq!(resolved_scopes, vec!["function"]);
    }

    #[gpui_kit::test]
    fn extension_syntax_applies_only_the_current_proxy_response(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::proxy::{ProxyRequest, ProxyResponse, ProxyRpc};

        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("disposable workspace");
        let path = workspace.path().join("page.html");
        let original = "<div>old</div>\n";
        let edited = "<section>new</section>\n";
        std::fs::write(&path, original).expect("source file");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            workspace.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        let view_path = path.to_string_lossy().into_owned();
        let workspace_path = workspace.path().to_string_lossy().into_owned();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(&view_path, window, cx).with_proxy(
                proxy.clone(),
                &workspace_path,
                window,
                cx,
            )
        });

        cx.run_until_parked();
        cx.background_executor
            .advance_clock(SEMANTIC_TOKEN_DEBOUNCE);
        cx.run_until_parked();
        let stale_request = rpc
            .rx()
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    id,
                    ProxyRequest::GetExtensionSyntax { path: request_path },
                ) if request_path == path => Some(id),
                _ => None,
            })
            .expect("initial extension syntax request");

        panel.update_in(cx, |panel, window, cx| {
            panel
                .editor
                .update(cx, |editor, cx| editor.replace_all(edited, window, cx));
        });
        cx.run_until_parked();
        rpc.handle_response(
            stale_request,
            Ok(ProxyResponse::GetExtensionSyntax {
                syntax: ahead_rpc::style::ExtensionSyntax {
                    rev: 1,
                    path: path.clone(),
                    len: original.len(),
                    captures: vec![ahead_rpc::style::SyntaxCapture {
                        start: 1,
                        end: 4,
                        scope: "tag".into(),
                    }],
                    comments: Some(ahead_rpc::style::LanguageComments {
                        line_comment: Some("<!--".into()),
                        block_comment: Some(("<!--".into(), "-->".into())),
                    }),
                },
            }),
        );
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            assert!(
                panel.extension_syntax_decorations.get_ranges(cx).is_empty(),
                "a response for pre-edit text must not decorate the new buffer"
            );
            assert!(panel.extension_comment_variables.is_none());
        });

        cx.background_executor
            .advance_clock(SEMANTIC_TOKEN_DEBOUNCE);
        cx.run_until_parked();
        let current_request = rpc
            .rx()
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    id,
                    ProxyRequest::GetExtensionSyntax { path: request_path },
                ) if request_path == path => Some(id),
                _ => None,
            })
            .expect("updated extension syntax request");
        rpc.handle_response(
            current_request,
            Ok(ProxyResponse::GetExtensionSyntax {
                syntax: ahead_rpc::style::ExtensionSyntax {
                    rev: 2,
                    path: path.clone(),
                    len: edited.len(),
                    captures: vec![ahead_rpc::style::SyntaxCapture {
                        start: 1,
                        end: 8,
                        scope: "tag".into(),
                    }],
                    comments: Some(ahead_rpc::style::LanguageComments {
                        line_comment: Some("<!--".into()),
                        block_comment: Some(("<!--".into(), "-->".into())),
                    }),
                },
            }),
        );
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            assert_eq!(
                panel
                    .extension_syntax_decorations
                    .get_ranges(cx)
                    .iter()
                    .map(|range| range.clone())
                    .collect::<Vec<_>>(),
                vec![1..8]
            );
            assert_eq!(
                panel
                    .extension_comment_variables
                    .as_ref()
                    .and_then(|comments| comments.line_comment.as_deref()),
                Some("<!--")
            );
        });

        proxy.route_core(
            ahead_rpc::core::CoreNotification::LanguageExtensionIssues {
                issues: Vec::new(),
            },
        );
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(SEMANTIC_TOKEN_DEBOUNCE);
        cx.run_until_parked();
        let refreshed_request = rpc
            .rx()
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    id,
                    ProxyRequest::GetExtensionSyntax { path: request_path },
                ) if request_path == path => Some(id),
                _ => None,
            })
            .expect("extension discovery refreshes syntax for the open buffer");
        rpc.handle_response(
            refreshed_request,
            Ok(ProxyResponse::GetExtensionSyntax {
                syntax: ahead_rpc::style::ExtensionSyntax {
                    rev: 2,
                    path: path.clone(),
                    len: edited.len(),
                    captures: vec![ahead_rpc::style::SyntaxCapture {
                        start: 0,
                        end: 9,
                        scope: "tag".into(),
                    }],
                    comments: None,
                },
            }),
        );
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            assert_eq!(
                panel
                    .extension_syntax_decorations
                    .get_ranges(cx)
                    .iter()
                    .map(|range| range.clone())
                    .collect::<Vec<_>>(),
                vec![0..9],
                "the open editor should apply syntax from the refreshed extension catalog"
            );
        });
        std::fs::remove_file(path).expect("remove disposable source");
    }

    #[gpui_kit::test]
    fn inline_prediction_tracks_sessions_related_buffers_and_acceptance(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::proxy::{ProxyRequest, ProxyResponse, ProxyRpc};
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let path = directory.path().join("main.rs");
        std::fs::write(&path, "let value = ").expect("source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        let helper = directory.path().join("helper.rs");
        proxy.sync_editor_snapshot(helper.clone(), "first".into());
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().unwrap(), window, cx)
        });
        panel.update(cx, |panel, cx| {
            panel.proxy = Some(proxy.clone());
            panel.watch_prediction_context(&proxy, cx);
            panel.set_code_comments(Some("first".into()), Vec::new(), cx);
            panel
                .editor
                .update(cx, |editor, cx| editor.set_selected_range(12..12, cx));
        });
        rpc.rx().try_iter().for_each(drop);
        let request = |session: &str, cx: &mut gpui_kit::VisualTestContext| {
            panel.update(cx, |panel, cx| {
                panel.set_code_comments(Some(session.into()), Vec::new(), cx);
                panel.request_proxy_inline(cx);
            });
            let ProxyRpc::Request(
                id,
                ProxyRequest::GetInlineCompletions { session_id, .. },
            ) = rpc
                .rx()
                .try_iter()
                .find(|message| {
                    matches!(
                        message,
                        ProxyRpc::Request(
                            _,
                            ProxyRequest::GetInlineCompletions { .. }
                        )
                    )
                })
                .expect("inline request")
            else {
                panic!("inline request")
            };
            assert_eq!(session_id.as_deref(), Some(session));
            id
        };
        let reply = |id| {
            rpc.handle_response(
                id,
                Ok(ProxyResponse::GetInlineCompletions {
                    prediction: None,
                    completions: lsp_types::InlineCompletionResponse::Array(vec![
                        lsp_types::InlineCompletionItem {
                            insert_text: "calculate()".into(),
                            filter_text: None,
                            range: None,
                            command: None,
                            insert_text_format: None,
                        },
                    ]),
                    context_paths: vec!["helper.rs".into()],
                }),
            )
        };
        let stale = request("first", cx);
        panel.update(cx, |panel, cx| {
            panel.set_code_comments(Some("second".into()), Vec::new(), cx)
        });
        reply(stale);
        cx.run_until_parked();
        assert!(panel.read_with(cx, |panel, _| panel.ghost_text.is_none()));

        reply(request("second", cx));
        cx.run_until_parked();
        assert!(panel.read_with(cx, |panel, _| panel.ghost_text.is_some()));
        proxy.sync_editor_snapshot(
            directory.path().join("unrelated.md"),
            "unrelated edit".into(),
        );
        proxy.route_core(ahead_rpc::core::CoreNotification::WorkspaceFileChange {
            generation: 1,
            paths: vec![directory.path().join("unrelated.md")],
        });
        cx.run_until_parked();
        assert!(panel.read_with(cx, |panel, _| panel.ghost_text.is_some()));
        proxy.update_editor_text(helper.clone(), "changed".into());
        cx.run_until_parked();
        assert!(panel.read_with(cx, |panel, _| panel.ghost_text.is_none()));
        rpc.rx().try_iter().for_each(drop);

        reply(request("second", cx));
        cx.run_until_parked();
        proxy.route_core(ahead_rpc::core::CoreNotification::AheadNotification {
            notification:
                ahead_rpc::ahead::AheadNotification::PredictionContextChanged {
                    session_id: None,
                },
        });
        cx.run_until_parked();
        assert!(
            panel.read_with(cx, |panel, _| panel.ghost_text.is_none()),
            "instruction watcher notifications clear displayed suggestions"
        );

        reply(request("second", cx));
        cx.run_until_parked();
        proxy.route_ahead(
            ahead_rpc::ahead::AheadNotification::PredictionContextChanged {
                session_id: Some("second".into()),
            },
        );
        panel.update_in(cx, |panel, window, cx| panel.accept_ghost_text(window, cx));
        assert_eq!(
            panel.read_with(cx, |panel, cx| panel
                .editor
                .read(cx)
                .value()
                .to_string()),
            "let value = "
        );
        cx.run_until_parked();

        reply(request("second", cx));
        cx.run_until_parked();
        panel.update_in(cx, |panel, window, cx| panel.accept_ghost_text(window, cx));
        assert_eq!(
            panel.read_with(cx, |panel, cx| panel
                .editor
                .read(cx)
                .value()
                .to_string()),
            "let value = calculate()"
        );
        let editor = panel.read_with(cx, |panel, _| panel.editor.clone());
        let focus = editor.update(cx, |editor, cx| editor.focus_handle(cx));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.focus(&focus, cx);
            focus.dispatch_action(&Undo, window, cx);
        });
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.value().to_string()),
            "let value = "
        );
        rpc.rx().try_iter().for_each(drop);

        reply(request("second", cx));
        cx.run_until_parked();
        for (typed, remaining) in [("c", "alculate()"), ("a", "lculate()")] {
            editor.update_in(cx, |editor, window, cx| {
                editor.insert(typed, window, cx);
            });
            cx.run_until_parked();
            assert_eq!(
                panel.read_with(cx, |panel, _| panel.ghost_text.clone()),
                Some(remaining.into())
            );
        }
        panel.update_in(cx, |panel, window, cx| panel.accept_ghost_text(window, cx));
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.value().to_string()),
            "let value = calculate()"
        );
        for selection in [0..0, 0..12] {
            panel.update(cx, |panel, cx| {
                panel
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(12..12, cx));
            });
            cx.run_until_parked();
            reply(request("second", cx));
            cx.run_until_parked();
            panel.update_in(cx, |panel, window, cx| {
                panel.editor.update(cx, |editor, cx| {
                    editor.set_selected_range(selection, cx)
                });
                panel.accept_ghost_text(window, cx);
            });
            assert_eq!(
                editor.read_with(cx, |editor, _| editor.value().to_string()),
                "let value = calculate()",
                "acceptance rechecks caret/selection before observers run"
            );
        }
    }

    #[gpui_kit::test]
    fn inline_prediction_cancels_pending_work_on_editor_and_route_changes(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::ahead::AheadNotification;
        use ahead_rpc::proxy::{ProxyNotification, ProxyRequest, ProxyRpc};
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("disposable workspace");
        let path = directory.path().join("main.rs");
        std::fs::write(&path, "let value = ").expect("source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("source path"), window, cx)
        });
        panel.update(cx, |panel, cx| {
            panel.proxy = Some(proxy.clone());
            panel.watch_prediction_context(&proxy, cx);
        });
        for change in [
            "cursor",
            "selection",
            "session",
            "provider",
            "session-context",
            "invalidate",
            "supersede",
            "escape",
        ] {
            panel.update(cx, |panel, cx| {
                panel.set_code_comments(Some("session".into()), Vec::new(), cx);
                panel
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(12..12, cx));
            });
            cx.run_until_parked();
            rpc.rx().try_iter().for_each(drop);
            panel.update(cx, |panel, cx| panel.request_proxy_inline(cx));
            let ProxyRpc::Request(id, ProxyRequest::GetInlineCompletions { .. }) =
                rpc.rx().try_recv().expect("pending prediction")
            else {
                panic!("inline request")
            };
            cx.run_until_parked();
            proxy.route_ahead(AheadNotification::PredictionContextChanged {
                session_id: Some("unrelated".into()),
            });
            proxy.sync_editor_snapshot(
                directory.path().join("unrelated.md"),
                change.into(),
            );
            cx.run_until_parked();
            assert!(
                !rpc.rx().try_iter().any(|message| matches!(
                    message,
                    ProxyRpc::Notification(
                        ProxyNotification::CancelInlineCompletions { .. }
                    )
                )),
                "unrelated session/buffer changes retain the pending request"
            );
            panel.update_in(cx, |panel, window, cx| match change {
                "cursor" => panel
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(0..0, cx)),
                "selection" => panel
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(0..12, cx)),
                "session" => {
                    panel.set_code_comments(Some("other".into()), Vec::new(), cx)
                }
                "provider" => {
                    proxy.route_ahead(AheadNotification::PredictionContextChanged {
                        session_id: None,
                    })
                }
                "session-context" => {
                    proxy.route_ahead(AheadNotification::PredictionContextChanged {
                        session_id: Some("session".into()),
                    })
                }
                "invalidate" => panel.invalidate_editor_requests(),
                "supersede" => panel.request_proxy_inline(cx),
                "escape" => panel.handle_key(
                    &gpui_kit::KeyDownEvent {
                        keystroke: gpui_kit::Keystroke::parse("escape")
                            .expect("escape"),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                ),
                _ => unreachable!(),
            });
            cx.run_until_parked();
            let messages = rpc.rx().try_iter().collect::<Vec<_>>();
            assert!(messages.iter().any(|message| matches!(message,
                ProxyRpc::Notification(ProxyNotification::CancelInlineCompletions { request_id }) if *request_id == id
            )), "{change} must cancel the exact pending request");
            if change == "supersede" {
                let newer = messages
                    .iter()
                    .find_map(|message| match message {
                        ProxyRpc::Request(
                            id,
                            ProxyRequest::GetInlineCompletions { .. },
                        ) => Some(*id),
                        _ => None,
                    })
                    .expect("replacement request");
                assert!(!messages.iter().any(|message| matches!(message,
                    ProxyRpc::Notification(ProxyNotification::CancelInlineCompletions { request_id }) if *request_id == newer
                )), "old task cancellation must not cancel its replacement");
                panel.update(cx, |panel, _| panel.cancel_pending_inline());
                cx.run_until_parked();
            }
        }
    }

    #[gpui_kit::test]
    fn bound_prediction_waits_for_host_validation_and_rechecks_local_edits(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::ahead::{AheadNotification, AheadRequest, PredictionBinding};
        use ahead_rpc::proxy::{ProxyRequest, ProxyResponse, ProxyRpc};
        cx.update(gpui_kit::component::init);
        for change in [
            "valid",
            "stale",
            "error",
            "cursor",
            "session",
            "provider",
            "typing",
            "disconnect",
            "unrelated",
        ] {
            let directory = tempfile::tempdir().expect("disposable workspace");
            let path = directory.path().join("main.rs");
            std::fs::write(&path, "let value = ").expect("source");
            let proxy = crate::proxy_client::ProxyClient::new_for_test(
                directory.path().to_owned(),
            );
            let rpc = proxy.rpc_for_test();
            let (panel, cx) = cx.add_window_view(|window, cx| {
                CodePanel::new(path.to_str().expect("path"), window, cx)
            });
            panel.update(cx, |panel, cx| {
                panel.proxy = Some(proxy.clone());
                panel.watch_prediction_context(&proxy, cx);
                panel.set_code_comments(Some("session".into()), Vec::new(), cx);
                panel
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(12..12, cx));
            });
            cx.run_until_parked();
            rpc.rx().try_iter().for_each(drop);
            panel.update(cx, |panel, cx| panel.request_proxy_inline(cx));
            let ProxyRpc::Request(id, _) =
                rpc.rx().try_recv().expect("prediction request")
            else {
                panic!("request")
            };
            let binding = PredictionBinding {
                version: 1,
                request_id: "prediction".into(),
                context_sha256: "opaque-host-digest".into(),
                target_revision: 1,
            };
            rpc.handle_response(
                id,
                Ok(ProxyResponse::GetInlineCompletions {
                    prediction: Some(binding.clone()),
                    context_paths: Vec::new(),
                    completions: lsp_types::InlineCompletionResponse::Array(vec![
                        lsp_types::InlineCompletionItem {
                            insert_text: "calculate()".into(),
                            filter_text: None,
                            range: None,
                            command: None,
                            insert_text_format: None,
                        },
                    ]),
                }),
            );
            cx.run_until_parked();
            panel.update_in(cx, |panel, window, cx| {
                panel.accept_ghost_text(window, cx);
                panel.accept_ghost_text(window, cx);
                assert_eq!(
                    panel.editor.read(cx).value(),
                    "let value = ",
                    "no insertion before host acknowledgement"
                );
            });
            let ProxyRpc::Request(
                validation,
                ProxyRequest::ValidateInlinePrediction {
                    binding: sent,
                    path: sent_path,
                    position,
                    content_sha256,
                    replacement,
                },
            ) = rpc.rx().try_recv().expect("validation request")
            else {
                panic!("validation")
            };
            assert_eq!(sent, binding);
            assert_eq!(sent_path, path);
            assert_eq!(position, lsp_types::Position::new(0, 12));
            assert_eq!(replacement, "calculate()");
            assert_eq!(
                content_sha256,
                format!("{:x}", sha2::Sha256::digest(b"let value = "))
            );
            assert!(
                rpc.rx().is_empty(),
                "repeated Tab must not duplicate validation"
            );
            panel.update_in(cx, |panel, window, cx| match change {
                "cursor" => panel
                    .editor
                    .update(cx, |editor, cx| editor.set_selected_range(0..0, cx)),
                "session" => {
                    panel.set_code_comments(Some("other".into()), Vec::new(), cx)
                }
                "provider" => {
                    proxy.route_ahead(AheadNotification::PredictionContextChanged {
                        session_id: None,
                    })
                }
                "typing" => panel
                    .editor
                    .update(cx, |editor, cx| editor.insert("manual", window, cx)),
                "disconnect" => rpc.disconnect(),
                "unrelated" => {
                    proxy.route_ahead(AheadNotification::PredictionContextChanged {
                        session_id: Some("unrelated".into()),
                    });
                    proxy.sync_editor_snapshot(
                        directory.path().join("unrelated.md"),
                        "changed".into(),
                    );
                }
                _ => {}
            });
            cx.run_until_parked();
            let response = if change == "error" {
                Err(ahead_rpc::RpcError {
                    code: 0,
                    message: "host context unavailable".into(),
                })
            } else {
                Ok(ProxyResponse::ValidateInlinePrediction {
                    valid: change != "stale",
                })
            };
            rpc.handle_response(validation, response);
            cx.run_until_parked();
            let inserted = matches!(change, "valid" | "unrelated");
            let expected = if inserted {
                "let value = calculate()"
            } else if change == "typing" {
                "let value = manual"
            } else {
                "let value = "
            };
            assert_eq!(
                panel.read_with(cx, |panel, cx| panel
                    .editor
                    .read(cx)
                    .value()
                    .to_string()),
                expected,
                "{change}"
            );
            if inserted {
                let acceptance =
                    rpc.rx().try_iter().find_map(|message| match message {
                        ProxyRpc::Request(
                            request_id,
                            ProxyRequest::AheadRequest {
                                request:
                                    AheadRequest::RecordPredictionAcceptance {
                                        session_id,
                                        path,
                                        range,
                                        binding: accepted_binding,
                                        model_context,
                                        accepted_text_sha256,
                                    },
                            },
                        ) => Some((
                            request_id,
                            session_id,
                            path,
                            range,
                            accepted_binding,
                            model_context,
                            accepted_text_sha256,
                        )),
                        _ => None,
                    });
                let (
                    request_id,
                    session_id,
                    accepted_path,
                    range,
                    accepted_binding,
                    model_context,
                    accepted_text_sha256,
                ) = acceptance
                    .expect("accepted FIM is recorded in its AHEAD session");
                assert_eq!(session_id, "session");
                assert_eq!(accepted_path, "main.rs");
                assert_eq!(
                    range.start,
                    ahead_rpc::ahead::DisplayPosition { line: 0, col: 12 }
                );
                assert_eq!(
                    range.end,
                    ahead_rpc::ahead::DisplayPosition { line: 0, col: 23 }
                );
                assert_eq!(accepted_binding, binding);
                assert_eq!(model_context, None);
                assert_eq!(
                    accepted_text_sha256,
                    format!("{:x}", sha2::Sha256::digest(b"calculate()"))
                );
                rpc.handle_response(
                    request_id,
                    Ok(ProxyResponse::AheadResponse {
                        response: serde_json::Value::Null,
                    }),
                );
                cx.run_until_parked();
                let editor = panel.read_with(cx, |panel, _| panel.editor.clone());
                let focus = editor.update(cx, |editor, cx| editor.focus_handle(cx));
                cx.update(|window, cx| {
                    window.draw(cx).clear(cx);
                    window.focus(&focus, cx);
                    focus.dispatch_action(&Undo, window, cx);
                });
                assert_eq!(
                    editor.read_with(cx, |editor, _| editor.value().to_string()),
                    "let value = "
                );
            }
        }
    }

    #[test]
    fn rename_placeholder_supports_ranges_and_default_behavior_with_unicode() {
        use lsp_types::{Position, PrepareRenameResponse, Range};

        let text = "let café = 1;\n";
        let position = Position::new(0, 6);
        let range = Range::new(Position::new(0, 4), Position::new(0, 8));
        assert_eq!(
            rename_placeholder(
                text,
                position,
                PrepareRenameResponse::RangeWithPlaceholder {
                    range,
                    placeholder: "café".into(),
                },
            )
            .expect("LSP placeholder"),
            "café"
        );
        assert_eq!(
            rename_placeholder(
                text,
                position,
                PrepareRenameResponse::DefaultBehavior {
                    default_behavior: true,
                },
            )
            .expect("word at caret"),
            "café"
        );
    }

    #[gpui_kit::test]
    fn symbol_rename_emits_the_complete_workspace_edit_after_inline_confirmation(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::proxy::{ProxyRequest, ProxyResponse, ProxyRpc};
        use lsp_types::{
            Position, PrepareRenameResponse, Range, TextEdit, Url, WorkspaceEdit,
        };
        use std::collections::{HashMap, HashSet};

        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("disposable workspace");
        let path = workspace.path().join("main.rs");
        let other_path = workspace.path().join("lib.rs");
        let source = "fn greet() {}\n";
        std::fs::write(&path, source).expect("source file");
        std::fs::write(&other_path, "greet()\n").expect("referencing file");

        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            workspace.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        let (request_sender, request_receiver) = std::sync::mpsc::channel();
        panel.update(cx, |_, cx| {
            let panel_entity = cx.entity();
            cx.subscribe(
                &panel_entity,
                move |_, _, request: &super::WorkspaceEditRequest, _| {
                    request_sender
                        .send(request.clone())
                        .expect("rename event receiver");
                },
            )
            .detach();
        });
        let rename_position = Position::new(0, 4);
        panel.update_in(cx, |panel, window, cx| {
            panel.proxy = Some(proxy.clone());
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(4..4, cx);
            });
            panel.prepare_symbol_rename(window, cx);
        });

        let (prepare_id, prepare_path, prepare_position) = rpc
            .rx()
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    id,
                    ProxyRequest::PrepareRename { path, position },
                ) => Some((id, path, position)),
                _ => None,
            })
            .expect("prepare-rename request");
        assert_eq!(prepare_path, path);
        assert_eq!(prepare_position, rename_position);
        rpc.handle_response(
            prepare_id,
            Ok(ProxyResponse::PrepareRename {
                resp: PrepareRenameResponse::RangeWithPlaceholder {
                    range: Range::new(Position::new(0, 3), Position::new(0, 8)),
                    placeholder: "greet".into(),
                },
            }),
        );
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.rename_input.read(cx).value(), "greet");
            assert!(panel.pending_rename.is_some());
        });

        panel.update_in(cx, |panel, window, cx| {
            panel.rename_input.update(cx, |input, cx| {
                input.set_value("welcome", window, cx);
            });
            panel.confirm_symbol_rename(window, cx);
        });
        let (rename_id, request_path, request_position, new_name) = rpc
            .rx()
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    id,
                    ProxyRequest::Rename {
                        path,
                        position,
                        new_name,
                    },
                ) => Some((id, path, position, new_name)),
                _ => None,
            })
            .expect("rename request");
        assert_eq!(request_path, path);
        assert_eq!(request_position, rename_position);
        assert_eq!(new_name, "welcome");

        let mut edit = WorkspaceEdit::default();
        edit.changes = Some(HashMap::from([
            (
                Url::from_file_path(&path).expect("source URL"),
                vec![TextEdit {
                    range: Range::new(Position::new(0, 3), Position::new(0, 8)),
                    new_text: "welcome".into(),
                }],
            ),
            (
                Url::from_file_path(&other_path).expect("reference URL"),
                vec![TextEdit {
                    range: Range::new(Position::new(0, 0), Position::new(0, 5)),
                    new_text: "welcome".into(),
                }],
            ),
        ]));
        rpc.handle_response(rename_id, Ok(ProxyResponse::Rename { edit }));
        cx.run_until_parked();

        let request = request_receiver.try_recv().expect("workspace edit event");
        assert_eq!(request.source_path, path);
        assert!(matches!(
            &request.purpose,
            super::WorkspaceEditPurpose::Rename { old_name, new_name }
                if old_name == "greet" && new_name == "welcome"
        ));
        let documents = super::workspace_edit_text_edits(&request.edit)
            .expect("complete workspace edit");
        assert_eq!(
            documents
                .iter()
                .map(|document| document.path.clone())
                .collect::<HashSet<_>>(),
            HashSet::from([path.clone(), other_path.clone()])
        );
        assert_eq!(
            documents
                .iter()
                .map(|document| document.edits[0].new_text.as_str())
                .collect::<HashSet<_>>(),
            HashSet::from(["welcome"])
        );
        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.editor.read(cx).value(), source);
            assert!(panel.pending_rename.is_none());
        });
    }

    #[test]
    fn code_comment_moves_with_a_unique_quote_and_hides_ambiguous_ranges() {
        use ahead_rpc::ahead::{CodeComment, DisplayPosition, DisplayRange};
        let source = "fn selected() {}\n";
        let comment = CodeComment {
            id: "comment".into(),
            session_id: "session".into(),
            actor_id: "bob".into(),
            path: "src/lib.rs".into(),
            range: DisplayRange {
                start: DisplayPosition { line: 0, col: 3 },
                end: DisplayPosition { line: 0, col: 11 },
            },
            quote: "selected".into(),
            source_sha256: format!("{:x}", sha2::Sha256::digest(source.as_bytes())),
            body: "Review this".into(),
            created_at: String::new(),
            resolved_at: None,
            resolved_by: None,
        };
        assert_eq!(current_comment_range(&comment, source), Some(comment.range));
        assert_eq!(
            current_comment_range(&comment, "// moved\nfn selected() {}\n")
                .map(|range| range.start),
            Some(DisplayPosition { line: 1, col: 3 })
        );
        assert_eq!(
            current_comment_range(&comment, "// 😀 selected\n")
                .map(|range| range.start),
            Some(DisplayPosition { line: 0, col: 6 })
        );
        assert!(current_comment_range(&comment, "fn gone() {}\n").is_none());
        assert!(
            current_comment_range(&comment, "fn selected() {}\nfn selected() {}\n")
                .is_none()
        );
    }

    #[gpui_kit::test]
    fn host_editor_applies_clean_shared_change_and_preserves_conflicting_local_text(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::ahead::{AheadNotification, SharedBufferSnapshot};
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let path = directory.path().join("shared.rs");
        std::fs::write(&path, "old\n").expect("source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx).with_proxy(
                proxy.clone(),
                directory.path().to_str().expect("workspace"),
                window,
                cx,
            )
        });
        proxy.route_ahead(AheadNotification::SharedBufferChanged {
            session_id: "session".into(),
            previous_content: "old\n".into(),
            snapshot: SharedBufferSnapshot {
                path: "shared.rs".into(),
                revision: 2,
                content: "guest\n".into(),
            },
        });
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.editor.read(cx).value(), "guest\n");
            assert!(panel.shared_conflict.is_none());
            assert!(panel.host_shared_recovery_pending);
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.set_value("host\n", window, cx);
            });
        });
        proxy.route_ahead(AheadNotification::SharedBufferChanged {
            session_id: "session".into(),
            previous_content: "guest\n".into(),
            snapshot: SharedBufferSnapshot {
                path: "shared.rs".into(),
                revision: 3,
                content: "guest again\n".into(),
            },
        });
        cx.run_until_parked();
        panel.update_in(cx, |panel, window, cx| {
            assert_eq!(panel.editor.read(cx).value(), "host\n");
            assert_eq!(
                panel
                    .shared_conflict
                    .as_ref()
                    .map(|snapshot| snapshot.revision),
                Some(3)
            );
            panel.resolve_shared_conflict(false, window, cx);
            assert_eq!(panel.editor.read(cx).value(), "guest again\n");
            assert!(panel.shared_conflict.is_none());
        });
    }

    #[gpui_kit::test]
    fn guest_editor_records_unsynced_text_in_local_recovery(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::ahead::{AheadRequest, SharedBufferSnapshot};
        use ahead_rpc::proxy::{ProxyRequest, ProxyRpc};
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        proxy.enable_editor_recovery();
        let rpc = proxy.rpc_for_test();
        let session_id = uuid::Uuid::new_v4().to_string();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new_shared(
                session_id.clone(),
                SharedBufferSnapshot {
                    path: "shared.rs".into(),
                    revision: 1,
                    content: "host\n".into(),
                },
                true,
                window,
                cx,
            )
            .with_proxy(
                proxy.clone(),
                directory.path().to_str().expect("workspace"),
                window,
                cx,
            )
        });
        panel.update_in(cx, |panel, window, cx| {
            panel
                .editor
                .update(cx, |editor, cx| editor.replace_all("guest\n", window, cx));
        });
        cx.run_until_parked();
        panel.update(cx, |panel, cx| {
            assert!(panel.dirty);
            assert!(panel.queue_recovery(false, true, cx).is_some());
        });
        let snapshot = rpc
            .rx()
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    _,
                    ProxyRequest::AheadRequest {
                        request: AheadRequest::WriteEditorRecovery { snapshot },
                    },
                ) => Some(snapshot),
                _ => None,
            })
            .expect("guest recovery request");
        assert_eq!(
            snapshot.path,
            shared_draft_path(&session_id, "shared.rs").expect("draft path")
        );
        assert_eq!(snapshot.contents.as_deref(), Some("guest\n"));
    }

    #[gpui_kit::test]
    fn guest_editor_restores_private_draft_and_detects_changed_host(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::ahead::SharedBufferSnapshot;
        use ahead_rpc::file::EditorRecoverySnapshot;
        cx.update(gpui_kit::component::init);
        let session_id = uuid::Uuid::new_v4().to_string();
        let path = shared_draft_path(&session_id, "shared.rs").expect("draft path");
        assert!(shared_draft_path(&session_id, "../private.rs").is_none());
        assert!(is_shared_draft_path(&path));
        assert_eq!(
            shared_draft_file_path(&path, &session_id)
                .and_then(std::path::Path::to_str),
            Some("shared.rs")
        );
        assert!(
            shared_draft_file_path(&path, &uuid::Uuid::new_v4().to_string())
                .is_none()
        );
        assert!(
            shared_draft_file_path(
                std::path::Path::new(&format!(
                    ".ahead/shared-drafts/{session_id}/../private.rs"
                )),
                &session_id,
            )
            .is_none()
        );
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new_shared(
                session_id.clone(),
                SharedBufferSnapshot {
                    path: "shared.rs".into(),
                    revision: 2,
                    content: "host changed\n".into(),
                },
                true,
                window,
                cx,
            )
        });
        panel.update_in(cx, |panel, window, cx| {
            panel
                .restore_shared_draft(
                    EditorRecoverySnapshot {
                        buffer_id: uuid::Uuid::new_v4().to_string(),
                        revision: 1,
                        path,
                        contents: Some("unsynced local\n".into()),
                        saved_sha256: Some(format!(
                            "{:x}",
                            sha2::Sha256::digest(b"original host\n")
                        )),
                    },
                    window,
                    cx,
                )
                .expect("restore draft");
            assert_eq!(panel.editor.read(cx).value(), "unsynced local\n");
            assert!(panel.dirty);
            assert_eq!(
                panel
                    .shared_conflict
                    .as_ref()
                    .map(|snapshot| snapshot.revision),
                Some(2)
            );
        });
    }

    #[gpui_kit::test]
    fn guest_editor_keeps_local_text_on_stale_revision(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::ahead::SharedBufferSnapshot;
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new_shared(
                "session".into(),
                SharedBufferSnapshot {
                    path: "shared.rs".into(),
                    revision: 1,
                    content: "base\n".into(),
                },
                true,
                window,
                cx,
            )
        });
        panel.update_in(cx, |panel, window, cx| {
            panel
                .editor
                .update(cx, |editor, cx| editor.set_value("local\n", window, cx));
            panel.apply_shared_snapshot(
                SharedBufferSnapshot {
                    path: "shared.rs".into(),
                    revision: 2,
                    content: "remote\n".into(),
                },
                window,
                cx,
            );
            assert_eq!(panel.editor.read(cx).value(), "local\n");
            assert_eq!(
                panel
                    .shared_conflict
                    .as_ref()
                    .map(|snapshot| snapshot.revision),
                Some(2)
            );
        });
    }

    #[gpui_kit::test]
    fn guest_editor_retains_unsynced_text_after_role_loss(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::ahead::SharedBufferSnapshot;
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new_shared(
                "session".into(),
                SharedBufferSnapshot {
                    path: "shared.rs".into(),
                    revision: 1,
                    content: "base\n".into(),
                },
                true,
                window,
                cx,
            )
        });
        panel.update_in(cx, |panel, window, cx| {
            panel
                .editor
                .update(cx, |editor, cx| editor.set_value("unsynced\n", window, cx));
            panel.set_shared_editable(false, cx);
            panel.apply_shared_snapshot(
                SharedBufferSnapshot {
                    path: "shared.rs".into(),
                    revision: 2,
                    content: "remote\n".into(),
                },
                window,
                cx,
            );
            assert_eq!(panel.editor.read(cx).value(), "unsynced\n");
            assert!(panel.dirty);
            assert!(panel.shared_conflict.is_some());
            assert!(!panel.shared_editable);
            panel.set_shared_editable(true, cx);
            assert_eq!(panel.editor.read(cx).value(), "unsynced\n");
            assert!(panel.shared_conflict.is_some());
            assert!(panel.status.contains("conflict"));
        });
    }

    #[gpui_kit::test]
    fn two_editor_windows_converge_after_a_guest_edit(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::ahead::{
            AheadNotification, AheadRequest, SharedBufferEditResult,
            SharedBufferSnapshot,
        };
        use ahead_rpc::proxy::{ProxyRequest, ProxyResponse, ProxyRpc};
        cx.update(gpui_kit::component::init);
        let host_workspace = tempfile::tempdir().expect("host workspace");
        let guest_workspace = tempfile::tempdir().expect("guest workspace");
        let host_path = host_workspace.path().join("shared.rs");
        std::fs::write(&host_path, "original\n").expect("host file");
        let host_proxy = crate::proxy_client::ProxyClient::new_for_test(
            host_workspace.path().to_owned(),
        );
        let guest_proxy = crate::proxy_client::ProxyClient::new_for_test(
            guest_workspace.path().to_owned(),
        );
        let (host_panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(host_path.to_str().expect("host path"), window, cx)
                .with_proxy(
                    host_proxy.clone(),
                    host_workspace.path().to_str().expect("workspace"),
                    window,
                    cx,
                )
        });
        let (guest_panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new_shared(
                "session".into(),
                SharedBufferSnapshot {
                    path: "shared.rs".into(),
                    revision: 1,
                    content: "original\n".into(),
                },
                true,
                window,
                cx,
            )
            .with_proxy(
                guest_proxy.clone(),
                guest_workspace.path().to_str().expect("workspace"),
                window,
                cx,
            )
        });
        let guest_rpc = guest_proxy.rpc_for_test();
        let responder = std::thread::spawn(move || {
            while let Ok(message) = guest_rpc
                .rx()
                .recv_timeout(std::time::Duration::from_secs(5))
            {
                if let ProxyRpc::Request(
                    id,
                    ProxyRequest::AheadRequest {
                        request:
                            AheadRequest::ReplaceSharedBuffer {
                                session_id,
                                path,
                                expected_revision,
                                content,
                            },
                    },
                ) = message
                {
                    assert_eq!(session_id, "session");
                    assert_eq!(path, "shared.rs");
                    assert_eq!(expected_revision, 1);
                    assert_eq!(content, "guest edit\n");
                    let snapshot = SharedBufferSnapshot {
                        path,
                        revision: 2,
                        content,
                    };
                    guest_rpc.handle_response(
                        id,
                        Ok(ProxyResponse::AheadResponse {
                            response: serde_json::to_value(SharedBufferEditResult {
                                applied: true,
                                snapshot: snapshot.clone(),
                            })
                            .expect("edit response"),
                        }),
                    );
                    return (session_id, snapshot);
                }
            }
            panic!("guest edit was not sent");
        });
        guest_panel.update_in(cx, |panel, window, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.replace_all("guest edit\n", window, cx)
            });
        });
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(150));
        cx.run_until_parked();
        let (session_id, snapshot) =
            responder.join().expect("shared edit responder");
        host_proxy.route_ahead(AheadNotification::SharedBufferChanged {
            session_id,
            snapshot,
            previous_content: "original\n".into(),
        });
        cx.run_until_parked();
        host_panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.editor.read(cx).value(), "guest edit\n");
            assert!(panel.host_shared_recovery_pending);
        });
        guest_panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.editor.read(cx).value(), "guest edit\n");
            assert!(!panel.dirty);
            assert_eq!(panel.shared_revision, Some(2));
        });
        assert_eq!(
            std::fs::read_to_string(host_path).expect("host disk"),
            "original\n"
        );
    }

    #[gpui_kit::test]
    fn guest_editor_sends_revision_checked_live_edit(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::ahead::{
            AheadRequest, SharedBufferEditResult, SharedBufferSnapshot,
        };
        use ahead_rpc::proxy::{ProxyRequest, ProxyResponse, ProxyRpc};
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new_shared(
                "session".into(),
                SharedBufferSnapshot {
                    path: "shared.rs".into(),
                    revision: 1,
                    content: "base\n".into(),
                },
                true,
                window,
                cx,
            )
            .with_proxy(
                proxy.clone(),
                directory.path().to_str().expect("workspace"),
                window,
                cx,
            )
        });
        let responder = std::thread::spawn(move || {
            while let Ok(message) =
                rpc.rx().recv_timeout(std::time::Duration::from_secs(5))
            {
                if let ProxyRpc::Request(
                    id,
                    ProxyRequest::AheadRequest {
                        request:
                            AheadRequest::ReplaceSharedBuffer {
                                session_id,
                                path,
                                expected_revision,
                                content,
                            },
                    },
                ) = message
                {
                    assert_eq!(
                        (
                            session_id.as_str(),
                            path.as_str(),
                            expected_revision,
                            content.as_str()
                        ),
                        ("session", "shared.rs", 1, "edited\n")
                    );
                    rpc.handle_response(
                        id,
                        Ok(ProxyResponse::AheadResponse {
                            response: serde_json::to_value(SharedBufferEditResult {
                                applied: true,
                                snapshot: SharedBufferSnapshot {
                                    path,
                                    revision: 2,
                                    content,
                                },
                            })
                            .expect("response"),
                        }),
                    );
                    return;
                }
            }
            panic!("guest edit request was not sent");
        });
        panel.update_in(cx, |panel, window, cx| {
            panel
                .editor
                .update(cx, |editor, cx| editor.replace_all("edited\n", window, cx));
        });
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(150));
        cx.run_until_parked();
        responder.join().expect("responder");
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.shared_revision, Some(2));
            assert!(!panel.dirty);
            assert_eq!(panel.editor.read(cx).value(), "edited\n");
        });
    }

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
            &b"\xff\xfe\x00"[..],
            &b"\xff\xfe\x00\xd8"[..],
            &b"h\0i\0"[..],
            &b"caf\xe9"[..],
            &b"truncated \xf0\x9f"[..],
        ] {
            std::fs::write(path, bytes).expect("unsupported data");
            assert!(matches!(
                read_editor_file(path),
                Err(FileOpenError::UnsupportedText)
            ));
            assert_eq!(std::fs::read(path).expect("unchanged file"), bytes);
        }
        for (bytes, expected) in [
            (&b"\xff\xfeh\0i\0"[..], "hi"),
            (&b"\xfe\xff\0h\0i"[..], "hi"),
        ] {
            std::fs::write(path, bytes).expect("UTF-16 text");
            assert_eq!(read_editor_file(path).expect("UTF-16 text"), expected);
        }
        std::fs::remove_file(path).expect("remove fixture");
        assert!(
            matches!(read_editor_file(path), Err(FileOpenError::Read(error)) if error.kind() == std::io::ErrorKind::NotFound)
        );
        let denied = FileOpenError::Read(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        ));
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
        let utf16 = directory.path().join("utf16.txt");
        let utf16_text = "FIM context 日本語 🦀\r\n";
        std::fs::write(
            &utf16,
            ahead_core::encoding::encode_text(
                utf16_text,
                ahead_core::encoding::TextEncoding::Utf16LeBom,
            ),
        )
        .expect("UTF-16 text");
        let missing = directory.path().join("missing.txt");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        proxy.enable_editor_recovery();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(binary.to_str().expect("path"), window, cx).with_proxy(
                proxy.clone(),
                directory.path().to_str().expect("workspace"),
                window,
                cx,
            )
        });
        panel.update_in(cx, |panel, window, cx| {
            assert!(!panel.is_text_available());
            assert!(!panel.editor.read(cx).is_editable());
            assert!(panel.turn_context(cx).is_none());
            panel.save(window, cx);
            assert!(panel.queue_recovery(false, true, cx).is_none());
        });
        assert!(
            rpc.rx().try_recv().is_err(),
            "blocked tab must not reach the proxy"
        );

        panel.update_in(cx, |panel, window, cx| {
            panel.open_file(text.to_str().expect("path"), true, window, cx);
            assert!(panel.is_text_available());
            assert!(panel.editor.read(cx).is_editable());
            assert!(panel.editor.read(cx).value().is_empty());
        });
        assert!(rpc.rx().try_iter().any(|message| matches!(message,
            ProxyRpc::Notification(ProxyNotification::EditorSnapshot { path, content }) if path == text && content.is_empty()
        )), "a real empty file is a valid text buffer");

        panel.update_in(cx, |panel, window, cx| {
            panel.open_file(utf16.to_str().expect("path"), true, window, cx);
            assert!(panel.is_text_available());
            assert!(panel.editor.read(cx).is_editable());
            assert_eq!(panel.editor.read(cx).value().as_ref(), utf16_text);
        });
        assert!(rpc.rx().try_iter().any(|message| matches!(message,
            ProxyRpc::Notification(ProxyNotification::EditorSnapshot { path, content }) if path == utf16 && content == utf16_text
        )), "decoded UTF-16 text reaches the editor buffer");

        for path in [&binary, &missing] {
            panel.update_in(cx, |panel, window, cx| {
                panel.open_file(path.to_str().expect("path"), true, window, cx);
                assert_eq!(panel.file_path, path.to_str().expect("path"));
                assert!(!panel.is_text_available());
                assert!(!panel.editor.read(cx).is_editable());
                assert!(!panel.dirty);
                assert!(panel.turn_context(cx).is_none());
                panel.save(window, cx);
                window.focus(&panel.focus, cx);
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert!(cx.debug_bounds("file-open-error").is_some());
            cx.simulate_keystrokes("x cmd-v cmd-s");
            assert!(!panel.read_with(cx, |panel, _| panel.dirty));
            assert!(
                !rpc.rx().try_iter().any(|message| matches!(
                    message,
                    ProxyRpc::Notification(ProxyNotification::EditorSnapshot { .. })
                        | ProxyRpc::Request(
                            _,
                            ProxyRequest::SaveEditorBuffer { .. }
                        )
                )),
                "blocked tabs cannot publish or save empty buffers"
            );
        }
        assert_eq!(std::fs::read(&binary).expect("binary unchanged"), bytes);
        assert!(!missing.exists());
        panel.update_in(cx, |panel, window, cx| {
            panel.open_file(text.to_str().expect("path"), true, window, cx);
            panel
                .editor
                .update(cx, |editor, cx| editor.replace_all("unsaved", window, cx));
        });
        panel.update_in(cx, |panel, window, cx| {
            assert!(panel.dirty);
            panel.open_file(binary.to_str().expect("path"), true, window, cx);
            assert_eq!(panel.file_path, text.to_str().expect("path"));
            assert_eq!(panel.editor.read(cx).value().as_ref(), "unsaved");
        });
    }

    #[gpui_kit::test]
    fn file_open_recovery_preserves_missing_text_without_unlocking_binary_files(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let path = directory.path().join("missing.txt");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        let snapshot = ahead_rpc::file::EditorRecoverySnapshot {
            buffer_id: "recovered-buffer".into(),
            revision: 1,
            path: "missing.txt".into(),
            contents: Some("recovered text".into()),
            saved_sha256: Some("previous version".into()),
        };
        panel.update_in(cx, |panel, window, cx| {
            panel
                .restore_recovery(snapshot.clone(), window, cx)
                .expect("recover missing file");
            assert!(panel.is_text_available());
            assert!(panel.editor.read(cx).is_editable());
            assert!(panel.recovery_conflict);
        });
        assert!(panel.read_with(cx, |panel, _| panel.dirty));
        assert!(!path.exists(), "recovery never creates the disk file");

        let binary = directory.path().join("session.db");
        std::fs::write(&binary, b"SQLite format 3\0").expect("binary fixture");
        let blocked = cx.new_window_entity(|window, cx| {
            CodePanel::new(binary.to_str().expect("path"), window, cx)
        });
        blocked.update_in(cx, |panel, window, cx| {
            assert!(panel.restore_recovery(snapshot, window, cx).is_err());
            assert!(!panel.is_text_available());
            assert!(!panel.dirty);
        });
        assert_eq!(
            std::fs::read(binary).expect("unchanged binary"),
            b"SQLite format 3\0"
        );
    }

    #[test]
    fn completions_fuzzy_rank_before_five_item_limit_and_honor_server_sort_keys() {
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
                sort_text: Some("02".into()),
                ..Default::default()
            },
            lsp_types::CompletionItem {
                label: "first label".into(),
                filter_text: Some("doubled".into()),
                sort_text: Some("01".into()),
                ..Default::default()
            },
        ]);
        items.push(lsp_types::CompletionItem {
            label: "d_o_u".into(),
            sort_text: Some("00".into()),
            ..Default::default()
        });
        items.extend((3..=12).map(|index| lsp_types::CompletionItem {
            label: format!("candidate_{index}"),
            filter_text: Some("doubled".into()),
            sort_text: Some(format!("{index:02}")),
            ..Default::default()
        }));
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
            vec![
                "first label",
                "second label",
                "candidate_3",
                "candidate_4",
                "candidate_5",
            ]
        );
        assert_eq!(matches.len(), 5);
        assert!(!matches.iter().any(|item| item.item.label == "d_o_u"));
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
        let (range, replacement, caret, _) =
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
        let snippet_item = CompletionItem {
            label: "div".into(),
            insert_text: Some("<div>$0</div>".into()),
            insert_text_format: Some(lsp_types::InsertTextFormat::SNIPPET),
            ..Default::default()
        };
        let (range, replacement, caret, tabstops) =
            completion_edit("", Position::new(0, 0), &snippet_item)
                .expect("valid snippet completion");
        assert_eq!(range, 0..0);
        assert_eq!(replacement, "<div></div>");
        assert_eq!(caret, 5);
        assert_eq!(tabstops.expect("snippet tabstop")[0].ranges, vec![5..5]);

        let variable_source = "let result = value";
        let variable_position =
            Position::new(0, variable_source.encode_utf16().count() as u32);
        let variables = lsp_snippet::Variables::for_completion(
            variable_source,
            variable_source.len(),
            0,
            std::path::Path::new("/workspace/src/main.rs"),
            std::path::Path::new("/workspace"),
            "",
            "",
            None,
        );
        let variable_item = CompletionItem {
            label: "call".into(),
            insert_text: Some("${TM_FILENAME_BASE}(${TM_CURRENT_WORD})$0".into()),
            insert_text_format: Some(lsp_types::InsertTextFormat::SNIPPET),
            ..Default::default()
        };
        let (_, replacement, _, _) = completion_edit_with_variables(
            variable_source,
            variable_position,
            &variable_item,
            &variables,
        )
        .expect("completion expands variables");
        assert_eq!(replacement, "main(value)");

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
        let (_, replacement, caret, _) =
            completion_edit("calc", Position::new(0, 4), &boundary)
                .expect("boundary insertion");
        assert_eq!(
            replacement,
            "import { calculate } from './math';\ncalculate"
        );
        assert_eq!(caret, replacement.len());
    }

    #[gpui_kit::test]
    fn format_document_applies_proxy_edits_as_one_undoable_change(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::proxy::{ProxyRequest, ProxyResponse, ProxyRpc};
        use lsp_types::{Position, Range, TextEdit};

        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("disposable workspace");
        let path = directory.path().join("main.rs");
        let original = "fn main(){let count=1;}\n";
        std::fs::write(&path, original).expect("source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.proxy = Some(proxy);
            let cursor = original.find("1;").expect("value") + 1;
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(cursor..cursor, cx);
            });
            panel.format_document(window, cx);
        });
        let (request_id, request_path) = rpc
            .rx()
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    id,
                    ProxyRequest::GetDocumentFormatting { path },
                ) => Some((id, path)),
                _ => None,
            })
            .expect("formatting request");
        assert_eq!(request_path, path);
        rpc.handle_response(
            request_id,
            Ok(ProxyResponse::GetDocumentFormatting {
                edits: vec![TextEdit {
                    range: Range::new(Position::new(0, 0), Position::new(1, 0)),
                    new_text: "fn main() {\n    let count = 1;\n}\n".into(),
                }],
            }),
        );
        cx.run_until_parked();

        let formatted = "fn main() {\n    let count = 1;\n}\n";
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).value().to_string()),
            formatted
        );
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).selected_range()),
            formatted.find("1;").expect("formatted value") + 1
                ..formatted.find("1;").expect("formatted value") + 1
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

        panel.update_in(cx, |panel, window, cx| {
            panel.format_document(window, cx);
        });
        let stale_request_id = rpc
            .rx()
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    id,
                    ProxyRequest::GetDocumentFormatting { .. },
                ) => Some(id),
                _ => None,
            })
            .expect("second formatting request");
        panel.update_in(cx, |panel, window, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.replace_all("human edit\n", window, cx);
            });
        });
        rpc.handle_response(
            stale_request_id,
            Ok(ProxyResponse::GetDocumentFormatting {
                edits: vec![TextEdit {
                    range: Range::new(Position::new(0, 0), Position::new(1, 0)),
                    new_text: formatted.into(),
                }],
            }),
        );
        cx.run_until_parked();
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).value().to_string()),
            "human edit\n"
        );
        assert!(panel.update(cx, |panel, _| panel.status.is_empty()));
    }

    #[gpui_kit::test]
    fn signature_help_uses_the_proxy_reply_and_cycles_overloads(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::core::CoreNotification;
        use ahead_rpc::proxy::{ProxyNotification, ProxyRpc};
        use lsp_types::{
            ParameterInformation, ParameterLabel, Position, SignatureHelp,
            SignatureHelpTriggerKind, SignatureInformation,
        };

        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("disposable workspace");
        let path = directory.path().join("main.rs");
        let source = "call(value)";
        std::fs::write(&path, source).expect("source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        let cursor = source.find("value").expect("argument");
        panel.update_in(cx, |panel, _, cx| {
            panel.proxy = Some(proxy.clone());
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(cursor..cursor, cx);
            });
            panel.request_signature_help(cx);
        });

        let (request_id, request_path, position, context) = rpc
            .rx()
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Notification(ProxyNotification::SignatureHelp {
                    request_id,
                    path,
                    position,
                    context,
                }) => Some((request_id, path, position, context)),
                _ => None,
            })
            .expect("signature-help request");
        assert_eq!(request_path, path);
        assert_eq!(position, Position::new(0, cursor as u32));
        assert_eq!(context.trigger_kind, SignatureHelpTriggerKind::INVOKED);
        assert!(!context.is_retrigger);
        assert!(context.active_signature_help.is_none());

        let signature = |label: &str, parameter: &str| SignatureInformation {
            label: label.into(),
            documentation: None,
            parameters: Some(vec![ParameterInformation {
                label: ParameterLabel::Simple(parameter.into()),
                documentation: None,
            }]),
            active_parameter: Some(0),
        };
        proxy.route_core(CoreNotification::SignatureHelpResponse {
            request_id,
            resp: SignatureHelp {
                signatures: vec![
                    signature("call(value: i32)", "value: i32"),
                    signature("call(value: String)", "value: String"),
                ],
                active_signature: Some(0),
                active_parameter: Some(0),
            },
            plugin_id: PluginId(1),
        });
        cx.run_until_parked();

        assert!(panel.update(cx, |panel, _| panel.signature_help.is_some()));
        panel.update_in(cx, |panel, _, cx| {
            assert!(panel.cycle_signature_help(true, cx));
            assert_eq!(
                panel
                    .signature_help
                    .as_ref()
                    .and_then(|help| help.active_signature),
                Some(1)
            );
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert!(cx.debug_bounds("signature-help").is_some());

        panel.update_in(cx, |panel, _, cx| {
            panel.request_signature_help_after_content_change(cx);
        });
        let (retrigger_request_id, retrigger_context) = rpc
            .rx()
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Notification(ProxyNotification::SignatureHelp {
                    request_id,
                    context,
                    ..
                }) => Some((request_id, context)),
                _ => None,
            })
            .expect("signature-help content-change retrigger");
        assert_eq!(
            retrigger_context.trigger_kind,
            SignatureHelpTriggerKind::CONTENT_CHANGE
        );
        assert!(retrigger_context.is_retrigger);
        assert_eq!(
            retrigger_context
                .active_signature_help
                .as_ref()
                .and_then(|help| help.active_signature),
            Some(1)
        );
        proxy.route_core(CoreNotification::SignatureHelpResponse {
            request_id: retrigger_request_id,
            resp: retrigger_context.active_signature_help.unwrap(),
            plugin_id: PluginId(1),
        });
        cx.run_until_parked();

        panel.update_in(cx, |panel, _, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(0..0, cx);
            });
        });
        cx.run_until_parked();
        assert!(panel.update(cx, |panel, _| panel.signature_help.is_none()));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert!(cx.debug_bounds("signature-help").is_none());
    }

    #[test]
    fn signature_parameter_utf16_offsets_map_through_non_bmp_text() {
        use lsp_types::{
            ParameterInformation, ParameterLabel, SignatureHelp,
            SignatureInformation,
        };

        let label = "call(\"🦀\", value: String)";
        let start = label
            .split("value: String")
            .next()
            .expect("prefix")
            .encode_utf16()
            .count() as u32;
        let end = start + "value: String".encode_utf16().count() as u32;
        let help = SignatureHelp {
            signatures: vec![SignatureInformation {
                label: label.into(),
                documentation: None,
                parameters: Some(vec![ParameterInformation {
                    label: ParameterLabel::LabelOffsets([start, end]),
                    documentation: None,
                }]),
                active_parameter: Some(0),
            }],
            active_signature: None,
            active_parameter: None,
        };

        let view = signature_help_view(&help).expect("signature");
        assert_eq!(
            &view.label[view.active_parameter.expect("active parameter")],
            "value: String"
        );
    }

    #[test]
    fn workspace_edits_group_by_file_and_active_edit_helper_is_single_file() {
        use lsp_types::{Position, Range, TextEdit, Url, WorkspaceEdit};

        let directory = tempfile::tempdir().expect("disposable workspace");
        let path = directory.path().join("main.rs");
        let other_path = directory.path().join("other.rs");
        let edit_for = |path: &std::path::Path| TextEdit {
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            new_text: path.to_string_lossy().into_owned(),
        };
        let mut edit = WorkspaceEdit::default();
        edit.changes = Some(std::collections::HashMap::from([(
            Url::from_file_path(&path).expect("file URL"),
            vec![edit_for(&path)],
        )]));

        let documents =
            super::workspace_edit_text_edits(&edit).expect("active-file edit");
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].path, path);
        assert_eq!(documents[0].edits[0].new_text, path.to_string_lossy());
        assert_eq!(
            super::active_file_workspace_edit_text_edits(&path, &edit)
                .expect("active-file code action")[0]
                .new_text,
            path.to_string_lossy()
        );

        edit.changes.as_mut().expect("changes").insert(
            Url::from_file_path(&other_path).expect("other file URL"),
            vec![edit_for(&other_path)],
        );
        let documents =
            super::workspace_edit_text_edits(&edit).expect("workspace-wide edits");
        assert_eq!(documents.len(), 2);
        assert!(
            super::active_file_workspace_edit_text_edits(&path, &edit)
                .expect_err("code action remains single-file")
                .contains("another file")
        );
    }

    #[gpui_kit::test]
    fn code_action_menu_applies_an_active_file_edit_as_one_undo(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::plugin::PluginId;
        use ahead_rpc::proxy::{ProxyRequest, ProxyResponse, ProxyRpc};
        use gpui_kit::component::input::Undo;
        use lsp_types::{
            CodeAction, CodeActionOrCommand, Command, Position, Range, TextEdit,
            Url, WorkspaceEdit,
        };

        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("disposable workspace");
        let path = directory.path().join("main.rs");
        let original = "fn main() { let value = 1; }\n";
        std::fs::write(&path, original).expect("source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        let value_offset = original.find('1').expect("value");
        panel.update(cx, |panel, cx| {
            panel.proxy = Some(proxy.clone());
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(value_offset..value_offset, cx);
            });
            panel.request_code_actions(cx);
        });
        let (request_id, request_path, position) = rpc
            .rx()
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    id,
                    ProxyRequest::GetCodeActions {
                        path,
                        position,
                        diagnostics,
                    },
                ) => {
                    assert!(diagnostics.is_empty());
                    Some((id, path, position))
                }
                _ => None,
            })
            .expect("code-action request");
        assert_eq!(request_path, path);
        assert_eq!(position, Position::new(0, value_offset as u32));

        rpc.handle_response(
            request_id,
            Ok(ProxyResponse::GetCodeActionsResponse {
                actions: vec![
                    LspCodeAction {
                        plugin_id: PluginId(1),
                        action: CodeActionOrCommand::Command(Command {
                            title: "Another server's action".into(),
                            command: "fixture.otherServer".into(),
                            arguments: None,
                        }),
                    },
                    LspCodeAction {
                        plugin_id: PluginId(42),
                        action: CodeActionOrCommand::CodeAction(CodeAction {
                            title: "Change 1 to 2".into(),
                            data: Some(serde_json::json!({"lazy": true})),
                            ..Default::default()
                        }),
                    },
                ],
            }),
        );
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert!(cx.debug_bounds("code-actions").is_some());

        panel.update_in(cx, |panel, window, cx| {
            assert_eq!(panel.code_actions.len(), 2);
            assert_eq!(panel.code_actions[0].plugin_id, PluginId(1));
            assert_eq!(panel.code_actions[1].plugin_id, PluginId(42));
            panel.selected_code_action = 1;
            panel.apply_selected_code_action(window, cx);
        });
        let (resolve_id, resolve_plugin_id, unresolved) = rpc
            .rx()
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    id,
                    ProxyRequest::CodeActionResolve {
                        plugin_id,
                        action_item,
                    },
                ) => Some((id, plugin_id, *action_item)),
                _ => None,
            })
            .expect("resolve request for lazy code action");
        assert_eq!(resolve_plugin_id, PluginId(42));
        assert_eq!(unresolved.title, "Change 1 to 2");
        let mut edit = WorkspaceEdit::default();
        edit.changes = Some(std::collections::HashMap::from([(
            Url::from_file_path(&path).expect("file URL"),
            vec![TextEdit {
                range: Range::new(
                    Position::new(0, value_offset as u32),
                    Position::new(0, value_offset as u32 + 1),
                ),
                new_text: "2".into(),
            }],
        )]));
        rpc.handle_response(
            resolve_id,
            Ok(ProxyResponse::CodeActionResolveResponse {
                item: Box::new(CodeAction {
                    title: "Change 1 to 2".into(),
                    edit: Some(edit),
                    command: Some(Command {
                        title: String::new(),
                        command: "fixture.applyCodeAction".into(),
                        arguments: Some(vec![serde_json::json!({"accepted": true})]),
                    }),
                    ..Default::default()
                }),
            }),
        );
        cx.run_until_parked();
        let (command_id, command, arguments) = rpc
            .rx()
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    id,
                    ProxyRequest::ExecuteLspCommand {
                        plugin_id,
                        command,
                        arguments,
                    },
                ) => {
                    assert_eq!(plugin_id, PluginId(42));
                    Some((id, command, arguments))
                }
                _ => None,
            })
            .expect("advertised LSP command request");
        assert_eq!(command, "fixture.applyCodeAction");
        assert_eq!(arguments, [serde_json::json!({"accepted": true})]);
        rpc.handle_response(
            command_id,
            Ok(ProxyResponse::ExecuteLspCommandResponse {}),
        );
        cx.run_until_parked();
        let editor = panel.update(cx, |panel, cx| {
            assert_eq!(
                panel.editor.read(cx).value().to_string(),
                original.replace("= 1;", "= 2;")
            );
            assert!(panel.dirty);
            panel.editor.clone()
        });
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
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.status, "Applied code action");
        });
    }

    #[gpui_kit::test]
    fn cross_file_code_action_waits_for_workspace_edit_before_server_command(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::plugin::PluginId;
        use ahead_rpc::proxy::{ProxyRequest, ProxyResponse, ProxyRpc};
        use lsp_types::{
            CodeAction, CodeActionOrCommand, Command, Position, Range, TextEdit,
            Url, WorkspaceEdit,
        };

        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("disposable workspace");
        let path = directory.path().join("main.rs");
        let other_path = directory.path().join("lib.rs");
        std::fs::write(&path, "fn main() {}\n").expect("source");
        std::fs::write(&other_path, "fn helper() {}\n").expect("other source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        let (request_sender, request_receiver) = std::sync::mpsc::channel();
        panel.update(cx, |_, cx| {
            let panel_entity = cx.entity();
            cx.subscribe(
                &panel_entity,
                move |_, _, request: &super::WorkspaceEditRequest, _| {
                    request_sender
                        .send(request.clone())
                        .expect("workspace edit event receiver");
                },
            )
            .detach();
        });

        let mut edit = WorkspaceEdit::default();
        edit.changes = Some(std::collections::HashMap::from([(
            Url::from_file_path(&other_path).expect("other file URL"),
            vec![TextEdit {
                range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                new_text: "// updated by code action\n".into(),
            }],
        )]));
        panel.update_in(cx, |panel, window, cx| {
            panel.proxy = Some(proxy.clone());
            panel.code_actions = vec![crate::proxy_client::LspCodeAction {
                plugin_id: PluginId(1),
                action: CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Update helper and notify server".into(),
                    edit: Some(edit),
                    command: Some(Command {
                        title: "Refresh project state".into(),
                        command: "fixture.refreshProject".into(),
                        arguments: Some(vec![serde_json::json!({"updated": true})]),
                    }),
                    ..Default::default()
                }),
            }];
            panel.code_actions_position = Some(Position::new(0, 0));
            panel.apply_selected_code_action(window, cx);
        });
        cx.run_until_parked();

        let request = request_receiver
            .try_recv()
            .expect("cross-file workspace edit request");
        assert_eq!(request.source_path, path);
        assert!(matches!(
            &request.purpose,
            super::WorkspaceEditPurpose::CodeAction { title }
                if title == "Update helper and notify server"
        ));
        assert_eq!(
            super::workspace_edit_text_edits(&request.edit)
                .expect("workspace changes")[0]
                .path,
            other_path
        );
        assert!(rpc.rx().try_iter().all(|message| !matches!(
            message,
            ProxyRpc::Request(_, ProxyRequest::ExecuteLspCommand { .. })
        )));

        request.complete(Ok(request.generation));
        cx.run_until_parked();
        let (command_id, plugin_id, command, arguments) = rpc
            .rx()
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    id,
                    ProxyRequest::ExecuteLspCommand {
                        plugin_id,
                        command,
                        arguments,
                    },
                ) => Some((id, plugin_id, command, arguments)),
                _ => None,
            })
            .expect("server command after workspace edit completion");
        assert_eq!(plugin_id, PluginId(1));
        assert_eq!(command, "fixture.refreshProject");
        assert_eq!(arguments, [serde_json::json!({"updated": true})]);
        rpc.handle_response(
            command_id,
            Ok(ProxyResponse::ExecuteLspCommandResponse {}),
        );
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.status, "Applied code action");
        });
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
            if language != "text" {
                let registry =
                    gpui_kit::component::highlighter::LanguageRegistry::singleton();
                assert!(
                    registry
                        .language(language)
                        .expect("registered language")
                        .has_grammar()
                );
                let mut highlighter =
                    gpui_kit::component::highlighter::SyntaxHighlighter::new(
                        language,
                    );
                assert!(highlighter.update(None, &ropey::Rope::from(source), None));
                let tree = highlighter.tree().expect("parsed syntax tree");
                assert!(!tree.root_node().has_error(), "{name} did not parse");
                let theme =
                    gpui_kit::component::highlighter::HighlightTheme::default_dark();
                assert!(
                    highlighter.styles(&(0..source.len()), &*theme).len() > 1,
                    "{name} has no syntax highlight captures"
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
    fn ctrl_space_dispatches_one_lsp_completion_request(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use crate::app::ShowCompletions;
        use ahead_rpc::proxy::{ProxyNotification, ProxyRpc};
        use lsp_types::Position;

        cx.update(gpui_kit::component::init);
        cx.update(|cx| {
            cx.bind_keys([gpui_kit::KeyBinding::new(
                "ctrl-space",
                ShowCompletions,
                Some("AHEADCodeEditor > Input"),
            )]);
        });
        let directory = tempfile::tempdir().expect("workspace");
        let path = directory.path().join("main.ts");
        std::fs::write(&path, "cal\n").expect("source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        cx.run_until_parked();
        panel.update(cx, |panel, cx| {
            panel.proxy = Some(proxy.clone());
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(3..3, cx);
            });
        });
        let editor_focus =
            panel.update(cx, |panel, cx| panel.editor.read(cx).focus_handle(cx));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.focus(&editor_focus, cx);
        });

        cx.simulate_keystrokes("ctrl-space");
        let requests = rpc
            .rx()
            .try_iter()
            .filter_map(|message| match message {
                ProxyRpc::Notification(ProxyNotification::Completion {
                    request_id,
                    path,
                    input,
                    position,
                }) => Some((request_id, path, input, position)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 1, "Ctrl+Space should send one request");
        let (request_id, request_path, request_input, request_position) =
            requests.into_iter().next().expect("completion request");
        assert_eq!(request_path, path);
        assert_eq!(request_input, "cal");
        assert_eq!(request_position, Position::new(0, 3));
        proxy.route_core(ahead_rpc::core::CoreNotification::CompletionResponse {
            request_id,
            input: "cal".into(),
            resp: None,
            plugin_id: ahead_rpc::plugin::PluginId(0),
        });
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn null_lsp_completion_retries_after_indexing(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::proxy::{ProxyNotification, ProxyRpc};

        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("disposable workspace");
        let path = directory.path().join("main.ts");
        std::fs::write(&path, "cal\n").expect("source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        cx.run_until_parked();
        panel.update(cx, |panel, cx| {
            panel.proxy = Some(proxy.clone());
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(3..3, cx);
            });
            panel.request_proxy_completions(cx, false);
        });
        let completion_request_id = || {
            rpc.rx().try_iter().find_map(|message| match message {
                ProxyRpc::Notification(ProxyNotification::Completion {
                    request_id,
                    ..
                }) => Some(request_id),
                _ => None,
            })
        };
        let first_request = completion_request_id().expect("initial request");
        proxy.route_core(ahead_rpc::core::CoreNotification::CompletionResponse {
            request_id: first_request,
            input: "cal".into(),
            resp: None,
            plugin_id: PluginId(1),
        });
        cx.run_until_parked();
        assert!(!panel.update(cx, |panel, _| panel.show_completions));

        cx.background_executor
            .advance_clock(NULL_COMPLETION_RETRY_DELAY);
        cx.run_until_parked();
        let second_request = completion_request_id().expect("retry request");
        assert_ne!(first_request, second_request);
        proxy.route_core(ahead_rpc::core::CoreNotification::CompletionResponse {
            request_id: second_request,
            input: "cal".into(),
            resp: Some(lsp_types::CompletionResponse::Array(vec![
                lsp_types::CompletionItem {
                    label: "calculate".into(),
                    detail: Some("() -> number".into()),
                    ..Default::default()
                },
            ])),
            plugin_id: PluginId(1),
        });
        cx.run_until_parked();
        let (
            show_completions,
            request_id,
            generation,
            path,
            position,
            status,
            labels,
        ) = panel.read_with(cx, |panel, cx| {
            (
                panel.show_completions,
                panel.completion_request_id,
                panel.request_generation,
                panel.proxy_path(),
                panel.cursor_position(cx),
                panel.status.to_string(),
                panel
                    .completions
                    .iter()
                    .map(|completion| completion.item.label.clone())
                    .collect::<Vec<_>>(),
            )
        });
        assert!(
            show_completions,
            "completion response was not displayed: request {request_id}, generation {generation}, path {path:?}, position {position:?}, status {status:?}, labels {labels:?}"
        );
        assert_eq!(labels, ["calculate"]);
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
    fn snippet_completion_navigates_and_tracks_placeholder_edits(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use super::{NextSnippetTabstop, PreviousSnippetTabstop};
        use gpui_kit::component::input::Undo;
        use lsp_types::{CompletionItem, InsertTextFormat, Position};
        cx.update(gpui_kit::component::init);
        cx.update(|cx| {
            cx.bind_keys([
                gpui_kit::KeyBinding::new(
                    "tab",
                    NextSnippetTabstop,
                    Some("(AHEADCodeEditor && tab_action == true) > Input"),
                ),
                gpui_kit::KeyBinding::new(
                    "shift-tab",
                    PreviousSnippetTabstop,
                    Some("(AHEADCodeEditor && tab_action == true) > Input"),
                ),
            ]);
        });
        let directory = tempfile::tempdir().expect("workspace");
        let path = directory.path().join("main.rs");
        std::fs::write(&path, "calc\n").expect("source");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        let item = CompletionItem {
            label: "function".into(),
            insert_text: Some("fn ${1:name}(${2|arg,argument|}) {$0}".into()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        };
        let context = panel.update(cx, |panel, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(4..4, cx);
            });
            panel.current_completion_context(cx)
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.finish_completion(&context, Ok(item.clone()), window, cx)
        });
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).value().to_string()),
            "fn name() {}\n"
        );
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).selected_range()),
            3..7
        );
        assert!(panel.update(cx, |panel, _| panel.snippet_session.is_some()));

        let editor_focus =
            panel.update(cx, |panel, cx| panel.editor.read(cx).focus_handle(cx));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.focus(&editor_focus, cx);
            editor_focus.dispatch_action(&Undo, window, cx);
        });
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).value().to_string()),
            "calc\n"
        );
        assert!(!panel.update(cx, |panel, _| panel.dirty));
        assert!(panel.update(cx, |panel, _| panel.snippet_session.is_none()));

        let context = panel.update(cx, |panel, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(4..4, cx);
            });
            panel.current_completion_context(cx)
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.finish_completion(&context, Ok(item), window, cx)
        });

        let editor_focus =
            panel.update(cx, |panel, cx| panel.editor.read(cx).focus_handle(cx));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.focus(&editor_focus, cx);
        });
        cx.simulate_keystrokes("tab");
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).selected_range()),
            8..11
        );
        cx.simulate_keystrokes("down");
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).value().to_string()),
            "fn name(argument) {}\n"
        );
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).selected_range()),
            8..16
        );
        cx.simulate_keystrokes("up");
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).value().to_string()),
            "fn name(arg) {}\n"
        );
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).selected_range()),
            8..11
        );
        cx.simulate_keystrokes("arg");
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).value().to_string()),
            "fn name(arg) {}\n"
        );
        cx.simulate_keystrokes("shift-tab");
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).selected_range()),
            3..7
        );
        cx.simulate_keystrokes("tab");
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).selected_range()),
            8..11
        );
        cx.simulate_keystrokes("tab");
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).selected_range()),
            14..14
        );
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).value().to_string()),
            "fn name(arg) {}\n"
        );
        assert_eq!(
            panel.update(cx, |panel, cx| panel.cursor_position(cx)),
            Position::new(0, 14)
        );
        assert!(panel.update(cx, |panel, _| panel.snippet_session.is_none()));
        cx.simulate_keystrokes("tab");
        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).value().to_string()),
            "fn name(arg) {    }\n"
        );
    }

    #[gpui_kit::test]
    async fn confirmed_discard_waits_for_shared_recovery_cleanup(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::ahead::AheadRequest;
        use ahead_rpc::proxy::{ProxyRequest, ProxyResponse, ProxyRpc};
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let path = directory.path().join("main.rs");
        std::fs::write(&path, "saved\n").expect("source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx).with_proxy(
                proxy.clone(),
                directory.path().to_str().expect("workspace"),
                window,
                cx,
            )
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.host_shared_recovery_pending = true;
            panel.editor.update(cx, |editor, cx| {
                editor.replace_all("discard me\n", window, cx)
            });
        });
        let expected_path = path.clone();
        let responder = std::thread::spawn(move || {
            let mut attempts = 0;
            while let Ok(message) =
                rpc.rx().recv_timeout(std::time::Duration::from_secs(5))
            {
                if let ProxyRpc::Request(
                    id,
                    ProxyRequest::AheadRequest {
                        request: AheadRequest::DiscardSharedRemoteRecovery { path },
                    },
                ) = message
                {
                    assert_eq!(path, expected_path);
                    attempts += 1;
                    let result = if attempts == 1 {
                        Err(ahead_rpc::RpcError {
                            code: 0,
                            message: "recovery store unavailable".into(),
                        })
                    } else {
                        Ok(ProxyResponse::AheadResponse {
                            response: serde_json::json!({ "discarded": true }),
                        })
                    };
                    rpc.handle_response(id, result);
                    if attempts == 2 {
                        return;
                    }
                }
            }
            panic!("discard recovery requests were not sent");
        });
        let rejected =
            panel.update_in(cx, |panel, window, cx| panel.prepare_close(window, cx));
        cx.simulate_prompt_answer("Discard Changes");
        assert!(rejected.await.is_none());
        assert!(panel.update(cx, |panel, _| {
            panel.status.contains("recovery store unavailable")
        }));
        let accepted =
            panel.update_in(cx, |panel, window, cx| panel.prepare_close(window, cx));
        cx.simulate_prompt_answer("Discard Changes");
        assert!(accepted.await.is_some());
        assert!(!panel.update(cx, |panel, _| panel.host_shared_recovery_pending));
        responder.join().expect("discard responder");
        assert_eq!(std::fs::read_to_string(path).expect("disk"), "saved\n");
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
    fn external_disk_change_reloads_clean_buffer_and_rebases_selection(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::core::CoreNotification;

        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("disposable workspace");
        let path = workspace.path().join("main.rs");
        let original = "fn main() {\n    // keep caret here\n}\n";
        let updated = "fn main() {\n    // externally changed; keep caret here\n}\n";
        std::fs::write(&path, original).expect("write original file");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            workspace.path().to_owned(),
        );
        let view_path = path.to_string_lossy().into_owned();
        let workspace_path = workspace.path().to_string_lossy().into_owned();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(&view_path, window, cx).with_proxy(
                proxy.clone(),
                &workspace_path,
                window,
                cx,
            )
        });
        let original_caret = original.find("caret").expect("caret text") + 3;
        panel.update(cx, |panel, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(original_caret..original_caret, cx);
            });
        });

        std::fs::write(&path, updated).expect("external disk edit");
        proxy.route_core(CoreNotification::WorkspaceFileChange {
            generation: 0,
            paths: vec![path.clone()],
        });
        cx.run_until_parked();

        panel.read_with(cx, |panel, cx| {
            let editor = panel.editor.read(cx);
            let expected_caret = updated.find("caret").expect("updated caret") + 3;
            assert_eq!(editor.value(), updated);
            assert_eq!(editor.selected_range(), expected_caret..expected_caret);
            assert!(!panel.dirty);
            assert!(!panel.external_change_conflict);
            assert!(panel.status.contains("Reloaded"));
        });
    }

    #[gpui_kit::test]
    fn external_disk_change_keeps_dirty_buffer_and_requires_save_review(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::core::CoreNotification;

        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("disposable workspace");
        let path = workspace.path().join("main.rs");
        std::fs::write(&path, "original\n").expect("write original file");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            workspace.path().to_owned(),
        );
        let view_path = path.to_string_lossy().into_owned();
        let workspace_path = workspace.path().to_string_lossy().into_owned();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(&view_path, window, cx).with_proxy(
                proxy.clone(),
                &workspace_path,
                window,
                cx,
            )
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.replace_all("my unsaved edit\n", window, cx);
            });
        });

        std::fs::write(&path, "external edit\n").expect("external disk edit");
        proxy.route_core(CoreNotification::WorkspaceFileChange {
            generation: 0,
            paths: vec![path],
        });
        cx.run_until_parked();

        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.editor.read(cx).value(), "my unsaved edit\n");
            assert!(panel.dirty);
            assert!(panel.external_change_conflict);
            assert!(panel.status.contains("unsaved edits kept"));
        });
    }

    #[gpui_kit::test]
    fn external_rename_adopts_new_path_without_losing_unsaved_text(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::core::CoreNotification;

        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("disposable workspace");
        let old_path = workspace.path().join("src/main.rs");
        let new_path = workspace.path().join("src/lib.rs");
        std::fs::create_dir_all(old_path.parent().expect("source directory"))
            .expect("create source directory");
        std::fs::write(&old_path, "original\n").expect("write original file");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            workspace.path().to_owned(),
        );
        let old_path_text = old_path.to_string_lossy().into_owned();
        let workspace_path = workspace.path().to_string_lossy().into_owned();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(&old_path_text, window, cx).with_proxy(
                proxy.clone(),
                &workspace_path,
                window,
                cx,
            )
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.replace_all("my unsaved edit\n", window, cx);
            });
        });

        std::fs::rename(&old_path, &new_path).expect("rename workspace file");
        proxy.route_core(CoreNotification::WorkspaceFileRename {
            generation: 0,
            old_path: old_path.clone(),
            new_path: new_path.clone(),
        });
        proxy.route_core(CoreNotification::WorkspaceFileChange {
            generation: 0,
            paths: vec![old_path, new_path.clone()],
        });
        cx.run_until_parked();

        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.file_path, new_path.to_string_lossy().as_ref());
            assert_eq!(panel.editor.read(cx).value(), "my unsaved edit\n");
            assert!(panel.dirty);
            assert!(!panel.external_file_missing);
            assert!(!panel.external_change_conflict);
            assert!(panel.status.contains("File renamed"));
        });
    }

    #[gpui_kit::test]
    fn open_file_change_reloads_a_file_outside_workspace_search_results(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::core::{CoreNotification, FileChanged};

        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("disposable workspace");
        std::fs::write(workspace.path().join(".gitignore"), "ignored/\n")
            .expect("ignore generated files");
        let path = workspace.path().join("ignored/generated.rs");
        std::fs::create_dir_all(path.parent().expect("parent directory"))
            .expect("create generated directory");
        assert!(
            !ahead_core::search::WorkspaceFileIndex::new(
                workspace.path().to_owned()
            )
            .snapshot()
            .contains(&path),
            "the search index must exclude this open file"
        );
        std::fs::write(&path, "original\n").expect("write original file");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            workspace.path().to_owned(),
        );
        let view_path = path.to_string_lossy().into_owned();
        let workspace_path = workspace.path().to_string_lossy().into_owned();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(&view_path, window, cx).with_proxy(
                proxy.clone(),
                &workspace_path,
                window,
                cx,
            )
        });

        std::fs::write(&path, "latest disk version\n").expect("external disk edit");
        proxy.route_core(CoreNotification::OpenFileChanged {
            path: path.clone(),
            content: FileChanged::Change("stale event contents\n".into()),
        });
        cx.run_until_parked();

        panel.read_with(cx, |panel, cx| {
            assert_eq!(
                panel.editor.read(cx).value(),
                "latest disk version\n",
                "reload from disk to handle coalesced or delayed events"
            );
            assert!(!panel.dirty);
            assert!(!panel.external_change_conflict);
        });
    }

    #[gpui_kit::test]
    async fn external_delete_keeps_local_edits_and_cancel_does_not_recreate_file(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::core::{CoreNotification, FileChanged};

        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("disposable workspace");
        let path = workspace.path().join("src/main.rs");
        std::fs::create_dir_all(path.parent().expect("parent directory"))
            .expect("create source directory");
        std::fs::write(&path, "fn main() {}\n").expect("write original file");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            workspace.path().to_owned(),
        );
        let view_path = path.to_string_lossy().into_owned();
        let workspace_path = workspace.path().to_string_lossy().into_owned();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(&view_path, window, cx).with_proxy(
                proxy.clone(),
                &workspace_path,
                window,
                cx,
            )
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.replace_all("fn main() { /* local edit */ }\n", window, cx);
            });
        });

        std::fs::remove_file(&path).expect("external delete");
        proxy.route_core(CoreNotification::OpenFileChanged {
            path: path.clone(),
            content: FileChanged::Delete,
        });
        cx.run_until_parked();

        panel.read_with(cx, |panel, cx| {
            assert_eq!(
                panel.editor.read(cx).value(),
                "fn main() { /* local edit */ }\n"
            );
            assert!(panel.dirty);
            assert!(panel.external_change_conflict);
            assert!(panel.external_file_missing);
            assert!(panel.status.contains("Save will recreate the old path"));
        });

        let save = panel.update_in(cx, |panel, window, cx| {
            panel.save_after_recovery_review(window, cx)
        });
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        assert!(!save.await);
        assert!(!path.exists(), "Cancel must not recreate the deleted file");
        panel.read_with(cx, |panel, cx| {
            assert_eq!(
                panel.editor.read(cx).value(),
                "fn main() { /* local edit */ }\n"
            );
            assert!(panel.dirty);
            assert!(panel.external_file_missing);
        });
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
            panel
                .turn_context(cx)
                .expect("text context")
                .selection
                .expect("editor selection")
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

    #[test]
    fn inline_blame_tracks_line_end_and_relative_author_time() {
        assert_eq!(super::line_end_offset("αβ\nlast\n", 1), Some(4));
        assert_eq!(super::line_end_offset("αβ\nlast\n", 2), Some(9));
        assert_eq!(super::line_end_offset("αβ\nlast\n", 3), Some(10));
        assert_eq!(super::line_end_offset("", 1), Some(0));
        assert_eq!(super::line_end_offset("a", 2), None);
        assert_eq!(super::relative_blame_time(100, 100), "just now");
        assert_eq!(super::relative_blame_time(100, 3_700), "1 hour ago");
        assert_eq!(super::relative_blame_time(100, 7_300), "2 hours ago");
    }

    #[test]
    fn gutter_rows_are_limited_to_the_visible_editor_range() {
        assert_eq!(
            super::visible_gutter_rows(Some(80..100), 10_000, 40),
            79..101
        );
        assert_eq!(super::visible_gutter_rows(None, 10_000, 40), 0..41);
        assert_eq!(super::visible_gutter_rows(Some(99..110), 100, 40), 98..100);
    }

    #[test]
    fn gutter_diff_markers_ignore_diff_payload_when_line_geometry_is_stable() {
        use ahead_rpc::source_control::{DiffHunk, DiffHunkKind};

        let hunks = vec![
            DiffHunk {
                start: 3,
                len: 2,
                kind: DiffHunkKind::Added,
                old_start: 0,
                old_lines: Vec::new(),
            },
            DiffHunk {
                start: 6,
                len: 1,
                kind: DiffHunkKind::Modified,
                old_start: 6,
                old_lines: vec!["baseline".into()],
            },
            DiffHunk {
                start: 8,
                len: 0,
                kind: DiffHunkKind::Deleted,
                old_start: 8,
                old_lines: vec!["removed".into()],
            },
        ];
        let mut markers = super::GutterDiffMarkers::default();

        assert!(markers.update(&hunks, 10));
        assert_eq!(markers.hunk_lines.get(&3), Some(&(3, DiffHunkKind::Added)));
        assert_eq!(markers.hunk_lines.get(&4), Some(&(3, DiffHunkKind::Added)));
        assert_eq!(
            markers.hunk_lines.get(&6),
            Some(&(6, DiffHunkKind::Modified))
        );
        assert!(markers.deleted_lines.contains(&8));
        assert!(!markers.update(&hunks, 10));

        let mut changed_diff_payload = hunks.clone();
        let modified_hunk = changed_diff_payload
            .iter_mut()
            .find(|hunk| hunk.kind == DiffHunkKind::Modified)
            .expect("modified hunk");
        modified_hunk.old_lines = vec!["changed baseline text".into()];
        assert!(!markers.update(&changed_diff_payload, 10));

        let mut changed_geometry = hunks.clone();
        if let Some(added_hunk) = changed_geometry.first_mut() {
            added_hunk.len += 1;
        }
        assert!(markers.update(&changed_geometry, 10));
        assert_eq!(markers.hunk_lines.get(&5), Some(&(3, DiffHunkKind::Added)));

        assert!(!markers.update_line_count(10));
        assert!(markers.update_line_count(7));
        assert!(markers.deleted_lines.contains(&7));
        assert!(!markers.deleted_lines.contains(&8));
        assert!(!markers.update_line_count(7));

        assert!(markers.update(&hunks, 6));
        assert!(markers.deleted_lines.contains(&6));
    }

    #[gpui_kit::test(iterations = 10)]
    fn git_metadata_waits_for_typing_to_settle(cx: &mut gpui_kit::TestAppContext) {
        use ahead_rpc::proxy::{ProxyRequest, ProxyRpc};

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
            panel.proxy = Some(proxy);
            panel.workspace = directory.path().to_string_lossy().into_owned();
            panel.refresh_git_metadata(cx);
        });
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(100));
        cx.run_until_parked();

        let editor = panel.update(cx, |panel, _| panel.editor.clone());
        editor.update_in(cx, |editor, window, cx| {
            editor.replace_all("latest\n", window, cx);
        });
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(50));
        cx.run_until_parked();
        assert!(
            !rpc.rx().try_iter().any(|message| matches!(
                message,
                ProxyRpc::Request(_, ProxyRequest::GitFileState { .. })
            )),
            "typing should postpone the Git request"
        );

        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(500));
        cx.run_until_parked();
        assert!(
            !rpc.rx().try_iter().any(|message| matches!(
                message,
                ProxyRpc::Request(_, ProxyRequest::GitFileState { .. })
            )),
            "typing should keep postponing the Git request until the longer idle debounce expires"
        );
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(50));
        cx.run_until_parked();
        let request_content = rpc.rx().try_iter().find_map(|message| {
            if let ProxyRpc::Request(_, ProxyRequest::GitFileState { content, .. }) =
                message
            {
                Some(content)
            } else {
                None
            }
        });
        assert_eq!(request_content.as_deref(), Some("latest\n"));
    }

    #[gpui_kit::test]
    fn source_control_open_expands_first_git_hunk_after_refresh(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::proxy::{ProxyRequest, ProxyResponse, ProxyRpc};
        use ahead_rpc::source_control::{DiffHunk, DiffHunkKind, GitFileState};

        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let path = directory.path().join("main.ts");
        let source = (0..80)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        std::fs::write(&path, &source).expect("source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let rpc = proxy.rpc_for_test();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        panel.update(cx, |panel, cx| {
            panel.proxy = Some(proxy);
            panel.workspace = directory.path().to_string_lossy().into_owned();
            panel.reveal_first_git_hunk(cx);
        });
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(GIT_METADATA_REFRESH_DEBOUNCE);
        cx.run_until_parked();

        let (request_id, request_content) = rpc
            .rx()
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    request_id,
                    ProxyRequest::GitFileState { content, .. },
                ) => Some((request_id, content)),
                _ => None,
            })
            .expect("Git metadata request");
        assert_eq!(request_content, source);
        rpc.handle_response(
            request_id,
            Ok(ProxyResponse::GitFileState {
                state: GitFileState {
                    hunks: vec![DiffHunk {
                        start: 40,
                        len: 1,
                        kind: DiffHunkKind::Modified,
                        old_start: 40,
                        old_lines: vec!["old line".into()],
                    }],
                    blame: Vec::new(),
                },
            }),
        );
        cx.run_until_parked();

        panel.update(cx, |panel, cx| {
            assert_eq!(panel.expanded_hunk_start, Some(40));
            assert!(!panel.expand_first_git_hunk_after_refresh);
            assert!(panel.editor.read(cx).scroll_offset().y < gpui_kit::px(0.));
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("expanded-git-hunk").is_some());
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
            .advance_clock(GIT_METADATA_REFRESH_DEBOUNCE);
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
            .advance_clock(GIT_METADATA_REFRESH_DEBOUNCE);
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
            let key = panel.current_git_key().expect("Git key");
            panel.finish_git_refresh(key, Err("temporary Git error".into()), cx);
            assert_eq!(
                panel.git_state, state,
                "a transient error keeps the last gutter markers visible"
            );
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
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(200));
        cx.run_until_parked();
        assert!(
            rpc.rx().try_iter().all(|message| !matches!(
                message,
                ProxyRpc::Request(_, ProxyRequest::GitFileState { .. })
            )),
            "Git metadata waits for a typing pause"
        );

        editor.update_in(cx, |editor, window, cx| {
            editor.replace_all("typing latest\n", window, cx);
        });
        panel.update(cx, |panel, cx| {
            panel.refresh_git_metadata(cx);
            assert_eq!(panel.git_state, state, "old markers remain visible");
        });
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(299));
        cx.run_until_parked();
        assert!(
            rpc.rx().try_iter().all(|message| !matches!(
                message,
                ProxyRpc::Request(_, ProxyRequest::GitFileState { .. })
            )),
            "rapid edits coalesce into one Git computation"
        );

        proxy.route_core(ahead_rpc::core::CoreNotification::DiffInfo {
            diff: Default::default(),
        });
        panel.update(cx, |panel, cx| panel.refresh_git_metadata(cx));
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(299));
        cx.run_until_parked();
        assert!(
            rpc.rx().try_iter().all(|message| !matches!(
                message,
                ProxyRpc::Request(_, ProxyRequest::GitFileState { .. })
            )),
            "repository changes restart the debounce"
        );
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(1));
        cx.run_until_parked();
        let (external_change, content) = take_request();
        assert_eq!(content, "typing latest\n");
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

    #[test]
    fn git_hunk_preview_text_matches_the_visible_diff_rows() {
        let hunk = DiffHunk {
            start: 4,
            len: 2,
            kind: DiffHunkKind::Modified,
            old_start: 4,
            old_lines: vec!["old one".into(), "old two".into()],
        };

        assert_eq!(
            git_hunk_preview_text(&hunk, &["new one".into(), "new two".into()]),
            "- old one\n- old two\n+ new one\n+ new two\n"
        );
    }

    #[test]
    fn git_hunk_projection_inserts_removed_rows_without_changing_source_text() {
        let hunk = DiffHunk {
            start: 2,
            len: 2,
            kind: DiffHunkKind::Modified,
            old_start: 2,
            old_lines: vec!["old one".into(), "old two".into()],
        };
        let source = "before\nnew one\nnew two\nafter\n";

        let projection = git_hunk_projection(&hunk, source);

        assert_eq!(
            projection.text,
            "before\n- old one\n- old two\nnew one\nnew two\nafter\n"
        );
        assert_eq!(projection.inserted_rows, 2);
        assert_eq!(projection.deleted_ranges.len(), 2);
        assert_eq!(projection.added_ranges.len(), 2);
        assert_eq!(source, "before\nnew one\nnew two\nafter\n");
    }

    #[test]
    fn git_hunk_projection_preserves_newline_for_deletion_at_end_of_file() {
        let hunk = DiffHunk {
            start: 2,
            len: 0,
            kind: DiffHunkKind::Deleted,
            old_start: 2,
            old_lines: vec!["removed".into()],
        };

        let projection = git_hunk_projection(&hunk, "kept\n");

        assert_eq!(projection.text, "kept\n- removed\n");
        assert_eq!(projection.deleted_ranges.len(), 1);
        assert!(projection.added_ranges.is_empty());
    }

    #[test]
    fn merge_conflict_parser_supports_standard_diff3_and_both_sides() {
        let source = concat!(
            "before\n",
            "<<<<<<< HEAD\ncurrent\n=======\nincoming\n>>>>>>> topic\n",
            "middle\n",
            "<<<<<<< HEAD\nours\n||||||| base\nancestor\n=======\ntheirs\n>>>>>>> topic\n",
            "after\n",
        );
        let conflicts = parse_merge_conflicts(source);
        assert_eq!(conflicts.len(), 2);
        assert_eq!(conflicts[0].line, 2);
        assert_eq!(&source[conflicts[0].ours.clone()], "current\n");
        assert_eq!(&source[conflicts[0].theirs.clone()], "incoming\n");
        assert_eq!(
            merge_conflict_choice_text(
                source,
                &conflicts[0],
                MergeConflictChoice::Both,
            ),
            Some("current\nincoming\n".into())
        );
        assert_eq!(&source[conflicts[1].ours.clone()], "ours\n");
        assert_eq!(&source[conflicts[1].theirs.clone()], "theirs\n");
        assert_eq!(
            merge_conflict_choice_text(
                source,
                &conflicts[1],
                MergeConflictChoice::Incoming,
            ),
            Some("theirs\n".into())
        );
    }

    #[test]
    fn merge_conflict_parser_ignores_incomplete_or_nested_markers() {
        let source = concat!(
            "<<<<<<< unfinished\nleft\n",
            "<<<<<<< nested\ninner\n=======\nother\n>>>>>>> nested\n",
            "<<<<<<< complete\nleft\n=======\nright\n>>>>>>> complete\n",
        );
        let conflicts = parse_merge_conflicts(source);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(&source[conflicts[0].ours.clone()], "left\n");
        assert_eq!(&source[conflicts[0].theirs.clone()], "right\n");
    }

    #[gpui_kit::test]
    fn merge_conflict_action_resolves_only_the_conflict_under_the_caret(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let path = directory.path().join("conflict.rs");
        let source = concat!(
            "before\n<<<<<<< HEAD\ncurrent\n=======\nincoming\n>>>>>>> topic\nafter\n"
        );
        std::fs::write(&path, source).expect("conflict file");
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        panel.update(cx, |panel, cx| {
            let caret = source.find("current").expect("current side") + 1;
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(caret..caret, cx);
            });
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let incoming = cx
            .debug_bounds("resolve-merge-incoming")
            .expect("incoming resolution action");
        cx.simulate_click(incoming.center(), Default::default());

        assert_eq!(
            panel.update(cx, |panel, cx| panel.editor.read(cx).value().to_string()),
            "before\nincoming\nafter\n"
        );
        assert!(panel.update(cx, |panel, _| panel.dirty));
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
            panel.refresh_git_gutter_markers(cx);
            panel.git_key = panel.current_git_key();
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let change = cx.debug_bounds("git-change-1").expect("change marker");
        let gutter = cx.debug_bounds("editor-gutter").expect("gutter");
        let breakpoint = cx
            .debug_bounds("breakpoint-target-1")
            .expect("breakpoint target");
        assert_eq!(gutter.size.width, gpui_kit::px(EDITOR_GUTTER_WIDTH));
        let actions = cx
            .debug_bounds("gutter-actions-1")
            .expect("gutter action column");
        assert_eq!(change.size.width, gpui_kit::px(GIT_GUTTER_TRACK_WIDTH));
        assert_eq!(actions.size.width, gpui_kit::px(GUTTER_ACTION_WIDTH));
        assert_eq!(gutter.size.width, change.size.width + actions.size.width);
        assert!(breakpoint.origin.x >= change.origin.x + change.size.width);
        assert!(
            breakpoint.origin.x + breakpoint.size.width
                <= gutter.origin.x + gutter.size.width
        );
        cx.simulate_click(change.center(), Default::default());
        assert!(proxy.breakpoints_for(&path).is_empty());
        assert_eq!(
            panel.update(cx, |panel, _| panel.expanded_hunk_start),
            Some(1)
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("expanded-git-hunk").is_some());
        assert!(cx.debug_bounds("git-diff-projection-editor").is_some());
        panel.update(cx, |panel, cx| {
            let preview = panel.git_diff_preview.as_ref().expect("diff projection");
            assert!(preview.editor.read(cx).presentation().is_readonly());
            assert_eq!(preview.editor.read(cx).text().to_string(), "print(1)\n");
        });
        let copy = cx.debug_bounds("copy-git-hunk").expect("copy diff button");
        cx.simulate_click(copy.center(), Default::default());
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
            Some("+ print(1)\n".into())
        );
        let close = cx
            .debug_bounds("close-git-hunk")
            .expect("close diff preview");
        cx.simulate_click(close.center(), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("git-diff-projection-editor").is_none());

        let breakpoint = cx
            .debug_bounds("breakpoint-target-1")
            .expect("breakpoint control after closing diff preview");
        let change = cx
            .debug_bounds("git-change-1")
            .expect("change marker after close");
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
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let close = cx
            .debug_bounds("close-git-hunk")
            .expect("close reopened diff");
        cx.simulate_click(close.center(), Default::default());
        assert_eq!(panel.update(cx, |panel, _| panel.expanded_hunk_start), None);
    }

    #[gpui_kit::test]
    fn code_comment_actions_require_a_session_and_fit_the_gutter(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let path = directory.path().join("main.py");
        std::fs::write(&path, "print(1)\nprint(2)\n").expect("source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        panel.update(cx, |panel, cx| {
            panel.proxy = Some(proxy.clone());
            panel.comment_session_id = Some("test-session".into());
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let gutter = cx.debug_bounds("editor-gutter").expect("editor gutter");
        let add_comment = cx
            .debug_bounds("add-comment-selection")
            .expect("visible add-comment toolbar button before selecting code");
        assert_eq!(gutter.size.width, gpui_kit::px(EDITOR_GUTTER_WIDTH));
        assert!(add_comment.size.width > gpui_kit::px(60.));
        cx.simulate_click(add_comment.center(), Default::default());
        assert!(panel.update(cx, |panel, _| panel.comment_draft_target.is_none()));
        assert_eq!(
            panel.update(cx, |panel, _| panel.status.to_string()),
            "Select code to add a comment"
        );

        panel.update(cx, |panel, cx| {
            panel.editor.update(cx, |editor, cx| {
                editor.set_selected_range(0..5, cx);
            });
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let add_comment = cx
            .debug_bounds("add-comment-selection")
            .expect("visible add-comment toolbar button after selecting code");
        cx.debug_bounds("breakpoint-target-2")
            .expect("breakpoint control remains available on adjacent lines");
        let gutter_add_comment = cx
            .debug_bounds("add-comment-marker-1")
            .expect("selected code exposes an inline add-comment button");
        assert!(add_comment.size.width > gpui_kit::px(60.));
        assert_eq!(gutter_add_comment.size.width, gpui_kit::px(20.));
        assert!(
            gutter_add_comment.origin.x + gutter_add_comment.size.width
                <= gutter.origin.x + gutter.size.width
        );
        assert!(!proxy.breakpoints_for(&path).contains(&1));
        cx.simulate_click(gutter_add_comment.center(), Default::default());
        assert!(panel.update(cx, |panel, _| panel.comment_draft_target.is_some()));
        assert_eq!(
            panel.update(cx, |panel, _| panel.comment_popover_line),
            Some(1)
        );
        panel.update(cx, |panel, cx| {
            panel.comment_draft_target = None;
            panel.comment_popover_line = None;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let breakpoint = cx
            .debug_bounds("breakpoint-target-2")
            .expect("adjacent breakpoint control remains visible");
        cx.simulate_click(breakpoint.center(), Default::default());
        assert!(proxy.breakpoints_for(&path).contains(&2));

        let prior_generation = panel.update(cx, |panel, _| panel.comment_generation);
        let session_switch_target = panel
            .update(cx, |panel, cx| panel.selected_code_comment(cx))
            .expect("selected code remains available across session changes");
        panel.update(cx, |panel, cx| {
            panel.comment_draft_target = Some(session_switch_target);
            panel.comment_pending = true;
            panel.comment_error = Some("old session error".into());
            panel.set_code_comments(None, Vec::new(), cx);
        });
        assert_eq!(
            panel.update(cx, |panel, _| panel.comment_generation),
            prior_generation.wrapping_add(1)
        );
        assert!(!panel.update(cx, |panel, _| panel.comment_pending));
        assert!(panel.update(cx, |panel, _| panel.comment_error.is_none()));
        assert!(panel.update(cx, |panel, _| panel.comment_draft_target.is_none()));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("add-comment-selection").is_none());
        assert!(cx.debug_bounds("add-comment-marker-1").is_none());
        let target = panel
            .update(cx, |panel, cx| panel.selected_code_comment(cx))
            .expect("selected code remains available as a read-only context");
        panel.update_in(cx, |panel, window, cx| {
            panel.begin_code_comment(target, None, window, cx);
        });
        assert!(panel.update(cx, |panel, _| panel.comment_draft_target.is_none()));
        assert_eq!(
            panel.update(cx, |panel, _| panel.status.to_string()),
            "Start an AHEAD session before commenting"
        );

        panel.update(cx, |panel, cx| {
            panel.set_code_comments(Some("test-session".into()), Vec::new(), cx);
        });
        assert_ne!(
            panel.update(cx, |panel, _| panel.comment_generation),
            prior_generation,
            "requests from the prior visit to this session must remain stale"
        );
        let target = panel
            .update(cx, |panel, cx| panel.selected_code_comment(cx))
            .expect("selected code remains available");
        panel.update_in(cx, |panel, window, cx| {
            panel.begin_code_comment(
                target,
                Some("stale-session".into()),
                window,
                cx,
            );
        });
        assert!(panel.update(cx, |panel, _| panel.comment_draft_target.is_none()));
        assert_eq!(
            panel.update(cx, |panel, _| panel.status.to_string()),
            "The active AHEAD session changed; try again"
        );
        panel.update_in(cx, |panel, window, cx| {
            panel.comment_input.update(cx, |input, cx| {
                input.set_value("old session draft", window, cx);
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let add_comment = cx
            .debug_bounds("add-comment-selection")
            .expect("comment action stays visible when ready");
        cx.simulate_click(add_comment.center(), Default::default());
        assert!(panel.update(cx, |panel, _| panel.comment_draft_target.is_some()));
        assert_eq!(
            panel.update(cx, |panel, cx| panel
                .comment_input
                .read(cx)
                .value()
                .to_string()),
            ""
        );
    }

    #[gpui_kit::test]
    fn git_change_marker_previews_modified_and_deleted_hunks(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ahead_rpc::source_control::{DiffHunk, DiffHunkKind};

        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let path = directory.path().join("main.py");
        std::fs::write(&path, "new line\n").expect("source");
        let proxy = crate::proxy_client::ProxyClient::new_for_test(
            directory.path().to_owned(),
        );
        let (panel, cx) = cx.add_window_view(|window, cx| {
            CodePanel::new(path.to_str().expect("path"), window, cx)
        });
        panel.update(cx, |panel, cx| {
            panel.proxy = Some(proxy);
            cx.notify();
        });

        for (hunk, expected_diff, expected_projection) in [
            (
                DiffHunk {
                    start: 1,
                    len: 1,
                    kind: DiffHunkKind::Modified,
                    old_start: 1,
                    old_lines: vec!["old line".into()],
                },
                "- old line\n+ new line\n",
                "- old line\nnew line\n",
            ),
            (
                DiffHunk {
                    start: 1,
                    len: 0,
                    kind: DiffHunkKind::Deleted,
                    old_start: 1,
                    old_lines: vec!["removed line".into()],
                },
                "- removed line\n",
                "- removed line\nnew line\n",
            ),
        ] {
            panel.update(cx, |panel, cx| {
                panel.git_state.hunks = vec![hunk];
                panel.refresh_git_gutter_markers(cx);
                panel.git_key = panel.current_git_key();
                panel.expanded_hunk_start = None;
                cx.notify();
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));

            let marker = cx.debug_bounds("git-change-1").expect("change marker");
            cx.simulate_click(marker.center(), Default::default());
            assert_eq!(
                panel.update(cx, |panel, _| panel.expanded_hunk_start),
                Some(1)
            );
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert!(cx.debug_bounds("git-diff-projection-editor").is_some());
            panel.update(cx, |panel, cx| {
                let preview =
                    panel.git_diff_preview.as_ref().expect("diff projection");
                assert_eq!(
                    preview.editor.read(cx).text().to_string(),
                    expected_projection
                );
            });

            let copy = cx.debug_bounds("copy-git-hunk").expect("copy diff button");
            cx.simulate_click(copy.center(), Default::default());
            assert_eq!(
                cx.update(|_, cx| cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())),
                Some(expected_diff.into())
            );
        }
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
    fn formatting_edits_preserve_unicode_selection_and_reject_overlap() {
        use lsp_types::{Position, Range, TextEdit};

        let text = "a🦀b\n";
        let edit = TextEdit {
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: 1,
                    character: 0,
                },
            },
            new_text: "a 🦀 b\n".into(),
        };
        let (range, replacement, selection) =
            formatting_edit(text, &[edit.clone()], 5..6)
                .expect("formatting edit")
                .expect("changed text");
        assert_eq!(range, 0..text.len());
        assert_eq!(replacement, "a 🦀 b\n");
        assert_eq!(&replacement[selection], "b");

        let overlapping = [
            edit.clone(),
            TextEdit {
                range: Range {
                    start: Position {
                        line: 0,
                        character: 1,
                    },
                    end: Position {
                        line: 0,
                        character: 2,
                    },
                },
                new_text: "x".into(),
            },
        ];
        assert!(formatting_edit(text, &overlapping, 0..0).is_err());
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
