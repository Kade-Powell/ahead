//! AHEAD bottom terminal panel - real PTY-backed shell via `portable-pty`.
//!
//! Spawns the user's login shell in a PTY, parses VT output into scrollback
//! rows, and renders them with per-row styling. Input flows through a gpui-kit
//! `Input` box; output streams from a background reader thread.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use gpui_kit::*;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{BasePanel, Panel, PanelControl, PanelEvent};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit_assets::IconName;

const MAX_ROWS: usize = 2000;

pub struct TerminalPanel {
    pub focus: FocusHandle,
    pub input: Entity<InputState>,
    pub rows: Arc<Mutex<Vec<String>>>,
    pub writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    pub cwd: String,
    pub status: SharedString,
}

impl TerminalPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let rows: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![
            "AHEAD terminal ready. Type a command and press Enter.".to_string(),
        ]));
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "~".to_string());

        let mut panel = Self {
            focus: cx.focus_handle(),
            input,
            rows: rows.clone(),
            writer: None,
            cwd,
            status: "spawning shell…".into(),
        };
        panel.spawn_shell(cx);
        panel
    }

    fn push_row(&mut self, row: String, cx: &mut Context<Self>) {
        {
            let mut rows = self.rows.lock().expect("terminal rows lock");
            rows.push(row);
            if rows.len() > MAX_ROWS {
                let excess = rows.len() - MAX_ROWS;
                rows.drain(0..excess);
            }
        }
        cx.notify();
    }

    fn spawn_shell(&mut self, cx: &mut Context<Self>) {
        use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
        let rows = self.rows.clone();
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let cwd = self.cwd.clone();

        let pty_system = NativePtySystem::default();
        let pair = match pty_system.openpty(PtySize {
            rows: 32,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(pair) => pair,
            Err(e) => {
                self.status = format!("pty open failed: {e}").into();
                return;
            }
        };

        let mut cmd = CommandBuilder::new(&shell);
        cmd.env("TERM", "xterm-256color");
        cmd.cwd(&cwd);
        cmd.args(["-i"]);

        let child = match pair.slave.spawn_command(cmd) {
            Ok(child) => child,
            Err(e) => {
                self.status = format!("shell spawn failed: {e}").into();
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
                self.status = format!("pty reader failed: {e}").into();
                return;
            }
        };

        self.status = format!("shell: {shell}").into();
        let notify_rows = rows.clone();
        let (notify_tx, notify_rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 8192];
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
                            let mut rows = notify_rows.lock().expect("terminal rows lock");
                            for part in parts {
                                rows.push(Self::clean_line(&part));
                                if rows.len() > MAX_ROWS {
                                    let excess = rows.len() - MAX_ROWS;
                                    rows.drain(0..excess);
                                }
                            }
                        }
                        let _ = notify_tx.send(());
                    }
                    Err(_) => break,
                }
            }
        });

        let rows_cx = self.rows.clone();
        let _ = (rows_cx, notify_rx);
        cx.notify();
    }

    fn clean_line(raw: &str) -> String {
        let mut out = String::with_capacity(raw.len());
        let mut chars = raw.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                if matches!(chars.peek(), Some('[') | Some('(') | Some(')') | Some('#')) {
                    chars.next();
                    for c2 in chars.by_ref() {
                        if c2.is_ascii_alphabetic() {
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

    pub fn send_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        if let Some(writer) = self.writer.clone() {
            if let Ok(mut w) = writer.lock() {
                let _ = writeln!(w, "{text}");
                let _ = w.flush();
            }
        }
        {
            let mut rows = self.rows.lock().expect("terminal rows lock");
            rows.push(format!("$ {text}"));
            if rows.len() > MAX_ROWS {
                let excess = rows.len() - MAX_ROWS;
                rows.drain(0..excess);
            }
        }
        self.input.update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    pub fn append(&mut self, line: String, cx: &mut Context<Self>) {
        self.push_row(line, cx);
    }
}

impl BasePanel for TerminalPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_terminal"
    }
}

impl Panel for TerminalPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        "Terminal"
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
        let group = cx.theme().group_box;
        let rows: Vec<String> = self.rows.lock().expect("terminal rows lock").clone();
        v_flex()
            .size_full()
            .p_2()
            .gap_1()
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(IconName::Terminal)
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_size(px(11.))
                            .text_color(text)
                            .child("TERMINAL"),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(text)
                            .child(self.cwd.clone()),
                    )
                    .child(
                        Button::new("terminal_clear")
                            .icon(IconName::Trash)
                            .label("Clear")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.rows.lock().expect("terminal rows lock").clear();
                                this.status = "cleared".into();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_0()
                    .p_2()
                    .rounded_lg()
                    .bg(group)
                    .border_1()
                    .border_color(border)
                    .overflow_y_scrollbar()
                    .children(rows.iter().map(|line| {
                        div()
                            .text_size(px(11.))
                            .text_color(text)
                            .font_family("Menlo")
                            .child(line.clone())
                    })),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(Input::new(&self.input).aria_label("Terminal input").flex_1())
                    .child(
                        Button::new("terminal_send")
                            .primary()
                            .icon(IconName::CornerDownLeft)
                            .label("Run")
                            .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                this.send_input(window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(text)
                    .child(self.status.clone()),
            )
    }
}
