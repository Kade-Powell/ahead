//! Searchable editor commands, following Zed's `crates/command_palette` picker behavior.

use crate::app::ShellShortcut;
use crate::code_panel::EditorCommand;
use crate::workspace_panels::JustTask;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, WindowExt, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as FuzzyConfig, Matcher as FuzzyMatcher, Utf32Str};

#[derive(Clone, Copy)]
struct Command {
    name: &'static str,
    keywords: &'static str,
    shortcut: &'static str,
    action: ShellShortcut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CommandAction {
    Shell(ShellShortcut),
    RunJustTask {
        recipe: String,
        cwd: String,
        justfile: String,
    },
}

#[derive(Clone)]
struct PaletteCommand {
    name: String,
    keywords: String,
    detail: String,
    action: CommandAction,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct CommandContext {
    pub active_editor: bool,
    pub text_editor: bool,
    pub language_editor: bool,
    pub can_navigate_back: bool,
    pub can_navigate_forward: bool,
    pub can_start_or_continue_debugging: bool,
    pub can_stop_debugging: bool,
    pub can_step_debugging: bool,
    pub can_commit_staged: bool,
    pub can_close_other_editors: bool,
    pub can_close_editors_to_left: bool,
    pub can_close_editors_to_right: bool,
    pub can_move_editor_to_other_group: bool,
}

impl Command {
    fn available(self, context: CommandContext) -> bool {
        match self.action {
            ShellShortcut::SaveActiveFile | ShellShortcut::DebugKey("f9", false) => {
                context.text_editor
            }
            ShellShortcut::Editor(_) => context.language_editor,
            ShellShortcut::NavigateBack => context.can_navigate_back,
            ShellShortcut::NavigateForward => context.can_navigate_forward,
            ShellShortcut::CloseEditor | ShellShortcut::CloseAllEditors => {
                context.active_editor
            }
            ShellShortcut::CloseOtherEditors => context.can_close_other_editors,
            ShellShortcut::CloseEditorsToLeft => context.can_close_editors_to_left,
            ShellShortcut::CloseEditorsToRight => context.can_close_editors_to_right,
            ShellShortcut::MoveEditorToOtherGroup => {
                context.can_move_editor_to_other_group
            }
            ShellShortcut::CommitStagedChanges => context.can_commit_staged,
            ShellShortcut::DebugKey("f5", false) => {
                context.can_start_or_continue_debugging
            }
            ShellShortcut::DebugKey("f5", true) => context.can_stop_debugging,
            ShellShortcut::DebugKey("f10" | "f11", _) => context.can_step_debugging,
            _ => true,
        }
    }
}

const COMMANDS: &[Command] = &[
    Command {
        name: "Quick Open",
        keywords: "file finder",
        shortcut: "⌘P / Ctrl+P",
        action: ShellShortcut::QuickOpen,
    },
    Command {
        name: "Open File",
        keywords: "browse file",
        shortcut: "⌘O / Ctrl+O",
        action: ShellShortcut::OpenFileDialog,
    },
    Command {
        name: "Open Folder",
        keywords: "browse project workspace",
        shortcut: "",
        action: ShellShortcut::OpenFolderDialog,
    },
    Command {
        name: "Save File",
        keywords: "write buffer",
        shortcut: "⌘S / Ctrl+S",
        action: ShellShortcut::SaveActiveFile,
    },
    Command {
        name: "Close Editor",
        keywords: "close tab buffer",
        shortcut: "",
        action: ShellShortcut::CloseEditor,
    },
    Command {
        name: "Close Other Editors",
        keywords: "close tabs buffers keep active editor",
        shortcut: "",
        action: ShellShortcut::CloseOtherEditors,
    },
    Command {
        name: "Close Editors to the Left",
        keywords: "close tabs buffers left",
        shortcut: "",
        action: ShellShortcut::CloseEditorsToLeft,
    },
    Command {
        name: "Close Editors to the Right",
        keywords: "close tabs buffers right",
        shortcut: "",
        action: ShellShortcut::CloseEditorsToRight,
    },
    Command {
        name: "Close All Editors",
        keywords: "close tabs buffers all",
        shortcut: "",
        action: ShellShortcut::CloseAllEditors,
    },
    Command {
        name: "Move Editor to Other Group",
        keywords: "split pane group side by side",
        shortcut: "",
        action: ShellShortcut::MoveEditorToOtherGroup,
    },
    Command {
        name: "Go Back",
        keywords: "navigation previous location cursor history",
        shortcut: "Ctrl+-",
        action: ShellShortcut::NavigateBack,
    },
    Command {
        name: "Go Forward",
        keywords: "navigation next location cursor history",
        shortcut: "Ctrl+_",
        action: ShellShortcut::NavigateForward,
    },
    Command {
        name: "Go to Definition",
        keywords: "lsp symbol navigation declaration",
        shortcut: "F12",
        action: ShellShortcut::Editor(EditorCommand::GoToDefinition),
    },
    Command {
        name: "Find All References",
        keywords: "lsp symbol usages references",
        shortcut: "⇧F12 / Shift+F12",
        action: ShellShortcut::Editor(EditorCommand::FindReferences),
    },
    Command {
        name: "Go to Implementation",
        keywords: "lsp symbol navigation",
        shortcut: "⌘F12 / Ctrl+F12",
        action: ShellShortcut::Editor(EditorCommand::GoToImplementation),
    },
    Command {
        name: "Rename Symbol",
        keywords: "lsp refactor rename variable function",
        shortcut: "F2",
        action: ShellShortcut::Editor(EditorCommand::RenameSymbol),
    },
    Command {
        name: "Format Document",
        keywords: "lsp formatter format file",
        shortcut: "⇧⌥F / Shift+Alt+F",
        action: ShellShortcut::Editor(EditorCommand::FormatDocument),
    },
    Command {
        name: "Show Completions",
        keywords: "lsp autocomplete suggestions",
        shortcut: "Ctrl+Space",
        action: ShellShortcut::Editor(EditorCommand::ShowCompletions),
    },
    Command {
        name: "Show Signature Help",
        keywords: "lsp arguments parameter types",
        shortcut: "⌘⇧Space / Ctrl+Shift+Space",
        action: ShellShortcut::Editor(EditorCommand::ShowSignatureHelp),
    },
    Command {
        name: "Show Code Actions",
        keywords: "lsp quick fix refactor lightbulb",
        shortcut: "⌘. / Ctrl+.",
        action: ShellShortcut::Editor(EditorCommand::ShowCodeActions),
    },
    Command {
        name: "Search Workspace",
        keywords: "find in files",
        shortcut: "⌘⇧F / Ctrl+Shift+F",
        action: ShellShortcut::Search,
    },
    Command {
        name: "Files Explorer",
        keywords: "project files",
        shortcut: "⌘⇧E / Ctrl+Shift+E",
        action: ShellShortcut::Explorer,
    },
    Command {
        name: "Source Control",
        keywords: "git",
        shortcut: "Ctrl+Shift+G",
        action: ShellShortcut::SourceControl,
    },
    Command {
        name: "Stage All Changes",
        keywords: "git add stage files",
        shortcut: "",
        action: ShellShortcut::StageAllChanges,
    },
    Command {
        name: "Commit Staged Changes",
        keywords: "git commit",
        shortcut: "",
        action: ShellShortcut::CommitStagedChanges,
    },
    Command {
        name: "Problems",
        keywords: "diagnostics errors",
        shortcut: "⌘⇧M / Ctrl+Shift+M",
        action: ShellShortcut::Problems,
    },
    Command {
        name: "Settings",
        keywords: "preferences provider",
        shortcut: "⌘, / Ctrl+,",
        action: ShellShortcut::Settings,
    },
    Command {
        name: "Extensions",
        keywords: "install language server lsp gallery",
        shortcut: "⌘⇧X / Ctrl+Shift+X",
        action: ShellShortcut::Extensions,
    },
    Command {
        name: "AHEAD Guide",
        keywords: "help documentation how to",
        shortcut: "",
        action: ShellShortcut::Help,
    },
    Command {
        name: "New Terminal",
        keywords: "shell pty",
        shortcut: "",
        action: ShellShortcut::NewTerminal,
    },
    Command {
        name: "Show Debugger",
        keywords: "debug panel",
        shortcut: "⌘⇧D / Ctrl+Shift+D",
        action: ShellShortcut::Debug,
    },
    Command {
        name: "Start or Continue Debugging",
        keywords: "run resume",
        shortcut: "F5",
        action: ShellShortcut::DebugKey("f5", false),
    },
    Command {
        name: "Stop Debugging",
        keywords: "disconnect",
        shortcut: "Shift+F5",
        action: ShellShortcut::DebugKey("f5", true),
    },
    Command {
        name: "Toggle Breakpoint",
        keywords: "debug line",
        shortcut: "F9",
        action: ShellShortcut::DebugKey("f9", false),
    },
    Command {
        name: "Step Over",
        keywords: "debug",
        shortcut: "F10",
        action: ShellShortcut::DebugKey("f10", false),
    },
    Command {
        name: "Step Into",
        keywords: "debug",
        shortcut: "F11",
        action: ShellShortcut::DebugKey("f11", false),
    },
    Command {
        name: "Step Out",
        keywords: "debug",
        shortcut: "Shift+F11",
        action: ShellShortcut::DebugKey("f11", true),
    },
    Command {
        name: "AHEAD Agent",
        keywords: "chat assistant",
        shortcut: "⌃⌘I / Ctrl+Alt+I",
        action: ShellShortcut::Agent,
    },
    Command {
        name: "New AHEAD Session",
        keywords: "chat thread managed agent",
        shortcut: "",
        action: ShellShortcut::NewAheadSession,
    },
    Command {
        name: "Start External Agent",
        keywords: "chat thread ACP Pi Codex Claude Code",
        shortcut: "",
        action: ShellShortcut::NewExternalAgent,
    },
    Command {
        name: "Toggle Threads",
        keywords: "chat sidebar",
        shortcut: "",
        action: ShellShortcut::Threads,
    },
    Command {
        name: "Toggle Left Sidebar",
        keywords: "dock panel",
        shortcut: "⌘B / Ctrl+B",
        action: ShellShortcut::ToggleLeft,
    },
    Command {
        name: "Toggle Right Sidebar",
        keywords: "dock panel",
        shortcut: "⌥⌘B / Ctrl+Alt+B",
        action: ShellShortcut::ToggleRight,
    },
    Command {
        name: "Toggle Bottom Panel",
        keywords: "dock terminal panel",
        shortcut: "⌘J / Ctrl+J",
        action: ShellShortcut::ToggleBottom,
    },
    Command {
        name: "Tasks",
        keywords: "justfile recipes",
        shortcut: "",
        action: ShellShortcut::Tasks,
    },
    Command {
        name: "Language Servers",
        keywords: "lsp",
        shortcut: "",
        action: ShellShortcut::LanguageServers,
    },
    Command {
        name: "Zoom Agent Chat",
        keywords: "maximize chat",
        shortcut: "",
        action: ShellShortcut::ChatZoom,
    },
];

fn available_commands(
    context: CommandContext,
    tasks: &[JustTask],
) -> Vec<PaletteCommand> {
    let mut commands = COMMANDS
        .iter()
        .copied()
        .filter(|command| command.available(context))
        .map(|command| PaletteCommand {
            name: command.name.into(),
            keywords: command.keywords.into(),
            detail: platform_shortcut(command.shortcut).into(),
            action: CommandAction::Shell(command.action),
        })
        .collect::<Vec<_>>();
    commands.extend(tasks.iter().map(|task| PaletteCommand {
        name: format!("Run just {}", task.name),
        keywords: format!(
            "just task recipe {}",
            task.description.as_deref().unwrap_or_default()
        ),
        detail: task.cwd.to_string_lossy().into_owned(),
        action: CommandAction::RunJustTask {
            recipe: task.name.clone(),
            cwd: task.cwd.to_string_lossy().into_owned(),
            justfile: task.justfile.to_string_lossy().into_owned(),
        },
    }));
    commands
}

fn rank_commands(query: &str, commands: &[PaletteCommand]) -> Vec<PaletteCommand> {
    let query = query.trim();
    if query.is_empty() {
        return commands.to_vec();
    }
    let pattern = Pattern::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut matcher = FuzzyMatcher::new(FuzzyConfig::DEFAULT);
    let mut candidate_chars = Vec::new();
    let lower_query = query.to_ascii_lowercase();
    let mut matches = commands
        .iter()
        .filter_map(|command| {
            let name = command.name.to_ascii_lowercase();
            let keywords = command.keywords.to_ascii_lowercase();
            let title_score = pattern.score(
                Utf32Str::new(&command.name, &mut candidate_chars),
                &mut matcher,
            );
            let keyword_score = pattern.score(
                Utf32Str::new(&command.keywords, &mut candidate_chars),
                &mut matcher,
            );
            let score = if name == lower_query {
                (5, title_score.unwrap_or_default())
            } else if name.contains(&lower_query) {
                (4, title_score.unwrap_or_default())
            } else if keywords.split_whitespace().any(|word| word == lower_query) {
                (3, keyword_score.unwrap_or_default())
            } else if keywords
                .split_whitespace()
                .any(|word| word.starts_with(&lower_query))
            {
                (2, keyword_score.unwrap_or_default())
            } else if let Some(score) = title_score {
                (1, score)
            } else {
                (0, keyword_score?)
            };
            Some((score, command.clone()))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.name.cmp(&right.1.name))
    });
    matches.into_iter().map(|(_, command)| command).collect()
}

fn platform_shortcut(shortcut: &'static str) -> &'static str {
    let Some((mac, other)) = shortcut.split_once(" / ") else {
        return shortcut;
    };
    if cfg!(target_os = "macos") {
        mac
    } else {
        other
    }
}

#[cfg(test)]
mod guide_tests {
    use super::COMMANDS;

    #[test]
    fn command_palette_actions_appear_in_the_user_guide() {
        let guide = [
            include_str!("../../docs/guide/start-here.md"),
            include_str!("../../docs/guide/files-and-editing.md"),
            include_str!("../../docs/guide/search-and-git.md"),
            include_str!("../../docs/guide/terminal-and-debugging.md"),
            include_str!("../../docs/guide/agent-sessions.md"),
            include_str!("../../docs/guide/connections-and-settings.md"),
            include_str!("../../docs/guide/voice-and-sharing.md"),
            include_str!("../../docs/guide/shortcuts.md"),
        ]
        .join("\n");
        for command in COMMANDS {
            assert!(
                guide.to_lowercase().contains(&command.name.to_lowercase()),
                "Document Command Palette action: {}",
                command.name
            );
        }
    }
}

pub(crate) fn open(
    on_command: Box<dyn Fn(CommandAction, &mut Window, &mut App)>,
    context: CommandContext,
    tasks: Vec<JustTask>,
    window: &mut Window,
    cx: &mut App,
) {
    if window.has_active_dialog(cx) {
        return;
    }
    let previous_focus = window.focused(cx);
    let palette = cx.new(|cx| {
        CommandPalette::new(on_command, context, tasks, previous_focus, window, cx)
    });
    let input_focus = palette.read(cx).query.read(cx).focus_handle(cx);
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Command Palette")
            .width(px(720.))
            .close_button(false)
            .on_ok({
                let palette = palette.clone();
                move |_, window, cx| {
                    palette
                        .update(cx, |palette, cx| palette.run_selected(window, cx));
                    false
                }
            })
            .child(palette.clone())
    });
    window.focus(&input_focus, cx);
}

