use std::{
    collections::HashMap,
    io::{BufReader, BufWriter, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use ahead_rpc::{
    RpcError,
    core::CoreRpcHandler,
    dap_types::{
        self, ConfigurationDone, Continue, ContinueArguments, DapEvent, DapId,
        DapPayload, DapRequest, DapResponse, DapServer, DapSessionState,
        DebugTerminalRequest, DebugTerminalResponse, Disconnect,
        DisconnectArguments, Initialize, Launch, Next, NextArguments, Pause,
        PauseArguments, Request, RunDebugConfig, RunInTerminal,
        RunInTerminalArguments, RunInTerminalResponse, Scope, Scopes,
        ScopesArguments, ScopesResponse, SetBreakpoints, SetBreakpointsArguments,
        SetBreakpointsResponse, Source, SourceBreakpoint, StackTrace,
        StackTraceArguments, StackTraceResponse, StepIn, StepInArguments, StepOut,
        StepOutArguments, Terminate, ThreadId, Threads, ThreadsResponse, Variable,
        Variables, VariablesArguments, VariablesResponse,
    },
};
use anyhow::{Result, anyhow};
use crossbeam_channel::{Receiver, Sender};
use parking_lot::{Condvar, Mutex};
use serde_json::Value;

use super::{
    PluginCatalogRpcHandler,
    psp::{ResponseHandler, RpcCallback},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct DapClient {
    plugin_rpc: PluginCatalogRpcHandler,
    pub(crate) dap_rpc: DapRpcHandler,
    dap_server: DapServer,
    config: RunDebugConfig,
    breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>,
    terminated: bool,
    disconnected: bool,
    restarted: bool,
}

struct DapProcess {
    child: Child,
    sender: Sender<DapPayload>,
    generation: u64,
}

impl Drop for DapProcess {
    fn drop(&mut self) {
        match self.child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {}
            Err(error) => tracing::warn!(?error, "inspecting debug adapter exit"),
        }
        if let Err(error) = self.child.kill() {
            tracing::warn!(?error, "stopping debug adapter");
        }
        if let Err(error) = self.child.wait() {
            tracing::error!(?error, "reaping debug adapter");
        }
    }
}

impl Drop for DapClient {
    fn drop(&mut self) {
        self.dap_rpc.shutdown();
    }
}

impl DapClient {
    pub fn new(
        dap_server: DapServer,
        config: RunDebugConfig,
        breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>,
        plugin_rpc: PluginCatalogRpcHandler,
    ) -> Result<Self> {
        let dap_rpc = DapRpcHandler::new(config.dap_id, plugin_rpc.core_rpc.clone());

        Ok(Self {
            plugin_rpc,
            dap_server,
            config,
            dap_rpc,
            breakpoints,
            terminated: false,
            disconnected: false,
            restarted: false,
        })
    }

    pub fn start(
        dap_server: DapServer,
        config: RunDebugConfig,
        breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>,
        plugin_rpc: PluginCatalogRpcHandler,
    ) -> Result<DapRpcHandler> {
        let mut dap = Self::new(dap_server, config, breakpoints, plugin_rpc)?;
        let dap_rpc = dap.dap_rpc.clone();
        let worker_rpc = dap_rpc.clone();
        thread::Builder::new()
            .name("dap-client".into())
            .spawn(move || {
                let result = dap.start_process().and_then(|()| dap.initialize());
                if let Err(error) = result {
                    worker_rpc.stop_process(
                        None,
                        &format!(
                            "Could not start debugger {}: {error}",
                            dap.config.name
                        ),
                    );
                    dap.disconnected = true;
                } else {
                    dap.launch();
                }
                worker_rpc.mainloop(&mut dap);
            })?;
        // The catalog owns this handle before it can process another notification,
        // including Shutdown while the worker is still waiting for Initialize.
        Ok(dap_rpc)
    }

    fn start_process(&self) -> Result<()> {
        let mut active = self.dap_rpc.process.lock();
        if self.dap_rpc.shutting_down.load(Ordering::Acquire)
            || self.dap_rpc.stop_requested.load(Ordering::Acquire)
        {
            return Err(anyhow!("debugger is shutting down"));
        }
        if active.is_some() {
            return Err(anyhow!("debug adapter is already running"));
        }
        let program = self.dap_server.program.clone();
        let generation = self.dap_rpc.generation.fetch_add(1, Ordering::AcqRel) + 1;
        {
            let mut lifecycle = self.dap_rpc.lifecycle.lock();
            if self.dap_rpc.stop_requested.load(Ordering::Acquire) {
                return Err(anyhow!("debugger start was cancelled"));
            }
            *lifecycle = DapLifecycle {
                generation,
                revision: 0,
                state: DapSessionState::Starting,
                control_pending: false,
            };
            self.dap_rpc
                .core_rpc
                .dap_session_state(self.config.dap_id, DapSessionState::Starting);
        }
        let (sender, receiver) = crossbeam_channel::unbounded();
        let mut process = DapProcess {
            child: Self::process(
                &program,
                &self.dap_server.args,
                self.dap_server.cwd.as_ref(),
            )?,
            sender,
            generation,
        };
        let stdin = process
            .child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("missing adapter stdin"))?;
        let stdout = process
            .child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("missing adapter stdout"))?;
        let mut stderr = process
            .child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("missing adapter stderr"))?;
        *active = Some(process);
        drop(active);

        let monitored_rpc = self.dap_rpc.clone();
        let core_rpc = self.plugin_rpc.core_rpc.clone();
        thread::spawn(move || {
            loop {
                let exit = {
                    let mut active = monitored_rpc.process.lock();
                    let Some(process) = active
                        .as_mut()
                        .filter(|process| process.generation == generation)
                    else {
                        return;
                    };
                    process.child.try_wait()
                };
                let message = match exit {
                    Ok(None) => {
                        monitored_rpc.expire_requests(Instant::now());
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    Ok(Some(status)) => format!("debug adapter exited ({status})"),
                    Err(error) => {
                        format!("could not inspect debug adapter: {error}")
                    }
                };
                if monitored_rpc.stop_process(Some(generation), &message)
                    && !monitored_rpc.shutting_down.load(Ordering::Acquire)
                {
                    core_rpc.log(ahead_rpc::core::LogLevel::Error, message, None);
                }
                return;
            }
        });

        let writer_rpc = self.dap_rpc.clone();
        thread::spawn(move || {
            let mut writer = BufWriter::new(stdin);
            let result = (|| -> Result<()> {
                for payload in receiver {
                    let message = serde_json::to_string(&payload)?;
                    write!(
                        writer,
                        "Content-Length: {}\r\n\r\n{}",
                        message.len(),
                        message
                    )?;
                    writer.flush()?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                writer_rpc.stop_process(
                    Some(generation),
                    &format!("debug adapter write failed: {error}"),
                );
            }
        });

        let reader_rpc = self.dap_rpc.clone();
        let core_rpc = self.plugin_rpc.core_rpc.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let result = crate::plugin::lsp::read_message(&mut reader).and_then(
                    |message| reader_rpc.handle_server_message(generation, &message),
                );
                if let Err(error) = result {
                    let message = format!("debug adapter disconnected: {error}");
                    if reader_rpc.stop_process(Some(generation), &message)
                        && !reader_rpc.shutting_down.load(Ordering::Acquire)
                    {
                        core_rpc.log(
                            ahead_rpc::core::LogLevel::Error,
                            message,
                            None,
                        );
                    }
                    break;
                }
            }
        });

        thread::spawn(move || {
            // Bound each read even when an adapter writes a long line or binary data.
            let mut buffer = [0; 4096];
            loop {
                match stderr.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(length) => {
                        tracing::debug!(%program, stderr = %String::from_utf8_lossy(&buffer[..length]))
                    }
                    Err(error) => {
                        tracing::warn!(?error, %program, "reading debug adapter stderr");
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    fn process(
        server: &str,
        args: &[String],
        cwd: Option<&PathBuf>,
    ) -> Result<Child> {
        let mut process = Command::new(server);
        if let Some(cwd) = cwd {
            process.current_dir(cwd);
        }

        process.args(args);

        // CREATE_NO_WINDOW
        // (https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags)
        // TODO: We set this because
        #[cfg(target_os = "windows")]
        std::os::windows::process::CommandExt::creation_flags(
            &mut process,
            0x08000000,
        );
        let child = process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        Ok(child)
    }

    fn handle_host_request(
        &mut self,
        req: &DapRequest,
        generation: u64,
    ) -> Result<Value> {
        match req.command.as_str() {
            RunInTerminal::COMMAND => {
                let value = req
                    .arguments
                    .as_ref()
                    .ok_or_else(|| anyhow!("no arguments"))?;
                let args: RunInTerminalArguments =
                    serde_json::from_value(value.clone())?;
                {
                    let lifecycle = self.dap_rpc.lifecycle.lock();
                    if lifecycle.generation != generation
                        || !lifecycle.state.can_stop()
                        || self.dap_rpc.shutting_down.load(Ordering::Acquire)
                    {
                        return Err(anyhow!(
                            "debug session is no longer accepting terminal requests"
                        ));
                    }
                    self.plugin_rpc
                        .core_rpc
                        .run_in_terminal(DebugTerminalRequest {
                            dap_id: self.config.dap_id,
                            generation,
                            request_seq: req.seq,
                            arguments: args,
                        });
                }
                let deadline = Instant::now() + Duration::from_secs(30);
                let response = loop {
                    if self.dap_rpc.shutting_down.load(Ordering::Acquire)
                        || self.dap_rpc.stop_requested.load(Ordering::Acquire)
                        || self.dap_rpc.process.lock().is_none()
                    {
                        return Err(anyhow!(
                            "debugger disconnected while opening terminal"
                        ));
                    }
                    match self
                        .dap_rpc
                        .terminal_response_rx
                        .recv_timeout(Duration::from_millis(50))
                    {
                        Ok(response)
                            if response.dap_id == self.config.dap_id
                                && response.generation == generation
                                && response.request_seq == req.seq =>
                        {
                            break response;
                        }
                        Ok(_) => continue,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout)
                            if Instant::now() < deadline => {}
                        Err(error) => {
                            return Err(anyhow!(
                                "could not open debug terminal: {error}"
                            ));
                        }
                    }
                };
                if let Some(error) = response.error {
                    return Err(anyhow!("could not open debug terminal: {error}"));
                }
                let shell_process_id =
                    response.shell_process_id.ok_or_else(|| {
                        anyhow!("debug terminal did not report a process ID")
                    })?;
                let resp = RunInTerminalResponse {
                    process_id: None,
                    shell_process_id: Some(shell_process_id),
                };
                let resp = serde_json::to_value(resp)?;
                Ok(resp)
            }
            _ => Err(anyhow!("not implemented")),
        }
    }

    fn handle_host_event(
        &mut self,
        event: &DapEvent,
        token: (u64, u64),
    ) -> Result<()> {
        match event {
            DapEvent::Initialized(_) => {
                for (path, breakpoints) in self.breakpoints.clone().into_iter() {
                    match self.dap_rpc.set_breakpoints(path.clone(), breakpoints) {
                        Ok(breakpoints) => {
                            self.plugin_rpc.core_rpc.dap_breakpoints_resp(
                                self.config.dap_id,
                                path,
                                breakpoints.breakpoints.unwrap_or_default(),
                            );
                        }
                        Err(err) => {
                            tracing::error!("{:?}", err);
                        }
                    }
                }
                // send dap configurations here
                let rpc = self.dap_rpc.clone();
                self.dap_rpc.request_async::<ConfigurationDone>(
                    (),
                    move |rs: Result<(), RpcError>| {
                        if let Err(e) = rs {
                            rpc.report_error(
                                token.0,
                                format!(
                                    "Debugger configuration failed: {}",
                                    e.message
                                ),
                            );
                        }
                    },
                );
            }
            DapEvent::Stopped(stopped) => {
                let all_threads_stopped =
                    stopped.all_threads_stopped.unwrap_or_default();
                let mut stack_frames = HashMap::new();
                if all_threads_stopped {
                    if let Ok(response) = self.dap_rpc.threads() {
                        for thread in response.threads {
                            if let Ok(frames) = self.dap_rpc.stack_trace(thread.id) {
                                stack_frames.insert(thread.id, frames.stack_frames);
                            }
                        }
                    }
                }

                let current_thread = stopped.thread_id.or_else(|| {
                    all_threads_stopped
                        .then(|| stack_frames.keys().min().copied())
                        .flatten()
                });

                let active_frame = current_thread
                    .and_then(|thread_id| stack_frames.get(&thread_id))
                    .and_then(|stack_frames| stack_frames.first());

                let mut vars = Vec::new();
                if let Some(frame) = active_frame {
                    if let Ok(scopes) = self.dap_rpc.scopes(frame.id) {
                        for scope in scopes {
                            let result =
                                self.dap_rpc.variables(scope.variables_reference);
                            vars.push((scope, result.unwrap_or_default()));
                        }
                    }
                }

                let lifecycle = self.dap_rpc.lifecycle.lock();
                if lifecycle.token() == token
                    && lifecycle.state == DapSessionState::Stopped
                {
                    self.plugin_rpc.core_rpc.dap_stopped(
                        self.config.dap_id,
                        stopped.clone(),
                        stack_frames,
                        vars,
                    );
                }
            }
            DapEvent::Continued(_) => {}
            DapEvent::Exited(_exited) => {}
            DapEvent::Terminated(_) => {
                self.terminated = true;
                self.dap_rpc.stop_requested.store(true, Ordering::Release);
                self.dap_rpc
                    .stop_process(Some(token.0), "debug session ended");
                if let Err(err) = self.check_restart() {
                    tracing::error!("{:?}", err);
                }
            }
            DapEvent::Thread { .. } => {}
            DapEvent::Output(_) => {}
            DapEvent::Breakpoint { .. } => {}
            DapEvent::Module { .. } => {}
            DapEvent::LoadedSource { .. } => {}
            DapEvent::Process(_) => {}
            DapEvent::Capabilities(_) => {}
            DapEvent::Memory(_) => {}
        }
        Ok(())
    }

    pub(crate) fn initialize(&mut self) -> Result<()> {
        let params = dap_types::InitializeParams {
            client_id: Some("ahead".to_owned()),
            client_name: Some("Ahead".to_owned()),
            adapter_id: "".to_string(),
            locale: Some("en-us".to_owned()),
            lines_start_at_one: Some(true),
            columns_start_at_one: Some(true),
            path_format: Some("path".to_owned()),
            supports_variable_type: Some(true),
            supports_variable_paging: Some(false),
            supports_run_in_terminal_request: Some(true),
            supports_memory_references: Some(false),
            supports_progress_reporting: Some(false),
            supports_invalidated_event: Some(false),
        };

        let resp = self
            .dap_rpc
            .request::<Initialize>(params)
            .map_err(|e| anyhow!(e.message))?;
        self.dap_rpc.supports_terminate.store(
            resp.supports_terminate_request.unwrap_or(false),
            Ordering::Release,
        );

        Ok(())
    }

    fn check_restart(&mut self) -> Result<()> {
        if !self.restarted {
            return Ok(());
        }
        self.disconnected |= self.dap_rpc.process.lock().is_none();
        if !self.disconnected {
            return Ok(());
        }

        self.restarted = false;

        if self.disconnected {
            self.dap_rpc.stop_requested.store(false, Ordering::Release);
            self.start_process()?;
            self.initialize()?;
        }
        self.terminated = false;
        self.disconnected = false;

        self.launch();
        Ok(())
    }

    fn launch(&self) {
        let dap_rpc = self.dap_rpc.clone();
        let token = dap_rpc.lifecycle.lock().token();
        let config = self.config.clone();
        thread::spawn(move || {
            if let Err(error) = dap_rpc.launch(&config, token.0) {
                dap_rpc.stop_process(
                    Some(token.0),
                    &format!("Debugger launch failed: {error}"),
                );
            } else {
                dap_rpc.set_state(token.0, Some(token.1), DapSessionState::Running);
            }
        });
    }

    fn restart(&mut self, breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>) {
        self.restarted = true;
        self.breakpoints = breakpoints;
        if !self.terminated && !self.disconnected {
            self.dap_rpc.stop();
        } else if let Err(err) = self.check_restart() {
            tracing::error!("{:?}", err);
        }
    }
}

#[allow(clippy::large_enum_variant)]
pub enum DapRpc {
    HostRequest(u64, DapRequest),
    HostEvent(u64, u64, DapEvent),
    Restart(HashMap<PathBuf, Vec<SourceBreakpoint>>),
    Shutdown,
    Disconnected(u64),
}

struct PendingRequest {
    command: &'static str,
    deadline: Instant,
    response: ResponseHandler<DapResponse, RpcError>,
}

struct DapLifecycle {
    generation: u64,
    revision: u64,
    state: DapSessionState,
    control_pending: bool,
}

impl DapLifecycle {
    fn token(&self) -> (u64, u64) {
        (self.generation, self.revision)
    }
}

#[derive(Clone)]
pub struct DapRpcHandler {
    pub dap_id: DapId,
    core_rpc: CoreRpcHandler,
    rpc_tx: Sender<DapRpc>,
    rpc_rx: Receiver<DapRpc>,
    process: Arc<Mutex<Option<DapProcess>>>,
    generation: Arc<AtomicU64>,
    lifecycle: Arc<Mutex<DapLifecycle>>,
    stop_requested: Arc<AtomicBool>,
    supports_terminate: Arc<AtomicBool>,
    shutting_down: Arc<AtomicBool>,
    shutdown_complete: Arc<(Mutex<bool>, Condvar)>,
    terminal_response_tx: Sender<DebugTerminalResponse>,
    terminal_response_rx: Receiver<DebugTerminalResponse>,
    seq_counter: Arc<AtomicU64>,
    server_pending: Arc<Mutex<HashMap<u64, PendingRequest>>>,
}

impl DapRpcHandler {
    fn new(dap_id: DapId, core_rpc: CoreRpcHandler) -> Self {
        let (rpc_tx, rpc_rx) = crossbeam_channel::unbounded();
        let (terminal_response_tx, terminal_response_rx) =
            crossbeam_channel::unbounded();
        Self {
            dap_id,
            core_rpc,
            process: Arc::new(Mutex::new(None)),
            generation: Arc::new(AtomicU64::new(0)),
            lifecycle: Arc::new(Mutex::new(DapLifecycle {
                generation: 0,
                revision: 0,
                state: DapSessionState::Starting,
                control_pending: false,
            })),
            stop_requested: Arc::new(AtomicBool::new(false)),
            supports_terminate: Arc::new(AtomicBool::new(false)),
            shutting_down: Arc::new(AtomicBool::new(false)),
            shutdown_complete: Arc::new((Mutex::new(false), Condvar::new())),
            rpc_rx,
            rpc_tx,
            terminal_response_tx,
            terminal_response_rx,
            seq_counter: Arc::new(AtomicU64::new(0)),
            server_pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn answer_terminal_request(
        &self,
        response: DebugTerminalResponse,
    ) -> Result<()> {
        let lifecycle = self.lifecycle.lock();
        if response.dap_id != self.dap_id
            || response.generation != lifecycle.generation
            || !lifecycle.state.can_stop()
        {
            return Ok(());
        }
        self.terminal_response_tx.send(response)?;
        Ok(())
    }

    fn set_state(
        &self,
        generation: u64,
        revision: Option<u64>,
        state: DapSessionState,
    ) -> bool {
        let mut lifecycle = self.lifecycle.lock();
        if lifecycle.generation != generation
            || revision.is_some_and(|revision| revision != lifecycle.revision)
            || !lifecycle.state.is_active()
            || (lifecycle.state == DapSessionState::Stopping && state.can_stop())
        {
            return false;
        }
        lifecycle.revision += 1;
        if state == DapSessionState::Stopping {
            self.stop_requested.store(true, Ordering::Release);
        }
        lifecycle.state = state.clone();
        lifecycle.control_pending = false;
        self.core_rpc.dap_session_state(self.dap_id, state);
        true
    }

    pub(crate) fn report_error(&self, generation: u64, message: String) {
        let lifecycle = self.lifecycle.lock();
        if lifecycle.generation == generation && lifecycle.state.can_stop() {
            self.core_rpc.dap_error(self.dap_id, message);
        }
    }

    fn timeout_error(command: &str) -> RpcError {
        RpcError {
            code: 0,
            message: format!(
                "Debugger {command} request timed out. Try again or restart the debugger."
            ),
        }
    }

    fn report_timeout(&self, error: &RpcError) {
        if !self.shutting_down.load(Ordering::Acquire) {
            self.core_rpc.show_message(
                "Debugger".into(),
                lsp_types::ShowMessageParams {
                    typ: lsp_types::MessageType::ERROR,
                    message: error.message.clone(),
                },
            );
        }
    }

    fn expire_requests(&self, now: Instant) {
        let mut pending = self.server_pending.lock();
        let expired_ids = pending
            .iter()
            .filter_map(|(id, request)| (request.deadline <= now).then_some(*id))
            .collect::<Vec<_>>();
        let expired = expired_ids
            .into_iter()
            .filter_map(|id| pending.remove(&id))
            .collect::<Vec<_>>();
        drop(pending);
        for pending in expired {
            let error = Self::timeout_error(pending.command);
            self.report_timeout(&error);
            pending.response.invoke(Err(error));
        }
    }

    pub fn mainloop(&self, dap_client: &mut DapClient) {
        for msg in &self.rpc_rx {
            if self.shutting_down.load(Ordering::Acquire) {
                return;
            }
            match msg {
                DapRpc::HostRequest(generation, req) => {
                    if generation != self.generation.load(Ordering::Acquire) {
                        continue;
                    }
                    let result = dap_client.handle_host_request(&req, generation);
                    let seq = self.seq_counter.fetch_add(1, Ordering::Relaxed);
                    let resp = DapResponse {
                        seq,
                        request_seq: req.seq,
                        success: result.is_ok(),
                        command: req.command.clone(),
                        message: result.as_ref().err().map(|e| e.to_string()),
                        body: result.ok(),
                    };
                    if let Some(process) = self.process.lock().as_ref()
                        && process.generation == generation
                        && let Err(error) =
                            process.sender.send(DapPayload::Response(resp))
                    {
                        tracing::error!(?error, "replying to debug adapter");
                    }
                }
                DapRpc::HostEvent(generation, revision, event) => {
                    if generation != self.generation.load(Ordering::Acquire) {
                        continue;
                    }
                    if let Err(err) =
                        dap_client.handle_host_event(&event, (generation, revision))
                    {
                        tracing::error!("{:?}", err);
                    }
                }
                DapRpc::Restart(breakpoints) => {
                    dap_client.restart(breakpoints);
                }
                DapRpc::Shutdown => {
                    return;
                }
                DapRpc::Disconnected(generation) => {
                    if generation != self.generation.load(Ordering::Acquire) {
                        continue;
                    }
                    dap_client.disconnected = true;
                    if let Err(err) = dap_client.check_restart() {
                        tracing::error!("{:?}", err);
                    }
                }
            }
        }
    }

    fn stop_process(&self, generation: Option<u64>, message: &str) -> bool {
        self.finish_process(generation, message, None)
    }

    fn finish_process(
        &self,
        generation: Option<u64>,
        message: &str,
        failure: Option<String>,
    ) -> bool {
        let (generation, pending) = {
            let mut active = self.process.lock();
            if generation.is_some_and(|generation| {
                active
                    .as_ref()
                    .is_none_or(|process| process.generation != generation)
            }) {
                return false;
            }
            let pending = std::mem::take(&mut *self.server_pending.lock());
            let generation = active.as_ref().map(|process| process.generation);
            // Keep teardown serialized with process creation and other cleanup
            // callers, so shutdown completion cannot precede another reader's reap.
            drop(active.take());
            let state = if let Some(message) = failure {
                DapSessionState::Failed(message)
            } else if self.stop_requested.load(Ordering::Acquire)
                || self.shutting_down.load(Ordering::Acquire)
            {
                DapSessionState::Terminated
            } else {
                DapSessionState::Failed(message.to_string())
            };
            let current_generation = self.lifecycle.lock().generation;
            self.set_state(current_generation, None, state);
            (generation, pending)
        };
        for (_, handler) in pending {
            handler.response.invoke(Err(RpcError {
                code: 0,
                message: message.to_owned(),
            }));
        }
        if let Some(generation) = generation {
            if let Err(error) = self.rpc_tx.send(DapRpc::Disconnected(generation)) {
                tracing::error!(?error, "reporting debug adapter disconnection");
            }
        }
        generation.is_some()
    }

    pub fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        let rpc = self.clone();
        thread::spawn(move || {
            rpc.stop_process(None, "debugger shut down");
            if let Err(error) = rpc.rpc_tx.send(DapRpc::Shutdown) {
                tracing::error!(?error, "stopping debugger worker");
            }
            *rpc.shutdown_complete.0.lock() = true;
            rpc.shutdown_complete.1.notify_all();
        });
    }

    pub fn wait_for_shutdown_until(&self, deadline: Instant) -> bool {
        let mut complete = self.shutdown_complete.0.lock();
        while !*complete {
            if self
                .shutdown_complete
                .1
                .wait_until(&mut complete, deadline)
                .timed_out()
            {
                return *complete;
            }
        }
        true
    }

    fn request_async<R: Request>(
        &self,
        params: R::Arguments,
        f: impl RpcCallback<R::Result, RpcError> + 'static,
    ) {
        self.request_async_for_generation::<R>(
            params,
            self.generation.load(Ordering::Acquire),
            f,
        );
    }

    fn request_async_for_generation<R: Request>(
        &self,
        params: R::Arguments,
        generation: u64,
        f: impl RpcCallback<R::Result, RpcError> + 'static,
    ) {
        self.request_common::<R>(
            params,
            generation,
            Instant::now() + REQUEST_TIMEOUT,
            ResponseHandler::Callback(Box::new(
                |result: Result<DapResponse, RpcError>| {
                    let result = match result {
                        Ok(resp) => {
                            if resp.success {
                                serde_json::from_value(resp.body.into()).map_err(
                                    |e| RpcError {
                                        code: 0,
                                        message: e.to_string(),
                                    },
                                )
                            } else {
                                Err(RpcError {
                                    code: 0,
                                    message: resp.message.unwrap_or_default(),
                                })
                            }
                        }
                        Err(e) => Err(e),
                    };
                    Box::new(f).call(result);
                },
            )),
        );
    }

    fn request<R: Request>(
        &self,
        params: R::Arguments,
    ) -> Result<R::Result, RpcError> {
        self.request_with_timeout::<R>(params, REQUEST_TIMEOUT)
    }

    fn request_with_timeout<R: Request>(
        &self,
        params: R::Arguments,
        timeout: Duration,
    ) -> Result<R::Result, RpcError> {
        self.request_for_generation::<R>(
            params,
            self.generation.load(Ordering::Acquire),
            timeout,
        )
    }

    fn request_for_generation<R: Request>(
        &self,
        params: R::Arguments,
        generation: u64,
        timeout: Duration,
    ) -> Result<R::Result, RpcError> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let deadline = Instant::now() + timeout;
        let sequence = self.request_common::<R>(
            params,
            generation,
            deadline,
            ResponseHandler::Chan(tx),
        );
        let resp = rx.recv_deadline(deadline).map_err(|error| {
            let pending = sequence
                .and_then(|sequence| self.server_pending.lock().remove(&sequence));
            match error {
                crossbeam_channel::RecvTimeoutError::Timeout => {
                    let error = Self::timeout_error(R::COMMAND);
                    if pending.is_some() {
                        self.report_timeout(&error);
                    }
                    error
                }
                crossbeam_channel::RecvTimeoutError::Disconnected => RpcError {
                    code: 0,
                    message: format!("debugger {}: {error}", R::COMMAND),
                },
            }
        })??;
        if resp.success {
            let resp: R::Result =
                serde_json::from_value(resp.body.into()).map_err(|e| RpcError {
                    code: 0,
                    message: e.to_string(),
                })?;
            Ok(resp)
        } else {
            Err(RpcError {
                code: 0,
                message: resp.message.unwrap_or_default(),
            })
        }
    }

    fn request_common<R: Request>(
        &self,
        arguments: R::Arguments,
        generation: u64,
        deadline: Instant,
        rh: ResponseHandler<DapResponse, RpcError>,
    ) -> Option<u64> {
        let seq = self.seq_counter.fetch_add(1, Ordering::Relaxed);
        let arguments = match serde_json::to_value(arguments) {
            Ok(arguments) => arguments,
            Err(error) => {
                rh.invoke(Err(RpcError {
                    code: 0,
                    message: error.to_string(),
                }));
                return None;
            }
        };
        let active = self.process.lock();
        let Some(process) = active.as_ref().filter(|process| {
            process.generation == generation
                && !self.shutting_down.load(Ordering::Acquire)
        }) else {
            drop(active);
            rh.invoke(Err(RpcError {
                code: 0,
                message: "debug adapter is not connected".into(),
            }));
            return None;
        };
        self.server_pending.lock().insert(
            seq,
            PendingRequest {
                command: R::COMMAND,
                deadline,
                response: rh,
            },
        );
        let sent = process.sender.send(DapPayload::Request(DapRequest {
            seq,
            command: R::COMMAND.to_string(),
            arguments: Some(arguments),
        }));
        drop(active);
        if let Err(error) = sent {
            let handler = self.server_pending.lock().remove(&seq);
            if let Some(handler) = handler {
                handler.response.invoke(Err(RpcError {
                    code: 0,
                    message: format!("debug adapter write failed: {error}"),
                }));
            }
        }
        Some(seq)
    }

    fn handle_server_response(&self, resp: DapResponse) {
        let handler = self.server_pending.lock().remove(&resp.request_seq);
        if let Some(pending) = handler {
            if pending.deadline <= Instant::now() {
                let error = Self::timeout_error(pending.command);
                self.report_timeout(&error);
                pending.response.invoke(Err(error));
            } else {
                pending.response.invoke(Ok(resp));
            }
        }
    }

    fn handle_server_message(
        &self,
        generation: u64,
        message_str: &str,
    ) -> Result<()> {
        if generation != self.generation.load(Ordering::Acquire) {
            return Ok(());
        }
        let payload = serde_json::from_str::<DapPayload>(message_str)?;
        match payload {
            DapPayload::Request(req) => {
                if let Err(err) =
                    self.rpc_tx.send(DapRpc::HostRequest(generation, req))
                {
                    tracing::error!("{:?}", err);
                }
            }
            DapPayload::Event(event) => {
                let state = match &event {
                    DapEvent::Stopped(_) => Some(DapSessionState::Stopped),
                    DapEvent::Continued(_) => Some(DapSessionState::Running),
                    DapEvent::Terminated(_) => Some(DapSessionState::Stopping),
                    _ => None,
                };
                if let Some(state) = state {
                    self.set_state(generation, None, state);
                }
                let revision = self.lifecycle.lock().revision;
                if let Err(err) = self
                    .rpc_tx
                    .send(DapRpc::HostEvent(generation, revision, event))
                {
                    tracing::error!("{:?}", err);
                }
            }
            DapPayload::Response(resp) => {
                self.handle_server_response(resp);
            }
        }
        Ok(())
    }

    fn launch(&self, config: &RunDebugConfig, generation: u64) -> Result<()> {
        let params = serde_json::json!({
            "program": config.program,
            "args": config.args,
            "cwd": config.cwd,
            "runInTerminal": true,
            "env": config.env
        });
        let _resp = self
            .request_for_generation::<Launch>(params, generation, REQUEST_TIMEOUT)
            .map_err(|e| anyhow!(e.message))?;
        Ok(())
    }

    pub fn stop(&self) {
        let (generation, starting) = {
            let mut lifecycle = self.lifecycle.lock();
            if !lifecycle.state.can_stop() {
                return;
            }
            let target = (
                lifecycle.generation,
                lifecycle.state == DapSessionState::Starting,
            );
            self.stop_requested.store(true, Ordering::Release);
            lifecycle.state = DapSessionState::Stopping;
            lifecycle.control_pending = false;
            lifecycle.revision += 1;
            self.core_rpc
                .dap_session_state(self.dap_id, DapSessionState::Stopping);
            target
        };
        if starting {
            let rpc = self.clone();
            thread::spawn(move || {
                rpc.stop_process(Some(generation), "debugger start cancelled");
            });
            return;
        }
        let rpc = self.clone();
        thread::spawn(move || {
            let result = if rpc.supports_terminate.load(Ordering::Acquire) {
                rpc.terminate(generation)
            } else {
                rpc.disconnect_generation(generation)
            };
            if let Err(error) = result {
                tracing::error!(?error, "stopping debugger");
            }
        });
    }

    pub fn restart(&self, breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>) {
        if let Err(err) = self.rpc_tx.send(DapRpc::Restart(breakpoints)) {
            tracing::error!("{:?}", err);
        }
    }

    pub fn disconnect(&self) -> Result<()> {
        let generation = self.generation.load(Ordering::Acquire);
        self.disconnect_generation(generation)
    }

    fn disconnect_generation(&self, generation: u64) -> Result<()> {
        self.set_state(generation, None, DapSessionState::Stopping);
        let result = self.request_for_generation::<Disconnect>(
            DisconnectArguments {
                restart: false,
                terminate_debuggee: true,
                suspend_debuggee: false,
            },
            generation,
            super::psp::SERVER_SHUTDOWN_TIMEOUT,
        );
        self.finish_process(Some(generation), "debugger disconnected", result.as_ref().err().map(|error| format!("Debugger disconnect failed: {}. Adapter stopped; check the target process.", error.message)));
        result.map_err(|error| anyhow!(error.message))
    }

    fn terminate(&self, generation: u64) -> Result<()> {
        self.set_state(generation, None, DapSessionState::Stopping);
        let result = self.request_for_generation::<Terminate>(
            (),
            generation,
            super::psp::SERVER_SHUTDOWN_TIMEOUT,
        );
        self.finish_process(Some(generation), "debugger terminated", result.as_ref().err().map(|error| format!("Debugger termination failed: {}. Adapter stopped; check the target process.", error.message)));
        result.map_err(|error| anyhow!(error.message))
    }

    pub fn set_breakpoints_async(
        &self,
        file: PathBuf,
        breakpoints: Vec<SourceBreakpoint>,
        f: impl RpcCallback<SetBreakpointsResponse, RpcError> + 'static,
    ) {
        let params = SetBreakpointsArguments {
            source: Source {
                path: Some(file),
                name: None,
                source_reference: None,
                presentation_hint: None,
                origin: None,
                sources: None,
                adapter_data: None,
                checksums: None,
            },
            breakpoints: Some(breakpoints),
            source_modified: Some(false),
        };
        self.request_async::<SetBreakpoints>(params, f);
    }

    pub fn set_breakpoints(
        &self,
        file: PathBuf,
        breakpoints: Vec<SourceBreakpoint>,
    ) -> Result<SetBreakpointsResponse> {
        let params = SetBreakpointsArguments {
            source: Source {
                path: Some(file),
                name: None,
                source_reference: None,
                presentation_hint: None,
                origin: None,
                sources: None,
                adapter_data: None,
                checksums: None,
            },
            breakpoints: Some(breakpoints),
            source_modified: Some(false),
        };
        let resp = self
            .request::<SetBreakpoints>(params)
            .map_err(|e| anyhow!(e.message))?;
        Ok(resp)
    }

    pub fn continue_thread(&self, thread_id: ThreadId) {
        self.step_request::<Continue>(ContinueArguments { thread_id });
    }

    pub fn pause_thread(&self, thread_id: ThreadId) {
        let generation = self.generation.load(Ordering::Acquire);
        let rpc = self.clone();
        self.request_async_for_generation::<Pause>(
            PauseArguments { thread_id },
            generation,
            move |result: Result<(), RpcError>| {
                if let Err(error) = result {
                    rpc.report_error(
                        generation,
                        format!("Debugger pause failed: {}", error.message),
                    );
                }
            },
        );
    }

    pub fn threads(&self) -> Result<ThreadsResponse> {
        let resp = self
            .request::<Threads>(())
            .map_err(|e| anyhow!(e.message))?;
        Ok(resp)
    }

    pub fn stack_trace(&self, thread_id: ThreadId) -> Result<StackTraceResponse> {
        let params = StackTraceArguments {
            thread_id,
            ..Default::default()
        };
        let resp = self
            .request::<StackTrace>(params)
            .map_err(|e| anyhow!(e.message))?;
        Ok(resp)
    }

    pub fn scopes(&self, frame_id: usize) -> Result<Vec<Scope>> {
        let args = ScopesArguments { frame_id };

        let response = self
            .request::<Scopes>(args)
            .map_err(|e| anyhow!(e.message))?;
        Ok(response.scopes)
    }

    pub fn scopes_async(
        &self,
        frame_id: usize,
        f: impl RpcCallback<ScopesResponse, RpcError> + 'static,
    ) {
        let args = ScopesArguments { frame_id };

        self.request_async::<Scopes>(args, f);
    }

    pub fn variables(&self, variables_reference: usize) -> Result<Vec<Variable>> {
        let args = VariablesArguments {
            variables_reference,
            filter: None,
            start: None,
            count: None,
            format: None,
        };

        let response = self
            .request::<Variables>(args)
            .map_err(|e| anyhow!(e.message))?;
        Ok(response.variables)
    }

    pub fn variables_async(
        &self,
        variables_reference: usize,
        f: impl RpcCallback<VariablesResponse, RpcError> + 'static,
    ) {
        let args = VariablesArguments {
            variables_reference,
            filter: None,
            start: None,
            count: None,
            format: None,
        };

        self.request_async::<Variables>(args, f);
    }

    pub fn next(&self, thread_id: ThreadId) {
        let args = NextArguments {
            thread_id,
            granularity: None,
        };

        self.step_request::<Next>(args);
    }

    pub fn step_in(&self, thread_id: ThreadId) {
        let args = StepInArguments {
            thread_id,
            target_id: None,
            granularity: None,
        };

        self.step_request::<StepIn>(args);
    }

    pub fn step_out(&self, thread_id: ThreadId) {
        let args = StepOutArguments {
            thread_id,
            granularity: None,
        };

        self.step_request::<StepOut>(args);
    }

    fn step_request<R: Request>(&self, args: R::Arguments) {
        let token = {
            let mut lifecycle = self.lifecycle.lock();
            if lifecycle.state != DapSessionState::Stopped
                || lifecycle.control_pending
            {
                return;
            }
            lifecycle.control_pending = true;
            lifecycle.token()
        };
        let rpc = self.clone();
        self.request_async_for_generation::<R>(
            args,
            token.0,
            move |result: Result<R::Result, RpcError>| match result {
                Ok(_) => {
                    rpc.set_state(token.0, Some(token.1), DapSessionState::Running);
                }
                Err(error) => {
                    let mut lifecycle = rpc.lifecycle.lock();
                    if lifecycle.token() == token {
                        lifecycle.control_pending = false;
                    }
                    drop(lifecycle);
                    rpc.report_error(
                        token.0,
                        format!("Debugger {} failed: {}", R::COMMAND, error.message),
                    );
                }
            },
        );
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use ahead_rpc::core::CoreRpcHandler;

    fn config(directory: &std::path::Path, command: &str) -> RunDebugConfig {
        serde_json::from_value(serde_json::json!({
            "name": "Disposable adapter", "program": "unused-target",
            "debug-adapter": "/bin/sh", "debug-adapter-args": ["-c", command],
            "cwd": directory,
        }))
        .expect("debug config")
    }

    fn client(directory: &std::path::Path, command: &str) -> DapClient {
        DapClient::new(
            DapServer {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), command.into()],
                cwd: Some(directory.to_owned()),
            },
            config(directory, command),
            HashMap::new(),
            PluginCatalogRpcHandler::new(CoreRpcHandler::new()),
        )
        .expect("client")
    }

    fn wait_until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !ready() {
            assert!(
                Instant::now() < deadline,
                "adapter fixture did not become ready"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn assert_reaped(pid: u32) {
        assert!(
            !Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("inspect fixture PID")
                .success(),
            "fixture adapter {pid} is still alive"
        );
    }

    #[test]
    fn retired_generation_cannot_submit_launch_or_control_to_replacement() {
        let directory = tempfile::tempdir().expect("workspace");
        let client = client(directory.path(), "exec sleep 60");
        client.start_process().expect("adapter");
        let rpc = &client.dap_rpc;
        let retired = rpc.generation.load(Ordering::Acquire);
        rpc.stop_process(Some(retired), "replace adapter");
        client.start_process().expect("replacement");
        let (sender, receiver) = crossbeam_channel::unbounded();
        rpc.process.lock().as_mut().expect("replacement").sender = sender;

        assert!(rpc.launch(&client.config, retired).is_err());
        let (reply, result) = crossbeam_channel::bounded(1);
        rpc.request_async_for_generation::<Continue>(
            ContinueArguments {
                thread_id: serde_json::from_value(serde_json::json!(17))
                    .expect("thread"),
            },
            retired,
            move |response: Result<
                ahead_rpc::dap_types::ContinueResponse,
                RpcError,
            >| {
                reply.send(response).expect("response");
            },
        );
        assert!(
            result
                .recv_timeout(Duration::from_secs(1))
                .expect("immediate rejection")
                .is_err()
        );
        assert!(rpc.server_pending.lock().is_empty());
        assert!(
            receiver.try_recv().is_err(),
            "retired requests must not reach replacement stdin"
        );
    }

    #[test]
    fn queued_terminal_requests_are_rejected_after_stop_without_forwarding_to_editor()
     {
        let directory = tempfile::tempdir().expect("workspace");
        let mut client = client(directory.path(), "exec sleep 60");
        client.start_process().expect("adapter");
        let rpc = client.dap_rpc.clone();
        rpc.stop();
        rpc.core_rpc.rx().try_iter().for_each(drop);
        let error = client
            .handle_host_request(
                &DapRequest {
                    seq: 1,
                    command: RunInTerminal::COMMAND.into(),
                    arguments: Some(serde_json::json!({"args":["/bin/sleep","60"]})),
                },
                rpc.generation.load(Ordering::Acquire),
            )
            .expect_err("stopped session must not spawn a terminal");
        assert!(
            error
                .to_string()
                .contains("no longer accepting terminal requests")
        );
        assert!(!rpc.core_rpc.rx().try_iter().any(|message| matches!(message,
            ahead_rpc::core::CoreRpc::Notification(notification) if matches!(*notification,
                ahead_rpc::core::CoreNotification::RunInTerminal { .. }))));
        wait_until(|| rpc.process.lock().is_none());
    }

    #[test]
    fn terminal_handoff_preserves_request_identity_and_shell_pid() {
        let directory = tempfile::tempdir().expect("workspace");
        let core = CoreRpcHandler::new();
        let mut client = DapClient::new(
            DapServer {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "exec sleep 60".into()],
                cwd: Some(directory.path().to_owned()),
            },
            config(directory.path(), "exec sleep 60"),
            HashMap::new(),
            PluginCatalogRpcHandler::new(core.clone()),
        )
        .expect("client");
        client.start_process().expect("adapter");
        let rpc = client.dap_rpc.clone();
        let generation = rpc.generation.load(Ordering::Acquire);
        let dap_id = client.config.dap_id;
        let worker = thread::spawn(move || {
            client.handle_host_request(
                &DapRequest {
                    seq: 71,
                    command: RunInTerminal::COMMAND.into(),
                    arguments: Some(serde_json::json!({
                        "args": ["/bin/echo", "hello"],
                        "cwd": "/tmp",
                        "env": {"LANG": "en_US.UTF-8"}
                    })),
                },
                generation,
            )
        });
        let forwarded = loop {
            let message = core
                .rx()
                .recv_timeout(Duration::from_secs(2))
                .expect("terminal notification");
            if let ahead_rpc::core::CoreRpc::Notification(notification) = message
                && let ahead_rpc::core::CoreNotification::RunInTerminal { request } =
                    *notification
            {
                break request;
            }
        };
        assert_eq!(forwarded.dap_id, dap_id);
        assert_eq!(forwarded.generation, generation);
        assert_eq!(forwarded.request_seq, 71);
        assert_eq!(forwarded.arguments.args, ["/bin/echo", "hello"]);
        rpc.answer_terminal_request(DebugTerminalResponse {
            dap_id,
            generation,
            request_seq: 70,
            shell_process_id: Some(111),
            error: None,
        })
        .expect("stale answer");
        rpc.answer_terminal_request(DebugTerminalResponse {
            dap_id,
            generation,
            request_seq: 71,
            shell_process_id: Some(222),
            error: None,
        })
        .expect("matching answer");
        let body = worker
            .join()
            .expect("terminal worker")
            .expect("DAP response");
        let response: RunInTerminalResponse =
            serde_json::from_value(body).expect("terminal response");
        assert_eq!(response.process_id, None);
        assert_eq!(response.shell_process_id, Some(222));
        rpc.shutdown();
        assert!(
            rpc.wait_for_shutdown_until(Instant::now() + Duration::from_secs(3))
        );
    }

    #[test]
    fn continue_and_pause_queue_without_waiting_for_the_adapter_and_report_errors() {
        let directory = tempfile::tempdir().expect("workspace");
        let client = client(directory.path(), "exec sleep 60");
        client.start_process().expect("adapter");
        let rpc = &client.dap_rpc;
        let generation = rpc.generation.load(Ordering::Acquire);
        rpc.set_state(generation, None, DapSessionState::Stopped);
        let thread_id =
            serde_json::from_value(serde_json::json!(17)).expect("thread");
        for (command, send) in [
            (
                "continue",
                DapRpcHandler::continue_thread as fn(&DapRpcHandler, ThreadId),
            ),
            (
                "pause",
                DapRpcHandler::pause_thread as fn(&DapRpcHandler, ThreadId),
            ),
        ] {
            rpc.core_rpc.rx().try_iter().for_each(drop);
            send(rpc, thread_id);
            let sequence = *rpc
                .server_pending
                .lock()
                .keys()
                .next()
                .expect("queued request");
            rpc.handle_server_response(DapResponse {
                seq: 99,
                request_seq: sequence,
                success: false,
                command: command.into(),
                message: Some("control rejected".into()),
                body: None,
            });
            assert!(rpc.server_pending.lock().is_empty());
            assert!(rpc.core_rpc.rx().try_iter().any(|message| matches!(message,
                ahead_rpc::core::CoreRpc::Notification(notification) if matches!(*notification,
                    ahead_rpc::core::CoreNotification::DapError { ref message, .. }
                        if message.contains(command) && message.contains("control rejected")))));
            assert_eq!(rpc.lifecycle.lock().state, DapSessionState::Stopped);
        }
    }

    #[test]
    fn step_and_continue_admit_only_one_control_until_reply() {
        let directory = tempfile::tempdir().expect("workspace");
        let client = client(directory.path(), "exec sleep 60");
        client.start_process().expect("adapter");
        let rpc = &client.dap_rpc;
        let generation = rpc.generation.load(Ordering::Acquire);
        rpc.set_state(generation, None, DapSessionState::Stopped);
        let thread_id =
            serde_json::from_value(serde_json::json!(17)).expect("thread");

        rpc.continue_thread(thread_id);
        let first = *rpc
            .server_pending
            .lock()
            .keys()
            .next()
            .expect("continue request");
        rpc.next(thread_id);
        assert_eq!(rpc.server_pending.lock().len(), 1);
        assert!(rpc.lifecycle.lock().control_pending);

        rpc.handle_server_response(DapResponse {
            seq: 99,
            request_seq: first,
            success: false,
            command: "continue".into(),
            message: Some("control rejected".into()),
            body: None,
        });
        assert!(!rpc.lifecycle.lock().control_pending);
        assert_eq!(rpc.lifecycle.lock().state, DapSessionState::Stopped);

        rpc.next(thread_id);
        let second = *rpc
            .server_pending
            .lock()
            .keys()
            .next()
            .expect("step request");
        rpc.continue_thread(thread_id);
        assert_eq!(rpc.server_pending.lock().len(), 1);
        rpc.handle_server_response(DapResponse {
            seq: 100,
            request_seq: second,
            success: true,
            command: "next".into(),
            message: None,
            body: None,
        });
        assert_eq!(rpc.lifecycle.lock().state, DapSessionState::Running);
        assert!(!rpc.lifecycle.lock().control_pending);
    }

    #[test]
    fn stop_interrupts_initialization_and_reports_cleanup_without_waiting_for_timeout()
     {
        let directory = tempfile::tempdir().expect("workspace");
        let rpc = DapClient::start(
            DapServer {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "exec sleep 60".into()],
                cwd: Some(directory.path().into()),
            },
            config(directory.path(), "exec sleep 60"),
            HashMap::new(),
            PluginCatalogRpcHandler::new(CoreRpcHandler::new()),
        )
        .expect("worker");
        wait_until(|| {
            rpc.server_pending
                .lock()
                .values()
                .any(|pending| pending.command == "initialize")
        });
        let pid = rpc.process.lock().as_ref().expect("adapter").child.id();
        rpc.stop();
        wait_until(|| rpc.lifecycle.lock().state == DapSessionState::Terminated);
        assert!(rpc.process.lock().is_none());
        assert_reaped(pid);
        wait_until(|| rpc.server_pending.lock().is_empty());
        assert!(!rpc.set_state(
            rpc.generation.load(Ordering::Acquire),
            None,
            DapSessionState::Running
        ));
        rpc.shutdown();
        assert!(
            rpc.wait_for_shutdown_until(Instant::now() + Duration::from_secs(3))
        );
    }

    #[test]
    fn missing_adapter_and_launch_rejection_report_failed_sessions() {
        let directory = tempfile::tempdir().expect("workspace");
        let rpc = DapClient::start(
            DapServer {
                program: directory
                    .path()
                    .join("missing-adapter")
                    .to_string_lossy()
                    .into_owned(),
                args: Vec::new(),
                cwd: None,
            },
            config(directory.path(), "unused"),
            HashMap::new(),
            PluginCatalogRpcHandler::new(CoreRpcHandler::new()),
        )
        .expect("worker");
        wait_until(
            || matches!(&rpc.lifecycle.lock().state, DapSessionState::Failed(message) if message.contains("Could not start debugger")),
        );
        assert!(rpc.process.lock().is_none());
        rpc.shutdown();
        assert!(
            rpc.wait_for_shutdown_until(Instant::now() + Duration::from_secs(3))
        );

        let client = client(directory.path(), "exec sleep 60");
        client.start_process().expect("adapter");
        let rpc = &client.dap_rpc;
        let pid = rpc.process.lock().as_ref().expect("adapter").child.id();
        client.launch();
        wait_until(|| {
            rpc.server_pending
                .lock()
                .values()
                .any(|pending| pending.command == "launch")
        });
        let sequence = *rpc
            .server_pending
            .lock()
            .keys()
            .next()
            .expect("launch request");
        rpc.handle_server_response(DapResponse {
            seq: 99,
            request_seq: sequence,
            success: false,
            command: "launch".into(),
            message: Some("target does not exist".into()),
            body: None,
        });
        wait_until(
            || matches!(&rpc.lifecycle.lock().state, DapSessionState::Failed(message) if message.contains("target does not exist")),
        );
        assert!(rpc.process.lock().is_none());
        assert_reaped(pid);
    }

    #[test]
    fn step_errors_are_visible_and_late_acknowledgements_cannot_clear_new_stops() {
        let directory = tempfile::tempdir().expect("workspace");
        let mut client = client(directory.path(), "exec sleep 60");
        client.start_process().expect("adapter");
        let rpc = client.dap_rpc.clone();
        let generation = rpc.generation.load(Ordering::Acquire);
        let thread_id =
            serde_json::from_value(serde_json::json!(17)).expect("thread ID");
        rpc.set_state(generation, None, DapSessionState::Stopped);
        rpc.next(thread_id);
        let sequence = *rpc.server_pending.lock().keys().next().expect("step");
        rpc.handle_server_message(generation, r#"{"type":"event","seq":1,"event":"stopped","body":{"reason":"breakpoint","threadId":17}}"#).expect("new breakpoint");
        let token = rpc.lifecycle.lock().token();
        rpc.handle_server_response(DapResponse {
            seq: 99,
            request_seq: sequence,
            success: true,
            command: "next".into(),
            message: None,
            body: None,
        });
        assert_eq!(
            rpc.lifecycle.lock().state,
            DapSessionState::Stopped,
            "late step acknowledgement must not clear a new breakpoint"
        );
        rpc.core_rpc.rx().try_iter().for_each(drop);
        rpc.step_in(thread_id);
        let sequence = *rpc.server_pending.lock().keys().next().expect("step");
        rpc.handle_server_response(DapResponse {
            seq: 100,
            request_seq: sequence,
            success: false,
            command: "stepIn".into(),
            message: Some("cannot step here".into()),
            body: None,
        });
        assert!(rpc.core_rpc.rx().try_iter().any(|message| matches!(message,
            ahead_rpc::core::CoreRpc::Notification(notification) if matches!(*notification,
                ahead_rpc::core::CoreNotification::DapError { ref message, .. } if message.contains("cannot step here")))));
        assert_eq!(rpc.lifecycle.lock().state, DapSessionState::Stopped);
        rpc.set_state(generation, None, DapSessionState::Running);
        rpc.core_rpc.rx().try_iter().for_each(drop);
        let stopped = serde_json::from_value(
            serde_json::json!({"reason":"breakpoint","threadId":17}),
        )
        .expect("stop");
        client
            .handle_host_event(&DapEvent::Stopped(stopped), token)
            .expect("delayed metadata");
        assert!(
            rpc.core_rpc.rx().try_recv().is_err(),
            "stale stack response must not revive a stop"
        );
    }

    #[test]
    fn disconnect_requests_debuggee_termination_and_reaps_after_acknowledgement() {
        let directory = tempfile::tempdir().expect("workspace");
        let client = client(directory.path(), "exec sleep 60");
        client.start_process().expect("adapter");
        let rpc = client.dap_rpc.clone();
        let (sender, receiver) = crossbeam_channel::unbounded();
        let pid = {
            let mut process = rpc.process.lock();
            let process = process.as_mut().expect("adapter");
            process.sender = sender;
            process.child.id()
        };
        let worker = thread::spawn({
            let rpc = rpc.clone();
            move || rpc.disconnect()
        });
        let DapPayload::Request(request) = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("disconnect")
        else {
            panic!("request")
        };
        assert_eq!(request.command, "disconnect");
        assert_eq!(
            request.arguments,
            Some(
                serde_json::json!({"restart":false,"terminateDebuggee":true,"suspendDebuggee":false})
            )
        );
        assert_eq!(rpc.lifecycle.lock().state, DapSessionState::Stopping);
        rpc.handle_server_response(DapResponse {
            seq: 99,
            request_seq: request.seq,
            success: true,
            command: request.command,
            message: None,
            body: None,
        });
        worker.join().expect("worker").expect("disconnect response");
        assert_eq!(rpc.lifecycle.lock().state, DapSessionState::Terminated);
        assert!(rpc.process.lock().is_none());
        assert_reaped(pid);
    }

    #[test]
    fn shutdown_reaps_adapter_and_releases_reentrant_pending_callbacks() {
        let directory = tempfile::tempdir().expect("workspace");
        let client = client(directory.path(), "exec sleep 60");
        client.start_process().expect("start adapter");
        let rpc = client.dap_rpc.clone();
        let pid = rpc.process.lock().as_ref().expect("adapter").child.id();
        let (reply, result) = crossbeam_channel::bounded(1);
        let callback_rpc = rpc.clone();
        rpc.request_async::<Threads>(
            (),
            move |response: Result<ThreadsResponse, RpcError>| {
                assert!(response.is_err());
                assert!(callback_rpc.server_pending.lock().is_empty());
                assert!(
                    callback_rpc.threads().is_err(),
                    "new requests must fail without waiting"
                );
                reply.send(()).expect("callback result");
            },
        );
        rpc.shutdown();
        rpc.shutdown();
        assert!(
            rpc.wait_for_shutdown_until(Instant::now() + Duration::from_secs(3))
        );
        result
            .recv_timeout(Duration::from_secs(1))
            .expect("pending callback settled");
        assert_reaped(pid);
        assert!(
            client.start_process().is_err(),
            "shutdown prevents late process creation"
        );
    }

    #[test]
    fn stderr_is_drained_and_retired_connections_cannot_stop_replacements() {
        let directory = tempfile::tempdir().expect("workspace");
        let client = client(
            directory.path(),
            "dd if=/dev/zero bs=1024 count=128 >&2 2>/dev/null && printf ready > stderr-drained; exec sleep 60",
        );
        client.start_process().expect("start adapter");
        let rpc = client.dap_rpc.clone();
        let old_generation = rpc.generation.load(Ordering::Acquire);
        let old_pid = rpc.process.lock().as_ref().expect("adapter").child.id();
        wait_until(|| directory.path().join("stderr-drained").exists());
        rpc.stop_process(Some(old_generation), "test disconnect");
        assert_reaped(old_pid);
        client.start_process().expect("replacement adapter");
        let replacement =
            rpc.process.lock().as_ref().expect("replacement").child.id();
        rpc.stop_process(Some(old_generation), "late old reader error");
        assert_eq!(
            rpc.process
                .lock()
                .as_ref()
                .expect("replacement survives")
                .child
                .id(),
            replacement
        );
        rpc.shutdown();
        assert!(
            rpc.wait_for_shutdown_until(Instant::now() + Duration::from_secs(3))
        );
        assert_reaped(replacement);
    }

    #[test]
    fn adapter_eof_fails_pending_requests_without_waiting_for_timeout() {
        let directory = tempfile::tempdir().expect("workspace");
        let client = client(directory.path(), "read header; exit 7");
        client.start_process().expect("start adapter");
        let rpc = client.dap_rpc.clone();
        let pid = rpc.process.lock().as_ref().expect("adapter").child.id();
        let (reply, result) = crossbeam_channel::bounded(1);
        rpc.request_async::<Threads>(
            (),
            move |response: Result<ThreadsResponse, RpcError>| {
                reply.send(response.is_err()).expect("request result");
            },
        );
        assert!(
            result
                .recv_timeout(Duration::from_secs(3))
                .expect("EOF must settle request")
        );
        assert!(rpc.server_pending.lock().is_empty());
        assert_reaped(pid);
    }

    #[test]
    fn request_timeouts_settle_once_without_stopping_adapter_or_holding_locks() {
        let directory = tempfile::tempdir().expect("workspace");
        let client = client(directory.path(), "exec sleep 60");
        client.start_process().expect("start adapter");
        let rpc = client.dap_rpc.clone();
        let pid = rpc.process.lock().as_ref().expect("adapter").child.id();
        let (expired_sender, expired_receiver) = crossbeam_channel::bounded(1);
        rpc.core_rpc.rx().try_iter().for_each(drop);
        let (next_sender, next_receiver) = crossbeam_channel::bounded(1);
        let callback_rpc = rpc.clone();
        let expired_sequence = rpc.seq_counter.load(Ordering::Relaxed);
        rpc.request_async::<Threads>(
            (),
            move |response: Result<ThreadsResponse, RpcError>| {
                let error = response.expect_err("silent request must time out");
                assert!(error.message.contains("threads request timed out"));
                assert!(callback_rpc.server_pending.lock().is_empty());
                assert!(callback_rpc.process.lock().is_some());
                callback_rpc.request_async::<Threads>(
                    (),
                    move |response: Result<ThreadsResponse, RpcError>| {
                        next_sender.send(response).expect("next response");
                    },
                );
                expired_sender.send(()).expect("report timeout");
            },
        );
        rpc.server_pending
            .lock()
            .get_mut(&expired_sequence)
            .expect("pending request")
            .deadline = Instant::now();
        expired_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("process monitor expires async requests");
        let notification = rpc
            .core_rpc
            .rx()
            .recv_timeout(Duration::from_secs(1))
            .expect("user-facing timeout");
        assert!(
            matches!(notification, ahead_rpc::core::CoreRpc::Notification(notification)
            if matches!(notification.as_ref(), ahead_rpc::core::CoreNotification::ShowMessage { message, .. }
                if message.message.contains("threads request timed out")))
        );

        let next_sequence = *rpc
            .server_pending
            .lock()
            .keys()
            .next()
            .expect("retry request");
        let response = |request_seq| DapResponse {
            seq: 99,
            request_seq,
            success: true,
            command: Threads::COMMAND.into(),
            message: None,
            body: Some(serde_json::json!({ "threads": [] })),
        };
        rpc.handle_server_response(response(expired_sequence));
        assert!(
            next_receiver.try_recv().is_err(),
            "late reply must not claim a newer callback"
        );
        rpc.handle_server_response(response(next_sequence));
        assert!(
            next_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("retry response")
                .is_ok()
        );
        rpc.expire_requests(Instant::now() + REQUEST_TIMEOUT);
        assert!(rpc.server_pending.lock().is_empty());
        assert!(expired_receiver.try_recv().is_err());

        let error = rpc
            .request_with_timeout::<Threads>((), Duration::from_millis(5))
            .expect_err("sync request deadline");
        assert!(error.message.contains("threads request timed out"));
        assert!(rpc.server_pending.lock().is_empty());
        rpc.shutdown();
        assert!(
            rpc.wait_for_shutdown_until(Instant::now() + Duration::from_secs(3))
        );
        assert_reaped(pid);
    }

    #[test]
    fn adapter_exit_fails_requests_even_when_a_descendant_holds_the_pipe_open() {
        let directory = tempfile::tempdir().expect("workspace");
        let client = client(
            directory.path(),
            "read header; (sleep 2; printf done > descendant.done) & exit 7",
        );
        client.start_process().expect("start adapter");
        let rpc = client.dap_rpc.clone();
        let pid = rpc.process.lock().as_ref().expect("adapter").child.id();
        let (sender, receiver) = crossbeam_channel::bounded(1);
        rpc.request_async::<Threads>(
            (),
            move |response: Result<ThreadsResponse, RpcError>| {
                sender.send(response).expect("exit result");
            },
        );
        let result = receiver.recv_timeout(Duration::from_secs(1));
        // Let the short-lived fixture descendant finish before removing its directory.
        wait_until(|| directory.path().join("descendant.done").exists());
        let error = result
            .expect("must detect exit before pipe EOF")
            .expect_err("adapter exited");
        assert!(error.message.contains("debug adapter exited"));
        assert!(rpc.server_pending.lock().is_empty());
        assert_reaped(pid);
    }

    #[test]
    fn catalog_shutdown_owns_adapters_that_are_still_initializing() {
        use crate::plugin::{PluginCatalogNotification, catalog::PluginCatalog};
        let directory = tempfile::tempdir().expect("workspace");
        let plugin_rpc = PluginCatalogRpcHandler::new(CoreRpcHandler::new());
        let mut catalog =
            PluginCatalog::new(Some(directory.path().to_owned()), plugin_rpc);
        catalog.handle_notification(PluginCatalogNotification::DapStart {
            config: config(directory.path(), "echo $$ > adapter.pid; exec sleep 60"),
            breakpoints: HashMap::new(),
        });
        let pid_file = directory.path().join("adapter.pid");
        wait_until(|| {
            std::fs::read_to_string(&pid_file)
                .is_ok_and(|text| text.trim().parse::<u32>().is_ok())
        });
        let pid: u32 = std::fs::read_to_string(pid_file)
            .expect("PID file")
            .trim()
            .parse()
            .expect("PID");
        let started = Instant::now();
        catalog.handle_notification(PluginCatalogNotification::Shutdown);
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "must not wait for initialization timeout"
        );
        assert_reaped(pid);
    }
}
