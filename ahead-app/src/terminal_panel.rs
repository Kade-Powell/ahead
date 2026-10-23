//! AHEAD Interactive Terminal - Direct PTY Terminal Emulator
//!
//! True terminal experience matching Zed / VS Code:
//! - NO text input box: the terminal grid itself captures focus and keystrokes.
//! - Keystrokes are dispatched directly to the PTY stdin (Enter, Backspace, Tab,
//!   arrows, Ctrl+C, Ctrl+D, and raw characters).
//! - Shell stdout/stderr is parsed by Alacritty's VTE state machine.
//! - The GPUI view renders the parsed grid, selection, cursor, scrollback,
//!   alternate screen, ANSI colors, truecolor, and text attributes.

use std::{ops::Range, path::PathBuf, rc::Rc, time::Duration};

use alacritty_terminal::event::Event as TerminalEvent;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{
    Color as TerminalColor, CursorShape, NamedColor, Rgb,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent, PanelId, TabGroup,
};
use gpui_kit::component::input::{
    Copy as InputCopy, Input, InputState, Paste as InputPaste,
    SelectAll as InputSelectAll,
};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use gpui_kit_assets::IconName;

use ahead_rpc::ahead::AgentTerminalAuthLaunch;

use crate::terminal::{TerminalBackend, TerminalCell, TerminalSnapshot};

#[derive(Clone, Copy, PartialEq, Eq)]
struct TerminalRunStyle {
    foreground: TerminalColor,
    background: TerminalColor,
    flags: Flags,
    cursor: bool,
    selected: bool,
}

struct TerminalRun {
    text: String,
    style: TerminalRunStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalClipboardAction {
    Copy,
    Paste,
    PasteText,
}

#[derive(Clone, Copy)]
struct TerminalPalette {
    background: Hsla,
    foreground: Hsla,
    selection: Hsla,
    black: Hsla,
    red: Hsla,
    green: Hsla,
    yellow: Hsla,
    blue: Hsla,
    magenta: Hsla,
    cyan: Hsla,
    white: Hsla,
    bright_black: Hsla,
    bright_red: Hsla,
    bright_green: Hsla,
    bright_yellow: Hsla,
    bright_blue: Hsla,
    bright_magenta: Hsla,
    bright_cyan: Hsla,
    bright_white: Hsla,
}

pub struct TerminalPanel {
    pub focus: FocusHandle,
    terminal: TerminalTab,
    rename_input: Option<Entity<InputState>>,
    marked_text: Option<String>,
    scroll_remainder: f32,
    terminal_size: Option<(usize, usize)>,
    terminal_bounds: Option<Bounds<Pixels>>,
    cell_size: Option<(f32, f32)>,
    selection_drag_active: bool,
    mouse_reporting_button: Option<MouseButton>,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
    close_handler: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    new_terminal_handler: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    exit_handler: Option<Rc<dyn Fn(Option<i32>, &mut Context<Self>)>>,
}

struct TerminalTab {
    id: usize,
    terminal: Option<TerminalBackend>,
    shell_name: String,
    custom_title: Option<String>,
    status: SharedString,
}

impl TerminalTab {
    fn new(id: usize, cwd: String, shell: &str) -> Self {
        Self::new_with_command(
            id,
            cwd,
            shell.to_string(),
            Vec::new(),
            std::collections::HashMap::new(),
            None,
        )
    }

    fn new_with_command(
        id: usize,
        cwd: String,
        program: String,
        args: Vec<String>,
        env: std::collections::HashMap<String, String>,
        title: Option<String>,
    ) -> Self {
        let shell_name = std::path::Path::new(&program)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("shell")
            .to_string();
        let (terminal, status) =
            match TerminalBackend::new_with_command(&cwd, &program, args, env) {
                Ok(terminal) => (Some(terminal), SharedString::from("ready")),
                Err(error) => (None, format!("terminal error: {error}").into()),
            };
        Self {
            id,
            terminal,
            shell_name,
            custom_title: title,
            status,
        }
    }

    fn title(&self) -> String {
        self.custom_title
            .clone()
            .unwrap_or_else(|| self.default_title())
    }

    fn default_title(&self) -> String {
        format!("{} {}", self.shell_name, self.id)
    }

