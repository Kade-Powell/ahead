use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{Event as TerminalEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg, State};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
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
const SHARED_HISTORY_LINES: i32 = 500;
const AGENT_CONTEXT_LINES: usize = 120;
const AGENT_CONTEXT_BYTES: usize = 16 * 1024;

fn terminal_history_text<T: EventListener>(terminal: &Term<T>) -> String {
    let grid = terminal.grid();
    let first_line = grid
        .topmost_line()
        .0
        .max(grid.bottommost_line().0 - SHARED_HISTORY_LINES + 1);
    let mut text = String::new();
    let mut row = String::new();
    let mut current_line = None;
    for indexed in
        grid.iter_from(Point::new(Line(first_line - 1), Column(grid.columns() - 1)))
    {
        if current_line != Some(indexed.point.line) {
            if current_line.is_some() {
                text.push_str(row.trim_end());
                text.push('\n');
                row.clear();
            }
            current_line = Some(indexed.point.line);
        }
        if !indexed.cell.flags.contains(Flags::WIDE_CHAR_SPACER)
            && !indexed.cell.c.is_control()
        {
            row.push(indexed.cell.c);
        }
    }
    text.push_str(row.trim_end());
    text
}

fn bounded_terminal_context(history: &str) -> String {
    let mut lines = history
        .lines()
        .rev()
        .take(AGENT_CONTEXT_LINES)
        .collect::<Vec<_>>();
    lines.reverse();
    let mut excerpt = lines.join("\n");
    if excerpt.len() > AGENT_CONTEXT_BYTES {
        let mut start = excerpt.len() - AGENT_CONTEXT_BYTES;
        while !excerpt.is_char_boundary(start) {
            start += 1;
        }
        excerpt.replace_range(..start, "");
        if let Some(newline) = excerpt.find('\n') {
            excerpt.replace_range(..=newline, "");
        }
        excerpt.insert_str(0, "[Earlier terminal output omitted]\n");
    }
    excerpt
}

fn terminal_agent_context<T: EventListener>(terminal: &Term<T>) -> (String, bool) {
    if let Some(selection) = terminal
        .selection_to_string()
        .filter(|selection| !selection.trim().is_empty())
    {
        return (bounded_terminal_context(&selection), true);
    }

    (
        bounded_terminal_context(&terminal_history_text(terminal)),
        false,
    )
}

fn select_all_terminal_content<T: EventListener>(terminal: &mut Term<T>) {
    let (start, end) = {
        let grid = terminal.grid();
        (
            Point::new(grid.topmost_line(), Column(0)),
            Point::new(grid.bottommost_line(), grid.last_column()),
        )
    };
    let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
    selection.update(end, Side::Right);
    terminal.selection = Some(selection);
}

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
    pty_thread: Option<
        std::thread::JoinHandle<(EventLoop<tty::Pty, TerminalEventProxy>, State)>,
    >,
    shutdown_sender: Option<Sender<()>>,
    shutdown_complete: Receiver<()>,
    #[cfg(unix)]
    pty_file: Option<std::fs::File>,
    #[cfg(unix)]
    child_id: u32,
}

impl TerminalBackend {
    #[cfg(test)]
    pub(crate) fn new(cwd: &str, shell: &str) -> Result<Self, String> {
        Self::new_with_command(
            cwd,
            shell,
            Vec::new(),
            std::collections::HashMap::new(),
        )
    }

