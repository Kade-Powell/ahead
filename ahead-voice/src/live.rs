//! Realtime speech-provider transport for AHEAD voice calls.

mod realtime_venus;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc as tokio_mpsc};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest, http::header},
};

use crate::{VoiceCapture, VoiceEvent};

const LIVE_URL: &str = "wss://api.openai.com/v1/live/sessions";
const MAX_APPEND_BYTES: usize = 480;
const MAX_TRANSCRIPT_DELTA_BYTES: usize = 4_096;
const MAX_AUDIO_DELTA_BASE64_BYTES: usize = 128 * 1024;
const SELF_HOSTED_INPUT_RATE: u32 = 16_000;
const SELF_HOSTED_PCM16_CHUNK_BYTES: usize = SELF_HOSTED_INPUT_RATE as usize * 2 / 5;
const NEMOTRON_INPUT_CHUNK_SAMPLES: usize =
    SELF_HOSTED_INPUT_RATE as usize * 80 / 1000;
const PLAYBACK_SAMPLE_RATE_HZ: u32 = 24_000;
const MAX_PROVIDER_SAMPLE_RATE_HZ: u32 = 96_000;
const INPUT_MUTE_ACK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelfHostedRealtimeProfile {
    #[default]
    VllmOmniPcm16,
    NemotronVoiceChat,
    RealtimeVenus,
}

impl SelfHostedRealtimeProfile {
    fn input_audio_format(self) -> &'static str {
        match self {
            Self::VllmOmniPcm16 => "pcm16",
            Self::NemotronVoiceChat => "pcm_f32le",
            Self::RealtimeVenus => "pcm16",
        }
    }

    fn output_sample_rate_hz(self) -> u32 {
        match self {
            Self::VllmOmniPcm16 => PLAYBACK_SAMPLE_RATE_HZ,
            Self::NemotronVoiceChat => 22_050,
            Self::RealtimeVenus => PLAYBACK_SAMPLE_RATE_HZ,
        }
    }

    fn input_chunk_ms(self) -> u64 {
        match self {
            Self::VllmOmniPcm16 => 200,
            Self::NemotronVoiceChat => 80,
            Self::RealtimeVenus => 200,
        }
    }

    fn input_chunk_bytes(self) -> usize {
        match self {
            Self::VllmOmniPcm16 => SELF_HOSTED_PCM16_CHUNK_BYTES,
            Self::NemotronVoiceChat => NEMOTRON_INPUT_CHUNK_SAMPLES * 4,
            Self::RealtimeVenus => SELF_HOSTED_PCM16_CHUNK_BYTES,
        }
    }
}

#[derive(Clone)]
pub struct OpenAiLiveConfig {
    pub api_key: String,
    pub instructions: String,
    pub on_barge_in: Option<crate::BargeInHandler>,
}

impl std::fmt::Debug for OpenAiLiveConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenAiLiveConfig")
            .field("api_key", &"[REDACTED]")
            .field("instructions", &self.instructions)
            .field(
                "on_barge_in",
                &self.on_barge_in.as_ref().map(|_| "[HANDLER]"),
            )
            .finish()
    }
}

#[derive(Clone)]
pub struct SelfHostedRealtimeConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub profile: SelfHostedRealtimeProfile,
    pub instructions: String,
    pub on_barge_in: Option<crate::BargeInHandler>,
}

impl std::fmt::Debug for SelfHostedRealtimeConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SelfHostedRealtimeConfig")
            .field("endpoint", &self.endpoint)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("profile", &self.profile)
            .field("instructions", &self.instructions)
            .field(
                "on_barge_in",
                &self.on_barge_in.as_ref().map(|_| "[HANDLER]"),
            )
            .finish()
    }
}

#[derive(Clone)]
enum LiveProviderConfig {
    OpenAi(OpenAiLiveConfig),
    SelfHosted(SelfHostedRealtimeConfig),
}

impl LiveProviderConfig {
    fn endpoint(&self) -> Result<String, String> {
        match self {
            Self::OpenAi(_) => Ok(LIVE_URL.to_string()),
            Self::SelfHosted(config)
                if config.profile == SelfHostedRealtimeProfile::RealtimeVenus =>
            {
                realtime_venus::base_url(&config.endpoint)
            }
            Self::SelfHosted(config) => {
                self_hosted_realtime_url(&config.endpoint, &config.model)
            }
        }
    }

    fn api_key(&self) -> &str {
        match self {
            Self::OpenAi(config) => config.api_key.trim(),
            Self::SelfHosted(config) => config.api_key.trim(),
        }
    }

    fn instructions(&self) -> &str {
        match self {
            Self::OpenAi(config) => &config.instructions,
            Self::SelfHosted(config) => &config.instructions,
        }
    }

    fn default_output_sample_rate_hz(&self) -> u32 {
        match self {
            Self::OpenAi(_) => PLAYBACK_SAMPLE_RATE_HZ,
            Self::SelfHosted(config) => config.profile.output_sample_rate_hz(),
        }
    }

    fn is_self_hosted(&self) -> bool {
        matches!(self, Self::SelfHosted(_))
    }

    fn display_name(&self) -> &'static str {
        match self {
            Self::OpenAi(_) => "GPT-Live",
            Self::SelfHosted(config)
                if config.profile == SelfHostedRealtimeProfile::RealtimeVenus =>
            {
                "Realtime-Venus"
            }
            Self::SelfHosted(_) => "self-hosted voice server",
        }
    }

    fn on_barge_in(&self) -> Option<&crate::BargeInHandler> {
        match self {
            Self::OpenAi(config) => config.on_barge_in.as_ref(),
            Self::SelfHosted(config) => config.on_barge_in.as_ref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveCallEvent {
    Ready {
        session_id: String,
    },
    Capabilities {
        supports_barge_in: bool,
    },
    MicrophoneReady,
    SpeechStarted {
        generation: u64,
    },
    SpeechEnded {
        generation: u64,
    },
    PlaybackStarted,
    PlaybackEnded,
    InputTranscriptDelta {
        text: String,
        start_ms: u64,
        end_ms: u64,
    },
    OutputTranscriptDelta {
        text: String,
        start_ms: u64,
        end_ms: u64,
    },
    InputMuted {
        muted: bool,
    },
    DelegationCreated {
        delegation_id: String,
        offset_ms: u64,
        request: Option<String>,
    },
    UsageUpdated {
        duration_seconds: u64,
    },
    SessionFinalized {
        duration_seconds: Option<u64>,
    },
    AudioOutput(Vec<u8>),
    Closed,
    Error(String),
}

enum LiveCallCommand {
    Audio(Vec<u8>),
    SetInputMuted(bool),
    UpdateEditorContext(String),
    AppendThinking {
        delegation_id: Option<String>,
        content: String,
    },
    AppendCommentary {
        delegation_id: Option<String>,
        content: String,
    },
}

/// Owns a remote voice session. Audio messages and returned events use bounded
/// queues; callers must treat backpressure as a failed call, not drop speech.
pub struct LiveVoiceCall {
    commands: tokio_mpsc::Sender<LiveCallCommand>,
    events: Receiver<LiveCallEvent>,
    ready: Arc<AtomicBool>,
    event_overflowed: Arc<AtomicBool>,
    close: Arc<Notify>,
    input_muted: Arc<AtomicBool>,
    output_interrupted: Arc<AtomicBool>,
    output_playing: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl LiveVoiceCall {
    pub fn connect_openai(config: OpenAiLiveConfig) -> Result<Self, String> {
        Self::connect_with(LiveProviderConfig::OpenAi(config))
    }

    pub fn connect_self_hosted(
        config: SelfHostedRealtimeConfig,
    ) -> Result<Self, String> {
        Self::connect_with(LiveProviderConfig::SelfHosted(config))
    }

    fn connect_with(config: LiveProviderConfig) -> Result<Self, String> {
        if matches!(&config, LiveProviderConfig::OpenAi(_))
            && config.api_key().is_empty()
        {
            return Err(
                "Add an OpenAI API key in AHEAD settings before starting a call."
                    .into(),
            );
        }
        if config.instructions().chars().count() > 8_000 {
            return Err("Voice instructions are too long.".into());
        }
        if let LiveProviderConfig::SelfHosted(config) = &config {
            if config.model.trim().is_empty() {
                return Err(
                    "Enter the self-hosted voice model ID in AHEAD settings.".into(),
                );
            }
            let target =
                if config.profile == SelfHostedRealtimeProfile::RealtimeVenus {
                    realtime_venus::base_url(&config.endpoint)?
                } else {
                    self_hosted_realtime_url(&config.endpoint, &config.model)?
                };
            let remote = !matches!(
                url::Url::parse(&target)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_string))
                    .as_deref(),
                Some("localhost" | "127.0.0.1" | "::1")
            );
            if remote && config.api_key.trim().is_empty() {
                return Err(
                    "Set an API key for a remote self-hosted voice server.".into()
                );
            }
        }

        let (command_sender, command_receiver) = tokio_mpsc::channel(32);
        let (event_sender, events) = mpsc::sync_channel(128);
        let ready = Arc::new(AtomicBool::new(false));
        let event_overflowed = Arc::new(AtomicBool::new(false));
        let close = Arc::new(Notify::new());
        let input_muted = Arc::new(AtomicBool::new(false));
        let output_interrupted = Arc::new(AtomicBool::new(false));
        let output_playing = Arc::new(AtomicBool::new(false));
        let worker_ready = ready.clone();
        let worker_overflowed = event_overflowed.clone();
        let worker_close = close.clone();
        let worker_input_muted = input_muted.clone();
        let worker_output_interrupted = output_interrupted.clone();
        let worker_output_playing = output_playing.clone();
        let worker = thread::Builder::new()
            .name("ahead-gpt-live".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let terminal_events = event_sender.clone();
                let terminal_overflowed = worker_overflowed.clone();
                match runtime {
                    Ok(runtime) => runtime.block_on(async move {
                        match config {
                            LiveProviderConfig::SelfHosted(config)
                                if config.profile
                                    == SelfHostedRealtimeProfile::RealtimeVenus =>
                            {
                                realtime_venus::run(
                                    config,
                                    command_receiver,
                                    event_sender,
                                    worker_ready,
                                    worker_overflowed,
                                    worker_close,
                                    worker_input_muted,
                                    worker_output_interrupted,
                                    worker_output_playing,
                                )
                                .await;
                            }
                            config => {
                                Box::pin(run_live(
                                    config,
                                    command_receiver,
                                    event_sender,
                                    worker_ready,
                                    worker_overflowed,
                                    worker_close,
                                    worker_input_muted,
                                    worker_output_interrupted,
                                    worker_output_playing,
                                ))
                                .await;
                            }
                        }
                    }),
                    Err(error) => emit_event(
                        &event_sender,
                        &worker_overflowed,
                        LiveCallEvent::Error(format!(
                            "Could not start voice runtime: {error}"
                        )),
                    ),
                }
                emit_event(
                    &terminal_events,
                    &terminal_overflowed,
                    LiveCallEvent::Closed,
                );
            })
            .map_err(|error| format!("Could not start voice session: {error}"))?;

        Ok(Self {
            commands: command_sender,
            events,
            ready,
            event_overflowed,
            close,
            input_muted,
            output_interrupted,
            output_playing,
            worker: Some(worker),
        })
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    pub fn interrupt_output(&self) -> bool {
        if !self.output_playing.load(Ordering::Acquire) {
            return false;
        }
        self.output_interrupted.store(true, Ordering::Release);
        true
    }

    pub fn interruption_handler(&self) -> crate::BargeInHandler {
        let output_interrupted = self.output_interrupted.clone();
        let output_playing = self.output_playing.clone();
        Arc::new(move || {
            if output_playing.load(Ordering::Acquire) {
                output_interrupted.store(true, Ordering::Release);
            }
        })
    }

    pub fn append_audio(&self, pcm16le: Vec<u8>) -> Result<(), String> {
        if pcm16le.is_empty() || !pcm16le.len().is_multiple_of(2) {
            return Err("Voice audio must contain complete PCM16 samples.".into());
        }
        if pcm16le.len() > 96 * 1024 {
            return Err("Voice audio chunk exceeds the 96 KiB limit.".into());
        }
        if !self.is_ready() {
            return Err("Voice session is still connecting.".into());
        }
        self.try_send(LiveCallCommand::Audio(pcm16le))
    }

    pub fn set_input_muted(&self, muted: bool) -> Result<(), String> {
        if !self.is_ready() {
            return Err("Voice session is still connecting.".into());
        }
        if muted {
            self.input_muted.store(true, Ordering::Release);
        }
        self.try_send(LiveCallCommand::SetInputMuted(muted))
    }

    pub fn update_editor_context(&self, content: String) -> Result<(), String> {
        if content.trim().is_empty() || content.len() > 4_096 {
            return Err(
                "Editor context updates must contain 1–4096 UTF-8 bytes.".into()
            );
        }
        if !self.is_ready() {
            return Err("Voice session is still connecting.".into());
        }
        self.try_send(LiveCallCommand::UpdateEditorContext(content))
    }

    pub fn append_thinking(
        &self,
        delegation_id: Option<String>,
        content: String,
    ) -> Result<(), String> {
        self.append_text(LiveCallCommand::AppendThinking {
            delegation_id,
            content,
        })
    }

    pub fn append_commentary(
        &self,
        delegation_id: Option<String>,
        content: String,
    ) -> Result<(), String> {
        self.append_text(LiveCallCommand::AppendCommentary {
            delegation_id,
            content,
        })
    }

    fn append_text(&self, command: LiveCallCommand) -> Result<(), String> {
        if !self.is_ready() {
            return Err("Voice session is still connecting.".into());
        }
        let (LiveCallCommand::AppendThinking { content, .. }
        | LiveCallCommand::AppendCommentary { content, .. }) = &command
        else {
            return Err("Invalid voice context update.".into());
        };
        if content.trim().is_empty() || content.len() > MAX_APPEND_BYTES {
            return Err(
                "Voice context updates must contain 1–480 UTF-8 bytes.".into()
            );
        }
        self.try_send(command)
    }

    fn try_send(&self, command: LiveCallCommand) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                tokio_mpsc::error::TrySendError::Full(_) => {
                    "Voice transport is backpressured; end the call and try again."
                        .into()
                }
                tokio_mpsc::error::TrySendError::Closed(_) => {
                    "Voice session has ended.".into()
                }
            })
    }

    pub fn take_events(&self) -> Vec<LiveCallEvent> {
        let mut events = self.events.try_iter().collect::<Vec<_>>();
        if self.event_overflowed.swap(false, Ordering::AcqRel) {
            events.push(LiveCallEvent::Error(
                "Voice event queue overflowed; end the call and reconnect.".into(),
            ));
        }
        events
    }

    pub fn close(&mut self) {
        if self.worker.is_none() {
            return;
        }
        self.close.notify_one();
        self.worker = None;
    }
}