    fn set_custom_title(&mut self, title: &str) {
        let title = title.trim();
        self.custom_title = (!title.is_empty() && title != self.default_title())
            .then(|| title.to_string());
    }
}

impl TerminalPanel {
    pub fn new_with_id(id: usize, cx: &mut Context<Self>) -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "~".to_string());
        Self::new_with_cwd(id, cwd, cx)
    }

    pub fn new_with_cwd(
        id: usize,
        cwd: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let shell =
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        Self::new_with_shell(id, cwd.into(), &shell, cx)
    }

    pub(crate) fn new_with_shell(
        id: usize,
        cwd: String,
        shell: &str,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::from_tab(TerminalTab::new(id, cwd, shell), None, cx)
    }

    pub(crate) fn new_for_agent_auth(
        id: usize,
        launch: AgentTerminalAuthLaunch,
        exit_handler: impl Fn(Option<i32>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let tab = TerminalTab::new_with_command(
            id,
            launch.cwd.to_string_lossy().into_owned(),
            launch.program,
            launch.args,
            launch.env,
            Some(launch.name),
        );
        Self::from_tab(tab, Some(Rc::new(exit_handler)), cx)
    }

    fn from_tab(
        tab: TerminalTab,
        exit_handler: Option<Rc<dyn Fn(Option<i32>, &mut Context<Self>)>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let panel = Self {
            focus: cx.focus_handle(),
            terminal: tab,
            rename_input: None,
            marked_text: None,
            scroll_remainder: 0.0,
            terminal_size: None,
            terminal_bounds: None,
            cell_size: None,
            selection_drag_active: false,
            mouse_reporting_button: None,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
            close_handler: None,
            new_terminal_handler: None,
            exit_handler,
        };
        panel.start_output_pump(cx);
        panel
    }

    pub fn terminal_id(&self) -> usize {
        self.terminal.id
    }

    pub(crate) fn process_id(&self) -> Option<u32> {
        self.terminal.terminal.as_ref()?.process_id()
    }

    pub(crate) fn run_debug_command(
        &mut self,
        command: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let terminal = self
            .terminal
            .terminal
            .as_ref()
            .ok_or_else(|| self.terminal.status.to_string())?;
        terminal.send(format!("{command}\r").into_bytes())?;
        cx.notify();
        Ok(())
    }

    pub(crate) fn shutdown(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<crossbeam_channel::Receiver<()>> {
        let terminal = self.terminal.terminal.as_mut()?;
        if terminal.is_shutdown() {
            return None;
        }
        let complete = terminal.shutdown();
        self.terminal.status = "closed".into();
        cx.notify();
        Some(complete)
    }

    #[cfg(test)]
    pub(crate) fn is_shutdown(&self) -> bool {
        self.terminal
            .terminal
            .as_ref()
            .is_none_or(TerminalBackend::is_shutdown)
    }

    pub fn run_command(&mut self, command: &str, cx: &mut Context<Self>) {
        self.send_bytes(format!("{command}\r").into_bytes());
        cx.notify();
    }

    pub(crate) fn set_terminal_title(
        &mut self,
        title: &str,
        cx: &mut Context<Self>,
    ) {
        self.terminal.set_custom_title(title);
        cx.notify();
    }

    pub(crate) fn terminal_title(&self) -> String {
        self.terminal.title()
    }

    pub fn set_close_handler<F>(&mut self, handler: F)
    where
        F: Fn(&mut Window, &mut App) + 'static,
    {
        self.close_handler = Some(Rc::new(handler));
    }

    pub fn set_new_terminal_handler<F>(&mut self, handler: F)
    where
        F: Fn(&mut Window, &mut App) + 'static,
    {
        self.new_terminal_handler = Some(Rc::new(handler));
    }

    fn start_output_pump(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut polled_after_exit = false;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let running = this.update(cx, |this, cx| {
                    let received = this
                        .terminal
                        .terminal
                        .as_ref()
                        .into_iter()
                        .flat_map(|terminal| {
                            terminal.events().try_iter().collect::<Vec<_>>()
                        })
                        .collect::<Vec<_>>();
                    let had_events = !received.is_empty();
                    for event in received {
                        this.handle_terminal_event(event, cx);
                    }
                    if had_events {
                        cx.notify();
                    }
                    let running = this
                        .terminal
                        .terminal
                        .as_ref()
                        .is_some_and(TerminalBackend::is_running);
                    if !running
                        && let Some(terminal) = this.terminal.terminal.as_mut()
                    {
                        drop(terminal.shutdown());
                        cx.notify();
                    }
                    running
                });
                if matches!(running, Ok(true)) {
                    polled_after_exit = false;
                } else if polled_after_exit {
                    break;
                } else {
                    // The PTY can queue its final exit event just after this
                    // poll and before its IO thread finishes.
                    polled_after_exit = true;
                }
            }
        })
        .detach();
    }

    fn resize_to_bounds(
        &mut self,
        bounds: Bounds<Pixels>,
        font_size: Pixels,
        cx: &mut Context<Self>,
    ) {
        self.terminal_bounds = Some(bounds);
        let cell_width = (f32::from(font_size) * 0.6).max(1.0);
        let line_height = (f32::from(font_size) * 1.5).max(1.0);
        self.cell_size = Some((cell_width, line_height));
        let width = (f32::from(bounds.size.width) - 24.0).max(cell_width);
        let height = (f32::from(bounds.size.height) - 24.0).max(line_height);
        let columns = (width / cell_width).floor().max(1.0) as usize;
        let rows = (height / line_height).floor().max(1.0) as usize;

        if self.terminal_size == Some((columns, rows)) {
            return;
        }

        let Some(terminal) = self.terminal.terminal.as_ref() else {
            return;
        };

        match terminal.resize(columns, rows) {
            Ok(()) => {
                self.terminal_size = Some((columns, rows));
                cx.notify();
            }
            Err(error) => {
                self.terminal.status =
                    format!("terminal resize error: {error}").into();
                cx.notify();
            }
        }
    }

    fn begin_terminal_rename(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.rename_input.is_some() {
            return;
        }

        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |input, cx| {
            input.set_value(self.terminal.title(), window, cx);
        });
        input.focus_handle(cx).focus(window, cx);
        self.rename_input = Some(input);
        cx.notify();
    }

    fn finish_terminal_rename(
        &mut self,
        save: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.rename_input.take() else {
            return;
        };
        if save {
            self.terminal
                .set_custom_title(&input.read(cx).value().to_string());
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn handle_terminal_rename_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "enter" => self.finish_terminal_rename(true, window, cx),
            "escape" => self.finish_terminal_rename(false, window, cx),
            _ => return,
        }
        window.prevent_default();
        cx.stop_propagation();
    }

    fn active_terminal(&self) -> Option<&TerminalBackend> {
        self.terminal.terminal.as_ref()
    }

    fn snapshot(&self) -> TerminalSnapshot {
        self.active_terminal()
            .map(TerminalBackend::snapshot)
            .unwrap_or_default()
    }

    pub(crate) fn shared_snapshot_text(&self) -> String {
        let output = self
            .active_terminal()
            .map(TerminalBackend::shared_history_text)
            .unwrap_or_default();
        format!("{}\n{output}", self.terminal.title())
    }

    pub(crate) fn agent_context(&self) -> Option<(String, String, bool)> {
        let terminal = self.active_terminal()?;
        let (output, selected) = terminal.agent_context();
        (!output.trim().is_empty())
            .then(|| (self.terminal.title(), output, selected))
    }

    fn send_bytes(&mut self, bytes: Vec<u8>) {
        if let Some(terminal) = self.terminal.terminal.as_ref()
            && let Err(error) = terminal.send(bytes)
        {
            self.terminal.status = error.into();
        }
    }

    fn handle_terminal_event(
        &mut self,
        event: TerminalEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TerminalEvent::PtyWrite(text) => {
                if let Some(terminal) = self.terminal.terminal.as_ref()
                    && let Err(error) = terminal.send(text.into_bytes())
                {
                    self.terminal.status = error.into();
                }
            }
            TerminalEvent::Title(title) => self.terminal.shell_name = title,
            TerminalEvent::ResetTitle => {
                self.terminal.shell_name = "shell".to_string()
            }
            TerminalEvent::ChildExit(code) => {
                self.terminal.status = format!("shell exited ({code})").into();
                self.notify_exit_handler(Some(code), cx);
            }
            TerminalEvent::Exit => {
                self.terminal.status = "terminal closed".into();
                self.notify_exit_handler(None, cx);
            }
            TerminalEvent::Bell => self.terminal.status = "bell".into(),
            TerminalEvent::ClipboardStore(_, text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            TerminalEvent::ClipboardLoad(_, format) => {
                let text = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default();
                if let Some(terminal) = self.terminal.terminal.as_ref()
                    && let Err(error) = terminal.send(format(&text).into_bytes())
                {
                    self.terminal.status = error.into();
                }
            }
            TerminalEvent::Wakeup
            | TerminalEvent::MouseCursorDirty
            | TerminalEvent::ColorRequest(_, _)
            | TerminalEvent::TextAreaSizeRequest(_)
            | TerminalEvent::CursorBlinkingChange => {}
        }
    }

    fn notify_exit_handler(&mut self, code: Option<i32>, cx: &mut Context<Self>) {
        if let Some(handler) = self.exit_handler.take() {
            handler(code, cx);
        }
    }

    pub fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let selected_text = self
            .active_terminal()
            .and_then(TerminalBackend::selected_text);
        if let Some(scroll) = terminal_scroll_action(
            event.keystroke.key.as_str(),
            event.keystroke.modifiers,
            self.terminal_mode(),
        ) && self.active_terminal().is_some()
        {
            self.scroll_display(scroll);
            cx.notify();
            return true;
        }
        if terminal_select_all_action(
            event.keystroke.key.as_str(),
            event.keystroke.modifiers,
        ) && self.active_terminal().is_some()
        {
            self.select_all(cx);
            return true;
        }
        match terminal_clipboard_action(
            event.keystroke.key.as_str(),
            event.keystroke.modifiers,
            selected_text.is_some(),
        ) {
            Some(TerminalClipboardAction::Copy) => {
                self.copy_selection_to_clipboard(selected_text, cx);
                cx.notify();
                return true;
            }
            Some(TerminalClipboardAction::Paste) => {
                self.paste_from_clipboard(cx);
                cx.notify();
                return true;
            }
            Some(TerminalClipboardAction::PasteText) => {
                self.paste_text_from_clipboard(cx);
                cx.notify();
                return true;
            }
            None => {}
        }

        if should_defer_character_input(
            event.prefer_character_input,
            event.keystroke.key_char.is_some(),
        ) {
            return false;
        }

        if let Some(bytes) = terminal_key_bytes(event, self.terminal_mode()) {
            self.send_bytes(bytes);
            cx.notify();
            return true;
        }

        false
    }

    fn copy_selection_to_clipboard(
        &self,
        selected_text: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = selected_text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn copy_action(
        &mut self,
        _: &InputCopy,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected_text = self
            .active_terminal()
            .and_then(TerminalBackend::selected_text);
        self.copy_selection_to_clipboard(selected_text, cx);
        cx.notify();
    }

    fn paste_action(
        &mut self,
        _: &InputPaste,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.paste_from_clipboard(cx);
        cx.notify();
    }

    fn select_all_action(
        &mut self,
        _: &InputSelectAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_all(cx);
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        if let Some(terminal) = self.active_terminal() {
            terminal.select_all();
        }
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        self.copy_selection_to_primary(cx);
        cx.notify();
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn copy_selection_to_primary(&self, cx: &mut Context<Self>) {
        if let Some(text) = self
            .active_terminal()
            .and_then(TerminalBackend::selected_text)
            .filter(|text| !text.is_empty())
        {
            cx.write_to_primary(ClipboardItem::new_string(text));
        }
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn paste_primary_selection(&mut self, cx: &mut Context<Self>) {
        let result = cx
            .read_from_primary()
            .and_then(|item| item.text())
            .filter(|text| !text.is_empty())
            .and_then(|text| {
                self.active_terminal().map(|terminal| terminal.paste(&text))
            });
        if let Some(Err(error)) = result {
            self.terminal.status = error.into();
        }
        cx.notify();
    }

    fn paste_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let paste_result = cx.read_from_clipboard().and_then(|item| {
            if let Some(entry) = item.entries().first() {
                match entry {
                    ClipboardEntry::Image(image) if !image.bytes.is_empty() => {
                        return self
                            .active_terminal()
                            .map(|terminal| terminal.send(vec![0x16]));
                    }
                    ClipboardEntry::ExternalPaths(paths) => {
                        return terminal_paths_to_paste(paths.paths()).and_then(
                            |text| {
                                self.active_terminal()
                                    .map(|terminal| terminal.paste(&text))
                            },
                        );
                    }
                    _ => {}
                }
            }
            item.text().and_then(|text| {
                self.active_terminal().map(|terminal| terminal.paste(&text))
            })
        });
        if let Some(Err(error)) = paste_result {
            self.terminal.status = error.into();
        }
    }

    fn paste_text_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let paste_result = cx.read_from_clipboard().and_then(|item| {
            item.text().and_then(|text| {
                self.active_terminal().map(|terminal| terminal.paste(&text))
            })
        });
        if let Some(Err(error)) = paste_result {
            self.terminal.status = error.into();
        }
    }

    fn start_terminal_mouse(
        &mut self,
        button: MouseButton,
        position: Point<Pixels>,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(terminal) = self.active_terminal() else {
            return false;
        };
        let mode = terminal.mode();
        if !terminal_mouse_mode(mode, modifiers.shift) {
            return false;
        }
        let display_offset = terminal.display_offset();
        let Some((point, _)) = self.grid_point_at(position, display_offset) else {
            return false;
        };
        let Some(report) =
            terminal_mouse_button_report(point, button, modifiers, true, mode)
        else {
            return false;
        };

        terminal.clear_selection();
        self.send_bytes(report);
        self.selection_drag_active = false;
        self.mouse_reporting_button = Some(button);
        cx.notify();
        true
    }

    fn finish_terminal_mouse(
        &mut self,
        button: MouseButton,
        position: Point<Pixels>,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.mouse_reporting_button != Some(button) {
            return false;
        }
        self.mouse_reporting_button = None;
        let report = self.active_terminal().and_then(|terminal| {
            let mode = terminal.mode();
            let display_offset = terminal.display_offset();
            let (point, _) = self.grid_point_at(position, display_offset)?;
            terminal_mouse_button_report(point, button, modifiers, false, mode)
        });
        if let Some(report) = report {
            self.send_bytes(report);
        }
        self.selection_drag_active = false;
        cx.notify();
        true
    }

    fn start_selection(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(terminal) = self.active_terminal() else {
            return;
        };
        let selection_type = match event.click_count {
            1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => {
                terminal.clear_selection();
                self.selection_drag_active = false;
                cx.notify();
                return;
            }
        };
        let display_offset = terminal.display_offset();
        let Some((point, side)) = self.grid_point_at(event.position, display_offset)
        else {
            return;
        };
        terminal.set_selection(point, side, selection_type);
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        self.copy_selection_to_primary(cx);
        self.selection_drag_active = true;
        cx.notify();
    }

    fn update_selection(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(terminal) = self.active_terminal() else {
            return;
        };
        let mode = terminal.mode();
        let display_offset = terminal.display_offset();
        if self.mouse_reporting_button.is_some() {
            if terminal_mouse_mode(mode, event.modifiers.shift)
                && let Some((point, _)) =
                    self.grid_point_at(event.position, display_offset)
                && let Some(report) = terminal_mouse_move_report(
                    point,
                    event.pressed_button,
                    event.modifiers,
                    mode,
                )
            {
                self.send_bytes(report);
                cx.notify();
            }
            return;
        }
        if terminal_mouse_mode(mode, event.modifiers.shift)
            && event.pressed_button.is_none()
            && let Some((point, _)) =
                self.grid_point_at(event.position, display_offset)
            && let Some(report) =
                terminal_mouse_move_report(point, None, event.modifiers, mode)
        {
            self.send_bytes(report);
            cx.notify();
            return;
        }
        if !self.selection_drag_active || !event.dragging() {
            return;
        }
        let Some((point, side)) = self.grid_point_at(event.position, display_offset)
        else {
            return;
        };
        terminal.update_selection(point, side);
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        self.copy_selection_to_primary(cx);
        cx.notify();
    }

    fn finish_selection(&mut self) {
        self.selection_drag_active = false;
    }

    fn grid_point_at(
        &self,
        position: Point<Pixels>,
        display_offset: usize,
    ) -> Option<(GridPoint, Side)> {
        let bounds = self.terminal_bounds?;
        let (columns, rows) = self.terminal_size?;
        let (cell_width, line_height) = self.cell_size?;
        let columns = columns.max(1);
        let rows = rows.max(1);
        let x = f32::from(position.x - bounds.origin.x) - 12.0;
        let y = f32::from(position.y - bounds.origin.y) - 12.0;
        let raw_column = (x / cell_width).floor() as i32;
        let raw_row = (y / line_height).floor() as i32;
        let column = raw_column.clamp(0, columns.saturating_sub(1) as i32);
        let row = raw_row.clamp(0, rows.saturating_sub(1) as i32);
        let side = if x < 0.0 || y < 0.0 {
            Side::Left
        } else if raw_column >= columns as i32 || raw_row >= rows as i32 {
            Side::Right
        } else if x.rem_euclid(cell_width) > cell_width / 2.0 {
            Side::Right
        } else {
            Side::Left
        };
        let display_offset = i32::try_from(display_offset).unwrap_or(i32::MAX);
        Some((
            GridPoint::new(
                Line(row.saturating_sub(display_offset)),
                Column(column as usize),
            ),
            side,
        ))
    }

    fn terminal_mode(&self) -> TermMode {
        self.active_terminal()
            .map(TerminalBackend::mode)
            .unwrap_or_else(TermMode::empty)
    }

    fn scroll_display(&self, scroll: Scroll) {
        if let Some(terminal) = self.active_terminal() {
            terminal.scroll_display(scroll);
        }
    }
}

