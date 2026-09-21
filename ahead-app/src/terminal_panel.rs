//! AHEAD Interactive Terminal - Direct PTY Terminal Emulator
//!
//! True terminal experience matching Zed / VS Code:
//! - NO text input box: the terminal grid itself captures focus and keystrokes.
//! - Keystrokes are dispatched directly to the PTY stdin (Enter, Backspace, Tab,
//!   arrows, Ctrl+C, Ctrl+D, and raw characters).
//! - Shell stdout/stderr is parsed by Alacritty's VTE state machine.
//! - The GPUI view renders the parsed grid, cursor, scrollback, alternate screen,
//!   ANSI colors, truecolor, and text attributes.

use std::time::Duration;

use alacritty_terminal::event::Event as TerminalEvent;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{
    Color as TerminalColor, CursorShape, NamedColor, Rgb,
};
use gpui_kit::component::dock::{BasePanel, Panel, PanelControl, PanelEvent};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::*;

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
    terminal: Option<TerminalBackend>,
    pub cwd: String,
    pub shell_name: String,
    pub status: SharedString,
}

impl TerminalPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "~".to_string());
        let shell =
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let shell_name = std::path::Path::new(&shell)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("zsh")
            .to_string();
        let (terminal, status) = match TerminalBackend::new(&cwd, &shell) {
            Ok(terminal) => (Some(terminal), SharedString::from("ready")),
            Err(error) => (None, format!("terminal error: {error}").into()),
        };
        let panel = Self {
            focus: cx.focus_handle(),
            terminal,
            cwd,
            shell_name,
            status,
        };
        panel.start_output_pump(cx);
        panel
    }

    fn start_output_pump(&self, cx: &mut Context<Self>) {
        let Some(events) = self.terminal.as_ref().map(TerminalBackend::events)
        else {
            return;
        };
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let received = events.try_iter().collect::<Vec<_>>();
                if received.is_empty() {
                    continue;
                }
                if this
                    .update(cx, |this, cx| {
                        for event in received {
                            this.handle_terminal_event(event);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn snapshot(&self) -> TerminalSnapshot {
        self.terminal
            .as_ref()
            .map(TerminalBackend::snapshot)
            .unwrap_or_default()
    }

    fn send_bytes(&mut self, bytes: Vec<u8>) {
        if let Some(terminal) = self.terminal.as_ref()
            && let Err(error) = terminal.send(bytes)
        {
            self.status = error.into();
        }
    }

    fn handle_terminal_event(&mut self, event: TerminalEvent) {
        match event {
            TerminalEvent::PtyWrite(text) => self.send_bytes(text.into_bytes()),
            TerminalEvent::Title(title) => self.shell_name = title,
            TerminalEvent::ResetTitle => self.shell_name = "shell".to_string(),
            TerminalEvent::ChildExit(code) => {
                self.status = format!("shell exited ({code})").into();
            }
            TerminalEvent::Exit => self.status = "terminal closed".into(),
            TerminalEvent::Bell => self.status = "bell".into(),
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
        self.terminal
            .as_ref()
            .map(TerminalBackend::mode)
            .unwrap_or_else(TermMode::empty)
    }

    fn scroll_display(&self, scroll: Scroll) {
        if let Some(terminal) = self.terminal.as_ref() {
            terminal.scroll_display(scroll);
        }
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
}

impl Panel for TerminalPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        format!("ahead — {}", self.shell_name)
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

        v_flex()
            .size_full()
            .bg(term_bg)
            .track_focus(&self.focus)
            // Interactive Terminal Grid: captures keystrokes directly! No input box!
            .child(
                div()
                    .id("terminal-screen")
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .bg(term_bg)
                    .overflow_y_scrollbar()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this: &mut Self, _, window, cx| {
                            window.focus(&this.focus, cx);
                        }),
                    )
                    .on_scroll_wheel(cx.listener(
                        |this, event: &ScrollWheelEvent, _, cx| {
                            let lines = event.delta.pixel_delta(px(18.)).y / px(18.);
                            let delta = lines.round() as i32;
                            if delta != 0 {
                                this.scroll_display(Scroll::Delta(-delta));
                                cx.notify();
                            }
                        },
                    ))
                    .on_key_down(cx.listener(
                        |this: &mut Self, event: &KeyDownEvent, _, cx| {
                            this.handle_key(event, cx);
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