    pub(crate) fn new_with_command(
        cwd: &str,
        program: &str,
        args: Vec<String>,
        extra_env: std::collections::HashMap<String, String>,
    ) -> Result<Self, String> {
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
        env.extend(extra_env);
        let options = PtyOptions {
            shell: Some(PtyShell::new(program.to_string(), args)),
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
        #[cfg(unix)]
        let child_id = pty.child().id();
        #[cfg(unix)]
        let pty_file = pty.file().try_clone().map_err(|error| {
            format!("failed to retain terminal process handle: {error}")
        })?;
        let event_loop = EventLoop::new(
            terminal.clone(),
            TerminalEventProxy { events: events_tx },
            pty,
            false,
            false, // Alacritty's reference-test mode records PTY output in the process CWD.
        )
        .map_err(|error| format!("failed to create terminal event loop: {error}"))?;
        let pty_sender = event_loop.channel();
        let pty_thread = event_loop.spawn();
        let (shutdown_sender, shutdown_complete) = unbounded();

        Ok(Self {
            terminal,
            events,
            pty_sender,
            pty_thread: Some(pty_thread),
            shutdown_sender: Some(shutdown_sender),
            shutdown_complete,
            #[cfg(unix)]
            pty_file: Some(pty_file),
            #[cfg(unix)]
            child_id,
        })
    }

    pub(crate) fn events(&self) -> Receiver<TerminalEvent> {
        self.events.clone()
    }

    pub(crate) fn process_id(&self) -> Option<u32> {
        #[cfg(unix)]
        {
            Some(self.child_id)
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    pub(crate) fn send(&self, bytes: Vec<u8>) -> Result<(), String> {
        if !self.is_running() {
            return Err("terminal is closed".into());
        }
        self.pty_sender
            .send(Msg::Input(Cow::Owned(bytes)))
            .map_err(|error| format!("failed to write to terminal: {error}"))
    }

    pub(crate) fn paste(&self, text: &str) -> Result<(), String> {
        self.send(terminal_paste_bytes(self.mode(), text))
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        self.terminal.lock().selection_to_string()
    }

    pub(crate) fn select_all(&self) {
        select_all_terminal_content(&mut self.terminal.lock());
    }

    pub(crate) fn set_selection(
        &self,
        point: Point,
        side: Side,
        selection_type: SelectionType,
    ) {
        self.terminal.lock().selection =
            Some(Selection::new(selection_type, point, side));
    }

    pub(crate) fn update_selection(&self, point: Point, side: Side) {
        if let Some(selection) = self.terminal.lock().selection.as_mut() {
            selection.update(point, side);
        }
    }

    pub(crate) fn clear_selection(&self) {
        self.terminal.lock().selection = None;
    }

    pub(crate) fn display_offset(&self) -> usize {
        self.terminal.lock().grid().display_offset()
    }

    pub(crate) fn resize(&self, columns: usize, rows: usize) -> Result<(), String> {
        if !self.is_running() {
            return Err("terminal is closed".into());
        }
        let dimensions = TerminalDimensions { columns, rows };
        self.terminal.lock().resize(dimensions);
        self.pty_sender
            .send(Msg::Resize(terminal_window_size(columns, rows)))
            .map_err(|error| format!("failed to resize terminal: {error}"))
    }

    pub(crate) fn snapshot(&self) -> TerminalSnapshot {
        let terminal = self.terminal.lock();
        let content = terminal.renderable_content();
        let selection = content.selection;
        let cursor_shape = content.cursor.shape;
        let mut rows = BTreeMap::<i32, Vec<TerminalCell>>::new();
        for indexed in content.display_iter {
            let selected = selection.is_some_and(|selection| {
                selection.contains_cell(&indexed, indexed.point, cursor_shape)
            });
            rows.entry(indexed.point.line.0)
                .or_default()
                .push(TerminalCell {
                    character: indexed.cell.c,
                    foreground: indexed.cell.fg,
                    background: indexed.cell.bg,
                    flags: indexed.cell.flags,
                    line: indexed.point.line.0,
                    column: indexed.point.column.0,
                    selected,
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

    pub(crate) fn shared_history_text(&self) -> String {
        terminal_history_text(&self.terminal.lock())
    }

    pub(crate) fn agent_context(&self) -> (String, bool) {
        terminal_agent_context(&self.terminal.lock())
    }

    pub(crate) fn mode(&self) -> TermMode {
        *self.terminal.lock().mode()
    }

    pub(crate) fn scroll_display(&self, scroll: Scroll) {
        self.terminal.lock().scroll_display(scroll);
    }

    pub(crate) fn is_shutdown(&self) -> bool {
        self.pty_thread.is_none()
    }

    pub(crate) fn is_running(&self) -> bool {
        self.pty_thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }

    pub(crate) fn shutdown(&mut self) -> Receiver<()> {
        if let Some(pty_thread) = self.pty_thread.take() {
            drop(self.pty_sender.send(Msg::Shutdown));
            let complete = self.shutdown_sender.take();
            #[cfg(unix)]
            let process = (self.child_id, self.pty_file.take());
            std::thread::spawn(move || {
                match pty_thread.join() {
                    Ok(resources) => {
                        #[cfg(unix)]
                        if let Some(file) = process.1 {
                            terminate_pty_child(process.0, &file);
                        }
                        // Pty::drop reaps the child. Keep its resources alive
                        // until termination so a resistant shell cannot block GPUI.
                        drop(resources);
                    }
                    Err(_) => {
                        eprintln!("Terminal IO thread panicked during shutdown")
                    }
                }
                drop(complete);
            });
        }
        self.shutdown_complete.clone()
    }
}

fn terminal_paste_bytes(mode: TermMode, text: &str) -> Vec<u8> {
    let text = if mode.contains(TermMode::BRACKETED_PASTE) {
        format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r")
    };
    text.into_bytes()
}

impl Drop for TerminalBackend {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(unix)]
fn terminate_pty_child(child_id: u32, file: &std::fs::File) {
    use std::os::fd::AsRawFd;
    use std::time::Duration;

    let Ok(child) = i32::try_from(child_id) else {
        return;
    };
    if child <= 1 {
        return;
    }
    // The IO thread has stopped and is no longer reaping. WNOWAIT verifies
    // this is still our unreaped child and keeps its PID reserved until Drop.
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    if unsafe {
        libc::waitid(
            libc::P_PID,
            child_id,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    } != 0
    {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ECHILD) {
            eprintln!("Inspecting terminal child before shutdown: {error}");
        }
        return;
    }
    let foreground = unsafe { libc::tcgetpgrp(file.as_raw_fd()) };
    for signal in [libc::SIGTERM, libc::SIGKILL] {
        for group in [foreground, child] {
            // Never signal the editor's group or a PID from a different session.
            if group > 1 && unsafe { libc::getsid(group) } == child {
                signal_terminal_process(-group, signal);
            }
        }
        signal_terminal_process(child, signal);
        if signal == libc::SIGTERM {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    // The reader is gone. macOS can otherwise block a killed shell's exit
    // draining output that no longer has a consumer.
    #[cfg(target_os = "macos")]
    if unsafe { libc::tcflush(file.as_raw_fd(), libc::TCIOFLUSH) } != 0 {
        eprintln!(
            "Flushing terminal queues before reaping: {}",
            std::io::Error::last_os_error()
        );
    }
}

#[cfg(unix)]
fn signal_terminal_process(pid: i32, signal: i32) {
    if unsafe { libc::kill(pid, signal) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            eprintln!("Stopping terminal process {pid}: {error}");
        }
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
    pub(crate) selected: bool,
}

#[derive(Clone, Default)]
pub(crate) struct TerminalSnapshot {
    pub(crate) rows: Vec<Vec<TerminalCell>>,
    pub(crate) colors: Colors,
    pub(crate) cursor: Option<(i32, usize, CursorShape)>,
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::vte::ansi::Processor;
    use std::time::{Duration, Instant};

    #[test]
    fn shared_history_includes_scrollback_without_terminal_controls() {
        let mut terminal = Term::new(
            Config {
                scrolling_history: 10,
                ..Config::default()
            },
            &TermSize::new(10, 3),
            VoidListener,
        );
        let mut parser =
            Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new();
        for byte in b"one\r\ntwo\r\nthree\r\nfour\r\nfive" {
            parser.advance(&mut terminal, *byte);
        }
        let text = terminal_history_text(&terminal);
        assert!(text.contains("one\ntwo\nthree\nfour\nfive"), "{text:?}");
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn agent_terminal_context_is_bounded_and_keeps_utf8_boundaries() {
        let history = (0..AGENT_CONTEXT_LINES + 20)
            .map(|line| format!("line {line} café 日本語"))
            .collect::<Vec<_>>()
            .join("\n");
        let context = bounded_terminal_context(&history);

        assert!(context.contains("line 20 café 日本語"));
        assert!(!context.contains("line 19 café 日本語"));
        assert!(
            context.ends_with(&format!(
                "line {} café 日本語",
                AGENT_CONTEXT_LINES + 19
            ))
        );

        let long_history = format!("{}\nend", "🦀".repeat(AGENT_CONTEXT_BYTES));
        let byte_bounded = bounded_terminal_context(&long_history);
        assert!(byte_bounded.starts_with("[Earlier terminal output omitted]\n"));
        assert!(context.len() <= AGENT_CONTEXT_BYTES + 64);
        assert!(byte_bounded.len() <= AGENT_CONTEXT_BYTES + 64);
        assert!(byte_bounded.is_char_boundary(0));
        assert!(byte_bounded.is_char_boundary(byte_bounded.len()));
    }

    #[test]
    fn paste_normalizes_newlines_and_wraps_bracketed_paste() {
        assert_eq!(
            terminal_paste_bytes(TermMode::empty(), "one\r\ntwo\nthree"),
            b"one\rtwo\rthree"
        );
        assert_eq!(
            terminal_paste_bytes(TermMode::BRACKETED_PASTE, "one\n\x1b[31mtwo"),
            b"\x1b[200~one\n[31mtwo\x1b[201~"
        );
    }

    #[test]
    fn pasted_command_reaches_the_integrated_shell() {
        let workspace = tempfile::tempdir().expect("disposable workspace");
        let backend = TerminalBackend::new(
            workspace.path().to_str().expect("workspace path"),
            "/bin/sh",
        )
        .expect("start disposable shell");
        let marker = "AHEAD_TERMINAL_PASTE_OK";
        backend
            .paste(&format!("printf '{marker}\\n'\n"))
            .expect("paste into shell");

        let deadline = Instant::now() + Duration::from_secs(3);
        while !backend.shared_history_text().contains(marker) {
            assert!(
                Instant::now() < deadline,
                "pasted command did not reach the shell"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let shell_process = backend.process_id().expect("shell process ID") as i32;
        assert_terminal_shutdown(backend, &[shell_process]);
    }

    #[test]
    fn direct_auth_command_preserves_arguments_environment_and_working_directory() {
        let workspace = tempfile::tempdir().expect("disposable workspace");
        let expected_directory = workspace
            .path()
            .canonicalize()
            .expect("canonical workspace path")
            .to_string_lossy()
            .into_owned();
        let backend = TerminalBackend::new_with_command(
            &expected_directory,
            "/bin/sh",
            vec![
                "-c".to_string(),
                "printf '%s|%s\\n' \"$AHEAD_AUTH_TEST\" \"$PWD\"".to_string(),
            ],
            std::collections::HashMap::from([(
                "AHEAD_AUTH_TEST".to_string(),
                "launch-context-preserved".to_string(),
            )]),
        )
        .expect("start the advertised terminal login command");
        let expected_output =
            format!("launch-context-preserved|{expected_directory}");
        let deadline = Instant::now() + Duration::from_secs(3);
        while !backend.shared_history_text().contains(&expected_output) {
            assert!(
                Instant::now() < deadline,
                "terminal did not receive configured args/env/cwd: {}",
                backend.shared_history_text()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let child_process =
            backend.process_id().expect("terminal process ID") as i32;
        assert_terminal_shutdown(backend, &[child_process]);
    }

    #[test]
    fn shell_osc52_copy_emits_a_clipboard_event() {
        let workspace = tempfile::tempdir().expect("disposable workspace");
        let backend = TerminalBackend::new(
            workspace.path().to_str().expect("workspace path"),
            "/bin/sh",
        )
        .expect("start disposable shell");
        let events = backend.events();
        backend
            .send(b"printf '\\033]52;c;Y29weQ==\\007'\r".to_vec())
            .expect("send OSC 52 shell command");

        let deadline = Instant::now() + Duration::from_secs(3);
        let clipboard_text = loop {
            if let Some(text) = events.try_iter().find_map(|event| match event {
                TerminalEvent::ClipboardStore(_, text) => Some(text),
                _ => None,
            }) {
                break text;
            }
            assert!(
                Instant::now() < deadline,
                "shell OSC 52 request emitted no clipboard event"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(clipboard_text, "copy");

        let shell_process = backend.process_id().expect("shell process ID") as i32;
        assert_terminal_shutdown(backend, &[shell_process]);
    }

    #[test]
    fn terminal_selection_extracts_text_for_copy() {
        let mut terminal =
            Term::new(Config::default(), &TermSize::new(20, 4), VoidListener);
        let mut parser =
            Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new();
        for byte in b"hello world" {
            parser.advance(&mut terminal, *byte);
        }
        let mut selection = Selection::new(
            SelectionType::Simple,
            Point::new(Line(0), Column(0)),
            Side::Left,
        );
        selection.update(Point::new(Line(0), Column(4)), Side::Right);
        terminal.selection = Some(selection);

        assert_eq!(terminal.selection_to_string().as_deref(), Some("hello"));
    }

    #[test]
    fn agent_context_prefers_selected_output_and_falls_back_to_recent_history() {
        let mut terminal =
            Term::new(Config::default(), &TermSize::new(20, 4), VoidListener);
        let mut parser =
            Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new();
        for byte in b"error: selected\r\nother output" {
            parser.advance(&mut terminal, *byte);
        }

        let (recent, selected) = terminal_agent_context(&terminal);
        assert!(!selected);
        assert!(recent.contains("error: selected"));
        assert!(recent.contains("other output"));

        let mut selection = Selection::new(
            SelectionType::Simple,
            Point::new(Line(0), Column(0)),
            Side::Left,
        );
        selection.update(Point::new(Line(0), Column(14)), Side::Right);
        terminal.selection = Some(selection);

        let (context, selected) = terminal_agent_context(&terminal);
        assert!(selected);
        assert_eq!(context, "error: selected");
    }

    #[test]
    fn terminal_select_all_includes_scrollback_and_visible_output() {
        let mut terminal = Term::new(
            Config {
                scrolling_history: 10,
                ..Config::default()
            },
            &TermSize::new(20, 4),
            VoidListener,
        );
        let mut parser =
            Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new();
        for byte in b"one\r\ntwo\r\nthree\r\nfour\r\nfive" {
            parser.advance(&mut terminal, *byte);
        }

        select_all_terminal_content(&mut terminal);

        assert_eq!(
            terminal.selection_to_string().as_deref(),
            Some("one\ntwo\nthree\nfour\nfive")
        );
    }

    fn read_pid(path: &std::path::Path) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(pid) = std::fs::read_to_string(path)
                .ok()
                .and_then(|text| text.trim().parse::<i32>().ok())
            {
                assert!(pid > 1 && pid != std::process::id() as i32);
                return pid;
            }
            assert!(
                Instant::now() < deadline,
                "terminal fixture never wrote {path:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn process_exists(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    #[test]
    fn closing_terminal_stops_sighup_ignoring_shell_and_foreground_job() {
        let workspace = tempfile::tempdir().unwrap();
        let backend =
            TerminalBackend::new(workspace.path().to_str().unwrap(), "/bin/sh")
                .unwrap();
        backend.send(b"trap '' HUP TERM; echo $$ > shell.pid; sh -c 'echo $$ > foreground.pid; trap \"\" HUP TERM; exec sleep 60'\r".to_vec()).unwrap();
        let shell = read_pid(&workspace.path().join("shell.pid"));
        let foreground = read_pid(&workspace.path().join("foreground.pid"));
        assert_ne!(shell, foreground);

        assert_terminal_shutdown(backend, &[shell, foreground]);
    }

    fn assert_terminal_shutdown(backend: TerminalBackend, processes: &[i32]) {
        let shutdown_complete = backend.shutdown_complete.clone();
        drop(backend);

        let deadline = Instant::now() + Duration::from_secs(3);
        while processes.iter().any(|pid| process_exists(*pid))
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let shutdown_result = shutdown_complete.recv_deadline(deadline);
        let remaining = processes
            .iter()
            .copied()
            .filter(|pid| process_exists(*pid))
            .collect::<Vec<_>>();
        if !remaining.is_empty() {
            eprintln!("Terminal shutdown receipt: {shutdown_result:?}");
            match std::process::Command::new("ps")
                .args(["-o", "pid,ppid,pgid,sess,stat,comm", "-p"])
                .arg(
                    processes
                        .iter()
                        .map(i32::to_string)
                        .collect::<Vec<_>>()
                        .join(","),
                )
                .output()
            {
                Ok(output) => eprintln!(
                    "Terminal fixture processes={processes:?}:\n{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                ),
                Err(error) => eprintln!("Inspecting terminal fixture: {error}"),
            }
        }
        // Only the fixture processes created by this test; keep a failing test
        // from leaving its intentionally signal-resistant command behind.
        for pid in &remaining {
            unsafe {
                libc::kill(*pid, libc::SIGKILL);
            }
        }
        assert!(
            remaining.is_empty(),
            "terminal close left fixture processes alive: {remaining:?}"
        );
        assert!(
            matches!(
                shutdown_result,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected)
            ),
            "terminal cleanup did not finish: {shutdown_result:?}"
        );
    }

    #[test]
    fn closing_terminal_reaps_shell_with_output_queued_after_io_stops() {
        use std::io::Write;

        let workspace = tempfile::tempdir().unwrap();
        let backend =
            TerminalBackend::new(workspace.path().to_str().unwrap(), "/bin/sh")
                .unwrap();
        backend.send(b"trap '' HUP TERM; echo $$ > shell.pid; read gate; printf 'AHEAD pending output'; echo $$ > output.pid; read hold\r".to_vec()).unwrap();
        let shell = read_pid(&workspace.path().join("shell.pid"));
        backend.pty_sender.send(Msg::Shutdown).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !backend.pty_thread.as_ref().unwrap().is_finished() {
            assert!(Instant::now() < deadline, "terminal IO did not stop");
            std::thread::sleep(Duration::from_millis(10));
        }
        // Release the shell only after the reader has stopped, so its output
        // stays in the kernel queue throughout teardown.
        backend
            .pty_file
            .as_ref()
            .unwrap()
            .write_all(b"continue\r")
            .unwrap();
        assert_eq!(read_pid(&workspace.path().join("output.pid")), shell);

        assert_terminal_shutdown(backend, &[shell]);
    }

    #[test]
    fn terminal_shutdown_is_idempotent_and_retains_output_after_shell_exit() {
        let workspace = tempfile::tempdir().unwrap();
        let mut backend =
            TerminalBackend::new(workspace.path().to_str().unwrap(), "/bin/sh")
                .unwrap();
        backend
            .send(b"printf '\\101\\110\\105\\101\\104\\n'; exit\r".to_vec())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !backend.pty_thread.as_ref().unwrap().is_finished() {
            assert!(Instant::now() < deadline, "shell did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(backend.send(b"echo after exit\r".to_vec()).is_err());
        let output = backend
            .snapshot()
            .rows
            .into_iter()
            .flatten()
            .map(|cell| cell.character)
            .collect::<String>();
        assert!(output.contains("AHEAD"));
        let first = backend.shutdown();
        let second = backend.shutdown();
        for complete in [first, second] {
            assert!(matches!(
                complete.recv_timeout(Duration::from_secs(2)),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected)
            ));
        }
        assert!(backend.send(b"echo too late\r".to_vec()).is_err());
        assert!(backend.resize(80, 24).is_err());
        let after = backend
            .snapshot()
            .rows
            .into_iter()
            .flatten()
            .map(|cell| cell.character)
            .collect::<String>();
        assert_eq!(output, after);
    }
}