impl EntityInputHandler for TerminalPanel {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_text
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked_text.take().is_some() {
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = None;
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        self.send_bytes(text.into_bytes());
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = Some(new_text.to_string());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

fn terminal_key_bytes(event: &KeyDownEvent, mode: TermMode) -> Option<Vec<u8>> {
    let key = event.keystroke.key.as_str();
    let modifiers = event.keystroke.modifiers;
    let ctrl = modifiers.control;
    let alt = modifiers.alt;
    let shift = modifiers.shift;

    if cfg!(target_os = "macos") && modifiers.platform && !ctrl && !alt && !shift {
        let control = match key {
            "left" => Some(1),
            "right" => Some(5),
            "backspace" => Some(21),
            "delete" => Some(11),
            _ => None,
        };
        if let Some(control) = control {
            return Some(vec![control]);
        }
    }

    if alt && !ctrl && !shift {
        let sequence = match key {
            "b" | "left" => Some("\x1bb"),
            "f" | "right" => Some("\x1bf"),
            "." => Some("\x1b."),
            "delete" if cfg!(target_os = "macos") => Some("\x1bd"),
            _ => None,
        };
        if let Some(sequence) = sequence {
            return Some(sequence.as_bytes().to_vec());
        }
    }

    if ctrl && !alt && !shift && key == "backspace" {
        return Some(vec![23]);
    }

    if ctrl && !alt && !shift && key == "delete" {
        return Some(
            if cfg!(target_os = "windows") {
                "\x1bd"
            } else {
                "\x1b[3;5~"
            }
            .as_bytes()
            .to_vec(),
        );
    }

    if ctrl {
        let control = match key {
            "space" | "@" => Some(0),
            "backspace" => Some(8),
            "[" => Some(27),
            "\\" => Some(28),
            "]" => Some(29),
            "^" => Some(30),
            "_" => Some(31),
            "?" => Some(127),
            _ => key
                .chars()
                .next()
                .filter(|character| character.is_ascii_alphabetic())
                .map(|character| character.to_ascii_lowercase() as u8 - b'a' + 1),
        }?;
        return Some(if alt {
            vec![27, control]
        } else {
            vec![control]
        });
    }

    if shift && key == "enter" {
        return Some(b"\n".to_vec());
    }
    if key == "backspace" && alt {
        return Some(b"\x1b\x7f".to_vec());
    }

    if let Some(modifier_code) =
        (!(!shift && !alt)).then(|| terminal_modifier_code(shift, alt))
    {
        let sequence = match key {
            "up" => Some(format!("\x1b[1;{modifier_code}A")),
            "down" => Some(format!("\x1b[1;{modifier_code}B")),
            "right" => Some(format!("\x1b[1;{modifier_code}C")),
            "left" => Some(format!("\x1b[1;{modifier_code}D")),
            "home" => Some(format!("\x1b[1;{modifier_code}H")),
            "end" => Some(format!("\x1b[1;{modifier_code}F")),
            "f1" => Some(format!("\x1b[1;{modifier_code}P")),
            "f2" => Some(format!("\x1b[1;{modifier_code}Q")),
            "f3" => Some(format!("\x1b[1;{modifier_code}R")),
            "f4" => Some(format!("\x1b[1;{modifier_code}S")),
            "f5" => Some(format!("\x1b[15;{modifier_code}~")),
            "f6" => Some(format!("\x1b[17;{modifier_code}~")),
            "f7" => Some(format!("\x1b[18;{modifier_code}~")),
            "f8" => Some(format!("\x1b[19;{modifier_code}~")),
            "f9" => Some(format!("\x1b[20;{modifier_code}~")),
            "f10" => Some(format!("\x1b[21;{modifier_code}~")),
            "f11" => Some(format!("\x1b[23;{modifier_code}~")),
            "f12" => Some(format!("\x1b[24;{modifier_code}~")),
            _ => None,
        }?;
        return Some(sequence.into_bytes());
    }

    let sequence = match key {
        "enter" => Some("\r"),
        "backspace" => Some("\x7f"),
        "tab" if shift => Some("\x1b[Z"),
        "tab" => Some("\t"),
        "escape" => Some("\x1b"),
        "up" if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOA"),
        "up" => Some("\x1b[A"),
        "down" if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOB"),
        "down" => Some("\x1b[B"),
        "right" if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOC"),
        "right" => Some("\x1b[C"),
        "left" if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOD"),
        "left" => Some("\x1b[D"),
        "home" if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOH"),
        "home" => Some("\x1b[H"),
        "end" if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOF"),
        "end" => Some("\x1b[F"),
        "delete" => Some("\x1b[3~"),
        "insert" => Some("\x1b[2~"),
        "pageup" => Some("\x1b[5~"),
        "pagedown" => Some("\x1b[6~"),
        "f1" => Some("\x1bOP"),
        "f2" => Some("\x1bOQ"),
        "f3" => Some("\x1bOR"),
        "f4" => Some("\x1bOS"),
        "f5" => Some("\x1b[15~"),
        "f6" => Some("\x1b[17~"),
        "f7" => Some("\x1b[18~"),
        "f8" => Some("\x1b[19~"),
        "f9" => Some("\x1b[20~"),
        "f10" => Some("\x1b[21~"),
        "f11" => Some("\x1b[23~"),
        "f12" => Some("\x1b[24~"),
        "f13" => Some("\x1b[25~"),
        "f14" => Some("\x1b[26~"),
        "f15" => Some("\x1b[28~"),
        "f16" => Some("\x1b[29~"),
        "f17" => Some("\x1b[31~"),
        "f18" => Some("\x1b[32~"),
        "f19" => Some("\x1b[33~"),
        "f20" => Some("\x1b[34~"),
        _ => None,
    };
    if let Some(sequence) = sequence {
        return Some(sequence.as_bytes().to_vec());
    }

    let text = event
        .keystroke
        .key_char
        .as_deref()
        .or_else(|| (key.len() == 1).then_some(key))?;
    let mut bytes = Vec::with_capacity(text.len() + usize::from(alt));
    if alt {
        bytes.push(27);
    }
    bytes.extend_from_slice(text.as_bytes());
    Some(bytes)
}

fn terminal_mouse_mode(mode: TermMode, shift: bool) -> bool {
    mode.intersects(TermMode::MOUSE_MODE) && !shift
}

fn terminal_mouse_button_report(
    point: GridPoint,
    button: MouseButton,
    modifiers: Modifiers,
    pressed: bool,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let button = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::Navigate(_) => return None,
    };
    terminal_mouse_report(point, button, modifiers, pressed, mode)
}

fn terminal_mouse_move_report(
    point: GridPoint,
    button: Option<MouseButton>,
    modifiers: Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG) {
        return None;
    }
    let button = match button {
        Some(MouseButton::Left) => 32,
        Some(MouseButton::Middle) => 33,
        Some(MouseButton::Right) => 34,
        Some(MouseButton::Navigate(_)) => return None,
        None if mode.contains(TermMode::MOUSE_DRAG) => return None,
        None => 35,
    };
    terminal_mouse_report(point, button, modifiers, true, mode)
}

fn terminal_mouse_scroll_bytes(
    point: GridPoint,
    scroll_lines: i32,
    modifiers: Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if scroll_lines == 0 || !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let button = if scroll_lines > 0 { 64 } else { 65 };
    let mut reports = Vec::new();
    for _ in 0..scroll_lines.unsigned_abs() {
        reports.extend(terminal_mouse_report(point, button, modifiers, true, mode)?);
    }
    Some(reports)
}

fn terminal_mouse_report(
    point: GridPoint,
    button: u8,
    modifiers: Modifiers,
    pressed: bool,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if point.line.0 < 0 {
        return None;
    }
    let modifier_code = u8::from(modifiers.shift) * 4
        + u8::from(modifiers.alt) * 8
        + u8::from(modifiers.control) * 16;
    let button_with_modifiers = button + modifier_code;
    let row = point.line.0 as usize;
    let column = point.column.0;

    if mode.contains(TermMode::SGR_MOUSE) {
        let ending = if pressed { 'M' } else { 'm' };
        return Some(
            format!(
                "\x1b[<{button_with_modifiers};{};{}{ending}",
                column + 1,
                row + 1
            )
            .into_bytes(),
        );
    }

    let utf8 = mode.contains(TermMode::UTF8_MOUSE);
    let max_coordinate = if utf8 { 2015 } else { 223 };
    if row >= max_coordinate || column >= max_coordinate {
        return None;
    }
    let legacy_button = if pressed {
        button_with_modifiers
    } else {
        3 + modifier_code
    };
    let mut report = vec![0x1b, b'[', b'M', 32 + legacy_button];
    append_legacy_mouse_coordinate(column, utf8, &mut report);
    append_legacy_mouse_coordinate(row, utf8, &mut report);
    Some(report)
}

fn append_legacy_mouse_coordinate(
    coordinate: usize,
    utf8: bool,
    report: &mut Vec<u8>,
) {
    if utf8 && coordinate >= 95 {
        let coordinate = 33 + coordinate;
        report.push((0xc0 + coordinate / 64) as u8);
        report.push((0x80 + (coordinate & 63)) as u8);
    } else {
        report.push((33 + coordinate) as u8);
    }
}

fn terminal_alternate_scroll_bytes(scroll_lines: i32) -> Vec<u8> {
    let sequence: &[u8] = if scroll_lines > 0 {
        b"\x1bOA"
    } else {
        b"\x1bOB"
    };
    let count = scroll_lines.unsigned_abs() as usize;
    let mut bytes = Vec::with_capacity(count.saturating_mul(sequence.len()));
    for _ in 0..count {
        bytes.extend_from_slice(sequence);
    }
    bytes
}