impl Drop for LiveVoiceCall {
    fn drop(&mut self) {
        self.close();
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "One serialized runtime owns this call state."
)]
async fn run_live(
    config: LiveProviderConfig,
    mut commands: tokio_mpsc::Receiver<LiveCallCommand>,
    events: SyncSender<LiveCallEvent>,
    ready: Arc<AtomicBool>,
    overflowed: Arc<AtomicBool>,
    close: Arc<Notify>,
    input_muted: Arc<AtomicBool>,
    output_interrupted: Arc<AtomicBool>,
    output_playing: Arc<AtomicBool>,
) {
    let endpoint = match config.endpoint() {
        Ok(endpoint) => endpoint,
        Err(error) => {
            emit_event(&events, &overflowed, LiveCallEvent::Error(error));
            return;
        }
    };
    let mut request = match endpoint.into_client_request() {
        Ok(request) => request,
        Err(error) => {
            emit_event(
                &events,
                &overflowed,
                LiveCallEvent::Error(error.to_string()),
            );
            return;
        }
    };
    if !config.api_key().is_empty() {
        let authorization = match format!("Bearer {}", config.api_key()).parse() {
            Ok(value) => value,
            Err(error) => {
                emit_event(
                    &events,
                    &overflowed,
                    LiveCallEvent::Error(format!(
                        "Invalid voice server API key: {error}"
                    )),
                );
                return;
            }
        };
        request
            .headers_mut()
            .insert(header::AUTHORIZATION, authorization);
    }

    let connection = tokio::select! {
        () = close.notified() => return,
        result = tokio::time::timeout(Duration::from_secs(15), connect_async(request)) => result,
    };
    let (mut socket, _) = match connection {
        Ok(Ok(connection)) => connection,
        Ok(Err(error)) => {
            emit_event(
                &events,
                &overflowed,
                LiveCallEvent::Error(format!(
                    "Could not connect to {}: {error}",
                    config.display_name()
                )),
            );
            return;
        }
        Err(_) => {
            emit_event(
                &events,
                &overflowed,
                LiveCallEvent::Error(format!(
                    "Timed out connecting to {}.",
                    config.display_name()
                )),
            );
            return;
        }
    };

    let start = match &config {
        LiveProviderConfig::OpenAi(config) => json!({
            "type": "session.start",
            "event_id": format!("start_{}", uuid::Uuid::new_v4()),
            "session": {
                "model": "gpt-live-1",
                "instructions": config.instructions,
                "delegation": { "type": "client" },
                "audio": {
                    "format": { "type": "audio/pcm", "rate": 24000 },
                    "output": { "voice": "marin" }
                },
                "store": false
            }
        }),
        LiveProviderConfig::SelfHosted(config) => self_hosted_session_update(config),
    };
    if let Err(error) = send_json(&mut socket, start).await {
        emit_event(
            &events,
            &overflowed,
            LiveCallEvent::Error(format!(
                "Could not configure {} session: {error}",
                config.display_name()
            )),
        );
        return;
    }

    let mut session_ready = false;
    let mut close_deadline = None;
    let mut capture = None;
    let mut suppress_output = false;
    let mut openai_playback_gate = OpenAiPlaybackGate::default();
    let mut self_hosted_response_active = false;
    let mut self_hosted_barge_in_supported = false;
    let mut active_response_id: Option<String> = None;
    let mut active_output_item_id: Option<String> = None;
    let mut pending_input_mute: Option<(String, bool, tokio::time::Instant)> = None;
    let mut playback_tracker = SelfHostedPlaybackTracker::default();
    let on_barge_in = config.on_barge_in().cloned();
    let mut started_at = tokio::time::Instant::now();
    let mut pending_self_hosted_audio = SelfHostedAudioBuffer::default();
    let mut capture_poller = tokio::time::interval(Duration::from_millis(10));
    capture_poller.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if let Some(close_deadline) = close_deadline {
            tokio::select! {
                () = tokio::time::sleep_until(close_deadline) => {
                    emit_event(&events, &overflowed, LiveCallEvent::Error("GPT-Live did not confirm session closure.".into()));
                    break;
                }
                () = close.notified() => {
                    break;
                }
                message = socket.next() => {
                    if !receive_message(message, &mut socket, &config, &mut started_at, &events, &overflowed, &ready, &mut session_ready, &mut capture, &mut suppress_output, &mut openai_playback_gate, &mut self_hosted_response_active, &mut self_hosted_barge_in_supported, &mut active_response_id, &mut active_output_item_id, &mut pending_input_mute, &input_muted, &mut playback_tracker, &output_playing).await {
                        break;
                    }
                }
            }
        } else {
            let mute_ack_deadline = pending_input_mute
                .as_ref()
                .map(|(_, _, deadline)| *deadline);
            tokio::select! {
                () = async {
                    if let Some(deadline) = mute_ack_deadline {
                        tokio::time::sleep_until(deadline).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                }, if mute_ack_deadline.is_some() => {
                    emit_event(
                        &events,
                        &overflowed,
                        LiveCallEvent::Error("GPT-Live did not confirm the microphone state; the call was stopped.".into()),
                    );
                    break;
                }
                command = commands.recv() => {
                    let Some(command) = command else { break; };
                    match command {
                        LiveCallCommand::Audio(audio) if session_ready => {
                            if let Err(error) = send_audio(
                                &config,
                                &mut socket,
                                audio,
                                &mut pending_self_hosted_audio,
                            ).await {
                                emit_event(&events, &overflowed, LiveCallEvent::Error(format!("Voice audio send failed: {error}")));
                                break;
                            }
                        }
                        LiveCallCommand::Audio(_) => {
                            emit_event(&events, &overflowed, LiveCallEvent::Error("Voice audio arrived before GPT-Live was ready.".into()));
                        }
                        LiveCallCommand::SetInputMuted(muted) if session_ready => {
                            pending_self_hosted_audio.clear();
                            let mute_update = if config.is_self_hosted() {
                                input_muted.store(muted, Ordering::Release);
                                if let Some(capture) = capture.as_ref() {
                                    capture.set_muted(muted);
                                }
                                emit_event(
                                    &events,
                                    &overflowed,
                                    LiveCallEvent::InputMuted { muted },
                                );
                                Ok(())
                            } else {
                                if muted {
                                    input_muted.store(true, Ordering::Release);
                                    if let Some(capture) = capture.as_ref() {
                                        capture.set_muted(true);
                                    }
                                }
                                let (event_id, event) = openai_input_mute_event(muted);
                                pending_input_mute = Some((
                                    event_id,
                                    muted,
                                    tokio::time::Instant::now()
                                        + INPUT_MUTE_ACK_TIMEOUT,
                                ));
                                send_json(&mut socket, event).await
                            };
                            if let Err(error) = mute_update {
                                emit_event(&events, &overflowed, LiveCallEvent::Error(format!("Could not update microphone state: {error}")));
                                break;
                            }
                        }
                        LiveCallCommand::SetInputMuted(_) => {
                            emit_event(&events, &overflowed, LiveCallEvent::Error("Voice session is still connecting.".into()));
                        }
                        LiveCallCommand::AppendThinking { delegation_id, content } if session_ready => {
                            if let Err(error) = append_provider_text(&config, &mut socket, "thinking", delegation_id, content).await {
                                emit_event(&events, &overflowed, LiveCallEvent::Error(format!("Could not update voice session: {error}")));
                                break;
                            }
                        }
                        LiveCallCommand::UpdateEditorContext(content) if session_ready => {
                            if let Err(error) = update_provider_editor_context(&config, &mut socket, content).await {
                                emit_event(&events, &overflowed, LiveCallEvent::Error(format!("Could not update voice editor context: {error}")));
                                break;
                            }
                        }
                        LiveCallCommand::AppendCommentary { delegation_id, content } if session_ready => {
                            if let Err(error) = append_provider_text(&config, &mut socket, "commentary", delegation_id, content).await {
                                emit_event(&events, &overflowed, LiveCallEvent::Error(format!("Could not update voice session: {error}")));
                                break;
                            }
                        }
                        LiveCallCommand::AppendThinking { .. }
                        | LiveCallCommand::UpdateEditorContext(_)
                        | LiveCallCommand::AppendCommentary { .. } => {
                            emit_event(&events, &overflowed, LiveCallEvent::Error("Voice session is still connecting.".into()));
                        }
                    }
                }
                () = close.notified() => {
                    if let Some(capture) = capture.as_mut() {
                        capture.stop();
                    }
                    if session_ready {
                        if let Err(error) = send_json(&mut socket, json!({ "type": "session.close" })).await {
                            emit_event(&events, &overflowed, LiveCallEvent::Error(format!("Could not close GPT-Live session: {error}")));
                            break;
                        }
                        close_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(15));
                    } else {
                        break;
                    }
                }
                message = socket.next() => {
                    if !receive_message(message, &mut socket, &config, &mut started_at, &events, &overflowed, &ready, &mut session_ready, &mut capture, &mut suppress_output, &mut openai_playback_gate, &mut self_hosted_response_active, &mut self_hosted_barge_in_supported, &mut active_response_id, &mut active_output_item_id, &mut pending_input_mute, &input_muted, &mut playback_tracker, &output_playing).await {
                        break;
                    }
                }
                _ = capture_poller.tick(), if capture.is_some() => {
                    let mut capture_failed = false;
                    let muted = input_muted.load(Ordering::Acquire);
                    if let Some(capture) = capture.as_ref() {
                        capture.set_muted(muted);
                    }
                    if output_interrupted.swap(false, Ordering::AcqRel) {
                        suppress_output = true;
                        let was_playing = output_playing.swap(false, Ordering::AcqRel);
                        if !config.is_self_hosted() && was_playing {
                            openai_playback_gate.interrupt(elapsed_ms(started_at));
                        }
                        if let Some(capture) = capture.as_ref() {
                            let generation = capture.stop_playback();
                            if config.is_self_hosted() {
                                playback_tracker.reset_generation(generation);
                            }
                        }
                        if config.is_self_hosted() {
                            if self_hosted_response_active
                                && self_hosted_barge_in_supported
                            {
                                if let Err(error) = cancel_self_hosted_response(&mut socket).await {
                                    emit_event(&events, &overflowed, LiveCallEvent::Error(error));
                                    capture_failed = true;
                                } else {
                                    self_hosted_response_active = false;
                                }
                            }
                        } else if was_playing
                            && let Err(error) = interrupt_openai_speech(&mut socket).await
                        {
                            emit_event(
                                &events,
                                &overflowed,
                                LiveCallEvent::Error(format!(
                                    "Could not stop interrupted voice output: {error}"
                                )),
                            );
                            capture_failed = true;
                        }
                    }
                    if let Some(capture) = capture.as_ref() {
                        for event in capture.take_events() {
                            match event {
                                VoiceEvent::Ready => emit_event(&events, &overflowed, LiveCallEvent::MicrophoneReady),
                                VoiceEvent::SpeechStarted { generation } => {
                                    suppress_output = true;
                                    output_playing.store(false, Ordering::Release);
                                    if let Some(handler) = on_barge_in.as_ref() {
                                        handler();
                                    }
                                    if !config.is_self_hosted() {
                                        openai_playback_gate.speech_started(elapsed_ms(started_at));
                                        if let Err(error) = interrupt_openai_speech(&mut socket).await {
                                            emit_event(&events, &overflowed, LiveCallEvent::Error(format!("Could not stop interrupted voice output: {error}")));
                                            capture_failed = true;
                                            break;
                                        }
                                    }
                                    let playback_generation = capture.stop_playback();
                                    if config.is_self_hosted() {
                                        playback_tracker.reset_generation(playback_generation);
                                    }
                                    if config.is_self_hosted()
                                        && self_hosted_response_active
                                        && self_hosted_barge_in_supported
                                    {
                                        if let Err(error) = cancel_self_hosted_response(&mut socket).await {
                                            emit_event(&events, &overflowed, LiveCallEvent::Error(error));
                                            capture_failed = true;
                                            break;
                                        }
                                        self_hosted_response_active = false;
                                    }
                                    emit_event(&events, &overflowed, LiveCallEvent::SpeechStarted { generation });
                                }
                                VoiceEvent::SpeechEnded { generation } => {
                                    if !config.is_self_hosted() {
                                        openai_playback_gate.speech_ended(elapsed_ms(started_at));
                                    } else if let Err(error) = finish_self_hosted_turn(
                                        &config,
                                        &mut socket,
                                        &mut pending_self_hosted_audio,
                                    )
                                    .await
                                    {
                                        emit_event(&events, &overflowed, LiveCallEvent::Error(format!("Could not commit self-hosted speech: {error}")));
                                        capture_failed = true;
                                        break;
                                    }
                                    emit_event(&events, &overflowed, LiveCallEvent::SpeechEnded { generation });
                                }
                                VoiceEvent::PlaybackStarted => {
                                    output_playing.store(true, Ordering::Release);
                                    emit_event(&events, &overflowed, LiveCallEvent::PlaybackStarted);
                                }
                                VoiceEvent::PlaybackEnded => {
                                    output_playing.store(false, Ordering::Release);
                                    emit_event(&events, &overflowed, LiveCallEvent::PlaybackEnded);
                                }
                                VoiceEvent::PlaybackChunkPlayed { generation, frames } => {
                                    if config.is_self_hosted() {
                                        match playback_tracker.buffer_played(generation, frames) {
                                            Ok(Some(ack)) => {
                                                let mut event = json!({
                                                    "type": "playback.ack",
                                                    "response_id": ack.response_id,
                                                    "played_ms": ack.played_ms,
                                                    "committed_ms": ack.played_ms
                                                });
                                                if let Some(item_id) = ack.item_id {
                                                    event["item_id"] = Value::String(item_id);
                                                }
                                                if let Err(error) = send_json(&mut socket, event).await {
                                                    emit_event(&events, &overflowed, LiveCallEvent::Error(format!("Could not acknowledge played self-hosted audio: {error}")));
                                                    capture_failed = true;
                                                    break;
                                                }
                                            }
                                            Ok(None) => {}
                                            Err(error) => {
                                                emit_event(&events, &overflowed, LiveCallEvent::Error(error));
                                                capture_failed = true;
                                                break;
                                            }
                                        }
                                    }
                                }
                                VoiceEvent::AudioFrame { pcm16le, .. } => {
                                    if input_muted.load(Ordering::Acquire) {
                                        continue;
                                    }
                                    if let Err(error) = send_audio(
                                        &config,
                                        &mut socket,
                                        pcm16le,
                                        &mut pending_self_hosted_audio,
                                    ).await {
                                        emit_event(&events, &overflowed, LiveCallEvent::Error(format!("Voice audio send failed: {error}")));
                                        capture_failed = true;
                                        break;
                                    }
                                }
                                VoiceEvent::Error(error) => {
                                    emit_event(&events, &overflowed, LiveCallEvent::Error(error));
                                    capture_failed = true;
                                    break;
                                }
                            }
                        }
                    }
                    if capture_failed {
                        if let Some(capture) = capture.as_mut() {
                            capture.stop();
                        }
                        capture = None;
                        if let Err(error) = send_json(&mut socket, json!({ "type": "session.close" })).await {
                            emit_event(&events, &overflowed, LiveCallEvent::Error(format!("Could not close voice session after audio failure: {error}")));
                        }
                        close_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(15));
                    }
                }
            }
        }
    }
    if let Some(capture) = capture.as_mut() {
        capture.stop();
    }
    output_playing.store(false, Ordering::Release);
    ready.store(false, Ordering::Release);
}

