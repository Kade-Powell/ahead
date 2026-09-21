use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{Event as TerminalEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty::{self, Options as PtyOptions, Shell as PtyShell};
use alacritty_terminal::vte::ansi::{Color as TerminalColor, CursorShape};
use crossbeam_channel::{Receiver, Sender, unbounded};

const TERMINAL_COLUMNS: usize = 120;
const TERMINAL_ROWS: usize = 28;
const MAX_SCROLLBACK: usize = 10_000;

#[derive(Clone)]
struct TerminalEventProxy {
    events: Sender<TerminalEvent>,
}

impl EventListener for TerminalEventProxy {
    fn send_event(&self, event: TerminalEvent) {
        drop(self.events.send(event));
    }
}

#[derive(Clone, Copy)]
struct TerminalDimensions {
    columns: usize,
    rows: usize,
}

impl Dimensions for TerminalDimensions {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

fn terminal_window_size(columns: usize, rows: usize) -> WindowSize {
    WindowSize {
        num_lines: rows as u16,
        num_cols: columns as u16,
        cell_width: 0,
        cell_height: 0,
    }
}

pub(crate) struct TerminalBackend {
    terminal: Arc<FairMutex<Term<TerminalEventProxy>>>,
    events: Receiver<TerminalEvent>,
    pty_sender: EventLoopSender,
}

impl TerminalBackend {
    pub(crate) fn new(cwd: &str, shell: &str) -> Result<Self, String> {
        let (events_tx, events) = unbounded();
        let dimensions = TerminalDimensions {
            columns: TERMINAL_COLUMNS,
            rows: TERMINAL_ROWS,
        };
        let terminal = Arc::new(FairMutex::new(Term::new(
            Config {
                scrolling_history: MAX_SCROLLBACK,
                ..Config::default()
            },
            &dimensions,
            TerminalEventProxy {
                events: events_tx.clone(),
            },
        )));

        let mut env = std::collections::HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        env.insert("TERM_PROGRAM".to_string(), "ahead".to_string());
        env.insert(
            "TERM_PROGRAM_VERSION".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        );
        let options = PtyOptions {
            shell: Some(PtyShell::new(shell.to_string(), Vec::new())),
            working_directory: Some(PathBuf::from(cwd)),
            hold: false,
            env,
        };
        let pty = tty::new(
            &options,
            terminal_window_size(TERMINAL_COLUMNS, TERMINAL_ROWS),
            0,
        )
        .map_err(|error| format!("failed to create terminal: {error}"))?;
        let event_loop = EventLoop::new(
            terminal.clone(),
            TerminalEventProxy { events: events_tx },
            pty,
            false,
            false,
        )
        .map_err(|error| format!("failed to create terminal event loop: {error}"))?;
        let pty_sender = event_loop.channel();
        event_loop.spawn();

        Ok(Self {
            terminal,
            events,
            pty_sender,
        })
    }

    pub(crate) fn events(&self) -> Receiver<TerminalEvent> {
        self.events.clone()
    }

    pub(crate) fn send(&self, bytes: Vec<u8>) -> Result<(), String> {
        self.pty_sender
            .send(Msg::Input(Cow::Owned(bytes)))
            .map_err(|error| format!("failed to write to terminal: {error}"))
    }

    pub(crate) fn resize(&self, columns: usize, rows: usize) -> Result<(), String> {
        let dimensions = TerminalDimensions { columns, rows };
        self.terminal.lock().resize(dimensions);
        self.pty_sender
            .send(Msg::Resize(terminal_window_size(columns, rows)))
            .map_err(|error| format!("failed to resize terminal: {error}"))
    }

    pub(crate) fn snapshot(&self) -> TerminalSnapshot {
        let terminal = self.terminal.lock();
        let content = terminal.renderable_content();
        let mut rows = BTreeMap::<i32, Vec<TerminalCell>>::new();
        for indexed in content.display_iter {
            rows.entry(indexed.point.line.0)
                .or_default()
                .push(TerminalCell {
                    character: indexed.cell.c,
                    foreground: indexed.cell.fg,
                    background: indexed.cell.bg,
                    flags: indexed.cell.flags,
                    line: indexed.point.line.0,
                    column: indexed.point.column.0,
                });
        }
        let cursor = match content.cursor.shape {
            CursorShape::Hidden => None,
            shape => Some((
                content.cursor.point.line.0,
                content.cursor.point.column.0,
                shape,
            )),
        };
        TerminalSnapshot {
            rows: rows.into_values().collect(),
            colors: *content.colors,
            cursor,
        }
    }

    pub(crate) fn mode(&self) -> TermMode {
        *self.terminal.lock().mode()
    }

    pub(crate) fn scroll_display(&self, scroll: Scroll) {
        self.terminal.lock().scroll_display(scroll);
    }
}

impl Drop for TerminalBackend {
    fn drop(&mut self) {
        drop(self.pty_sender.send(Msg::Shutdown));
    }
}

#[derive(Clone)]
pub(crate) struct TerminalCell {
    pub(crate) character: char,
    pub(crate) foreground: TerminalColor,
    pub(crate) background: TerminalColor,
    pub(crate) flags: Flags,
    pub(crate) line: i32,
    pub(crate) column: usize,
}

#[derive(Clone)]
pub(crate) struct TerminalSnapshot {
    pub(crate) rows: Vec<Vec<TerminalCell>>,
    pub(crate) colors: Colors,
    pub(crate) cursor: Option<(i32, usize, CursorShape)>,
}

impl Default for TerminalSnapshot {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            colors: Colors::default(),
            cursor: None,
        }
    }
}