fn terminal_clipboard_action(
    key: &str,
    modifiers: Modifiers,
    has_selection: bool,
) -> Option<TerminalClipboardAction> {
    if cfg!(target_os = "macos") {
        if key == "v"
            && modifiers.control
            && modifiers.platform
            && !modifiers.alt
            && !modifiers.shift
        {
            return Some(TerminalClipboardAction::PasteText);
        }
        if modifiers.platform
            && !modifiers.control
            && !modifiers.alt
            && !modifiers.shift
        {
            return match key {
                "c" => Some(TerminalClipboardAction::Copy),
                "v" => Some(TerminalClipboardAction::Paste),
                _ => None,
            };
        }
        return None;
    }

    if cfg!(target_os = "windows")
        && key == "v"
        && modifiers.control
        && modifiers.alt
        && !modifiers.shift
    {
        return Some(TerminalClipboardAction::PasteText);
    }
    if modifiers.alt {
        return None;
    }

    if key == "c" && modifiers.control && modifiers.shift {
        return Some(TerminalClipboardAction::Copy);
    }
    if key == "v" && modifiers.control && modifiers.shift {
        return Some(TerminalClipboardAction::Paste);
    }
    if key == "insert" && modifiers.control && !modifiers.shift {
        return Some(TerminalClipboardAction::Copy);
    }
    if key == "insert" && modifiers.shift && !modifiers.control {
        return Some(TerminalClipboardAction::Paste);
    }
    if cfg!(target_os = "windows")
        && key == "v"
        && modifiers.control
        && !modifiers.shift
    {
        return Some(TerminalClipboardAction::Paste);
    }
    if cfg!(target_os = "windows")
        && key == "c"
        && modifiers.control
        && !modifiers.shift
        && has_selection
    {
        return Some(TerminalClipboardAction::Copy);
    }

    None
}

fn terminal_select_all_action(key: &str, modifiers: Modifiers) -> bool {
    if key != "a" || modifiers.alt {
        return false;
    }

    if cfg!(target_os = "macos") {
        modifiers.platform && !modifiers.control && !modifiers.shift
    } else {
        modifiers.control && modifiers.shift
    }
}

fn terminal_scroll_action(
    key: &str,
    modifiers: Modifiers,
    mode: TermMode,
) -> Option<Scroll> {
    if mode.contains(TermMode::ALT_SCREEN) || modifiers.control || modifiers.alt {
        return None;
    }

    if cfg!(target_os = "macos") && modifiers.platform {
        if modifiers.shift {
            return None;
        }
        return match key {
            "up" => Some(Scroll::PageUp),
            "down" => Some(Scroll::PageDown),
            "home" => Some(Scroll::Top),
            "end" => Some(Scroll::Bottom),
            _ => None,
        };
    }

    if modifiers.platform || !modifiers.shift {
        return None;
    }
    match key {
        "up" => Some(Scroll::Delta(1)),
        "down" => Some(Scroll::Delta(-1)),
        "pageup" => Some(Scroll::PageUp),
        "pagedown" => Some(Scroll::PageDown),
        "home" => Some(Scroll::Top),
        "end" => Some(Scroll::Bottom),
        _ => None,
    }
}

fn should_defer_character_input(
    prefer_character_input: bool,
    has_key_char: bool,
) -> bool {
    prefer_character_input && has_key_char
}

fn terminal_paths_to_paste(paths: &[PathBuf]) -> Option<String> {
    let mut text = String::new();
    for path in paths {
        let Some(path) = path.to_str() else {
            continue;
        };
        let Ok(quoted_path) = shlex::try_quote(path) else {
            continue;
        };
        text.push(' ');
        text.push_str(quoted_path.as_ref());
    }
    if text.is_empty() {
        return None;
    }
    text.push(' ');
    Some(text)
}

fn terminal_modifier_code(shift: bool, alt: bool) -> u8 {
    1 + u8::from(shift) + (u8::from(alt) * 2)
}

impl BasePanel for TerminalPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_terminal"
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

impl Panel for TerminalPanel {
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.terminal_title();
        let rename_input = self.rename_input.clone();
        let is_renaming = rename_input.is_some();
        let group = self.tab_group.clone();
        let panel_id = self.panel_id;
        let close_handler = self.close_handler.clone();
        h_flex()
            .items_center()
            .gap_1()
            .when_some(rename_input, |row, input| {
                row.child(
                    h_flex()
                        .items_center()
                        .gap_1()
                        .on_key_down(cx.listener(Self::handle_terminal_rename_key))
                        .child(
                            Input::new(&input).aria_label("Terminal name").flex_1(),
                        )
                        .child(
                            Button::new(format!(
                                "save_terminal_title_{}",
                                panel_id.as_u64()
                            ))
                            .icon(IconName::Check)
                            .ghost()
                            .tooltip("Save terminal name (Enter)")
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.finish_terminal_rename(true, window, cx);
                                },
                            )),
                        )
                        .child(
                            Button::new(format!(
                                "cancel_terminal_title_{}",
                                panel_id.as_u64()
                            ))
                            .icon(IconName::X)
                            .ghost()
                            .tooltip("Cancel renaming (Esc)")
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.finish_terminal_rename(false, window, cx);
                                },
                            )),
                        ),
                )
            })
            .when(!is_renaming, |row| row.child(title))
            .when(!is_renaming, |row| {
                row.child(
                    Button::new("close_terminal_panel")
                        .icon(IconName::X)
                        .ghost()
                        .tooltip("Close Terminal")
                        .on_click(move |_, window, cx| {
                            if let Some(handler) = close_handler.as_ref() {
                                handler(window, cx);
                            } else if let Some(group) = group.as_ref() {
                                let is_last = group
                                    .read_with(cx, |group, _| {
                                        group.panels().len() == 1
                                    })
                                    .unwrap_or(false);
                                if is_last {
                                    return;
                                } else {
                                    _ = group.update(cx, |group, cx| {
                                        group.close_panel(panel_id, cx);
                                    });
                                }
                            }
                        }),
                )
            })
    }
    fn toolbar_buttons(
        &mut self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        let handler = self.new_terminal_handler.clone();
        Some(vec![
            Button::new(SharedString::from(format!(
                "new_terminal_{}",
                self.panel_id.as_u64()
            )))
            .icon(IconName::Plus)
            .tooltip("New Terminal")
            .on_click(move |_, window, cx| {
                if let Some(handler) = handler.as_ref() {
                    handler(window, cx);
                }
            }),
        ])
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
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let term_bg = cx
            .theme()
            .highlight_theme
            .style
            .editor_background
            .unwrap_or_else(|| cx.theme().input_background());
        let term_fg = cx
            .theme()
            .highlight_theme
            .style
            .editor_foreground
            .unwrap_or(cx.theme().foreground);
        let mono_font = cx.theme().mono_font_family.clone();
        let mono_size = cx.theme().mono_font_size;
        let palette = TerminalPalette {
            background: term_bg,
            foreground: term_fg,
            black: cx.theme().background,
            red: cx.theme().red,
            green: cx.theme().green,
            yellow: cx.theme().yellow,
            blue: cx.theme().blue,
            magenta: cx.theme().magenta,
            cyan: cx.theme().cyan,
            white: term_fg,
            bright_black: cx.theme().muted_foreground,
            bright_red: cx.theme().red_light,
            bright_green: cx.theme().green_light,
            bright_yellow: cx.theme().yellow_light,
            bright_blue: cx.theme().blue_light,
            bright_magenta: cx.theme().magenta_light,
            bright_cyan: cx.theme().cyan_light,
            bright_white: term_fg,
            selection: cx.theme().primary.opacity(0.42),
        };
        let snapshot = self.snapshot();
        let cursor = snapshot.cursor;
        let colors = snapshot.colors;
        let context_menu_enabled =
            !self.terminal_mode().intersects(TermMode::MOUSE_MODE);
        let terminal_panel = cx.entity();
        let terminal_panel_for_canvas = terminal_panel.clone();
        let focus = self.focus.clone();

        v_flex()
            .size_full()
            .bg(term_bg)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::copy_action))
            .on_action(cx.listener(Self::paste_action))
            .on_action(cx.listener(Self::select_all_action))
            .on_key_down(cx.listener(
                |this: &mut Self, event: &KeyDownEvent, _, cx| {
                    if this.handle_key(event, cx) {
                        cx.stop_propagation();
                    }
                },
            ))
            // Interactive Terminal Grid: captures keystrokes directly! No input box!
            .child(
                div()
                    .id("terminal-screen")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .bg(term_bg)
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, window, cx| {
                                terminal_panel_for_canvas.update(cx, |this, cx| {
                                    this.resize_to_bounds(bounds, mono_size, cx);
                                });
                                window.handle_input(
                                    &focus,
                                    ElementInputHandler::new(
                                        bounds,
                                        terminal_panel_for_canvas.clone(),
                                    ),
                                    cx,
                                );
                            },
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(
                            |this: &mut Self, event: &MouseDownEvent, window, cx| {
                                window.focus(&this.focus, cx);
                                if !this.start_terminal_mouse(
                                    event.button,
                                    event.position,
                                    event.modifiers,
                                    cx,
                                ) {
                                    this.start_selection(event, cx);
                                }
                            },
                        ),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(
                            |this: &mut Self, event: &MouseDownEvent, window, cx| {
                                if this.start_terminal_mouse(
                                    event.button,
                                    event.position,
                                    event.modifiers,
                                    cx,
                                ) {
                                    window.focus(&this.focus, cx);
                                }
                            },
                        ),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(
                            |this: &mut Self, event: &MouseDownEvent, window, cx| {
                                let mouse_reported = this.start_terminal_mouse(
                                    event.button,
                                    event.position,
                                    event.modifiers,
                                    cx,
                                );
                                window.focus(&this.focus, cx);
                                if !mouse_reported {
                                    #[cfg(any(
                                        target_os = "linux",
                                        target_os = "freebsd"
                                    ))]
                                    this.paste_primary_selection(cx);
                                }
                            },
                        ),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(
                            |this: &mut Self, event: &MouseUpEvent, _, cx| {
                                if !this.finish_terminal_mouse(
                                    event.button,
                                    event.position,
                                    event.modifiers,
                                    cx,
                                ) {
                                    this.finish_selection();
                                }
                            },
                        ),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(
                            |this: &mut Self, event: &MouseUpEvent, _, cx| {
                                if !this.finish_terminal_mouse(
                                    event.button,
                                    event.position,
                                    event.modifiers,
                                    cx,
                                ) {
                                    this.finish_selection();
                                }
                            },
                        ),
                    )
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(
                            |this: &mut Self, event: &MouseUpEvent, _, cx| {
                                this.finish_terminal_mouse(
                                    event.button,
                                    event.position,
                                    event.modifiers,
                                    cx,
                                );
                            },
                        ),
                    )
                    .on_mouse_up_out(
                        MouseButton::Right,
                        cx.listener(
                            |this: &mut Self, event: &MouseUpEvent, _, cx| {
                                this.finish_terminal_mouse(
                                    event.button,
                                    event.position,
                                    event.modifiers,
                                    cx,
                                );
                            },
                        ),
                    )
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(
                            |this: &mut Self, event: &MouseUpEvent, _, cx| {
                                this.finish_terminal_mouse(
                                    event.button,
                                    event.position,
                                    event.modifiers,
                                    cx,
                                );
                            },
                        ),
                    )
                    .on_mouse_up_out(
                        MouseButton::Middle,
                        cx.listener(
                            |this: &mut Self, event: &MouseUpEvent, _, cx| {
                                this.finish_terminal_mouse(
                                    event.button,
                                    event.position,
                                    event.modifiers,
                                    cx,
                                );
                            },
                        ),
                    )
                    .on_mouse_move(cx.listener(
                        |this: &mut Self, event: &MouseMoveEvent, _, cx| {
                            this.update_selection(event, cx);
                        },
                    ))
                    .on_scroll_wheel(cx.listener(
                        move |this, event: &ScrollWheelEvent, _, cx| {
                            if event.touch_phase == TouchPhase::Started {
                                this.scroll_remainder = 0.0;
                            }
                            let line_height = mono_size * 1.5;
                            this.scroll_remainder +=
                                event.delta.pixel_delta(line_height).y / line_height;
                            let delta = this.scroll_remainder.trunc() as i32;
                            this.scroll_remainder -= delta as f32;
                            if delta != 0 {
                                let mode = this.terminal_mode();
                                if terminal_mouse_mode(mode, event.modifiers.shift) {
                                    let report = this.active_terminal().and_then(
                                        |terminal| {
                                            let display_offset =
                                                terminal.display_offset();
                                            let (point, _) = this.grid_point_at(
                                                event.position,
                                                display_offset,
                                            )?;
                                            terminal_mouse_scroll_bytes(
                                                point,
                                                delta,
                                                event.modifiers,
                                                mode,
                                            )
                                        },
                                    );
                                    if let Some(report) = report {
                                        this.send_bytes(report);
                                    }
                                } else if mode.contains(
                                    TermMode::ALT_SCREEN
                                        | TermMode::ALTERNATE_SCROLL,
                                ) && !event.modifiers.shift
                                {
                                    this.send_bytes(
                                        terminal_alternate_scroll_bytes(delta),
                                    );
                                } else {
                                    this.scroll_display(Scroll::Delta(delta));
                                }
                                cx.notify();
                            }
                        },
                    ))
                    .context_menu(move |menu, _, _| {
                        if !context_menu_enabled {
                            return menu;
                        }

                        let rename_panel = terminal_panel.clone();
                        let copy_panel = terminal_panel.clone();
                        let select_all_panel = terminal_panel.clone();
                        let paste_panel = terminal_panel.clone();
                        let paste_text_panel = terminal_panel.clone();
                        menu.item(PopupMenuItem::new("Rename Terminal").on_click(
                            move |_, window, cx| {
                                rename_panel.update(cx, |this, cx| {
                                    this.begin_terminal_rename(window, cx);
                                });
                            },
                        ))
                        .item(PopupMenuItem::new("Copy").on_click(
                            move |_, window, cx| {
                                copy_panel.update(cx, |this, cx| {
                                    window.focus(&this.focus, cx);
                                    let selected_text = this
                                        .active_terminal()
                                        .and_then(TerminalBackend::selected_text);
                                    this.copy_selection_to_clipboard(
                                        selected_text,
                                        cx,
                                    );
                                    cx.notify();
                                });
                            },
                        ))
                        .item(PopupMenuItem::new("Select All").on_click(
                            move |_, window, cx| {
                                select_all_panel.update(cx, |this, cx| {
                                    window.focus(&this.focus, cx);
                                    this.select_all(cx);
                                });
                            },
                        ))
                        .item(PopupMenuItem::new("Paste").on_click(
                            move |_, window, cx| {
                                paste_panel.update(cx, |this, cx| {
                                    window.focus(&this.focus, cx);
                                    this.paste_from_clipboard(cx);
                                    cx.notify();
                                });
                            },
                        ))
                        .item(
                            PopupMenuItem::new("Paste Text").on_click(
                                move |_, window, cx| {
                                    paste_text_panel.update(cx, |this, cx| {
                                        window.focus(&this.focus, cx);
                                        this.paste_text_from_clipboard(cx);
                                        cx.notify();
                                    });
                                },
                            ),
                        )
                    })
                    .children(snapshot.rows.into_iter().map(move |line| {
                        let mut row = h_flex().flex_nowrap().whitespace_nowrap();
                        for run in terminal_runs(&line, cursor) {
                            let mut foreground = terminal_color(
                                run.style.foreground,
                                &colors,
                                palette,
                            );
                            let mut background = terminal_color(
                                run.style.background,
                                &colors,
                                palette,
                            );
                            if run.style.flags.contains(Flags::INVERSE)
                                || run.style.cursor
                            {
                                std::mem::swap(&mut foreground, &mut background);
                            }
                            if run.style.flags.contains(Flags::DIM) {
                                foreground = foreground.opacity(0.7);
                            }
                            if run.style.selected {
                                foreground = palette.foreground;
                                background = palette.selection;
                            }
                            let mut cell = div()
                                .text_size(mono_size)
                                .text_color(foreground)
                                .font_family(mono_font.clone())
                                .line_height(relative(1.5))
                                .whitespace_nowrap()
                                .bg(background)
                                .child(run.text);
                            if run.style.flags.contains(Flags::BOLD) {
                                cell = cell.font_weight(FontWeight::BOLD);
                            }
                            if run.style.flags.contains(Flags::ITALIC) {
                                cell = cell.italic();
                            }
                            if run.style.flags.intersects(Flags::ALL_UNDERLINES) {
                                cell = cell.underline();
                            }
                            row = row.child(cell);
                        }
                        row
                    })),
            )
    }
}

