use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{Event as TerminalEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg, State};
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
            true,
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

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