struct CommandPalette {
    query: Entity<InputState>,
    commands: Vec<PaletteCommand>,
    results: Vec<PaletteCommand>,
    selected_index: usize,
    scroll: ScrollHandle,
    previous_focus: Option<FocusHandle>,
    on_command: Box<dyn Fn(CommandAction, &mut Window, &mut App)>,
    _escape_interceptor: Subscription,
}

impl CommandPalette {
    fn new(
        on_command: Box<dyn Fn(CommandAction, &mut Window, &mut App)>,
        context: CommandContext,
        tasks: Vec<JustTask>,
        previous_focus: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let commands = available_commands(context, &tasks);
        let query = cx.new(|cx| InputState::new(window, cx));
        let focus = query.read(cx).focus_handle(cx);
        let escape_interceptor =
            cx.intercept_keystrokes(move |event, window, cx| {
                if focus.is_focused(window)
                    && event.keystroke.key.as_str() == "escape"
                {
                    window.close_dialog(cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            });
        cx.subscribe(&query, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.results =
                    rank_commands(&this.query.read(cx).value(), &this.commands);
                this.selected_index = 0;
                this.scroll.scroll_to_item(0);
                cx.notify();
            }
        })
        .detach();
        Self {
            query,
            results: commands.clone(),
            commands,
            selected_index: 0,
            scroll: ScrollHandle::new(),
            previous_focus,
            on_command,
            _escape_interceptor: escape_interceptor,
        }
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "down" if !self.results.is_empty() => {
                self.selected_index = (self.selected_index + 1) % self.results.len();
                self.scroll.scroll_to_item(self.selected_index);
                cx.notify();
            }
            "up" if !self.results.is_empty() => {
                self.selected_index = (self.selected_index + self.results.len() - 1)
                    % self.results.len();
                self.scroll.scroll_to_item(self.selected_index);
                cx.notify();
            }
            _ => {}
        }
    }

    fn run_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(command) = self.results.get(self.selected_index).cloned() {
            self.run(command, window, cx);
        }
    }

    fn run(
        &mut self,
        command: PaletteCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.close_dialog(cx);
        if let Some(focus) = &self.previous_focus {
            window.focus(focus, cx);
        }
        (self.on_command)(command.action, window, cx);
    }
}