fn terminal_runs(
    line: &[TerminalCell],
    cursor: Option<(i32, usize, CursorShape)>,
) -> Vec<TerminalRun> {
    let visible_flags = Flags::BOLD
        | Flags::ITALIC
        | Flags::ALL_UNDERLINES
        | Flags::DIM
        | Flags::INVERSE
        | Flags::HIDDEN;
    let mut runs: Vec<TerminalRun> = Vec::new();
    for cell in line {
        let style = TerminalRunStyle {
            foreground: cell.foreground,
            background: cell.background,
            flags: cell.flags & visible_flags,
            cursor: cursor.is_some_and(|(line, column, _)| {
                cell.line == line && cell.column == column
            }),
            selected: cell.selected,
        };
        let character = if style.flags.contains(Flags::HIDDEN)
            || cell.flags.contains(Flags::WIDE_CHAR_SPACER)
        {
            ' '
        } else {
            cell.character
        };
        if let Some(last) = runs.last_mut()
            && last.style == style
        {
            last.text.push(character);
        } else {
            runs.push(TerminalRun {
                text: character.to_string(),
                style,
            });
        }
    }
    runs
}

fn terminal_color(
    color: TerminalColor,
    colors: &Colors,
    palette: TerminalPalette,
) -> Hsla {
    match color {
        TerminalColor::Named(name) => match name {
            NamedColor::Black => palette.black,
            NamedColor::Red => palette.red,
            NamedColor::Green => palette.green,
            NamedColor::Yellow => palette.yellow,
            NamedColor::Blue => palette.blue,
            NamedColor::Magenta => palette.magenta,
            NamedColor::Cyan => palette.cyan,
            NamedColor::White => palette.white,
            NamedColor::BrightBlack => palette.bright_black,
            NamedColor::BrightRed => palette.bright_red,
            NamedColor::BrightGreen => palette.bright_green,
            NamedColor::BrightYellow => palette.bright_yellow,
            NamedColor::BrightBlue => palette.bright_blue,
            NamedColor::BrightMagenta => palette.bright_magenta,
            NamedColor::BrightCyan => palette.bright_cyan,
            NamedColor::BrightWhite => palette.bright_white,
            NamedColor::Foreground | NamedColor::BrightForeground => {
                palette.foreground
            }
            NamedColor::Background => palette.background,
            NamedColor::Cursor => palette.foreground,
            NamedColor::DimBlack => palette.black,
            NamedColor::DimRed => palette.red,
            NamedColor::DimGreen => palette.green,
            NamedColor::DimYellow => palette.yellow,
            NamedColor::DimBlue => palette.blue,
            NamedColor::DimMagenta => palette.magenta,
            NamedColor::DimCyan => palette.cyan,
            NamedColor::DimWhite => palette.white,
            NamedColor::DimForeground => palette.foreground,
        },
        TerminalColor::Spec(rgb) => rgb_to_hsla(rgb),
        TerminalColor::Indexed(index) => colors[index as usize]
            .map(rgb_to_hsla)
            .unwrap_or_else(|| rgb_to_hsla(indexed_rgb(index))),
    }
}

fn rgb_to_hsla(rgb: Rgb) -> Hsla {
    Rgba {
        r: f32::from(rgb.r) / 255.,
        g: f32::from(rgb.g) / 255.,
        b: f32::from(rgb.b) / 255.,
        a: 1.,
    }
    .into()
}

