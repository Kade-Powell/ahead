//! AHEAD Interactive Terminal - Direct PTY Terminal Emulator
//!
//! True terminal experience matching Zed / VS Code:
//! - NO text input box: the terminal grid itself captures focus and keystrokes.
//! - Keystrokes are dispatched directly to the PTY stdin (Enter, Backspace, Tab,
//!   arrows, Ctrl+C, Ctrl+D, and raw characters).
//! - Shell stdout/stderr stream into scrollback rows with VT cleanup.
//! - Header shows active shell (`ahead — zsh`), working directory, and clear action.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use gpui_kit::*;

use gpui_kit::component::button::Button;
use gpui_kit::component::dock::{BasePanel, Panel, PanelControl, PanelEvent};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme};
use gpui_kit_assets::IconName;

const MAX_ROWS: usize = 2000;

pub struct TerminalPanel {
    pub focus: FocusHandle,
    pub rows: Arc<Mutex<Vec<String>>>,
    pub writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    pub cwd: String,
    pub shell_name: String,
    pub status: SharedString,
}

impl TerminalPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rows: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "~".to_string());
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let shell_name = std::path::Path::new(&shell)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("zsh")
            .to_string();

        let mut panel = Self {
            focus: cx.focus_handle(),
            rows: rows.clone(),
            writer: None,
            cwd,
            shell_name,
            status: "ready".into(),
        };
        panel.spawn_pty(cx);
        panel
    }

    fn spawn_pty(&mut self, cx: &mut Context<Self>) {
        use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
        let rows = self.rows.clone();
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let cwd = self.cwd.clone();

        let pty_system = NativePtySystem::default();
        let pair = match pty_system.openpty(PtySize {
            rows: 28,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(pair) => pair,
            Err(e) => {
                self.status = format!("pty error: {e}").into();
                return;
            }
        };

        let mut cmd = CommandBuilder::new(&shell);
        cmd.env("TERM", "xterm-256color");
        cmd.cwd(&cwd);
        cmd.args(["-l"]);

        let child = match pair.slave.spawn_command(cmd) {
            Ok(child) => child,
            Err(e) => {
                self.status = format!("spawn error: {e}").into();
                return;
            }
        };
        drop(child);

        let writer = pair.master.take_writer().ok().map(|w| {
            let boxed: Box<dyn Write + Send> = Box::new(w);
            Arc::new(Mutex::new(boxed))
        });
        self.writer = writer;

        let mut reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(e) => {
                self.status = format!("reader error: {e}").into();
                return;
            }
        };

        let notify_rows = rows.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut pending = String::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(&buf[..n]).replace('\r', "");
                        pending.push_str(&chunk);
                        let mut parts: Vec<String> =
                            pending.split('\n').map(|s| s.to_string()).collect();
                        pending = parts.pop().unwrap_or_default();
                        {
                            let mut r = notify_rows.lock().expect("terminal rows lock");
                            for part in parts {
                                r.push(Self::strip_ansi(&part));
                                if r.len() > MAX_ROWS {
                                    let excess = r.len() - MAX_ROWS;
                                    r.drain(0..excess);
                                }
                            }
                            if !pending.is_empty() {
                                r.push(Self::strip_ansi(&pending));
                                pending.clear();
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        cx.notify();
    }

    fn strip_ansi(raw: &str) -> String {
        let mut out = String::with_capacity(raw.len());
        let mut chars = raw.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                if matches!(chars.peek(), Some('[') | Some('(') | Some(')') | Some('#') | Some(']')) {
                    chars.next();
                    for c2 in chars.by_ref() {
                        if c2.is_ascii_alphabetic() || c2 == '\x07' || c2 == '\\' {
                            break;
                        }
                    }
                }
                continue;
            }
            if c.is_control() && c != '\t' {
                continue;
            }
            out.push(c);
        }
        out
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        if let Some(writer) = self.writer.as_ref() {
            if let Ok(mut w) = writer.lock() {
                let _ = w.write_all(bytes);
                let _ = w.flush();
            }
        }
    }

    pub fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let ctrl = event.keystroke.modifiers.control;

        if ctrl {
            match key {
                "c" => self.write_bytes(b"\x03"),
                "d" => self.write_bytes(b"\x04"),
                "z" => self.write_bytes(b"\x1a"),
                "l" => self.write_bytes(b"\x0c"),
                "a" => self.write_bytes(b"\x01"),
                "e" => self.write_bytes(b"\x05"),
                "k" => self.write_bytes(b"\x0b"),
                "u" => self.write_bytes(b"\x15"),
                "w" => self.write_bytes(b"\x17"),
                _ => {}
            }
            cx.notify();
            return;
        }

        match key {
            "enter" => self.write_bytes(b"\r"),
            "backspace" => self.write_bytes(b"\x7f"),
            "tab" => self.write_bytes(b"\t"),
            "escape" => self.write_bytes(b"\x1b"),
            "up" => self.write_bytes(b"\x1b[A"),
            "down" => self.write_bytes(b"\x1b[B"),
            "right" => self.write_bytes(b"\x1b[C"),
            "left" => self.write_bytes(b"\x1b[D"),
            "home" => self.write_bytes(b"\x1b[H"),
            "end" => self.write_bytes(b"\x1b[F"),
            _ => {
                if let Some(ch) = event.keystroke.key_char.as_deref() {
                    self.write_bytes(ch.as_bytes());
                } else if key.len() == 1 {
                    self.write_bytes(key.as_bytes());
                }
            }
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let text = cx.theme().sidebar_foreground;
        let is_dark = cx.theme().mode.is_dark();
        let term_bg = if is_dark { gpui_kit::rgb(0x0C0E14) } else { gpui_kit::rgb(0xFAFAFA) };
        let term_fg = if is_dark { gpui_kit::rgb(0xE6EDF3) } else { gpui_kit::rgb(0x1F2328) };
        let rows = self.rows.lock().expect("terminal rows lock").clone();

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
                    .on_key_down(cx.listener(|this: &mut Self, event: &KeyDownEvent, _, cx| {
                        this.handle_key(event, cx);
                    }))
                    .children(
                        rows.iter().map(|line| {
                            div()
                                .text_size(px(12.))
                                .text_color(term_fg)
                                .font_family("Menlo")
                                .line_height(px(18.))
                                .child(if line.is_empty() { " ".to_string() } else { line.clone() })
                        })
                    )
            )
            // Bottom Status strip
            .child(
                h_flex()
                    .h(px(22.))
                    .items_center()
                    .justify_between()
                    .px_3()
                    .border_t_1()
                    .border_color(border)
                    .text_size(px(11.))
                    .text_color(text)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(IconName::Terminal)
                            .child(format!("ahead — {} · {}", self.shell_name, self.cwd))
                    )
                    .child(
                        Button::new("clear_term_btn")
                            .icon(IconName::Trash)
                            .label("Clear")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.rows.lock().expect("terminal rows lock").clear();
                                cx.notify();
                            }))
                    )
            )
    }
}