#[expect(
    clippy::too_many_arguments,
    reason = "One serialized runtime owns this call state."
)]
async fn receive_message(
    message: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    config: &LiveProviderConfig,
    started_at: &mut tokio::time::Instant,
    events: &SyncSender<LiveCallEvent>,
    overflowed: &AtomicBool,
    ready: &AtomicBool,
    session_ready: &mut bool,
    capture: &mut Option<VoiceCapture>,
    suppress_output: &mut bool,
    openai_playback_gate: &mut OpenAiPlaybackGate,
    self_hosted_response_active: &mut bool,
    self_hosted_barge_in_supported: &mut bool,
    active_response_id: &mut Option<String>,
    active_output_item_id: &mut Option<String>,
    pending_input_mute: &mut Option<(String, bool, tokio::time::Instant)>,
    input_muted: &AtomicBool,
    playback_tracker: &mut SelfHostedPlaybackTracker,
    output_playing: &Arc<AtomicBool>,
) -> bool {
    let message = match message {
        Some(Ok(Message::Text(text))) => text.to_string(),
        Some(Ok(Message::Ping(payload))) => {
            if let Err(error) = socket.send(Message::Pong(payload)).await {
                emit_event(
                    events,
                    overflowed,
                    LiveCallEvent::Error(error.to_string()),
                );
                return false;
            }
            return true;
        }
        Some(Ok(Message::Close(_))) | None => {
            ready.store(false, Ordering::Release);
            if *session_ready {
                emit_event(
                    events,
                    overflowed,
                    LiveCallEvent::Error(format!(
                        "{} connection closed before session.closed.",
                        config.display_name()
                    )),
                );
            } else {
                emit_event(
                    events,
                    overflowed,
                    LiveCallEvent::Error(format!(
                        "{} connection closed during startup.",
                        config.display_name()
                    )),
                );
            }
            return false;
        }
        Some(Ok(_)) => return true,
        Some(Err(error)) => {
            ready.store(false, Ordering::Release);
            emit_event(
                events,
                overflowed,
                LiveCallEvent::Error(format!(
                    "{} connection failed: {error}",
                    config.display_name()
                )),
            );
            return false;
        }
    };

    let value = match serde_json::from_str::<Value>(&message) {
        Ok(value) => value,
        Err(error) => {
            emit_event(
                events,
                overflowed,
                LiveCallEvent::Error(format!(
                    "{} sent invalid event JSON: {error}",
                    config.display_name()
                )),
            );
            return true;
        }
    };
    match value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "session.started" | "session.created" | "session.updated" => {
            if config.is_self_hosted() && !*session_ready {
                match parse_self_hosted_barge_in_capability(&value) {
                    Ok(supports_barge_in) => {
                        *self_hosted_barge_in_supported = supports_barge_in;
                    }
                    Err(error) => {
                        emit_event(events, overflowed, LiveCallEvent::Error(error));
                        return false;
                    }
                }
            }
            let Some(session_id) = value
                .get("session")
                .and_then(|session| session.get("id"))
                .and_then(Value::as_str)
                .or_else(|| value.get("session_id").and_then(Value::as_str))
            else {
                emit_event(
                    events,
                    overflowed,
                    LiveCallEvent::Error(format!(
                        "{} started without a session ID.",
                        config.display_name()
                    )),
                );
                return false;
            };
            if !*session_ready {
                *started_at = tokio::time::Instant::now();
                *session_ready = true;
                ready.store(true, Ordering::Release);
                emit_event(
                    events,
                    overflowed,
                    LiveCallEvent::Ready {
                        session_id: session_id.to_string(),
                    },
                );
                if config.is_self_hosted() {
                    emit_event(
                        events,
                        overflowed,
                        LiveCallEvent::Capabilities {
                            supports_barge_in: *self_hosted_barge_in_supported,
                        },
                    );
                }
                match VoiceCapture::start(output_playing.clone()) {
                    Ok(native_capture) => *capture = Some(native_capture),
                    Err(error) => {
                        emit_event(events, overflowed, LiveCallEvent::Error(error));
                        return true;
                    }
                }
            }
        }
        "session.input_transcript.delta" => {
            if let Some(end_ms) = value.get("end_ms").and_then(Value::as_u64) {
                openai_playback_gate.input_transcript_ended(end_ms);
            }
            emit_transcript(&value, true, events, overflowed);
        }
        "session.output_transcript.delta" => {
            let should_show = value
                .get("start_ms")
                .and_then(Value::as_u64)
                .is_some_and(|start_ms| {
                    if config.is_self_hosted() {
                        !*suppress_output
                    } else {
                        openai_playback_gate.output_transcript_started(start_ms)
                    }
                });
            if should_show {
                emit_transcript(&value, false, events, overflowed);
            }
        }
        "session.delegation.created" => {
            emit_delegation(&value, events, overflowed);
        }
        "session.input_audio.muted" | "session.input_audio.unmuted" => {
            if let Some(muted) = acknowledge_input_mute(&value, pending_input_mute) {
                input_muted.store(muted, Ordering::Release);
                if let Some(capture) = capture.as_ref() {
                    capture.set_muted(muted);
                }
                emit_event(events, overflowed, LiveCallEvent::InputMuted { muted });
            }
        }
        "session.usage.updated" => {
            if let Some(duration_seconds) = parse_usage_seconds(&value) {
                emit_event(
                    events,
                    overflowed,
                    LiveCallEvent::UsageUpdated { duration_seconds },
                );
            }
        }
        "conversation.item.input_audio_transcription.completed" => {
            let offset_ms = elapsed_ms(*started_at);
            emit_provider_transcript(&value, true, offset_ms, events, overflowed);
        }
        "response.output_audio_transcript.delta" => {
            let offset_ms = elapsed_ms(*started_at);
            emit_provider_transcript(&value, false, offset_ms, events, overflowed);
        }
        "response.function_call_arguments.done" => {
            emit_function_call(&value, elapsed_ms(*started_at), events, overflowed);
        }
        "response.created" => {
            if config.is_self_hosted() {
                *suppress_output = false;
            }
            *active_response_id = value
                .get("response_id")
                .and_then(Value::as_str)
                .or_else(|| {
                    value
                        .get("response")
                        .and_then(|response| response.get("id"))
                        .and_then(Value::as_str)
                })
                .map(str::to_string);
            *active_output_item_id = None;
            if config.is_self_hosted() {
                *self_hosted_response_active = true;
            }
        }
        "response.output_item.added" => {
            if let Some(response_id) =
                value.get("response_id").and_then(Value::as_str)
            {
                *active_response_id = Some(response_id.to_string());
            }
            let item = value.get("item");
            if item
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
                == Some("message")
            {
                *active_output_item_id = item
                    .and_then(|item| item.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
        }
        "response.done" => {
            if config.is_self_hosted() {
                *self_hosted_response_active = false;
                if let Some(response_id) = value
                    .get("response_id")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        value
                            .get("response")
                            .and_then(|response| response.get("id"))
                            .and_then(Value::as_str)
                    })
                {
                    playback_tracker.response_done(response_id);
                    if active_response_id.as_deref() == Some(response_id) {
                        *active_response_id = None;
                        *active_output_item_id = None;
                    }
                }
            }
        }
        "session.output_audio.delta" | "response.output_audio.delta" => {
            if let Some(encoded) = value.get("delta").and_then(Value::as_str) {
                if encoded.len() > MAX_AUDIO_DELTA_BASE64_BYTES {
                    emit_event(
                        events,
                        overflowed,
                        LiveCallEvent::Error(format!(
                            "{} audio chunk exceeded the bounded playback size.",
                            config.display_name()
                        )),
                    );
                    return true;
                }
                match STANDARD.decode(encoded) {
                    Ok(audio) if audio.len().is_multiple_of(2) => {
                        let sample_rate = value
                            .get("sample_rate_hz")
                            .and_then(Value::as_u64)
                            .unwrap_or_else(|| {
                                u64::from(config.default_output_sample_rate_hz())
                            });
                        let Ok(sample_rate) = u32::try_from(sample_rate) else {
                            emit_event(
                                events,
                                overflowed,
                                LiveCallEvent::Error(format!(
                                    "{} returned an unsupported audio sample rate.",
                                    config.display_name()
                                )),
                            );
                            return true;
                        };
                        if sample_rate == 0
                            || sample_rate > MAX_PROVIDER_SAMPLE_RATE_HZ
                        {
                            emit_event(
                                events,
                                overflowed,
                                LiveCallEvent::Error(format!(
                                    "{} returned an unsupported audio sample rate ({sample_rate} Hz).",
                                    config.display_name()
                                )),
                            );
                            return true;
                        }
                        if value.get("format").and_then(Value::as_str).is_some_and(
                            |format| !matches!(format, "pcm16" | "pcm_s16le"),
                        ) {
                            emit_event(
                                events,
                                overflowed,
                                LiveCallEvent::Error(format!(
                                    "{} returned unsupported output audio format.",
                                    config.display_name()
                                )),
                            );
                            return true;
                        }
                        let playback_audio = if sample_rate
                            == PLAYBACK_SAMPLE_RATE_HZ
                        {
                            audio
                        } else {
                            match resample_pcm16le(
                                &audio,
                                sample_rate,
                                PLAYBACK_SAMPLE_RATE_HZ,
                            ) {
                                Ok(audio) => audio,
                                Err(error) => {
                                    emit_event(
                                        events,
                                        overflowed,
                                        LiveCallEvent::Error(format!(
                                            "Could not prepare {} audio for playback: {error}",
                                            config.display_name()
                                        )),
                                    );
                                    return true;
                                }
                            }
                        };
                        let output_suppressed = if config.is_self_hosted() {
                            *suppress_output
                        } else {
                            openai_playback_gate.is_suppressed()
                        };
                        if !output_suppressed && let Some(capture) = capture.as_ref()
                        {
                            if config.is_self_hosted() {
                                let response_id = value
                                    .get("response_id")
                                    .and_then(Value::as_str)
                                    .or(active_response_id.as_deref());
                                let item_id = value
                                    .get("item_id")
                                    .and_then(Value::as_str)
                                    .or(active_output_item_id.as_deref());
                                let (Some(response_id), Some(item_id)) =
                                    (response_id, item_id)
                                else {
                                    emit_event(
                                        events,
                                        overflowed,
                                        LiveCallEvent::Error(
                                            "Self-hosted audio arrived without a response and item ID; playback cannot be acknowledged safely.".into(),
                                        ),
                                    );
                                    return false;
                                };
                                if let Err(error) = playback_tracker.queue_buffer(
                                    response_id,
                                    Some(item_id),
                                    playback_audio.len() / 2,
                                ) {
                                    emit_event(
                                        events,
                                        overflowed,
                                        LiveCallEvent::Error(error),
                                    );
                                    return false;
                                }
                            }
                            if let Err(error) = capture.play_pcm16le(&playback_audio)
                            {
                                emit_event(
                                    events,
                                    overflowed,
                                    LiveCallEvent::Error(error),
                                );
                                return false;
                            }
                        }
                    }
                    Ok(_) => emit_event(
                        events,
                        overflowed,
                        LiveCallEvent::Error(format!(
                            "{} returned incomplete PCM16 audio.",
                            config.display_name()
                        )),
                    ),
                    Err(error) => emit_event(
                        events,
                        overflowed,
                        LiveCallEvent::Error(format!(
                            "{} returned invalid audio: {error}",
                            config.display_name()
                        )),
                    ),
                }
            }
        }
        "error" => {
            let message = value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .or_else(|| value.get("error").and_then(Value::as_str))
                .unwrap_or("Voice server rejected a voice request");
            emit_event(
                events,
                overflowed,
                LiveCallEvent::Error(message.to_string()),
            );
        }
        "session.closed" => {
            ready.store(false, Ordering::Release);
            emit_event(
                events,
                overflowed,
                LiveCallEvent::SessionFinalized {
                    duration_seconds: parse_usage_seconds(&value),
                },
            );
            return false;
        }
        _ => {}
    }
    true
}