fn indexed_rgb(index: u8) -> Rgb {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match index {
        0 => Rgb { r: 0, g: 0, b: 0 },
        1 => Rgb { r: 205, g: 0, b: 0 },
        2 => Rgb { r: 0, g: 205, b: 0 },
        3 => Rgb {
            r: 205,
            g: 205,
            b: 0,
        },
        4 => Rgb { r: 0, g: 0, b: 238 },
        5 => Rgb {
            r: 205,
            g: 0,
            b: 205,
        },
        6 => Rgb {
            r: 0,
            g: 205,
            b: 205,
        },
        7 => Rgb {
            r: 229,
            g: 229,
            b: 229,
        },
        8 => Rgb {
            r: 127,
            g: 127,
            b: 127,
        },
        9 => Rgb { r: 255, g: 0, b: 0 },
        10 => Rgb { r: 0, g: 255, b: 0 },
        11 => Rgb {
            r: 255,
            g: 255,
            b: 0,
        },
        12 => Rgb {
            r: 92,
            g: 92,
            b: 255,
        },
        13 => Rgb {
            r: 255,
            g: 0,
            b: 255,
        },
        14 => Rgb {
            r: 0,
            g: 255,
            b: 255,
        },
        15 => Rgb {
            r: 255,
            g: 255,
            b: 255,
        },
        16..=231 => {
            let index = index - 16;
            let red = LEVELS[(index / 36) as usize];
            let green = LEVELS[((index / 6) % 6) as usize];
            let blue = LEVELS[(index % 6) as usize];
            Rgb {
                r: red,
                g: green,
                b: blue,
            }
        }
        232..=255 => {
            let value = 8 + (index - 232) * 10;
            Rgb {
                r: value,
                g: value,
                b: value,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::Processor;

    fn terminal_key_bytes_for(shortcut: &str) -> Option<Vec<u8>> {
        let event = KeyDownEvent {
            keystroke: Keystroke::parse(shortcut).expect("terminal shortcut"),
            is_held: false,
            prefer_character_input: false,
        };
        terminal_key_bytes(&event, TermMode::empty())
    }

    fn terminal_scroll_action_for(shortcut: &str, mode: TermMode) -> Option<Scroll> {
        let keystroke = Keystroke::parse(shortcut).expect("terminal shortcut");
        terminal_scroll_action(keystroke.key.as_str(), keystroke.modifiers, mode)
    }

    #[::core::prelude::v1::test]
    fn custom_terminal_title_overrides_and_clears_to_dynamic_title() {
        let mut tab = TerminalTab {
            id: 4,
            terminal: None,
            shell_name: "zsh".to_string(),
            custom_title: None,
            status: "ready".into(),
        };
        assert_eq!(tab.title(), "zsh 4");

        tab.set_custom_title("  Build  ");
        assert_eq!(tab.title(), "Build");

        tab.shell_name = "project".to_string();
        assert_eq!(tab.title(), "Build");
        tab.set_custom_title("project 4");
        assert_eq!(tab.title(), "project 4");

        tab.set_custom_title("  ");
        assert_eq!(tab.title(), "project 4");
        assert!(tab.custom_title.is_none());
    }

    #[::core::prelude::v1::test]
    fn vte_output_updates_cells_and_cursor() {
        let mut terminal =
            Term::new(Config::default(), &TermSize::new(20, 4), VoidListener);
        let mut parser =
            Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new();
        for byte in b"\x1b[31mred\x1b[0m" {
            parser.advance(&mut terminal, *byte);
        }

        let mut content = terminal.renderable_content();
        let first = content.display_iter.next().expect("first terminal cell");
        assert_eq!(first.cell.c, 'r');
        assert_eq!(first.cell.fg, TerminalColor::Named(NamedColor::Red));
        assert_eq!(content.cursor.point.column.0, 3);
    }

    #[::core::prelude::v1::test]
    fn terminal_mouse_tracking_keeps_shift_available_for_selection() {
        assert!(terminal_mouse_mode(TermMode::MOUSE_REPORT_CLICK, false));
        assert!(!terminal_mouse_mode(TermMode::MOUSE_REPORT_CLICK, true));
        assert!(!terminal_mouse_mode(TermMode::empty(), false));
    }

    #[::core::prelude::v1::test]
    fn terminal_mouse_reports_sgr_clicks_and_modifiers() {
        let point = GridPoint::new(Line(2), Column(3));
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let modifiers = Modifiers {
            control: true,
            shift: true,
            ..Modifiers::default()
        };

        assert_eq!(
            terminal_mouse_button_report(
                point,
                MouseButton::Left,
                modifiers,
                true,
                mode,
            ),
            Some(b"\x1b[<20;4;3M".to_vec())
        );
        assert_eq!(
            terminal_mouse_button_report(
                point,
                MouseButton::Left,
                modifiers,
                false,
                mode,
            ),
            Some(b"\x1b[<20;4;3m".to_vec())
        );
    }

    #[::core::prelude::v1::test]
    fn terminal_mouse_reports_legacy_and_utf8_coordinates() {
        let origin = GridPoint::new(Line(0), Column(0));
        assert_eq!(
            terminal_mouse_button_report(
                origin,
                MouseButton::Left,
                Modifiers::default(),
                true,
                TermMode::MOUSE_REPORT_CLICK,
            ),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );

        let wide_column = GridPoint::new(Line(0), Column(100));
        assert_eq!(
            terminal_mouse_button_report(
                wide_column,
                MouseButton::Left,
                Modifiers::default(),
                true,
                TermMode::MOUSE_REPORT_CLICK | TermMode::UTF8_MOUSE,
            ),
            Some(vec![0x1b, b'[', b'M', 32, 194, 133, 33])
        );
        assert!(
            terminal_mouse_button_report(
                GridPoint::new(Line(0), Column(223)),
                MouseButton::Left,
                Modifiers::default(),
                true,
                TermMode::MOUSE_REPORT_CLICK,
            )
            .is_none()
        );
    }

    #[::core::prelude::v1::test]
    fn terminal_mouse_motion_respects_click_drag_and_motion_modes() {
        let point = GridPoint::new(Line(0), Column(0));
        assert!(
            terminal_mouse_move_report(
                point,
                None,
                Modifiers::default(),
                TermMode::MOUSE_DRAG,
            )
            .is_none()
        );
        assert_eq!(
            terminal_mouse_move_report(
                point,
                Some(MouseButton::Left),
                Modifiers::default(),
                TermMode::MOUSE_DRAG,
            ),
            Some(vec![0x1b, b'[', b'M', 64, 33, 33])
        );
        assert_eq!(
            terminal_mouse_move_report(
                point,
                None,
                Modifiers::default(),
                TermMode::MOUSE_MOTION,
            ),
            Some(vec![0x1b, b'[', b'M', 67, 33, 33])
        );
    }

    #[::core::prelude::v1::test]
    fn terminal_mouse_wheel_and_alternate_screen_scroll_match_zed() {
        let point = GridPoint::new(Line(0), Column(0));
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(
            terminal_mouse_scroll_bytes(point, 2, Modifiers::default(), mode,),
            Some(b"\x1b[<64;1;1M\x1b[<64;1;1M".to_vec())
        );
        assert_eq!(
            terminal_mouse_scroll_bytes(point, -1, Modifiers::default(), mode,),
            Some(b"\x1b[<65;1;1M".to_vec())
        );
        assert_eq!(terminal_alternate_scroll_bytes(2), b"\x1bOA\x1bOA".to_vec());
        assert_eq!(terminal_alternate_scroll_bytes(-1), b"\x1bOB".to_vec());
    }

    #[::core::prelude::v1::test]
    fn terminal_clipboard_shortcuts_match_zed_platform_defaults() {
        let control = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        let control_shift = Modifiers {
            control: true,
            shift: true,
            ..Modifiers::default()
        };
        let command = Modifiers {
            platform: true,
            ..Modifiers::default()
        };
        let control_command = Modifiers {
            control: true,
            platform: true,
            ..Modifiers::default()
        };
        let control_alt = Modifiers {
            control: true,
            alt: true,
            ..Modifiers::default()
        };

        if cfg!(target_os = "macos") {
            assert_eq!(
                terminal_clipboard_action("c", command, false),
                Some(TerminalClipboardAction::Copy)
            );
            assert_eq!(
                terminal_clipboard_action("v", command, false),
                Some(TerminalClipboardAction::Paste)
            );
            assert_eq!(
                terminal_clipboard_action("v", control_command, false),
                Some(TerminalClipboardAction::PasteText)
            );
            assert_eq!(terminal_clipboard_action("v", control, false), None);
            assert_eq!(terminal_clipboard_action("c", control, true), None);
        } else if cfg!(target_os = "windows") {
            assert_eq!(
                terminal_clipboard_action("c", control_shift, false),
                Some(TerminalClipboardAction::Copy)
            );
            assert_eq!(
                terminal_clipboard_action("v", control, false),
                Some(TerminalClipboardAction::Paste)
            );
            assert_eq!(
                terminal_clipboard_action("v", control_alt, false),
                Some(TerminalClipboardAction::PasteText)
            );
            assert_eq!(
                terminal_clipboard_action("c", control, true),
                Some(TerminalClipboardAction::Copy)
            );
            assert_eq!(terminal_clipboard_action("c", control, false), None);
        } else {
            assert_eq!(
                terminal_clipboard_action("c", control_shift, false),
                Some(TerminalClipboardAction::Copy)
            );
            assert_eq!(
                terminal_clipboard_action("v", control_shift, false),
                Some(TerminalClipboardAction::Paste)
            );
            assert_eq!(terminal_clipboard_action("v", control, false), None);
            assert_eq!(terminal_clipboard_action("c", control, false), None);
            assert_eq!(terminal_clipboard_action("c", control, true), None);
        }

        assert_eq!(
            terminal_clipboard_action(
                "insert",
                Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
                false,
            ),
            (!cfg!(target_os = "macos")).then_some(TerminalClipboardAction::Copy)
        );
        assert_eq!(
            terminal_clipboard_action(
                "insert",
                Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
                false,
            ),
            (!cfg!(target_os = "macos")).then_some(TerminalClipboardAction::Paste)
        );
    }

    #[::core::prelude::v1::test]
    fn printable_terminal_input_is_deferred_to_gpui_text_input() {
        assert!(should_defer_character_input(true, true));
        assert!(!should_defer_character_input(true, false));
        assert!(!should_defer_character_input(false, true));
    }

    #[::core::prelude::v1::test]
    fn select_all_shortcut_matches_zed_without_consuming_plain_control_a() {
        let primary = Modifiers {
            platform: true,
            ..Modifiers::default()
        };
        if cfg!(target_os = "macos") {
            assert!(terminal_select_all_action("a", primary));
        } else {
            assert!(!terminal_select_all_action("a", primary));
        }

        let alt_primary = Modifiers {
            alt: true,
            ..primary
        };
        assert!(!terminal_select_all_action("a", alt_primary));

        let control_shift = Modifiers {
            control: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            terminal_select_all_action("a", control_shift),
            !cfg!(target_os = "macos")
        );

        let control = Modifiers {
            control: true,
            platform: !cfg!(target_os = "macos"),
            ..Modifiers::default()
        };
        assert!(!terminal_select_all_action("a", control));
    }

    #[::core::prelude::v1::test]
    fn plain_control_a_is_forwarded_to_the_pty() {
        assert_eq!(terminal_key_bytes_for("ctrl-a"), Some(vec![1]));
    }

    #[::core::prelude::v1::test]
    fn zed_shell_editing_shortcuts_reach_the_terminal() {
        for (shortcut, expected) in [
            ("alt-b", b"\x1bb".as_slice()),
            ("alt-f", b"\x1bf".as_slice()),
            ("alt-left", b"\x1bb".as_slice()),
            ("alt-right", b"\x1bf".as_slice()),
            ("alt-.", b"\x1b.".as_slice()),
            ("ctrl-backspace", b"\x17".as_slice()),
        ] {
            assert_eq!(terminal_key_bytes_for(shortcut).as_deref(), Some(expected));
        }

        let ctrl_delete = if cfg!(target_os = "windows") {
            b"\x1bd".as_slice()
        } else {
            b"\x1b[3;5~".as_slice()
        };
        assert_eq!(
            terminal_key_bytes_for("ctrl-delete").as_deref(),
            Some(ctrl_delete)
        );

        if cfg!(target_os = "macos") {
            for (shortcut, expected) in [
                ("cmd-left", b"\x01".as_slice()),
                ("cmd-right", b"\x05".as_slice()),
                ("cmd-backspace", b"\x15".as_slice()),
                ("cmd-delete", b"\x0b".as_slice()),
                ("alt-delete", b"\x1bd".as_slice()),
            ] {
                assert_eq!(
                    terminal_key_bytes_for(shortcut).as_deref(),
                    Some(expected)
                );
            }
        }
    }

    #[::core::prelude::v1::test]
    fn zed_terminal_scroll_shortcuts_match_the_active_platform() {
        assert!(matches!(
            terminal_scroll_action_for("shift-up", TermMode::empty()),
            Some(Scroll::Delta(1))
        ));
        assert!(matches!(
            terminal_scroll_action_for("shift-down", TermMode::empty()),
            Some(Scroll::Delta(-1))
        ));
        assert!(matches!(
            terminal_scroll_action_for("shift-pageup", TermMode::empty()),
            Some(Scroll::PageUp)
        ));
        assert!(matches!(
            terminal_scroll_action_for("shift-pagedown", TermMode::empty()),
            Some(Scroll::PageDown)
        ));
        assert!(matches!(
            terminal_scroll_action_for("shift-home", TermMode::empty()),
            Some(Scroll::Top)
        ));
        assert!(matches!(
            terminal_scroll_action_for("shift-end", TermMode::empty()),
            Some(Scroll::Bottom)
        ));

        if cfg!(target_os = "macos") {
            assert!(matches!(
                terminal_scroll_action_for("cmd-up", TermMode::empty()),
                Some(Scroll::PageUp)
            ));
            assert!(matches!(
                terminal_scroll_action_for("cmd-down", TermMode::empty()),
                Some(Scroll::PageDown)
            ));
            assert!(matches!(
                terminal_scroll_action_for("cmd-home", TermMode::empty()),
                Some(Scroll::Top)
            ));
            assert!(matches!(
                terminal_scroll_action_for("cmd-end", TermMode::empty()),
                Some(Scroll::Bottom)
            ));
        }

        assert!(
            terminal_scroll_action_for("ctrl-shift-up", TermMode::empty()).is_none()
        );
        assert!(
            terminal_scroll_action_for("shift-up", TermMode::ALT_SCREEN).is_none()
        );
    }

    #[::core::prelude::v1::test]
    fn terminal_path_paste_quotes_paths_for_shell_input() {
        let paths = [
            PathBuf::from("/tmp/ordinary.rs"),
            PathBuf::from("/tmp/name with spaces.rs"),
            PathBuf::from("/tmp/naïve's.rs"),
        ];
        let text = terminal_paths_to_paste(&paths).expect("quotable paths");
        assert_eq!(
            shlex::split(text.trim()).expect("quoted shell words"),
            paths
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        );
    }
}

#[cfg(all(test, unix))]
mod terminal_clipboard_tests {
    use super::{InputCopy, InputPaste, InputSelectAll, TermMode, TerminalPanel};
    use ahead_rpc::ahead::AgentTerminalAuthLaunch;
    use gpui_kit::{
        ClipboardEntry, ClipboardItem, Entity, ExternalPaths, Modifiers,
        MouseButton, TestAppContext, point, px,
    };
    use std::time::Duration;

    async fn wait_for_pty_io(cx: &mut TestAppContext) {
        cx.background_executor
            .timer(Duration::from_millis(10))
            .await;
        // The GPUI test clock does not wait for native PTY child processes.
        std::thread::sleep(Duration::from_millis(10));
    }

    async fn wait_for_terminal_text(
        panel: &Entity<TerminalPanel>,
        expected: &str,
        cx: &mut TestAppContext,
    ) -> bool {
        for _ in 0..300 {
            if panel.read_with(cx, |panel, _| {
                panel.active_terminal().is_some_and(|terminal| {
                    terminal.shared_history_text().contains(expected)
                })
            }) {
                return true;
            }
            wait_for_pty_io(cx).await;
        }
        false
    }

    async fn wait_for_shell_ready(
        panel: &Entity<TerminalPanel>,
        cx: &mut TestAppContext,
    ) {
        panel.update(cx, |panel, cx| {
            panel.run_command("printf '%s%s\\n' 'AHEAD_TEST_SHELL_' 'READY'", cx);
        });
        assert!(
            wait_for_terminal_text(panel, "AHEAD_TEST_SHELL_READY", cx).await,
            "shell should execute its startup marker"
        );
    }

    #[gpui_kit::test(iterations = 20)]
    async fn agent_terminal_auth_reports_the_child_exit_status(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("disposable auth workspace");
        let cwd = workspace.path().to_path_buf();
        let exit_status = std::sync::Arc::new(std::sync::Mutex::new(None));
        let callback_status = exit_status.clone();
        let (panel, cx) = cx.add_window_view(move |_, cx| {
            TerminalPanel::new_for_agent_auth(
                0,
                AgentTerminalAuthLaunch {
                    name: "Test agent sign-in".into(),
                    program: "/bin/sh".into(),
                    args: vec![
                        "-c".into(),
                        "printf '%s\\n' 'AHEAD_AGENT_AUTH_STARTED'; exit 0".into(),
                    ],
                    env: std::collections::HashMap::new(),
                    cwd,
                },
                move |status, _| {
                    *callback_status.lock().expect("exit status lock") =
                        Some(status);
                },
                cx,
            )
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            wait_for_terminal_text(&panel, "AHEAD_AGENT_AUTH_STARTED", cx).await,
            "agent sign-in process should start before its exit is checked"
        );

        for _ in 0..300 {
            if exit_status.lock().expect("exit status lock").is_some() {
                break;
            }
            wait_for_pty_io(cx).await;
        }
        let observed_exit_status = *exit_status.lock().expect("exit status lock");
        let terminal_state = panel.read_with(cx, |panel, _| {
            let terminal = panel.terminal.terminal.as_ref();
            (
                panel.terminal.status.to_string(),
                terminal.map(|terminal| terminal.is_running()),
                terminal.map(|terminal| terminal.events().is_empty()),
                terminal.and_then(|terminal| terminal.process_id()),
            )
        });
        assert_eq!(
            observed_exit_status,
            Some(Some(0)),
            "terminal state after waiting for agent auth exit: {terminal_state:?}"
        );
        if let Some(shutdown) = panel.update(cx, |panel, cx| panel.shutdown(cx)) {
            match shutdown.recv_timeout(Duration::from_secs(3)) {
                Ok(()) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    panic!("sign-in terminal process should stop")
                }
            }
        }
    }

    #[gpui_kit::test]
    async fn editor_clipboard_actions_round_trip_text_through_terminal(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("disposable terminal workspace");
        let cwd = workspace.path().to_string_lossy().into_owned();
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            let panel = TerminalPanel::new_with_shell(0, cwd, "/bin/sh", cx);
            window.focus(&panel.focus, cx);
            panel
        });

        let actions_available = cx.update(|window, cx| {
            let _ = window.draw(cx);
            (
                window.is_action_available(&InputCopy, cx),
                window.is_action_available(&InputPaste, cx),
                window.is_action_available(&InputSelectAll, cx),
            )
        });
        assert_eq!(actions_available, (true, true, true));

        wait_for_shell_ready(&panel, cx).await;
        let marker = "AHEAD_TERMINAL_UI_PASTE_OK";
        let completed_marker = "AHEAD_TERMINAL_UI_DONE";
        let sentinel = "AHEAD_TERMINAL_INPUT_DRAINED";
        let paste_count_path = workspace.path().join("paste-count");
        cx.write_to_clipboard(ClipboardItem::new_string(format!(
            "printf '%s%s%s\\n' 'AHEAD_TERMINAL_UI_' 'PASTE_' 'OK'; printf x >> paste-count; printf '%s%s\\n' 'AHEAD_TERMINAL_UI_' 'DONE'\n"
        )));
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-v"
        } else if cfg!(target_os = "windows") {
            "ctrl-v"
        } else {
            "ctrl-shift-v"
        });
        cx.run_until_parked();

        assert!(
            wait_for_terminal_text(&panel, completed_marker, cx).await,
            "pasted command should execute, not merely echo"
        );
        assert!(panel.read_with(cx, |panel, _| {
            panel.active_terminal().is_some_and(|terminal| {
                terminal.shared_history_text().contains(marker)
            })
        }));

        panel.update(cx, |panel, cx| {
            panel.run_command(
                "printf '%s%s%s\\n' 'AHEAD_TERMINAL_INPUT_' 'DRAINED' ''",
                cx,
            );
        });
        assert!(
            wait_for_terminal_text(&panel, sentinel, cx).await,
            "terminal should accept shell input after paste"
        );
        assert_eq!(
            std::fs::read_to_string(paste_count_path)
                .expect("pasted command should create its count file"),
            "x",
            "one paste should execute the command once"
        );

        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-a"
        } else {
            "ctrl-shift-a"
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-shift-c"
        });
        cx.run_until_parked();
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("copy action should write selected terminal output");
        assert!(copied.contains(marker), "copied terminal text: {copied:?}");
        assert!(panel.read_with(cx, |panel, _| {
            panel
                .active_terminal()
                .is_some_and(|terminal| terminal.is_running())
        }));

        if let Some(shutdown) = panel.update(cx, |panel, cx| panel.shutdown(cx)) {
            match shutdown.recv_timeout(Duration::from_secs(3)) {
                Ok(()) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    panic!("terminal process should stop")
                }
            }
        }
    }

    #[gpui_kit::test]
    async fn edit_menu_actions_paste_and_copy_terminal_text(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("disposable terminal workspace");
        let cwd = workspace.path().to_string_lossy().into_owned();
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            let panel = TerminalPanel::new_with_shell(0, cwd, "/bin/sh", cx);
            window.focus(&panel.focus, cx);
            panel
        });

        wait_for_shell_ready(&panel, cx).await;
        let marker = "AHEAD_TERMINAL_EDIT_MENU_PASTE_OK";
        cx.write_to_clipboard(ClipboardItem::new_string(format!(
            "printf '%s\\n' '{marker}'\n"
        )));
        cx.update(|window, cx| {
            window.dispatch_action(Box::new(InputPaste), cx);
        });
        assert!(
            wait_for_terminal_text(&panel, marker, cx).await,
            "the Edit > Paste action should send clipboard text to the PTY"
        );

        panel.update(cx, |panel, cx| panel.select_all(cx));
        cx.update(|window, cx| {
            window.dispatch_action(Box::new(InputCopy), cx);
        });
        cx.run_until_parked();
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("Edit > Copy should copy selected terminal output");
        assert!(copied.contains(marker), "copied terminal text: {copied:?}");

        if let Some(shutdown) = panel.update(cx, |panel, cx| panel.shutdown(cx)) {
            match shutdown.recv_timeout(Duration::from_secs(3)) {
                Ok(()) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    panic!("terminal process should stop")
                }
            }
        }
    }

    #[gpui_kit::test]
    async fn pasted_file_path_is_shell_safe_in_the_integrated_terminal(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("disposable terminal workspace");
        let cwd = workspace.path().to_string_lossy().into_owned();
        let target_path = workspace
            .path()
            .join("file with spaces; touch injected; # and apostrophe's.txt");
        let content = "AHEAD_TERMINAL_PATH_CONTENT_OK";
        std::fs::write(&target_path, content).expect("write path paste fixture");
        let injected_path = workspace.path().join("injected");
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            let panel = TerminalPanel::new_with_shell(0, cwd, "/bin/sh", cx);
            window.focus(&panel.focus, cx);
            panel
        });

        wait_for_shell_ready(&panel, cx).await;
        cx.write_to_clipboard(ClipboardItem {
            entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(
                vec![target_path.clone()].into(),
            ))],
        });
        panel.update(cx, |panel, _| {
            panel
                .active_terminal()
                .expect("terminal should be active")
                .send(b"cat ".to_vec())
                .expect("command prefix should reach the shell");
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-v"
        } else if cfg!(target_os = "windows") {
            "ctrl-v"
        } else {
            "ctrl-shift-v"
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();

        let mut content_seen = false;
        for _ in 0..300 {
            content_seen = panel.read_with(cx, |panel, _| {
                panel.active_terminal().is_some_and(|terminal| {
                    terminal.shared_history_text().contains(content)
                })
            });
            if content_seen {
                break;
            }
            wait_for_pty_io(cx).await;
        }
        assert!(content_seen, "pasted path should resolve to its file");
        assert!(
            !injected_path.exists(),
            "shell syntax in a filename must not execute"
        );

        if let Some(shutdown) = panel.update(cx, |panel, cx| panel.shutdown(cx)) {
            match shutdown.recv_timeout(Duration::from_secs(3)) {
                Ok(()) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    panic!("terminal process should stop")
                }
            }
        }
    }

    #[gpui_kit::test]
    async fn shell_osc52_copy_reaches_the_gpui_clipboard(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("disposable terminal workspace");
        let cwd = workspace.path().to_string_lossy().into_owned();
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            let panel = TerminalPanel::new_with_shell(0, cwd, "/bin/sh", cx);
            window.focus(&panel.focus, cx);
            panel
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        wait_for_shell_ready(&panel, cx).await;
        panel.update(cx, |panel, cx| {
            panel.run_command(
                "printf '\\033]52;c;Y29weQ==\\007'; printf '%s%s\\n' 'AHEAD_TEST_OSC52_' 'DONE'",
                cx,
            );
        });
        assert!(
            wait_for_terminal_text(&panel, "AHEAD_TEST_OSC52_DONE", cx).await,
            "shell should execute the OSC 52 command"
        );
        let mut clipboard_text = None;
        for _ in 0..300 {
            clipboard_text = cx.read_from_clipboard().and_then(|item| item.text());
            if clipboard_text.as_deref() == Some("copy") {
                break;
            }
            wait_for_pty_io(cx).await;
        }
        assert_eq!(clipboard_text.as_deref(), Some("copy"));

        if let Some(shutdown) = panel.update(cx, |panel, cx| panel.shutdown(cx)) {
            match shutdown.recv_timeout(Duration::from_secs(3)) {
                Ok(()) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    panic!("terminal process should stop")
                }
            }
        }
    }

    #[gpui_kit::test]
    async fn control_c_interrupts_the_foreground_pty_process(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("disposable terminal workspace");
        let cwd = workspace.path().to_string_lossy().into_owned();
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            let panel = TerminalPanel::new_with_shell(0, cwd, "/bin/sh", cx);
            window.focus(&panel.focus, cx);
            panel
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        wait_for_shell_ready(&panel, cx).await;
        let started_path = workspace.path().join("ctrl-c-started");
        let exit_path = workspace.path().join("ctrl-c-exit");
        let started_argument =
            super::terminal_paths_to_paste(std::slice::from_ref(&started_path))
                .expect("quote started marker path");
        let exit_argument =
            super::terminal_paths_to_paste(std::slice::from_ref(&exit_path))
                .expect("quote exit marker path");
        panel.update(cx, |panel, cx| {
            panel.run_command(
                &format!(
                    "touch {started_argument}; sleep 30; printf '%s' \"$?\" > {exit_argument}"
                ),
                cx,
            );
        });

        let mut child_started = false;
        for _ in 0..1_000 {
            child_started = started_path.exists();
            if child_started {
                break;
            }
            wait_for_pty_io(cx).await;
        }
        let terminal_output = panel.read_with(cx, |panel, _| {
            panel
                .active_terminal()
                .map(|terminal| terminal.shared_history_text())
                .unwrap_or_default()
        });
        assert!(
            child_started,
            "foreground sleep should start within 10 seconds; terminal output:\n{terminal_output}"
        );

        cx.simulate_keystrokes("ctrl-c");
        let mut interrupted = false;
        for _ in 0..300 {
            interrupted = std::fs::read_to_string(&exit_path)
                .is_ok_and(|status| status == "130");
            if interrupted {
                break;
            }
            wait_for_pty_io(cx).await;
        }
        assert!(
            interrupted,
            "Ctrl-C should interrupt the foreground process"
        );

        if let Some(shutdown) = panel.update(cx, |panel, cx| panel.shutdown(cx)) {
            match shutdown.recv_timeout(Duration::from_secs(3)) {
                Ok(()) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    panic!("terminal process should stop")
                }
            }
        }
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[gpui_kit::test]
    async fn terminal_primary_selection_copy_and_middle_click_paste(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("disposable terminal workspace");
        let cwd = workspace.path().to_string_lossy().into_owned();
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            let panel = TerminalPanel::new_with_shell(0, cwd, "/bin/sh", cx);
            window.focus(&panel.focus, cx);
            panel
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        wait_for_shell_ready(&panel, cx).await;
        let marker = "AHEAD_TERMINAL_PRIMARY_SELECTION_OK";
        let command =
            "printf '%s%s%s\\n' 'AHEAD_TERMINAL_PRIMARY_' 'SELECTION_' 'OK'\n";
        cx.update(|_, cx| cx.write_to_primary(ClipboardItem::new_string(command)));
        let mouse_position = point(px(80.), px(80.));
        cx.simulate_mouse_down(
            mouse_position,
            MouseButton::Middle,
            Modifiers::default(),
        );
        cx.simulate_mouse_up(
            mouse_position,
            MouseButton::Middle,
            Modifiers::default(),
        );

        let mut output_seen = false;
        for _ in 0..300 {
            output_seen = panel.read_with(cx, |panel, _| {
                panel.active_terminal().is_some_and(|terminal| {
                    terminal.shared_history_text().contains(marker)
                })
            });
            if output_seen {
                break;
            }
            wait_for_pty_io(cx).await;
        }
        assert!(
            output_seen,
            "middle-click should paste the primary selection"
        );

        panel.update(cx, |panel, cx| panel.select_all(cx));
        let selected_text = cx.update(|_, cx| {
            cx.read_from_primary()
                .and_then(|item| item.text())
                .unwrap_or_default()
        });
        assert!(
            selected_text.contains(marker),
            "terminal selection should update the primary selection"
        );

        if let Some(shutdown) = panel.update(cx, |panel, cx| panel.shutdown(cx)) {
            match shutdown.recv_timeout(Duration::from_secs(3)) {
                Ok(()) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    panic!("terminal process should stop")
                }
            }
        }
    }

    #[gpui_kit::test]
    async fn terminal_mouse_reports_reach_an_interactive_shell(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let workspace = tempfile::tempdir().expect("disposable terminal workspace");
        let cwd = workspace.path().to_string_lossy().into_owned();
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            let panel = TerminalPanel::new_with_shell(0, cwd, "/bin/sh", cx);
            window.focus(&panel.focus, cx);
            panel
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        wait_for_shell_ready(&panel, cx).await;
        panel.update(cx, |panel, cx| {
            panel.run_command(
                "stty raw -echo; printf '\\033[?1000h\\033[?1006h'; printf '%s%s\\n' 'AHEAD_MOUSE_' 'READY'; cat -v",
                cx,
            );
        });
        assert!(
            wait_for_terminal_text(&panel, "AHEAD_MOUSE_READY", cx).await,
            "shell should execute the mouse-report command"
        );
        let expected_modes = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let mut modes_ready = false;
        for _ in 0..300 {
            modes_ready = panel.read_with(cx, |panel, _| {
                panel
                    .active_terminal()
                    .is_some_and(|terminal| terminal.mode().contains(expected_modes))
            });
            if modes_ready {
                break;
            }
            wait_for_pty_io(cx).await;
        }
        if !modes_ready {
            let diagnostics = panel.read_with(cx, |panel, _| {
                panel.active_terminal().map(|terminal| {
                    format!(
                        "mode={:?}, output={:?}",
                        terminal.mode(),
                        terminal.shared_history_text()
                    )
                })
            });
            if let Some(shutdown) = panel.update(cx, |panel, cx| panel.shutdown(cx))
            {
                let _ = shutdown.recv_timeout(Duration::from_secs(3));
            }
            panic!("shell should enable SGR mouse reporting: {diagnostics:?}");
        }

        let mouse_position = point(px(80.), px(80.));
        let (line, column) = panel
            .read_with(cx, |panel, _| {
                let terminal = panel.active_terminal()?;
                let (point, _) = panel
                    .grid_point_at(mouse_position, terminal.display_offset())?;
                Some((point.line.0, point.column.0))
            })
            .expect("mouse position should map to a terminal cell");
        let press = format!("^[[<0;{};{}M", column + 1, line + 1);
        let release = format!("^[[<0;{};{}m", column + 1, line + 1);

        cx.simulate_mouse_down(
            mouse_position,
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_up(
            mouse_position,
            MouseButton::Left,
            Modifiers::default(),
        );

        let mut reports_seen = false;
        for _ in 0..300 {
            reports_seen = panel.read_with(cx, |panel, _| {
                panel.active_terminal().is_some_and(|terminal| {
                    let output = terminal.shared_history_text();
                    output.contains(&press) && output.contains(&release)
                })
            });
            if reports_seen {
                break;
            }
            wait_for_pty_io(cx).await;
        }
        assert!(reports_seen, "mouse press and release should reach the PTY");

        if let Some(shutdown) = panel.update(cx, |panel, cx| panel.shutdown(cx)) {
            match shutdown.recv_timeout(Duration::from_secs(3)) {
                Ok(()) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    panic!("terminal process should stop")
                }
            }
        }
    }
}
