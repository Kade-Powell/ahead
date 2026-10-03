//! AHEAD Interactive Terminal - Direct PTY Terminal Emulator
//!
//! True terminal experience matching Zed / VS Code:
//! - NO text input box: the terminal grid itself captures focus and keystrokes.
//! - Keystrokes are dispatched directly to the PTY stdin (Enter, Backspace, Tab,
//!   arrows, Ctrl+C, Ctrl+D, and raw characters).
//! - Shell stdout/stderr is parsed by Alacritty's VTE state machine.
//! - The GPUI view renders the parsed grid, cursor, scrollback, alternate screen,
//!   ANSI colors, truecolor, and text attributes.

use std::{ops::Range, rc::Rc, time::Duration};

use alacritty_terminal::event::Event as TerminalEvent;
use alacritty_terminal::grid::Scroll;
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
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::*;
use gpui_kit_assets::IconName;

use crate::terminal::{TerminalBackend, TerminalCell, TerminalSnapshot};

#[derive(Clone, Copy, PartialEq, Eq)]
struct TerminalRunStyle {
    foreground: TerminalColor,
    background: TerminalColor,
    flags: Flags,
    cursor: bool,
}

struct TerminalRun {
    text: String,
    style: TerminalRunStyle,
}

#[derive(Clone, Copy)]
struct TerminalPalette {
    background: Hsla,
    foreground: Hsla,
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
    marked_text: Option<String>,
    scroll_remainder: f32,
    terminal_size: Option<(usize, usize)>,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
    close_handler: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    new_terminal_handler: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}

struct TerminalTab {
    id: usize,
    terminal: Option<TerminalBackend>,
    shell_name: String,
    status: SharedString,
}

impl TerminalTab {
    fn new(id: usize, cwd: String, shell: &str) -> Self {
        let shell_name = std::path::Path::new(shell)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("shell")
            .to_string();
        let (terminal, status) = match TerminalBackend::new(&cwd, shell) {
            Ok(terminal) => (Some(terminal), SharedString::from("ready")),
            Err(error) => (None, format!("terminal error: {error}").into()),
        };
        Self {
            id,
            terminal,
            shell_name,
            status,
        }
    }

    fn title(&self) -> String {
        format!("{} {}", self.shell_name, self.id)
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
        let tab = TerminalTab::new(id, cwd, shell);
        let panel = Self {
            focus: cx.focus_handle(),
            terminal: tab,
            marked_text: None,
            scroll_remainder: 0.0,
            terminal_size: None,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
            close_handler: None,
            new_terminal_handler: None,
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
                        this.handle_terminal_event(event);
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
                if !matches!(running, Ok(true)) {
                    break;
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
        let cell_width = (f32::from(font_size) * 0.6).max(1.0);
        let line_height = (f32::from(font_size) * 1.5).max(1.0);
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

    fn send_bytes(&mut self, bytes: Vec<u8>) {
        if let Some(terminal) = self.terminal.terminal.as_ref()
            && let Err(error) = terminal.send(bytes)
        {
            self.terminal.status = error.into();
        }
    }

    fn handle_terminal_event(&mut self, event: TerminalEvent) {
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
            }
            TerminalEvent::Exit => self.terminal.status = "terminal closed".into(),
            TerminalEvent::Bell => self.terminal.status = "bell".into(),
            TerminalEvent::Wakeup
            | TerminalEvent::MouseCursorDirty
            | TerminalEvent::ClipboardStore(_, _)
            | TerminalEvent::ClipboardLoad(_, _)
            | TerminalEvent::ColorRequest(_, _)
            | TerminalEvent::TextAreaSizeRequest(_)
            | TerminalEvent::CursorBlinkingChange => {}
        }
    }

    pub fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if let Some(bytes) = terminal_key_bytes(event, self.terminal_mode()) {
            self.send_bytes(bytes);
            cx.notify();
        }
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
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let title = self.terminal.title();
        let group = self.tab_group.clone();
        let panel_id = self.panel_id;
        let close_handler = self.close_handler.clone();
        h_flex().items_center().gap_1().child(title).child(
            Button::new("close_terminal_panel")
                .icon(IconName::X)
                .ghost()
                .tooltip("Close Terminal")
                .on_click(move |_, window, cx| {
                    if let Some(handler) = close_handler.as_ref() {
                        handler(window, cx);
                    } else if let Some(group) = group.as_ref() {
                        let is_last = group
                            .read_with(cx, |group, _| group.panels().len() == 1)
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
        };
        let snapshot = self.snapshot();
        let cursor = snapshot.cursor;
        let colors = snapshot.colors;
        let terminal_panel = cx.entity();
        let focus = self.focus.clone();

        v_flex()
            .size_full()
            .bg(term_bg)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(
                |this: &mut Self, event: &KeyDownEvent, _, cx| {
                    this.handle_key(event, cx);
                    cx.stop_propagation();
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
                                terminal_panel.update(cx, |this, cx| {
                                    this.resize_to_bounds(bounds, mono_size, cx);
                                });
                                window.handle_input(
                                    &focus,
                                    ElementInputHandler::new(
                                        bounds,
                                        terminal_panel.clone(),
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
                        cx.listener(|this: &mut Self, _, window, cx| {
                            window.focus(&this.focus, cx);
                        }),
                    )
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
                                this.scroll_display(Scroll::Delta(delta));
                                cx.notify();
                            }
                        },
                    ))
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
}