fn openai_input_mute_event(muted: bool) -> (String, Value) {
    let event_id = format!("input_mute_{}", uuid::Uuid::new_v4());
    let event = json!({
        "type": if muted {
            "session.input_audio.mute"
        } else {
            "session.input_audio.unmute"
        },
        "event_id": event_id
    });
    (event_id, event)
}

fn acknowledge_input_mute(
    value: &Value,
    pending_input_mute: &mut Option<(String, bool, tokio::time::Instant)>,
) -> Option<bool> {
    let muted = match value.get("type").and_then(Value::as_str) {
        Some("session.input_audio.muted") => true,
        Some("session.input_audio.unmuted") => false,
        _ => return None,
    };
    let client_event_id = value.get("client_event_id").and_then(Value::as_str)?;
    let (expected_event_id, expected_muted, deadline) =
        pending_input_mute.as_ref()?;
    if client_event_id != expected_event_id
        || muted != *expected_muted
        || tokio::time::Instant::now() >= *deadline
    {
        return None;
    }
    pending_input_mute.take();
    Some(muted)
}

fn parse_usage_seconds(value: &Value) -> Option<u64> {
    let seconds = value.get("usage")?.get("seconds")?.as_f64()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some(Duration::try_from_secs_f64(seconds.round()).ok()?.as_secs())
}

fn emit_transcript(
    value: &Value,
    input: bool,
    events: &SyncSender<LiveCallEvent>,
    overflowed: &AtomicBool,
) {
    let (Some(text), Some(start_ms), Some(end_ms)) = (
        value
            .get("delta")
            .and_then(Value::as_str)
            .filter(|text| text.len() <= MAX_TRANSCRIPT_DELTA_BYTES),
        value.get("start_ms").and_then(Value::as_u64),
        value.get("end_ms").and_then(Value::as_u64),
    ) else {
        return;
    };
    let event = if input {
        LiveCallEvent::InputTranscriptDelta {
            text: text.to_string(),
            start_ms,
            end_ms,
        }
    } else {
        LiveCallEvent::OutputTranscriptDelta {
            text: text.to_string(),
            start_ms,
            end_ms,
        }
    };
    emit_event(events, overflowed, event);
}