impl Render for CommandPalette {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let focus = self.query.read(cx).focus_handle(cx);
        let muted = cx.theme().muted_foreground;
        let results = self.results.iter().enumerate().map(|(index, command)| {
            let command = command.clone();
            let click_command = command.clone();
            ListItem::new(index)
                .w_full()
                .selected(index == self.selected_index)
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .gap_2()
                        .child(command.name.clone())
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(muted)
                                .child(command.detail.clone()),
                        ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run(click_command.clone(), window, cx)
                }))
        });
        let footer = if self.results.is_empty() {
            "No matching commands"
        } else {
            "↑↓ navigate · ↵ run · Esc close"
        };
        let results_height = px((self.results.len().clamp(1, 8) * 27) as f32);
        v_flex()
            .w_full()
            .track_focus(&focus)
            .on_key_down(cx.listener(Self::handle_key_down))
            .gap_2()
            .child(
                Input::new(&self.query)
                    .cleanable(true)
                    .aria_label("Search editor commands"),
            )
            .child(
                v_flex()
                    .id("command-palette-results")
                    .h(results_height)
                    .track_scroll(&self.scroll)
                    .overflow_y_scroll()
                    .children(results)
                    .vertical_scrollbar(&self.scroll),
            )
            .child(div().text_size(px(11.)).text_color(muted).child(footer))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CommandAction, CommandContext, CommandPalette, EditorCommand, JustTask,
        ShellShortcut, available_commands, open, rank_commands,
    };
    use gpui_kit::component::WindowExt;
    use gpui_kit::component::input::InputState;
    use gpui_kit::prelude::*;
    use gpui_kit::{
        AppContext, Context, Focusable, IntoElement, Render, TestAppContext, Window,
        div,
    };
    use std::sync::{Arc, Mutex};

    struct TestRoot;

    impl Render for TestRoot {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> impl IntoElement {
            div()
                .children(gpui_kit::component::Root::render_dialog_layer(window, cx))
        }
    }

    #[test]
    fn filters_commands_by_name_and_keyword() {
        let commands = available_commands(
            CommandContext {
                text_editor: true,
                ..CommandContext::default()
            },
            &[],
        );
        assert_eq!(
            rank_commands("breakpoint", &commands)[0].action,
            CommandAction::Shell(ShellShortcut::DebugKey("f9", false))
        );
        assert_eq!(
            rank_commands("git", &commands)[0].action,
            CommandAction::Shell(ShellShortcut::SourceControl)
        );
        assert_eq!(
            rank_commands("stage all", &commands)[0].action,
            CommandAction::Shell(ShellShortcut::StageAllChanges)
        );
        assert_eq!(
            rank_commands("extensions", &commands)[0].action,
            CommandAction::Shell(ShellShortcut::Extensions)
        );
        assert_eq!(
            rank_commands("new ahead", &commands)[0].action,
            CommandAction::Shell(ShellShortcut::NewAheadSession)
        );
        assert_eq!(
            rank_commands("claude", &commands)[0].action,
            CommandAction::Shell(ShellShortcut::NewExternalAgent)
        );
        let commands = available_commands(
            CommandContext {
                active_editor: true,
                ..CommandContext::default()
            },
            &[],
        );
        assert_eq!(
            rank_commands("close all", &commands)[0].action,
            CommandAction::Shell(ShellShortcut::CloseAllEditors)
        );
        assert!(rank_commands("unlikely unknown action", &commands).is_empty());
    }

    #[test]
    fn palette_hides_commands_without_their_editor_or_debug_target() {
        let commands = available_commands(CommandContext::default(), &[]);
        for action in [
            ShellShortcut::SaveActiveFile,
            ShellShortcut::CloseEditor,
            ShellShortcut::CloseOtherEditors,
            ShellShortcut::CloseEditorsToLeft,
            ShellShortcut::CloseEditorsToRight,
            ShellShortcut::CloseAllEditors,
            ShellShortcut::MoveEditorToOtherGroup,
            ShellShortcut::DebugKey("f9", false),
            ShellShortcut::DebugKey("f5", false),
            ShellShortcut::DebugKey("f5", true),
            ShellShortcut::DebugKey("f10", false),
            ShellShortcut::CommitStagedChanges,
            ShellShortcut::NavigateBack,
            ShellShortcut::NavigateForward,
        ] {
            assert!(
                !commands.iter().any(|command| {
                    command.action == CommandAction::Shell(action)
                })
            );
        }
        assert!(commands.iter().any(|command| command.action
            == CommandAction::Shell(ShellShortcut::OpenFileDialog)));

        let commands = available_commands(
            CommandContext {
                active_editor: true,
                text_editor: true,
                language_editor: true,
                can_navigate_back: true,
                can_navigate_forward: true,
                can_start_or_continue_debugging: true,
                can_stop_debugging: true,
                can_step_debugging: true,
                can_commit_staged: true,
                can_close_other_editors: true,
                can_close_editors_to_left: true,
                can_close_editors_to_right: true,
                can_move_editor_to_other_group: true,
            },
            &[],
        );
        assert!(commands.iter().any(|command| command.action
            == CommandAction::Shell(ShellShortcut::SaveActiveFile)));
        assert!(commands.iter().any(|command| command.action
            == CommandAction::Shell(ShellShortcut::CloseEditor)));
        for action in [
            ShellShortcut::CloseOtherEditors,
            ShellShortcut::CloseEditorsToLeft,
            ShellShortcut::CloseEditorsToRight,
            ShellShortcut::CloseAllEditors,
            ShellShortcut::MoveEditorToOtherGroup,
        ] {
            assert!(
                commands.iter().any(|command| {
                    command.action == CommandAction::Shell(action)
                })
            );
        }
        assert!(commands.iter().any(|command| command.action
            == CommandAction::Shell(ShellShortcut::DebugKey("f10", false))));
        assert!(commands.iter().any(|command| command.action
            == CommandAction::Shell(ShellShortcut::CommitStagedChanges)));
        assert!(commands.iter().any(|command| command.action
            == CommandAction::Shell(ShellShortcut::NavigateBack)));
        assert!(commands.iter().any(|command| command.action
            == CommandAction::Shell(ShellShortcut::NavigateForward)));
    }

    #[test]
    fn language_commands_require_an_open_text_editor_with_language_services() {
        let language_commands = [
            EditorCommand::GoToDefinition,
            EditorCommand::FindReferences,
            EditorCommand::GoToImplementation,
            EditorCommand::RenameSymbol,
            EditorCommand::FormatDocument,
            EditorCommand::ShowCompletions,
            EditorCommand::ShowSignatureHelp,
            EditorCommand::ShowCodeActions,
        ];
        for context in [
            CommandContext::default(),
            CommandContext {
                text_editor: true,
                ..CommandContext::default()
            },
        ] {
            let commands = available_commands(context, &[]);
            assert!(language_commands.iter().all(|action| {
                !commands.iter().any(|command| {
                    command.action
                        == CommandAction::Shell(ShellShortcut::Editor(*action))
                })
            }));
        }

        let commands = available_commands(
            CommandContext {
                text_editor: true,
                language_editor: true,
                ..CommandContext::default()
            },
            &[],
        );
        for action in language_commands {
            assert!(commands.iter().any(|command| {
                command.action == CommandAction::Shell(ShellShortcut::Editor(action))
            }));
        }
        assert_eq!(
            rank_commands("signature", &commands)[0].action,
            CommandAction::Shell(ShellShortcut::Editor(
                EditorCommand::ShowSignatureHelp
            ))
        );
    }

    #[test]
    fn discovered_just_recipes_are_searchable_palette_actions() {
        let workspace = tempfile::tempdir().expect("workspace");
        let cwd = workspace.path().to_string_lossy().into_owned();
        let justfile = workspace
            .path()
            .join("justfile")
            .to_string_lossy()
            .into_owned();
        let task = JustTask {
            justfile: workspace.path().join("justfile"),
            cwd: workspace.path().to_owned(),
            name: "format".into(),
            description: Some("Format Rust and TypeScript".into()),
        };
        let commands = available_commands(CommandContext::default(), &[task]);
        assert_eq!(
            rank_commands("format", &commands)[0].action,
            CommandAction::RunJustTask {
                recipe: "format".into(),
                cwd,
                justfile,
            }
        );
    }

    #[gpui_kit::test]
    fn palette_filters_and_runs_selected_command(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let root = cx.new(|_| TestRoot);
            gpui_kit::component::Root::new(root, window, cx)
        });
        let selected = Arc::new(Mutex::new(None));
        let selected_for_callback = selected.clone();
        cx.update(|window, cx| {
            open(
                Box::new(move |command, _, _| {
                    *selected_for_callback.lock().unwrap() = Some(command);
                }),
                CommandContext::default(),
                Vec::new(),
                window,
                cx,
            );
            window.draw(cx).clear(cx);
        });
        // The command is selected through the same dialog Enter path as a native user.
        cx.simulate_keystrokes("enter");
        assert_eq!(
            *selected.lock().unwrap(),
            Some(CommandAction::Shell(ShellShortcut::QuickOpen))
        );
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn typed_query_filters_and_runs_a_discovered_just_recipe(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let root = cx.new(|_| TestRoot);
            gpui_kit::component::Root::new(root, window, cx)
        });
        let selected = Arc::new(Mutex::new(None));
        let selected_for_callback = selected.clone();
        let workspace = tempfile::tempdir().expect("workspace");
        let cwd = workspace.path().to_string_lossy().into_owned();
        let justfile = workspace
            .path()
            .join("justfile")
            .to_string_lossy()
            .into_owned();
        let task = JustTask {
            justfile: workspace.path().join("justfile"),
            cwd: workspace.path().to_owned(),
            name: "format".into(),
            description: Some("Format project files".into()),
        };
        let palette = cx.update(|window, cx| {
            let palette = cx.new(|cx| {
                CommandPalette::new(
                    Box::new(move |command, _, _| {
                        *selected_for_callback.lock().unwrap() = Some(command);
                    }),
                    CommandContext {
                        active_editor: true,
                        text_editor: true,
                        ..CommandContext::default()
                    },
                    vec![task],
                    None,
                    window,
                    cx,
                )
            });
            let focus = palette.read(cx).query.read(cx).focus_handle(cx);
            window.open_dialog(cx, {
                let palette = palette.clone();
                move |dialog, _, _| {
                    dialog
                        .title("Command Palette")
                        .on_ok({
                            let palette = palette.clone();
                            move |_, window, cx| {
                                palette.update(cx, |palette, cx| {
                                    palette.run_selected(window, cx)
                                });
                                false
                            }
                        })
                        .child(palette.clone())
                }
            });
            window.focus(&focus, cx);
            palette
        });
        let query = palette.read_with(cx, |palette, _| palette.query.clone());
        cx.simulate_keystrokes("down down down down down down down down down");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        palette.read_with(cx, |palette, _| {
            assert_eq!(palette.selected_index, 9);
            assert!(palette.scroll.offset().y < gpui_kit::px(0.));
        });
        query.update_in(cx, |input: &mut InputState, window, cx| {
            input.replace_all("format".to_string(), window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        palette.read_with(cx, |palette, _| {
            assert_eq!(
                palette.results[0].action,
                CommandAction::RunJustTask {
                    recipe: "format".into(),
                    cwd: cwd.clone(),
                    justfile: justfile.clone(),
                }
            );
            assert_eq!(palette.selected_index, 0);
        });
        cx.simulate_keystrokes("enter");
        assert_eq!(
            *selected.lock().unwrap(),
            Some(CommandAction::RunJustTask {
                recipe: "format".into(),
                cwd,
                justfile,
            })
        );
    }
}