fn emit_delegation(
    value: &Value,
    events: &SyncSender<LiveCallEvent>,
    overflowed: &AtomicBool,
) {
    let Some(delegation) = value.get("delegation") else {
        return;
    };
    if delegation.get("target").and_then(Value::as_str) != Some("client") {
        return;
    }
    let (Some(delegation_id), Some(offset_ms)) = (
        delegation.get("id").and_then(Value::as_str),
        value.get("offset_ms").and_then(Value::as_u64),
    ) else {
        return;
    };
    emit_event(
        events,
        overflowed,
        LiveCallEvent::DelegationCreated {
            delegation_id: delegation_id.to_string(),
            offset_ms,
            request: None,
        },
    );
}

fn emit_provider_transcript(
    value: &Value,
    input: bool,
    offset_ms: u64,
    events: &SyncSender<LiveCallEvent>,
    overflowed: &AtomicBool,
) {
    let text = if input {
        value.get("transcript").and_then(Value::as_str)
    } else {
        value.get("delta").and_then(Value::as_str)
    };
    let Some(text) = text.filter(|text| {
        !text.trim().is_empty() && text.len() <= MAX_TRANSCRIPT_DELTA_BYTES
    }) else {
        return;
    };
    let start_ms = if input {
        offset_ms.saturating_sub(15_000)
    } else {
        offset_ms
    };
    let event = if input {
        LiveCallEvent::InputTranscriptDelta {
            text: text.to_string(),
            start_ms,
            end_ms: offset_ms,
        }
    } else {
        LiveCallEvent::OutputTranscriptDelta {
            text: text.to_string(),
            start_ms,
            end_ms: offset_ms,
        }
    };
    emit_event(events, overflowed, event);
}

fn emit_function_call(
    value: &Value,
    offset_ms: u64,
    events: &SyncSender<LiveCallEvent>,
    overflowed: &AtomicBool,
) {
    let call_id = value.get("call_id").and_then(Value::as_str);
    let arguments = value.get("arguments").and_then(Value::as_str);
    if value
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(|name| name != "delegate_to_ahead")
    {
        return;
    }
    let (Some(call_id), Some(arguments)) = (call_id, arguments) else {
        return;
    };
    if call_id.is_empty() || arguments.len() > MAX_APPEND_BYTES * 8 {
        return;
    }
    let Ok(arguments) = serde_json::from_str::<Value>(arguments) else {
        return;
    };
    let Some(request) = arguments
        .get("request")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|request| !request.is_empty() && request.len() <= 4_000)
    else {
        return;
    };
    emit_event(
        events,
        overflowed,
        LiveCallEvent::DelegationCreated {
            delegation_id: call_id.to_string(),
            offset_ms,
            request: Some(request.to_string()),
        },
    );
}

fn elapsed_ms(started_at: tokio::time::Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn self_hosted_realtime_url(endpoint: &str, model: &str) -> Result<String, String> {
    let mut url = url::Url::parse(endpoint.trim()).map_err(|error| {
        format!("Enter a valid self-hosted voice server URL: {error}")
    })?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            "Put voice server credentials in the API key field, not the URL.".into(),
        );
    }
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    match url.scheme() {
        "http" if loopback => url
            .set_scheme("ws")
            .map_err(|()| "Could not configure the self-hosted voice URL.".to_string())?,
        "https" => url
            .set_scheme("wss")
            .map_err(|()| "Could not configure the self-hosted voice URL.".to_string())?,
        "ws" if loopback => {}
        "wss" => {}
        _ => return Err("Use HTTPS for a remote voice server; unencrypted WebSocket is allowed only on this computer.".into()),
    }
    let path = url.path().to_string();
    if path == "/" {
        url.set_path("/v1/realtime");
    } else if path.ends_with("/v1") {
        url.set_path(&format!("{path}/realtime"));
    } else if !path.ends_with("/v1/realtime") {
        return Err("The self-hosted endpoint must be a server origin, /v1, or /v1/realtime URL.".into());
    }
    let preserved_query = url
        .query_pairs()
        .filter(|(key, _)| key != "model" && key != "duplex")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    url.set_query(None);
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in preserved_query {
            query.append_pair(&key, &value);
        }
        query.append_pair("duplex", "1");
        query.append_pair("model", model.trim());
    }
    Ok(url.to_string())
}

fn self_hosted_session_update(config: &SelfHostedRealtimeConfig) -> Value {
    json!({
        "type": "session.update",
        "event_id": format!("setup_{}", uuid::Uuid::new_v4()),
        "session": {
            "type": "realtime",
            "model": config.model.trim(),
            "modalities": ["audio", "text"],
            "instructions": config.instructions,
            "input_audio_format": config.profile.input_audio_format(),
            "output_audio_format": "pcm16",
            "sample_rate_hz": SELF_HOSTED_INPUT_RATE,
            "audio": {
                "input": { "sample_rate_hz": SELF_HOSTED_INPUT_RATE },
                "output": { "sample_rate_hz": config.profile.output_sample_rate_hz() }
            },
            "turn_detection": null,
            "tools": [{
                "type": "function",
                "name": "delegate_to_ahead",
                "description": "Send every request about the current code, workspace, or AHEAD session to the managed AHEAD agent. Use it for code questions, teaching, and all work requests. The agent can inspect unsaved files and highlight or point at code. Do not answer these requests yourself.",
                "parameters": {
                    "type": "object",
                    "properties": { "request": { "type": "string", "maxLength": 4000 } },
                    "required": ["request"],
                    "additionalProperties": false
                }
            }],
            "tool_choice": "auto",
            "playback_commit_policy": "ack_only",
            "extra_body": { "auto_response": true }
        }
    })
}

async fn send_audio(
    config: &LiveProviderConfig,
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    pcm16le: Vec<u8>,
    pending_self_hosted_audio: &mut SelfHostedAudioBuffer,
) -> Result<(), String> {
    if !config.is_self_hosted() {
        return send_json(
            socket,
            json!({ "type": "session.input_audio.append", "audio": STANDARD.encode(pcm16le) }),
        )
        .await;
    }
    let LiveProviderConfig::SelfHosted(self_hosted_config) = config else {
        return Err("Invalid self-hosted voice configuration.".into());
    };
    let profile = self_hosted_config.profile;
    pending_self_hosted_audio.append_input(&pcm16le, profile)?;
    while let Some((chunk, audio_end_ms)) =
        pending_self_hosted_audio.take_chunk(profile)
    {
        send_self_hosted_audio_chunk(socket, profile, chunk, audio_end_ms).await?;
    }
    Ok(())
}

async fn finish_self_hosted_turn(
    config: &LiveProviderConfig,
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    pending_audio: &mut SelfHostedAudioBuffer,
) -> Result<(), String> {
    let LiveProviderConfig::SelfHosted(config) = config else {
        return Err("Invalid self-hosted voice configuration.".into());
    };
    if let Some((chunk, audio_end_ms)) =
        pending_audio.finish_chunk(config.profile)?
    {
        send_self_hosted_audio_chunk(socket, config.profile, chunk, audio_end_ms)
            .await?;
    }
    send_json(
        socket,
        json!({
            "type": "input_audio_buffer.commit",
            "event_id": format!("commit_{}", uuid::Uuid::new_v4()),
            "final": true
        }),
    )
    .await
}

async fn send_self_hosted_audio_chunk(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    profile: SelfHostedRealtimeProfile,
    chunk: Vec<u8>,
    audio_end_ms: u64,
) -> Result<(), String> {
    send_json(
        socket,
        json!({
            "type": "input_audio_buffer.append",
            "audio": STANDARD.encode(chunk),
            "input_audio_format": profile.input_audio_format(),
            "format": profile.input_audio_format(),
            "sample_rate_hz": SELF_HOSTED_INPUT_RATE,
            "duration_ms": profile.input_chunk_ms(),
            "audio_end_ms": audio_end_ms
        }),
    )
    .await
}

#[derive(Default)]
struct SelfHostedAudioBuffer {
    pending_pcm24k: Vec<u8>,
    pending_input: Vec<u8>,
    audio_end_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingPlaybackBuffer {
    generation: u64,
    response_id: String,
    item_id: Option<String>,
    frames: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlaybackAck {
    response_id: String,
    item_id: Option<String>,
    played_ms: u64,
}

#[derive(Default)]
struct OpenAiPlaybackGate {
    suppressed: bool,
    user_speech_active: bool,
    interrupted_at_ms: u64,
    latest_input_end_ms: Option<u64>,
    latest_output_start_ms: Option<u64>,
    fallback_resume_after_ms: Option<u64>,
}

impl OpenAiPlaybackGate {
    fn interrupt(&mut self, at_ms: u64) {
        self.suppressed = true;
        self.user_speech_active = false;
        self.interrupted_at_ms = at_ms;
        self.latest_output_start_ms = None;
        self.fallback_resume_after_ms = None;
    }

    fn speech_started(&mut self, at_ms: u64) {
        self.interrupt(at_ms);
        self.user_speech_active = true;
    }

    fn speech_ended(&mut self, at_ms: u64) {
        self.user_speech_active = false;
        self.latest_output_start_ms = None;
        if !self
            .latest_input_end_ms
            .is_some_and(|end_ms| end_ms > self.interrupted_at_ms)
        {
            // The native detector reports speech end after 800 ms of silence.
            // Use that known delay if a transcript timestamp has not arrived yet.
            self.fallback_resume_after_ms = Some(at_ms.saturating_sub(900));
        }
        self.try_resume();
    }

    fn input_transcript_ended(&mut self, end_ms: u64) {
        self.latest_input_end_ms = Some(
            self.latest_input_end_ms
                .map_or(end_ms, |latest| latest.max(end_ms)),
        );
        self.try_resume();
    }

    fn output_transcript_started(&mut self, start_ms: u64) -> bool {
        if self.suppressed && self.user_speech_active {
            return false;
        }
        self.latest_output_start_ms = Some(
            self.latest_output_start_ms
                .map_or(start_ms, |latest| latest.max(start_ms)),
        );
        self.try_resume();
        !self.suppressed
    }

    fn is_suppressed(&self) -> bool {
        self.suppressed
    }

    fn try_resume(&mut self) {
        if !self.suppressed || self.user_speech_active {
            return;
        }
        let resume_after_ms = self
            .latest_input_end_ms
            .filter(|end_ms| *end_ms > self.interrupted_at_ms)
            .or(self.fallback_resume_after_ms)
            .unwrap_or(self.interrupted_at_ms);
        if self
            .latest_output_start_ms
            .is_some_and(|start_ms| start_ms > resume_after_ms)
        {
            self.suppressed = false;
        }
    }
}

#[derive(Default)]
struct SelfHostedPlaybackTracker {
    generation: u64,
    pending: VecDeque<PendingPlaybackBuffer>,
    played_frames: HashMap<String, u64>,
    completed_responses: HashSet<String>,
}

impl SelfHostedPlaybackTracker {
    fn queue_buffer(
        &mut self,
        response_id: &str,
        item_id: Option<&str>,
        frames: usize,
    ) -> Result<(), String> {
        if response_id.is_empty() || frames == 0 || self.pending.len() >= 128 {
            return Err(
                "Self-hosted voice playback queue is invalid or full.".into()
            );
        }
        self.pending.push_back(PendingPlaybackBuffer {
            generation: self.generation,
            response_id: response_id.to_string(),
            item_id: item_id.map(str::to_string),
            frames: frames as u64,
        });
        Ok(())
    }

    fn buffer_played(
        &mut self,
        generation: u64,
        frames: u64,
    ) -> Result<Option<PlaybackAck>, String> {
        if generation != self.generation {
            return Ok(None);
        }
        let buffer = self.pending.pop_front().ok_or_else(|| {
            "Native playback completed without a queued self-hosted audio buffer."
                .to_string()
        })?;
        if buffer.frames != frames {
            return Err(
                "Native playback length did not match the queued self-hosted audio."
                    .into(),
            );
        }
        if buffer.generation != generation {
            return Err(
                "Native playback completed in the wrong playback generation.".into(),
            );
        }
        let played_frames = self
            .played_frames
            .entry(buffer.response_id.clone())
            .or_default();
        *played_frames = played_frames.saturating_add(frames);
        let ack = PlaybackAck {
            response_id: buffer.response_id.clone(),
            item_id: buffer.item_id,
            played_ms: played_frames.saturating_mul(1_000)
                / u64::from(PLAYBACK_SAMPLE_RATE_HZ),
        };
        self.remove_finished_response(&ack.response_id);
        Ok(Some(ack))
    }

    fn response_done(&mut self, response_id: &str) {
        self.completed_responses.insert(response_id.to_string());
        self.remove_finished_response(response_id);
    }

    fn reset_generation(&mut self, generation: u64) {
        self.generation = generation;
        self.pending.clear();
        self.played_frames.clear();
        self.completed_responses.clear();
    }

    fn remove_finished_response(&mut self, response_id: &str) {
        if self.completed_responses.contains(response_id)
            && !self
                .pending
                .iter()
                .any(|buffer| buffer.response_id == response_id)
        {
            self.played_frames.remove(response_id);
            self.completed_responses.remove(response_id);
        }
    }
}

impl SelfHostedAudioBuffer {
    fn clear(&mut self) {
        self.pending_pcm24k.clear();
        self.pending_input.clear();
    }

    fn append_input(
        &mut self,
        pcm24k: &[u8],
        profile: SelfHostedRealtimeProfile,
    ) -> Result<(), String> {
        if pcm24k.is_empty() || !pcm24k.len().is_multiple_of(2) {
            return Err(
                "Self-hosted voice input must contain complete PCM16 samples."
                    .into(),
            );
        }
        self.pending_pcm24k.extend_from_slice(pcm24k);
        let complete_input_bytes = self.pending_pcm24k.len() / 6 * 6;
        if complete_input_bytes == 0 {
            return Ok(());
        }
        let remainder = self.pending_pcm24k.split_off(complete_input_bytes);
        let aligned_input = std::mem::replace(&mut self.pending_pcm24k, remainder);
        let input_pcm16 = resample_24k_to_16k(&aligned_input)?;
        match profile {
            SelfHostedRealtimeProfile::VllmOmniPcm16
            | SelfHostedRealtimeProfile::RealtimeVenus => {
                self.pending_input.extend(input_pcm16);
            }
            SelfHostedRealtimeProfile::NemotronVoiceChat => {
                self.pending_input
                    .extend(pcm16le_to_pcm_f32le(&input_pcm16)?);
            }
        }
        Ok(())
    }

    fn take_chunk(
        &mut self,
        profile: SelfHostedRealtimeProfile,
    ) -> Option<(Vec<u8>, u64)> {
        let chunk_bytes = profile.input_chunk_bytes();
        if self.pending_input.len() < chunk_bytes {
            return None;
        }
        let remainder = self.pending_input.split_off(chunk_bytes);
        let chunk = std::mem::replace(&mut self.pending_input, remainder);
        self.audio_end_ms =
            self.audio_end_ms.saturating_add(profile.input_chunk_ms());
        Some((chunk, self.audio_end_ms))
    }

    fn finish_chunk(
        &mut self,
        profile: SelfHostedRealtimeProfile,
    ) -> Result<Option<(Vec<u8>, u64)>, String> {
        if !self.pending_pcm24k.is_empty() {
            self.pending_pcm24k.resize(6, 0);
            let input_pcm16 = resample_24k_to_16k(&self.pending_pcm24k)?;
            self.pending_pcm24k.clear();
            match profile {
                SelfHostedRealtimeProfile::VllmOmniPcm16
                | SelfHostedRealtimeProfile::RealtimeVenus => {
                    self.pending_input.extend(input_pcm16);
                }
                SelfHostedRealtimeProfile::NemotronVoiceChat => {
                    self.pending_input
                        .extend(pcm16le_to_pcm_f32le(&input_pcm16)?);
                }
            }
        }
        if self.pending_input.is_empty() {
            return Ok(None);
        }

        self.pending_input.resize(profile.input_chunk_bytes(), 0);
        let chunk = std::mem::take(&mut self.pending_input);
        self.audio_end_ms =
            self.audio_end_ms.saturating_add(profile.input_chunk_ms());
        Ok(Some((chunk, self.audio_end_ms)))
    }
}

fn resample_24k_to_16k(pcm16le: &[u8]) -> Result<Vec<u8>, String> {
    if pcm16le.is_empty() || !pcm16le.len().is_multiple_of(2) {
        return Err(
            "Self-hosted voice input must contain complete PCM16 samples.".into(),
        );
    }
    let samples = pcm16le
        .as_chunks::<2>()
        .0
        .iter()
        .map(|bytes| i32::from(i16::from_le_bytes([bytes[0], bytes[1]])))
        .collect::<Vec<_>>();
    if samples.len() % 3 != 0 {
        return Err("Self-hosted voice input must contain a 24 kHz frame aligned to 3 samples.".into());
    }
    let output_count = samples.len() * 2 / 3;
    let mut output = Vec::with_capacity(output_count * 2);
    for index in 0..output_count {
        let source_numerator = index * 3;
        let left = source_numerator / 2;
        let right = left + 1;
        let sample = if source_numerator.is_multiple_of(2) {
            samples[left]
        } else {
            (samples[left] + samples[right]) / 2
        };
        let sample = i16::try_from(sample).map_err(
            |_| "Resampled self-hosted voice input exceeded PCM16 bounds.",
        )?;
        output.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(output)
}

fn pcm16le_to_pcm_f32le(pcm16le: &[u8]) -> Result<Vec<u8>, String> {
    if pcm16le.is_empty() || !pcm16le.len().is_multiple_of(2) {
        return Err(
            "Self-hosted voice input must contain complete PCM16 samples.".into(),
        );
    }
    let mut output = Vec::with_capacity(pcm16le.len() * 2);
    for bytes in pcm16le.as_chunks::<2>().0 {
        let sample = f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32768.0;
        output.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(output)
}

fn resample_pcm16le(
    pcm16le: &[u8],
    input_rate_hz: u32,
    output_rate_hz: u32,
) -> Result<Vec<u8>, String> {
    if pcm16le.is_empty() || !pcm16le.len().is_multiple_of(2) {
        return Err("Audio must contain complete PCM16 samples.".into());
    }
    if input_rate_hz == 0 || output_rate_hz == 0 {
        return Err("Audio sample rates must be positive.".into());
    }
    if input_rate_hz == output_rate_hz {
        return Ok(pcm16le.to_vec());
    }

    let samples = pcm16le
        .as_chunks::<2>()
        .0
        .iter()
        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
        .collect::<Vec<_>>();
    let sample_count = u128::try_from(samples.len())
        .map_err(|_| "Audio buffer is too large to resample safely.")?;
    let output_rate = u128::from(output_rate_hz);
    let input_rate = u128::from(input_rate_hz);
    let output_length = sample_count
        .checked_mul(output_rate)
        .and_then(|length| length.checked_add(input_rate / 2))
        .ok_or("Audio buffer is too large to resample safely.")?
        / input_rate;
    let output_length = usize::try_from(output_length)
        .map_err(|_| "Audio buffer is too large to resample safely.")?;
    if output_length == 0 {
        return Ok(Vec::new());
    }
    let mut output = Vec::with_capacity(output_length * 2);
    for index in 0..output_length {
        let source_numerator = u128::try_from(index)
            .map_err(|_| "Audio buffer is too large to resample safely.")?
            .checked_mul(u128::from(input_rate_hz))
            .ok_or("Audio buffer is too large to resample safely.")?;
        let left_index =
            usize::try_from((source_numerator / output_rate).min(sample_count - 1))
                .map_err(|_| "Audio buffer is too large to resample safely.")?;
        let right_index = (left_index + 1).min(samples.len() - 1);
        let fraction = u32::try_from(source_numerator % output_rate)
            .map_err(|_| "Audio sample rate is too large to resample safely.")?;
        let denominator = i64::from(output_rate_hz);
        let weighted_sample = i64::from(samples[left_index])
            * i64::from(output_rate_hz - fraction)
            + i64::from(samples[right_index]) * i64::from(fraction);
        let rounded_sample = if weighted_sample >= 0 {
            weighted_sample.saturating_add(denominator / 2) / denominator
        } else {
            weighted_sample.saturating_sub(denominator / 2) / denominator
        };
        let sample = i16::try_from(rounded_sample)
            .map_err(|_| "Resampled audio exceeded PCM16 bounds.")?;
        output.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(output)
}

async fn append_provider_text(
    config: &LiveProviderConfig,
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    kind: &str,
    delegation_id: Option<String>,
    content: String,
) -> Result<(), String> {
    match config {
        LiveProviderConfig::OpenAi(_) => {
            let event = if kind == "thinking" {
                "session.thinking.append"
            } else {
                "session.commentary.append"
            };
            append_context(socket, event, delegation_id, content).await
        }
        LiveProviderConfig::SelfHosted(_) => {
            let Some(call_id) = delegation_id else {
                return Ok(());
            };
            send_json(
                socket,
                json!({
                    "type": "conversation.item.create",
                    "item": {
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": content
                    }
                }),
            )
            .await?;
            send_json(socket, json!({ "type": "response.create" })).await
        }
    }
}

async fn update_provider_editor_context(
    config: &LiveProviderConfig,
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    content: String,
) -> Result<(), String> {
    send_json(socket, editor_context_update_event(config, &content)).await
}

fn editor_context_update_event(config: &LiveProviderConfig, content: &str) -> Value {
    match config {
        LiveProviderConfig::OpenAi(_) => json!({
            "type": "session.thinking.append",
            "event_id": format!("editor_context_{}", uuid::Uuid::new_v4()),
            "delegation_id": null,
            "content": bounded_openai_editor_context(content)
        }),
        LiveProviderConfig::SelfHosted(config) => json!({
            "type": "session.update",
            "event_id": format!("editor_context_{}", uuid::Uuid::new_v4()),
            "session": {
                "instructions": format!(
                    "{}\n\n{}",
                    config.instructions,
                    content
                )
            }
        }),
    }
}

fn bounded_openai_editor_context(content: &str) -> String {
    if content.len() <= MAX_APPEND_BYTES {
        return content.to_string();
    }
    let suffix = "…";
    let mut end = MAX_APPEND_BYTES - suffix.len();
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &content[..end])
}

async fn cancel_self_hosted_response(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Result<(), String> {
    send_json(socket, json!({ "type": "response.cancel" })).await?;
    send_json(socket, json!({ "type": "output_audio_buffer.clear" })).await
}

async fn interrupt_openai_speech(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Result<(), String> {
    send_json(socket, openai_interruption_instruction()).await
}

fn openai_interruption_instruction() -> Value {
    json!({
        "type": "session.instructions.append",
        "event_id": format!("interrupt_{}", uuid::Uuid::new_v4()),
        "delegation_id": null,
        "content": "Stop your current spoken response immediately and listen. Continue the conversation normally after the user finishes."
    })
}

fn emit_event(
    sender: &SyncSender<LiveCallEvent>,
    overflowed: &AtomicBool,
    event: LiveCallEvent,
) {
    if let Err(TrySendError::Full(_)) = sender.try_send(event) {
        overflowed.store(true, Ordering::Release);
    }
}

fn parse_self_hosted_barge_in_capability(value: &Value) -> Result<bool, String> {
    let capabilities = value
        .get("session")
        .and_then(|session| session.get("capabilities"))
        .ok_or_else(|| {
            "Self-hosted voice server did not negotiate realtime duplex capabilities.".to_string()
        })?;
    if capabilities
        .get("supports_input_append")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err(
            "Self-hosted voice server does not support continuous microphone input."
                .into(),
        );
    }
    Ok(capabilities
        .get("supports_barge_in")
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

async fn send_json(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    value: Value,
) -> Result<(), String> {
    let text = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|error| error.to_string())
}

async fn append_context(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    event: &str,
    delegation_id: Option<String>,
    content: String,
) -> Result<(), String> {
    send_json(
        socket,
        json!({
            "type": event,
            "event_id": format!("update_{}", uuid::Uuid::new_v4()),
            "delegation_id": delegation_id,
            "content": content,
        }),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_call(
        command_capacity: usize,
    ) -> (LiveVoiceCall, tokio_mpsc::Receiver<LiveCallCommand>) {
        let (commands, command_receiver) = tokio_mpsc::channel(command_capacity);
        let (event_sender, events) = mpsc::sync_channel(1);
        drop(event_sender);
        (
            LiveVoiceCall {
                commands,
                events,
                ready: Arc::new(AtomicBool::new(true)),
                event_overflowed: Arc::new(AtomicBool::new(false)),
                close: Arc::new(Notify::new()),
                input_muted: Arc::new(AtomicBool::new(false)),
                output_interrupted: Arc::new(AtomicBool::new(false)),
                output_playing: Arc::new(AtomicBool::new(false)),
                worker: None,
            },
            command_receiver,
        )
    }

    #[test]
    fn configuration_debug_redacts_api_key() {
        let config = OpenAiLiveConfig {
            api_key: "secret-token".into(),
            instructions: "Be concise".into(),
            on_barge_in: None,
        };
        let debug = format!("{config:?}");
        assert!(!debug.contains("secret-token"));
        assert!(debug.contains("REDACTED"));
    }

    #[test]
    fn microphone_mute_is_not_queued_before_provider_readiness() {
        let (call, mut commands) = test_call(1);
        call.ready.store(false, Ordering::Release);

        assert_eq!(
            call.set_input_muted(true),
            Err("Voice session is still connecting.".into())
        );
        assert!(!call.input_muted.load(Ordering::Acquire));
        assert!(matches!(
            commands.try_recv(),
            Err(tokio_mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn openai_playback_stays_suppressed_until_a_fresh_reply_after_barge_in() {
        let mut gate = OpenAiPlaybackGate::default();
        gate.speech_started(1_000);
        assert!(gate.is_suppressed());

        assert!(!gate.output_transcript_started(900));
        gate.input_transcript_ended(1_500);
        gate.speech_ended(2_300);
        assert!(gate.is_suppressed());

        assert!(!gate.output_transcript_started(1_499));
        assert!(gate.output_transcript_started(1_501));
        assert!(!gate.is_suppressed());
    }

    #[test]
    fn openai_playback_waits_for_the_transcript_when_audio_vad_ends_first() {
        let mut gate = OpenAiPlaybackGate::default();
        gate.speech_started(1_000);
        gate.speech_ended(2_300);
        assert!(gate.is_suppressed());

        assert!(!gate.output_transcript_started(1_350));
        gate.input_transcript_ended(1_400);
        assert!(gate.is_suppressed());
        assert!(gate.output_transcript_started(1_401));
        assert!(!gate.is_suppressed());
    }

    #[test]
    fn openai_output_transcript_resumes_after_a_non_speech_interruption() {
        let mut gate = OpenAiPlaybackGate::default();
        gate.interrupt(2_000);
        assert!(!gate.output_transcript_started(1_999));
        assert!(gate.output_transcript_started(2_001));
    }

    #[test]
    fn self_hosted_configuration_requires_remote_auth_and_redacts_key() {
        let config = SelfHostedRealtimeConfig {
            endpoint: "https://voice.example.com/v1".into(),
            api_key: String::new(),
            model: "voice-model".into(),
            profile: SelfHostedRealtimeProfile::VllmOmniPcm16,
            instructions: "Be concise".into(),
            on_barge_in: None,
        };
        let error = LiveVoiceCall::connect_self_hosted(config.clone())
            .err()
            .expect("remote server must require an API key");
        assert!(error.contains("API key"));

        let debug = format!(
            "{:?}",
            SelfHostedRealtimeConfig {
                api_key: "private-token".into(),
                ..config
            }
        );
        assert!(!debug.contains("private-token"));
        assert!(debug.contains("REDACTED"));
    }

    #[test]
    fn transcript_events_require_timing_and_keep_provider_offsets() {
        let (sender, receiver) = mpsc::sync_channel(4);
        let overflowed = AtomicBool::new(false);
        emit_transcript(
            &json!({ "delta": "show this", "start_ms": 120, "end_ms": 430 }),
            true,
            &sender,
            &overflowed,
        );
        assert_eq!(
            receiver.try_recv(),
            Ok(LiveCallEvent::InputTranscriptDelta {
                text: "show this".into(),
                start_ms: 120,
                end_ms: 430,
            })
        );

        emit_transcript(
            &json!({ "delta": "missing times" }),
            false,
            &sender,
            &overflowed,
        );
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
    }

    #[test]
    fn openai_mute_commands_wait_for_matching_provider_acknowledgments() {
        let (mute_event_id, mute_event) = openai_input_mute_event(true);
        assert_eq!(mute_event["type"], "session.input_audio.mute");
        assert_eq!(mute_event["event_id"], mute_event_id);

        let mute_deadline = tokio::time::Instant::now() + INPUT_MUTE_ACK_TIMEOUT;
        let mut pending = Some((mute_event_id.clone(), true, mute_deadline));
        assert_eq!(
            acknowledge_input_mute(
                &json!({
                    "type": "session.input_audio.muted",
                    "client_event_id": "stale_event"
                }),
                &mut pending,
            ),
            None
        );
        assert!(pending.as_ref().is_some_and(|(event_id, muted, deadline)| {
            event_id == &mute_event_id && *muted && *deadline == mute_deadline
        }));
        assert_eq!(
            acknowledge_input_mute(
                &json!({
                    "type": "session.input_audio.unmuted",
                    "client_event_id": mute_event_id
                }),
                &mut pending,
            ),
            None
        );
        assert_eq!(
            acknowledge_input_mute(
                &json!({
                    "type": "session.input_audio.muted",
                    "client_event_id": mute_event["event_id"]
                }),
                &mut pending,
            ),
            Some(true)
        );
        assert_eq!(pending, None);

        let mut expired = Some((
            mute_event_id,
            true,
            tokio::time::Instant::now() - Duration::from_secs(1),
        ));
        assert_eq!(
            acknowledge_input_mute(
                &json!({
                    "type": "session.input_audio.muted",
                    "client_event_id": mute_event["event_id"]
                }),
                &mut expired,
            ),
            None
        );
        assert!(expired.is_some());

        let (unmute_event_id, unmute_event) = openai_input_mute_event(false);
        assert_eq!(unmute_event["type"], "session.input_audio.unmute");
        assert_eq!(unmute_event["event_id"], unmute_event_id);
        let mut pending = Some((
            unmute_event_id.clone(),
            false,
            tokio::time::Instant::now() + INPUT_MUTE_ACK_TIMEOUT,
        ));
        assert_eq!(
            acknowledge_input_mute(
                &json!({
                    "type": "session.input_audio.unmuted",
                    "client_event_id": unmute_event_id
                }),
                &mut pending,
            ),
            Some(false)
        );
        assert_eq!(pending, None);
    }

    #[test]
    fn only_client_delegations_are_forwarded_to_ahead() {
        let (sender, receiver) = mpsc::sync_channel(2);
        let overflowed = AtomicBool::new(false);
        let event = json!({
            "delegation": {
                "id": "delegation_1",
                "target": "client"
            },
            "offset_ms": 1200
        });
        emit_delegation(&event, &sender, &overflowed);
        assert_eq!(
            receiver.try_recv(),
            Ok(LiveCallEvent::DelegationCreated {
                delegation_id: "delegation_1".into(),
                offset_ms: 1200,
                request: None,
            })
        );

        emit_delegation(
            &json!({
                "delegation": {
                    "id": "delegation_2",
                    "target": "responses"
                },
                "offset_ms": 2400
            }),
            &sender,
            &overflowed,
        );
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
    }

    #[test]
    fn cloud_barge_in_uses_live_instruction_event() {
        let event = openai_interruption_instruction();
        assert_eq!(event["type"], "session.instructions.append");
        assert_eq!(event["delegation_id"], Value::Null);
        assert!(
            event["event_id"]
                .as_str()
                .is_some_and(|id| { id.starts_with("interrupt_") })
        );
        assert!(event["content"].as_str().is_some_and(|content| {
            !content.is_empty() && content.len() < MAX_APPEND_BYTES
        }));
    }

    #[test]
    fn openai_usage_seconds_are_read_as_cumulative_duration() {
        assert_eq!(
            parse_usage_seconds(&json!({ "usage": { "seconds": 15.4 } })),
            Some(15)
        );
        assert_eq!(
            parse_usage_seconds(&json!({ "usage": { "seconds": -1 } })),
            None
        );
        assert_eq!(parse_usage_seconds(&json!({ "usage": {} })), None);
    }

    #[test]
    fn event_queue_overflow_is_reported_on_poll() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let overflowed = Arc::new(AtomicBool::new(false));
        emit_event(&sender, &overflowed, LiveCallEvent::Closed);
        emit_event(
            &sender,
            &overflowed,
            LiveCallEvent::Ready {
                session_id: "s1".into(),
            },
        );
        let mut events = receiver.try_iter().collect::<Vec<_>>();
        if overflowed.swap(false, Ordering::AcqRel) {
            events.push(LiveCallEvent::Error(
                "Voice event queue overflowed; end the call and reconnect.".into(),
            ));
        }
        assert_eq!(events.len(), 2);
        assert!(matches!(events.last(), Some(LiveCallEvent::Error(_))));
    }

    #[test]
    fn microphone_mute_takes_effect_even_when_control_queue_is_full() {
        let (call, _receiver) = test_call(1);
        assert!(
            call.commands
                .try_send(LiveCallCommand::SetInputMuted(false))
                .is_ok()
        );

        assert!(call.set_input_muted(true).is_err());
        assert!(call.input_muted.load(Ordering::Acquire));
    }

    #[test]
    fn self_hosted_url_uses_duplex_route_and_configured_model() {
        assert_eq!(
            self_hosted_realtime_url(
                "http://127.0.0.1:8091/v1?api-version=1",
                "nvidia/NVIDIA-NemotronLabs-VoiceChat-11B"
            )
            .as_deref(),
            Ok(
                "ws://127.0.0.1:8091/v1/realtime?api-version=1&duplex=1&model=nvidia%2FNVIDIA-NemotronLabs-VoiceChat-11B"
            )
        );
    }

    #[test]
    fn self_hosted_url_rejects_insecure_remote_endpoints() {
        assert!(
            self_hosted_realtime_url("ws://voice.example.com/v1", "model").is_err()
        );
        assert!(
            self_hosted_realtime_url("https://voice.example.com/v1", "model")
                .is_ok()
        );
        assert!(
            self_hosted_realtime_url(
                "https://user:secret@voice.example.com/v1",
                "model"
            )
            .is_err()
        );
    }

    #[test]
    fn self_hosted_runtime_requires_input_streaming_and_negotiates_barge_in() {
        let session = json!({
            "session": {
                "capabilities": {
                    "supports_input_append": true,
                    "supports_barge_in": false
                }
            }
        });
        assert_eq!(parse_self_hosted_barge_in_capability(&session), Ok(false));

        let session = json!({
            "session": {
                "capabilities": {
                    "supports_input_append": true,
                    "supports_barge_in": true
                }
            }
        });
        assert_eq!(parse_self_hosted_barge_in_capability(&session), Ok(true));

        assert!(
            parse_self_hosted_barge_in_capability(&json!({"session": {}})).is_err()
        );
        assert!(
            parse_self_hosted_barge_in_capability(&json!({
                "session": {"capabilities": {"supports_input_append": false}}
            }))
            .is_err()
        );
    }

    #[test]
    fn self_hosted_audio_resampling_produces_200_ms_server_chunks() {
        let input = vec![0u8; 24_000 * 2 / 5];
        let output = resample_24k_to_16k(&input).expect("resample one 200ms frame");
        assert_eq!(output.len(), SELF_HOSTED_PCM16_CHUNK_BYTES);
    }

    #[test]
    fn self_hosted_audio_buffer_accepts_native_frames_with_uneven_resampling_phase()
    {
        let frame = i16::to_le_bytes(16_384).repeat(512);
        let mut audio = SelfHostedAudioBuffer::default();
        for _ in 0..4 {
            audio
                .append_input(&frame, SelfHostedRealtimeProfile::NemotronVoiceChat)
                .expect("append arbitrary native callback size");
        }

        let (chunk, audio_end_ms) = audio
            .take_chunk(SelfHostedRealtimeProfile::NemotronVoiceChat)
            .expect("four native callbacks contain an 80 ms model frame");
        assert_eq!(chunk.len(), NEMOTRON_INPUT_CHUNK_SAMPLES * 4);
        assert_eq!(audio_end_ms, 80);
        for sample in chunk.chunks_exact(4) {
            let sample =
                f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]);
            assert!((sample - 0.5).abs() < 0.001);
        }
    }

    #[test]
    fn self_hosted_audio_buffer_pads_and_flushes_the_final_partial_frame() {
        let frame = i16::to_le_bytes(8_192).repeat(1_024);
        let mut audio = SelfHostedAudioBuffer::default();
        audio
            .append_input(&frame, SelfHostedRealtimeProfile::NemotronVoiceChat)
            .expect("append one native microphone frame");

        let (chunk, audio_end_ms) = audio
            .finish_chunk(SelfHostedRealtimeProfile::NemotronVoiceChat)
            .expect("flush the final model frame")
            .expect("emit padded final frame");

        assert_eq!(chunk.len(), NEMOTRON_INPUT_CHUNK_SAMPLES * 4);
        assert_eq!(audio_end_ms, 80);
        assert!(audio.pending_input.is_empty());
        assert!(audio.pending_pcm24k.is_empty());
        assert!(
            audio
                .finish_chunk(SelfHostedRealtimeProfile::NemotronVoiceChat)
                .expect("empty flush")
                .is_none()
        );
    }

    #[test]
    fn nemotron_session_uses_vllm_realtime_tool_and_audio_contract() {
        let config = SelfHostedRealtimeConfig {
            endpoint: "https://voice.example.com/v1".into(),
            api_key: "private-token".into(),
            model: "nemotron-voicechat".into(),
            profile: SelfHostedRealtimeProfile::NemotronVoiceChat,
            instructions: "Delegate code tasks.".into(),
            on_barge_in: None,
        };
        let update = self_hosted_session_update(&config);
        let session = &update["session"];
        assert_eq!(session["input_audio_format"], "pcm_f32le");
        assert_eq!(session["sample_rate_hz"], 16_000);
        assert_eq!(session["audio"]["output"]["sample_rate_hz"], 22_050);
        assert_eq!(session["tools"][0]["name"], "delegate_to_ahead");
        assert_eq!(session["tools"][0]["type"], "function");
        assert_eq!(session["tool_choice"], "auto");
        assert_eq!(session["playback_commit_policy"], "ack_only");
        assert_eq!(session["extra_body"]["auto_response"], true);
        assert!(session["extra_body"].get("realtime_tools").is_none());
        assert!(!update.to_string().contains("private-token"));
    }

    #[test]
    fn self_hosted_playback_acknowledges_cumulative_audio_that_was_played() {
        let mut tracker = SelfHostedPlaybackTracker::default();
        tracker
            .queue_buffer("response-1", Some("item-1"), 12_000)
            .expect("queue first buffer");
        tracker
            .queue_buffer("response-1", Some("item-1"), 6_000)
            .expect("queue second buffer");

        assert_eq!(
            tracker.buffer_played(0, 12_000).expect("ack first buffer"),
            Some(PlaybackAck {
                response_id: "response-1".into(),
                item_id: Some("item-1".into()),
                played_ms: 500,
            })
        );
        assert_eq!(
            tracker.buffer_played(0, 6_000).expect("ack second buffer"),
            Some(PlaybackAck {
                response_id: "response-1".into(),
                item_id: Some("item-1".into()),
                played_ms: 750,
            })
        );

        tracker.response_done("response-1");
        assert!(tracker.pending.is_empty());
        assert!(tracker.played_frames.is_empty());
    }

    #[test]
    fn self_hosted_playback_discards_callbacks_from_interrupted_generations() {
        let mut tracker = SelfHostedPlaybackTracker::default();
        tracker
            .queue_buffer("response-old", Some("item-old"), 12_000)
            .expect("queue interrupted buffer");
        tracker.reset_generation(1);
        tracker
            .queue_buffer("response-new", Some("item-new"), 6_000)
            .expect("queue new response");

        assert_eq!(tracker.buffer_played(0, 12_000), Ok(None));
        assert_eq!(
            tracker.buffer_played(1, 6_000).expect("ack new response"),
            Some(PlaybackAck {
                response_id: "response-new".into(),
                item_id: Some("item-new".into()),
                played_ms: 250,
            })
        );
    }

    #[test]
    fn editor_context_updates_use_provider_native_context_events() {
        let openai = LiveProviderConfig::OpenAi(OpenAiLiveConfig {
            api_key: "private-token".into(),
            instructions: "Delegate code tasks.".into(),
            on_barge_in: None,
        });
        let openai_update = editor_context_update_event(
            &openai,
            "Active unsaved editor context: `src/main.rs`.",
        );
        assert_eq!(openai_update["type"], "session.thinking.append");
        assert_eq!(openai_update["delegation_id"], Value::Null);
        assert_eq!(
            openai_update["content"],
            "Active unsaved editor context: `src/main.rs`."
        );
        let oversized = format!(
            "Active editor path and nearby code: {} END_SENTINEL",
            "é".repeat(MAX_APPEND_BYTES)
        );
        let bounded_update = editor_context_update_event(&openai, &oversized);
        let bounded_content = bounded_update["content"]
            .as_str()
            .expect("bounded editor context");
        assert!(bounded_content.len() <= MAX_APPEND_BYTES);
        assert!(bounded_content.ends_with('…'));
        assert!(!bounded_content.contains("END_SENTINEL"));

        let self_hosted = LiveProviderConfig::SelfHosted(SelfHostedRealtimeConfig {
            endpoint: "https://voice.example.com/v1".into(),
            api_key: "private-token".into(),
            model: "nemotron-voicechat".into(),
            profile: SelfHostedRealtimeProfile::NemotronVoiceChat,
            instructions: "Delegate code tasks.".into(),
            on_barge_in: None,
        });
        let self_hosted_update = editor_context_update_event(
            &self_hosted,
            "Active unsaved editor context: `src/main.rs`.",
        );
        assert_eq!(self_hosted_update["type"], "session.update");
        assert!(
            self_hosted_update["session"]["instructions"]
                .as_str()
                .is_some_and(|instructions| {
                    instructions.contains("Delegate code tasks.")
                        && instructions.contains("Active unsaved editor context")
                })
        );
        assert!(!self_hosted_update.to_string().contains("private-token"));
    }

    #[test]
    fn provider_audio_resampling_converts_22050_hz_to_native_playback_rate() {
        let input = i16::to_le_bytes(12_000).repeat(22_050);
        let output = resample_pcm16le(&input, 22_050, PLAYBACK_SAMPLE_RATE_HZ)
            .expect("resample provider audio");

        assert_eq!(output.len(), 24_000 * 2);
        assert!(
            output
                .chunks_exact(2)
                .all(|sample| i16::from_le_bytes([sample[0], sample[1]]) == 12_000)
        );
    }

    #[test]
    fn provider_audio_resampling_rejects_malformed_pcm_and_rates() {
        assert!(resample_pcm16le(&[0], 22_050, PLAYBACK_SAMPLE_RATE_HZ).is_err());
        assert!(resample_pcm16le(&[0, 0], 0, PLAYBACK_SAMPLE_RATE_HZ).is_err());
    }

    #[test]
    fn self_hosted_function_call_emits_managed_agent_request() {
        let (sender, receiver) = mpsc::sync_channel(2);
        let overflowed = AtomicBool::new(false);
        emit_function_call(
            &json!({
                "call_id": "call_1",
                "name": "delegate_to_ahead",
                "arguments": "{\"request\":\"Explain this function\"}"
            }),
            510,
            &sender,
            &overflowed,
        );
        assert_eq!(
            receiver.try_recv(),
            Ok(LiveCallEvent::DelegationCreated {
                delegation_id: "call_1".into(),
                offset_ms: 510,
                request: Some("Explain this function".into()),
            })
        );
    }
}
