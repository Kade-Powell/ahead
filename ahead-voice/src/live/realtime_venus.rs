use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
    },
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use tokenizers::Tokenizer;
use tokio::{
    sync::{Notify, mpsc},
    time::Instant,
};

use crate::{VoiceCapture, VoiceEvent};

use super::{
    LiveCallCommand, LiveCallEvent, SelfHostedAudioBuffer, SelfHostedRealtimeConfig,
    SelfHostedRealtimeProfile, emit_event,
};

const PROTOCOL_TOKENS: [&str; 12] = [
    "<|speak|>",
    "<|listen|>",
    "<|turn_eos|>",
    "<|chunk_eos|>",
    "<unit>",
    "</unit>",
    "<image>",
    "</image>",
    "<delegate>",
    "</delegate>",
    "<backend>",
    "</backend>",
];

const MAX_DELEGATE_TOKENS: usize = 8_192;
const MAX_TEXT_TOKENS: usize = 32_768;
const MAX_DELEGATE_CHARS: usize = 4_000;

pub(super) fn base_url(endpoint: &str) -> Result<String, String> {
    let url = url::Url::parse(endpoint.trim()).map_err(|error| {
        format!("Enter a valid Realtime-Venus server URL: {error}")
    })?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            "Put voice server credentials in the API key field, not the URL.".into(),
        );
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(
            "The Realtime-Venus server URL cannot contain a query or fragment."
                .into(),
        );
    }
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => {
            return Err(
                "Use HTTPS for a remote Realtime-Venus server; HTTP is allowed only on this computer."
                    .into(),
            );
        }
    }
    Ok(url.to_string().trim_end_matches('/').to_string())
}

pub(super) fn validate_config(
    config: &SelfHostedRealtimeConfig,
) -> Result<(), String> {
    base_url(&config.endpoint)?;
    if config.model.trim().is_empty() {
        return Err(
            "Enter the Realtime-Venus Hugging Face model ID in AHEAD settings."
                .into(),
        );
    }
    if config.instructions.chars().count() > 8_000 {
        return Err("Voice instructions are too long.".into());
    }
    let parsed = url::Url::parse(config.endpoint.trim()).map_err(|error| {
        format!("Enter a valid Realtime-Venus server URL: {error}")
    })?;
    let is_loopback =
        matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if !is_loopback && config.api_key.trim().is_empty() {
        return Err(
            "Set a bearer token for the authenticated Realtime-Venus endpoint."
                .into(),
        );
    }
    Ok(())
}

fn validate_session_capacity(opened: &Value) -> Result<(), String> {
    let max_sessions = opened
        .get("capabilities")
        .and_then(|capabilities| capabilities.get("max_sessions"))
        .or_else(|| opened.get("max_sessions"))
        .and_then(Value::as_u64);
    if max_sessions == Some(1) {
        Ok(())
    } else {
        Err(
            "Realtime-Venus server must report max_sessions=1 for its loaded model."
                .into(),
        )
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "One serialized runtime owns this call state."
)]
pub(super) async fn run(
    config: SelfHostedRealtimeConfig,
    mut commands: mpsc::Receiver<LiveCallCommand>,
    events: SyncSender<LiveCallEvent>,
    ready: Arc<AtomicBool>,
    overflowed: Arc<AtomicBool>,
    close: Arc<Notify>,
    input_muted: Arc<AtomicBool>,
    output_interrupted: Arc<AtomicBool>,
    output_playing: Arc<AtomicBool>,
) {
    if let Err(error) = validate_config(&config) {
        emit_event(&events, &overflowed, LiveCallEvent::Error(error));
        return;
    }
    let endpoint = match base_url(&config.endpoint) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            emit_event(&events, &overflowed, LiveCallEvent::Error(error));
            return;
        }
    };
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            emit_event(
                &events,
                &overflowed,
                LiveCallEvent::Error(format!(
                    "Could not create voice client: {error}"
                )),
            );
            return;
        }
    };
    let model = config.model.trim().to_string();
    let tokenizer = match load_tokenizer(&model).await {
        Ok(tokenizer) => tokenizer,
        Err(error) => {
            emit_event(&events, &overflowed, LiveCallEvent::Error(error));
            return;
        }
    };

    let session_id = uuid::Uuid::new_v4().to_string();
    let opened =
        match open_session(&client, &config, &endpoint, &session_id, &model).await {
            Ok(opened) => opened,
            Err(error) => {
                emit_event(
                    &events,
                    &overflowed,
                    LiveCallEvent::Error(format!(
                        "Could not open Realtime-Venus session: {error}"
                    )),
                );
                return;
            }
        };
    let Some(opened_session_id) = opened.get("session_id").and_then(Value::as_str)
    else {
        emit_event(
            &events,
            &overflowed,
            LiveCallEvent::Error(
                "Realtime-Venus opened a session without an ID.".into(),
            ),
        );
        return;
    };
    if opened_session_id != session_id {
        emit_event(
            &events,
            &overflowed,
            LiveCallEvent::Error(
                "Realtime-Venus returned a different session ID.".into(),
            ),
        );
        return;
    }
    let Some(incarnation) = opened.get("incarnation").and_then(Value::as_u64) else {
        emit_event(
            &events,
            &overflowed,
            LiveCallEvent::Error(
                "Realtime-Venus opened a session without an incarnation.".into(),
            ),
        );
        return;
    };
    let capabilities = opened.get("capabilities").unwrap_or(&opened);
    if let Err(error) = validate_session_capacity(&opened) {
        delete_session(&client, &config, &endpoint, &session_id, incarnation).await;
        emit_event(&events, &overflowed, LiveCallEvent::Error(error));
        return;
    }
    let special_tokens = capabilities
        .get("special_token_ids")
        .or_else(|| opened.get("special_token_ids"))
        .and_then(Value::as_object)
        .map(|tokens| {
            tokens
                .iter()
                .filter_map(|(name, id)| {
                    id.as_u64()
                        .and_then(|id| u32::try_from(id).ok())
                        .map(|id| (name.clone(), id))
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let mut decoder = match VenusTokenDecoder::new(tokenizer, &special_tokens) {
        Ok(decoder) => decoder,
        Err(error) => {
            delete_session(&client, &config, &endpoint, &session_id, incarnation)
                .await;
            emit_event(&events, &overflowed, LiveCallEvent::Error(error));
            return;
        }
    };
    let delegate_audio_filtered = capabilities
        .get("delegate_audio_filtered")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let opened_at_ms = opened
        .get("opened_at_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    if let Err(error) = append_prefill(
        &client,
        &config,
        &endpoint,
        &session_id,
        incarnation,
        "ahead-session-instructions",
        &format!(
            "{}\n\nThe user is speaking with AHEAD in an active coding session. For code questions, request a verified explanation from the managed AHEAD agent using the <delegate>…</delegate> protocol. Do not invent editor locations. AHEAD presents code highlights and pointers separately.",
            config.instructions
        ),
        false,
    )
    .await
    {
        delete_session(&client, &config, &endpoint, &session_id, incarnation).await;
        emit_event(
            &events,
            &overflowed,
            LiveCallEvent::Error(format!("Could not configure Realtime-Venus: {error}")),
        );
        return;
    }
    let mut output_generation_active = true;
    let mut suppress_current_generation = true;

    let mut capture = match VoiceCapture::start(output_playing.clone()) {
        Ok(capture) => capture,
        Err(error) => {
            delete_session(&client, &config, &endpoint, &session_id, incarnation)
                .await;
            emit_event(&events, &overflowed, LiveCallEvent::Error(error));
            return;
        }
    };
    let started_at = Instant::now();
    ready.store(true, Ordering::Release);
    emit_event(
        &events,
        &overflowed,
        LiveCallEvent::Ready {
            session_id: session_id.clone(),
        },
    );
    emit_event(
        &events,
        &overflowed,
        LiveCallEvent::Capabilities {
            supports_barge_in: false,
        },
    );

    let upload_failed = Arc::new(AtomicBool::new(false));
    let (audio_sender, audio_receiver) = mpsc::channel(16);
    let (ack_sender, ack_receiver) = mpsc::channel(16);
    let upload_task = tokio::spawn(upload_audio(
        client.clone(),
        config.clone(),
        endpoint.clone(),
        session_id.clone(),
        incarnation,
        audio_receiver,
        input_muted.clone(),
        events.clone(),
        overflowed.clone(),
        upload_failed.clone(),
    ));
    let ack_task = tokio::spawn(send_playback_acks(
        client.clone(),
        config.clone(),
        endpoint.clone(),
        session_id.clone(),
        incarnation,
        ack_receiver,
        events.clone(),
        overflowed.clone(),
        upload_failed.clone(),
    ));

    let mut input_audio = SelfHostedAudioBuffer::default();
    let mut event_seq = 0_u64;
    let mut minimum_input_seq = 0_u64;
    let mut active_output_generation: Option<String> = None;
    let mut interrupted_generation: Option<String> = None;
    let mut playback = VenusPlaybackTracker::default();
    playback.reset(capture.generation());
    let mut pending_prefill = VecDeque::<PendingPrefill>::new();
    let mut capture_poller = tokio::time::interval(Duration::from_millis(10));
    capture_poller.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let output_url = format!("{endpoint}/sessions/{session_id}/output");
    let mut output_poll =
        Box::pin(poll_output(&client, &config, &output_url, incarnation));
    let mut last_usage_update = Instant::now();
    let on_barge_in = config.on_barge_in.clone();

    'call: loop {
        if !output_generation_active
            && let Some(prefill) = pending_prefill.pop_front()
        {
            let suppress_response = prefill.suppress_response;
            match append_prefill(
                &client,
                &config,
                &endpoint,
                &session_id,
                incarnation,
                &prefill.work_id,
                &prefill.content,
                prefill.resume_generation,
            )
            .await
            {
                Ok(()) => {
                    output_generation_active = true;
                    suppress_current_generation = suppress_response;
                }
                Err(error) => {
                    emit_event(
                        &events,
                        &overflowed,
                        LiveCallEvent::Error(format!(
                            "Could not update Realtime-Venus context: {error}"
                        )),
                    );
                    break;
                }
            }
        }
        if upload_failed.load(Ordering::Acquire) {
            break;
        }
        tokio::select! {
            () = close.notified() => break,
            command = commands.recv() => {
                let Some(command) = command else { break; };
                match command {
                    LiveCallCommand::Audio(_) => emit_event(
                        &events,
                        &overflowed,
                        LiveCallEvent::Error("Realtime-Venus audio is captured by the native audio engine.".into()),
                    ),
                    LiveCallCommand::SetInputMuted(muted) => {
                        input_muted.store(muted, Ordering::Release);
                        capture.set_muted(muted);
                        input_audio.clear();
                        emit_event(&events, &overflowed, LiveCallEvent::InputMuted { muted });
                    }
                    LiveCallCommand::UpdateEditorContext(content) => {
                        if pending_prefill.len() >= 16 {
                            emit_event(&events, &overflowed, LiveCallEvent::Error("Realtime-Venus context queue is full.".into()));
                            break 'call;
                        }
                        pending_prefill.push_back(PendingPrefill {
                            work_id: format!("editor-context-{}", uuid::Uuid::new_v4()),
                            content: format!("[Current editor context]\n{content}"),
                            resume_generation: false,
                            suppress_response: true,
                        });
                    }
                    LiveCallCommand::AppendThinking { delegation_id, content } => {
                        if pending_prefill.len() >= 16 {
                            emit_event(&events, &overflowed, LiveCallEvent::Error("Realtime-Venus context queue is full.".into()));
                            break 'call;
                        }
                        pending_prefill.push_back(PendingPrefill {
                            work_id: delegation_id.unwrap_or_else(|| format!("thinking-{}", uuid::Uuid::new_v4())),
                            content,
                            resume_generation: false,
                            suppress_response: true,
                        });
                    }
                    LiveCallCommand::AppendCommentary { delegation_id, content } => {
                        if pending_prefill.len() >= 16 {
                            emit_event(&events, &overflowed, LiveCallEvent::Error("Realtime-Venus context queue is full.".into()));
                            break 'call;
                        }
                        pending_prefill.push_back(PendingPrefill {
                            work_id: delegation_id.unwrap_or_else(|| format!("commentary-{}", uuid::Uuid::new_v4())),
                            content,
                            resume_generation: true,
                            suppress_response: false,
                        });
                    }
                }
            }
            _ = capture_poller.tick() => {
                if output_interrupted.swap(false, Ordering::AcqRel) {
                    output_playing.store(false, Ordering::Release);
                    let playback_generation = capture.stop_playback();
                    playback.reset(playback_generation);
                }
                capture.set_muted(input_muted.load(Ordering::Acquire));
                for event in capture.take_events() {
                    match event {
                        VoiceEvent::Ready => emit_event(&events, &overflowed, LiveCallEvent::MicrophoneReady),
                        VoiceEvent::SpeechStarted { generation } => {
                            minimum_input_seq = event_seq.saturating_add(1);
                            interrupted_generation = active_output_generation.clone();
                            if output_playing.swap(false, Ordering::AcqRel) {
                                let playback_generation = capture.stop_playback();
                                playback.reset(playback_generation);
                                if let Some(handler) = on_barge_in.as_ref() {
                                    handler();
                                }
                            }
                            emit_event(&events, &overflowed, LiveCallEvent::SpeechStarted { generation });
                        }
                        VoiceEvent::SpeechEnded { generation } => {
                            if let Err(error) = queue_audio_chunks(
                                &mut input_audio,
                                SelfHostedRealtimeProfile::RealtimeVenus,
                                true,
                                &audio_sender,
                                &mut event_seq,
                            ) {
                                emit_event(&events, &overflowed, LiveCallEvent::Error(error));
                                capture.stop();
                                break 'call;
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
                            if let Some(ack) = playback.played(generation, frames)
                                && let Err(error) = ack_sender.try_send(ack)
                            {
                                    let message = match error {
                                        mpsc::error::TrySendError::Full(_) => {
                                            "Realtime-Venus playback acknowledgement queue is full."
                                        }
                                        mpsc::error::TrySendError::Closed(_) => {
                                            "Realtime-Venus playback acknowledgements stopped."
                                        }
                                    };
                                    emit_event(&events, &overflowed, LiveCallEvent::Error(message.into()));
                                    capture.stop();
                                    break 'call;
                            }
                        }
                        VoiceEvent::AudioFrame { pcm16le, .. } => {
                            if input_muted.load(Ordering::Acquire) {
                                continue;
                            }
                            if let Err(error) = input_audio.append_input(
                                &pcm16le,
                                SelfHostedRealtimeProfile::RealtimeVenus,
                            ) {
                                emit_event(&events, &overflowed, LiveCallEvent::Error(error));
                                capture.stop();
                                break 'call;
                            }
                            if let Err(error) = queue_audio_chunks(
                                &mut input_audio,
                                SelfHostedRealtimeProfile::RealtimeVenus,
                                false,
                                &audio_sender,
                                &mut event_seq,
                            ) {
                                emit_event(&events, &overflowed, LiveCallEvent::Error(error));
                                capture.stop();
                                break 'call;
                            }
                        }
                        VoiceEvent::Error(error) => {
                            emit_event(&events, &overflowed, LiveCallEvent::Error(error));
                            capture.stop();
                            break 'call;
                        }
                    }
                }
            }
            result = &mut output_poll => {
                match result {
                    Ok(Some(mut step)) => {
                        if step.get("session_id").and_then(Value::as_str) != Some(session_id.as_str())
                            || step.get("incarnation").and_then(Value::as_u64) != Some(incarnation)
                        {
                            emit_event(&events, &overflowed, LiveCallEvent::Error("Realtime-Venus output crossed the active session incarnation.".into()));
                            break;
                        }
                        if let Some(inner) = step.get("model_output_step") {
                            step = inner.clone();
                        }
                        let input_cutoff = step.get("input_seq_cutoff").and_then(Value::as_u64).unwrap_or(0);
                        let generation_id = step.get("generation_id").and_then(Value::as_str).unwrap_or_default().to_string();
                        let step_seq = step.get("step_seq").and_then(Value::as_u64).unwrap_or(0);
                        let at_ms = step.get("at_ms").and_then(Value::as_u64).unwrap_or(opened_at_ms);
                        let offset_ms = at_ms.saturating_sub(opened_at_ms);
                        let token_ids = step.get("total_token_ids")
                            .and_then(Value::as_array)
                            .map(|ids| ids.iter().filter_map(|id| id.as_u64().and_then(|id| u32::try_from(id).ok())).collect::<Vec<_>>())
                            .unwrap_or_default();
                        let turn_finished = step.get("turn_finished").and_then(Value::as_bool).unwrap_or(false);
                        if interrupted_generation.as_deref() == Some(generation_id.as_str()) {
                            if turn_finished {
                                interrupted_generation = None;
                                active_output_generation = None;
                                output_generation_active = false;
                                suppress_current_generation = false;
                            }
                            output_poll = Box::pin(poll_output(&client, &config, &output_url, incarnation));
                            continue;
                        }
                        if turn_finished {
                            if active_output_generation.as_deref() == Some(generation_id.as_str()) {
                                active_output_generation = None;
                            }
                        } else {
                            active_output_generation = Some(generation_id.clone());
                        }
                        if input_cutoff < minimum_input_seq {
                            if turn_finished {
                                interrupted_generation = None;
                                active_output_generation = None;
                                output_generation_active = false;
                                suppress_current_generation = false;
                            }
                            output_poll = Box::pin(poll_output(&client, &config, &output_url, incarnation));
                            continue;
                        }
                        let decoded = match decoder.feed(&generation_id, &token_ids, turn_finished) {
                            Ok(decoded) => decoded,
                            Err(error) => {
                                emit_event(&events, &overflowed, LiveCallEvent::Error(error));
                                break;
                            }
                        };
                        if !suppress_current_generation && !decoded.text_delta.is_empty() {
                            emit_event(&events, &overflowed, LiveCallEvent::OutputTranscriptDelta {
                                text: decoded.text_delta,
                                start_ms: offset_ms,
                                end_ms: offset_ms,
                            });
                        }
                        for (index, request) in decoded.delegations.into_iter().enumerate() {
                            if suppress_current_generation {
                                break;
                            }
                            let delegation_id = format!("venus-{generation_id}-{step_seq}-{index}");
                            emit_event(&events, &overflowed, LiveCallEvent::InputTranscriptDelta {
                                text: request.clone(),
                                start_ms: offset_ms,
                                end_ms: offset_ms,
                            });
                            emit_event(&events, &overflowed, LiveCallEvent::DelegationCreated {
                                delegation_id,
                                offset_ms,
                                request: Some(request),
                            });
                        }
                        if !suppress_current_generation
                            && let Some(audio) = decode_audio(&step)
                            && (!decoded.saw_delegate || delegate_audio_filtered)
                        {
                            match capture.play_pcm16le(&audio) {
                                Ok(()) => {
                                    if playback.queue(&generation_id, audio.len() / 2).is_err() {
                                        emit_event(&events, &overflowed, LiveCallEvent::Error("Realtime-Venus playback queue is full.".into()));
                                        break;
                                    }
                                    emit_event(&events, &overflowed, LiveCallEvent::AudioOutput(audio));
                                }
                                Err(error) => {
                                    emit_event(&events, &overflowed, LiveCallEvent::Error(error));
                                    break;
                                }
                            }
                        }
                        output_generation_active = !turn_finished;
                        if turn_finished {
                            suppress_current_generation = false;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        emit_event(&events, &overflowed, LiveCallEvent::Error(format!("Realtime-Venus output failed: {error}")));
                        break;
                    }
                }
                output_poll = Box::pin(poll_output(&client, &config, &output_url, incarnation));
            }
        }
        if last_usage_update.elapsed() >= Duration::from_secs(15) {
            emit_event(
                &events,
                &overflowed,
                LiveCallEvent::UsageUpdated {
                    duration_seconds: started_at.elapsed().as_secs(),
                },
            );
            last_usage_update = Instant::now();
        }
    }

    ready.store(false, Ordering::Release);
    output_playing.store(false, Ordering::Release);
    capture.stop();
    upload_task.abort();
    ack_task.abort();
    delete_session(&client, &config, &endpoint, &session_id, incarnation).await;
    emit_event(
        &events,
        &overflowed,
        LiveCallEvent::SessionFinalized {
            duration_seconds: Some(started_at.elapsed().as_secs()),
        },
    );
}

struct PendingPrefill {
    work_id: String,
    content: String,
    resume_generation: bool,
    suppress_response: bool,
}

struct AudioUpload {
    event_seq: u64,
    start_ms: u64,
    end_ms: u64,
    pcm16le: Vec<u8>,
}

fn audio_payload(pcm16le: Vec<u8>, muted: bool) -> Vec<u8> {
    if muted {
        vec![0; pcm16le.len()]
    } else {
        pcm16le
    }
}

struct VenusPlaybackAck {
    utterance_id: String,
    cumulative_played_chunks: u64,
}

#[derive(Default)]
struct VenusPlaybackTracker {
    generation: u64,
    pending: VecDeque<(u64, String)>,
    played_chunks: HashMap<String, u64>,
}

impl VenusPlaybackTracker {
    fn queue(&mut self, utterance_id: &str, frames: usize) -> Result<(), ()> {
        if utterance_id.is_empty() || frames == 0 || self.pending.len() >= 128 {
            return Err(());
        }
        self.pending
            .push_back((frames as u64, utterance_id.to_string()));
        Ok(())
    }

    fn played(&mut self, generation: u64, frames: u64) -> Option<VenusPlaybackAck> {
        if generation != self.generation {
            return None;
        }
        let (queued_frames, utterance_id) = self.pending.pop_front()?;
        if frames != queued_frames {
            return None;
        }
        let played = self.played_chunks.entry(utterance_id.clone()).or_default();
        *played = played.saturating_add(1);
        Some(VenusPlaybackAck {
            utterance_id,
            cumulative_played_chunks: *played,
        })
    }

    fn reset(&mut self, generation: u64) {
        self.generation = generation;
        self.pending.clear();
        self.played_chunks.clear();
    }
}

fn queue_audio_chunks(
    audio: &mut SelfHostedAudioBuffer,
    profile: SelfHostedRealtimeProfile,
    finish: bool,
    sender: &mpsc::Sender<AudioUpload>,
    event_seq: &mut u64,
) -> Result<(), String> {
    loop {
        let next = if finish {
            audio.finish_chunk(profile)?
        } else {
            audio.take_chunk(profile)
        };
        let Some((pcm16le, end_ms)) = next else {
            return Ok(());
        };
        let start_ms = end_ms.saturating_sub(profile.input_chunk_ms());
        *event_seq = event_seq.saturating_add(1);
        sender
            .try_send(AudioUpload {
                event_seq: *event_seq,
                start_ms,
                end_ms,
                pcm16le,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => {
                    "Realtime-Venus audio upload queue is full.".to_string()
                }
                mpsc::error::TrySendError::Closed(_) => {
                    "Realtime-Venus audio upload stopped.".to_string()
                }
            })?;
        if finish {
            return Ok(());
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "The bounded worker uses the active call's shared lifecycle state."
)]
async fn upload_audio(
    client: reqwest::Client,
    config: SelfHostedRealtimeConfig,
    endpoint: String,
    session_id: String,
    incarnation: u64,
    mut receiver: mpsc::Receiver<AudioUpload>,
    input_muted: Arc<AtomicBool>,
    events: SyncSender<LiveCallEvent>,
    overflowed: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
) {
    while let Some(upload) = receiver.recv().await {
        let pcm16le =
            audio_payload(upload.pcm16le, input_muted.load(Ordering::Acquire));
        let body = json!({
            "session_id": session_id,
            "incarnation": incarnation,
            "event_seq": upload.event_seq,
            "data": STANDARD.encode(pcm16le),
            "sample_rate_hz": 16_000,
            "start_ms": upload.start_ms,
            "end_ms": upload.end_ms,
            "format": "pcm_s16le",
            "channels": 1,
            "sample_width_bytes": 2
        });
        if let Err(error) = request_json(
            &client,
            &config,
            client
                .post(format!("{endpoint}/sessions/{session_id}/audio"))
                .json(&body),
        )
        .await
        {
            failed.store(true, Ordering::Release);
            emit_event(
                &events,
                &overflowed,
                LiveCallEvent::Error(format!(
                    "Realtime-Venus audio upload failed: {error}"
                )),
            );
            return;
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "The bounded worker sends acknowledgments for one active server session."
)]
async fn send_playback_acks(
    client: reqwest::Client,
    config: SelfHostedRealtimeConfig,
    endpoint: String,
    session_id: String,
    incarnation: u64,
    mut receiver: mpsc::Receiver<VenusPlaybackAck>,
    events: SyncSender<LiveCallEvent>,
    overflowed: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
) {
    while let Some(ack) = receiver.recv().await {
        let result = request_json(
            &client,
            &config,
            client
                .post(format!("{endpoint}/sessions/{session_id}/playback_ack"))
                .json(&json!({
                    "session_id": session_id,
                    "incarnation": incarnation,
                    "utterance_id": ack.utterance_id,
                    "cumulative_played_chunks": ack.cumulative_played_chunks
                })),
        )
        .await;
        if let Err(error) = result {
            failed.store(true, Ordering::Release);
            emit_event(
                &events,
                &overflowed,
                LiveCallEvent::Error(format!(
                    "Realtime-Venus playback acknowledgement failed: {error}"
                )),
            );
            return;
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "These are the fields of one upstream prefill operation."
)]
async fn append_prefill(
    client: &reqwest::Client,
    config: &SelfHostedRealtimeConfig,
    endpoint: &str,
    session_id: &str,
    incarnation: u64,
    work_id: &str,
    content: &str,
    resume_generation: bool,
) -> Result<(), String> {
    request_json(
        client,
        config,
        client
            .post(format!("{endpoint}/sessions/{session_id}/prefill"))
            .json(&json!({
                "session_id": session_id,
                "incarnation": incarnation,
                "work_id": work_id,
                "attempt_id": uuid::Uuid::new_v4().to_string(),
                "text_list": [content],
                "visibility": "private",
                "resume_generation": resume_generation
            })),
    )
    .await
    .map(|_| ())
}

async fn poll_output(
    client: &reqwest::Client,
    config: &SelfHostedRealtimeConfig,
    output_url: &str,
    incarnation: u64,
) -> Result<Option<Value>, String> {
    let mut request = client.post(output_url).query(&[
        ("incarnation", incarnation.to_string()),
        ("timeout_s", "0.5".to_string()),
    ]);
    if !config.api_key.trim().is_empty() {
        request = request.bearer_auth(config.api_key.trim());
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Could not read output: {error}"))?;
    if response.status() == reqwest::StatusCode::NO_CONTENT
        || response.status() == reqwest::StatusCode::REQUEST_TIMEOUT
    {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("Server returned HTTP {}.", response.status()));
    }
    response
        .json::<Value>()
        .await
        .map(Some)
        .map_err(|error| format!("Invalid output response: {error}"))
}

async fn open_session(
    client: &reqwest::Client,
    config: &SelfHostedRealtimeConfig,
    endpoint: &str,
    session_id: &str,
    model: &str,
) -> Result<Value, String> {
    request_json(
        client,
        config,
        client
            .post(format!("{endpoint}/sessions"))
            .json(&json!({"session_id": session_id, "model": model})),
    )
    .await
}

async fn request_json(
    _client: &reqwest::Client,
    config: &SelfHostedRealtimeConfig,
    mut request: reqwest::RequestBuilder,
) -> Result<Value, String> {
    if !config.api_key.trim().is_empty() {
        request = request.bearer_auth(config.api_key.trim());
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Server returned HTTP {}.", response.status()));
    }
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(Value::Null);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("Could not read server response: {error}"))?;
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("Server returned invalid JSON: {error}"))
}

async fn delete_session(
    client: &reqwest::Client,
    config: &SelfHostedRealtimeConfig,
    endpoint: &str,
    session_id: &str,
    incarnation: u64,
) {
    let mut request = client
        .delete(format!("{endpoint}/sessions/{session_id}"))
        .query(&[
            ("incarnation", incarnation.to_string()),
            ("reason", "client_closed".to_string()),
        ]);
    if !config.api_key.trim().is_empty() {
        request = request.bearer_auth(config.api_key.trim());
    }
    if let Err(error) = request.send().await {
        tracing::warn!(%error, "could not close Realtime-Venus session");
    }
}

fn decode_audio(step: &Value) -> Option<Vec<u8>> {
    let audio = step.get("audio")?;
    let sample_rate = audio.get("sample_rate_hz")?.as_u64()?;
    let format = audio.get("format")?.as_str()?;
    if sample_rate != 24_000 || format != "pcm_s16le" {
        return None;
    }
    let encoded = audio.get("data")?.as_str()?;
    STANDARD.decode(encoded).ok().filter(|bytes| {
        !bytes.is_empty()
            && bytes.len() <= 96 * 1024
            && bytes.len().is_multiple_of(2)
    })
}

async fn load_tokenizer(model: &str) -> Result<Tokenizer, String> {
    if std::path::Path::new(model).is_file() {
        let path = model.to_string();
        return tokio::task::spawn_blocking(move || {
            Tokenizer::from_file(&path).map_err(|error| {
                format!("Could not load tokenizer from {path}: {error}")
            })
        })
        .await
        .map_err(|error| format!("Could not load voice tokenizer: {error}"))?;
    }

    let url = match model {
        "inclusionAI/Realtime-Venus/Realtime-Venus-Audio" => Some(
            "https://huggingface.co/inclusionAI/Realtime-Venus/resolve/main/Realtime-Venus-Audio/tokenizer.json",
        ),
        "inclusionAI/Realtime-Venus/Realtime-Venus-Omni" => Some(
            "https://huggingface.co/inclusionAI/Realtime-Venus/resolve/main/Realtime-Venus-Omni/tokenizer.json",
        ),
        _ => None,
    };
    if let Some(url) = url {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|error| {
                format!("Could not create tokenizer downloader: {error}")
            })?;
        let response = client.get(url).send().await.map_err(|error| {
            format!("Could not download the Realtime-Venus tokenizer: {error}")
        })?;
        if response.url().scheme() != "https" {
            return Err(
                "Realtime-Venus tokenizer download did not use HTTPS.".into()
            );
        }
        if !response.status().is_success() {
            return Err(format!(
                "Could not download the Realtime-Venus tokenizer (HTTP {}).",
                response.status()
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > 32 * 1024 * 1024)
        {
            return Err("The Realtime-Venus tokenizer file is too large.".into());
        }
        let bytes = response.bytes().await.map_err(|error| {
            format!("Could not read the Realtime-Venus tokenizer: {error}")
        })?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err("The Realtime-Venus tokenizer file is too large.".into());
        }
        return tokio::task::spawn_blocking(move || {
            Tokenizer::from_bytes(bytes).map_err(|error| {
                format!("Could not parse the Realtime-Venus tokenizer: {error}")
            })
        })
        .await
        .map_err(|error| format!("Could not load voice tokenizer: {error}"))?;
    }

    let model = model.to_string();
    tokio::task::spawn_blocking(move || {
        Tokenizer::from_pretrained(&model, None).map_err(|error| {
            format!(
                "Could not load the Realtime-Venus tokenizer for {model}: {error}"
            )
        })
    })
    .await
    .map_err(|error| format!("Could not load voice tokenizer: {error}"))?
}

pub(super) struct VenusTokenDecoder {
    tokenizer: Tokenizer,
    protocol_ids: HashSet<u32>,
    delegate_open_id: u32,
    delegate_close_id: u32,
    turn_end_id: u32,
    generation_id: Option<String>,
    normal_ids: Vec<u32>,
    normal_emitted: String,
    delegate_ids: Vec<u32>,
    inside_delegate: bool,
    invalid_delegate: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct VenusDecodedStep {
    pub text_delta: String,
    pub delegations: Vec<String>,
    pub saw_delegate: bool,
}

impl VenusTokenDecoder {
    pub(super) fn new(
        tokenizer: Tokenizer,
        special_token_ids: &HashMap<String, u32>,
    ) -> Result<Self, String> {
        for token in PROTOCOL_TOKENS {
            let Some(server_id) = special_token_ids.get(token) else {
                return Err(format!(
                    "Realtime-Venus did not report the {token} token ID."
                ));
            };
            let local_id = tokenizer.token_to_id(token).ok_or_else(|| {
                format!("The configured tokenizer does not contain {token}.")
            })?;
            if local_id != *server_id {
                return Err(format!(
                    "The local tokenizer's {token} ID does not match the server."
                ));
            }
        }
        let protocol_ids = PROTOCOL_TOKENS
            .iter()
            .filter_map(|token| special_token_ids.get(*token).copied())
            .collect::<HashSet<_>>();
        if protocol_ids.len() != PROTOCOL_TOKENS.len() {
            return Err(
                "Realtime-Venus reported duplicate protocol token IDs.".into()
            );
        }
        let delegate_open_id = special_token_ids["<delegate>"];
        let delegate_close_id = special_token_ids["</delegate>"];
        let turn_end_id = special_token_ids["<|turn_eos|>"];
        Ok(Self {
            tokenizer,
            protocol_ids,
            delegate_open_id,
            delegate_close_id,
            turn_end_id,
            generation_id: None,
            normal_ids: Vec::new(),
            normal_emitted: String::new(),
            delegate_ids: Vec::new(),
            inside_delegate: false,
            invalid_delegate: false,
        })
    }

    pub(super) fn feed(
        &mut self,
        generation_id: &str,
        token_ids: &[u32],
        turn_finished: bool,
    ) -> Result<VenusDecodedStep, String> {
        if self.generation_id.as_deref() != Some(generation_id) {
            self.reset_generation(generation_id);
        }
        let mut step = VenusDecodedStep {
            saw_delegate: self.inside_delegate,
            ..Default::default()
        };
        for token_id in token_ids.iter().copied() {
            if token_id == self.delegate_open_id {
                step.saw_delegate = true;
                self.flush_normal(&mut step)?;
                if self.inside_delegate {
                    self.invalid_delegate = true;
                    self.delegate_ids.clear();
                } else {
                    self.inside_delegate = true;
                    self.invalid_delegate = false;
                    self.delegate_ids.clear();
                }
                continue;
            }
            if self.inside_delegate
                && (token_id == self.delegate_close_id
                    || token_id == self.turn_end_id)
            {
                step.saw_delegate = true;
                if !self.invalid_delegate {
                    let query = self
                        .tokenizer
                        .decode(&self.delegate_ids, false)
                        .map_err(|error| {
                            format!(
                                "Could not decode a Realtime-Venus request: {error}"
                            )
                        })?
                        .trim()
                        .to_string();
                    if !query.is_empty()
                        && query.chars().count() <= MAX_DELEGATE_CHARS
                    {
                        step.delegations.push(query);
                    }
                }
                self.delegate_ids.clear();
                self.inside_delegate = false;
                self.invalid_delegate = false;
                continue;
            }
            if self.protocol_ids.contains(&token_id) {
                step.saw_delegate |=
                    self.inside_delegate || token_id == self.delegate_close_id;
                if self.inside_delegate {
                    self.invalid_delegate = true;
                    self.delegate_ids.clear();
                }
                continue;
            }
            if self.inside_delegate {
                if !self.invalid_delegate {
                    self.delegate_ids.push(token_id);
                    if self.delegate_ids.len() > MAX_DELEGATE_TOKENS {
                        self.delegate_ids.clear();
                        self.invalid_delegate = true;
                    }
                }
            } else {
                self.normal_ids.push(token_id);
                if self.normal_ids.len() > MAX_TEXT_TOKENS {
                    return Err(
                        "Realtime-Venus generated too much unsegmented text.".into(),
                    );
                }
            }
        }
        self.flush_normal(&mut step)?;
        if turn_finished {
            self.reset_generation("");
        }
        Ok(step)
    }

    fn flush_normal(&mut self, step: &mut VenusDecodedStep) -> Result<(), String> {
        let decoded =
            self.tokenizer
                .decode(&self.normal_ids, false)
                .map_err(|error| {
                    format!("Could not decode Realtime-Venus speech: {error}")
                })?;
        if let Some(delta) = decoded.strip_prefix(&self.normal_emitted) {
            step.text_delta.push_str(delta);
            self.normal_emitted = decoded;
        }
        Ok(())
    }

    fn reset_generation(&mut self, generation_id: &str) {
        self.generation_id =
            (!generation_id.is_empty()).then(|| generation_id.to_string());
        self.normal_ids.clear();
        self.normal_emitted.clear();
        self.delegate_ids.clear();
        self.inside_delegate = false;
        self.invalid_delegate = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenizers::models::bpe::BPE;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        task::JoinHandle,
    };

    fn make_decoder() -> VenusTokenDecoder {
        let vocab: [(String, u32); 16] = PROTOCOL_TOKENS
            .iter()
            .enumerate()
            .map(|(index, token)| {
                (
                    (*token).to_string(),
                    u32::try_from(index).expect("fixed token fixture size"),
                )
            })
            .chain([
                ("Explain".to_string(), 12),
                ("this".to_string(), 13),
                ("code".to_string(), 14),
                ("Hello".to_string(), 15),
            ])
            .collect::<Vec<_>>()
            .try_into()
            .expect("fixed tokenizer fixture size");
        let tokenizer = Tokenizer::new(
            BPE::builder()
                .vocab_and_merges(vocab, Vec::<(String, String)>::new())
                .build()
                .expect("build BPE fixture"),
        );
        let special = PROTOCOL_TOKENS
            .iter()
            .enumerate()
            .map(|(index, token)| {
                (
                    (*token).to_string(),
                    u32::try_from(index).expect("fixed token fixture size"),
                )
            })
            .collect();
        VenusTokenDecoder::new(tokenizer, &special).expect("valid token map")
    }

    async fn mock_http_server(
        request_count: usize,
    ) -> (String, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local mock server");
        let address = listener.local_addr().expect("read mock server address");
        let server = tokio::spawn(async move {
            let mut requests = Vec::with_capacity(request_count);
            for _ in 0..request_count {
                let (mut stream, _) =
                    listener.accept().await.expect("accept mock request");
                let request = read_http_request(&mut stream).await;
                requests.push(
                    String::from_utf8(request).expect("HTTP request is UTF-8"),
                );
                let response_body = r#"{"session_id":"test-session","incarnation":7,"max_sessions":1,"step_seq":1}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("write mock response");
            }
            requests
        });
        (format!("http://{address}"), server)
    }

    async fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            if let Some(header_end) =
                request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
            {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if request.len() >= header_end + 4 + content_length {
                    break;
                }
            }
            let read = stream.read(&mut chunk).await.expect("read mock request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..read]);
        }
        request
    }

    fn request_body(request: &str) -> Value {
        let (_, body) = request.split_once("\r\n\r\n").expect("HTTP body");
        serde_json::from_str(body).expect("JSON request body")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn venus_http_routes_match_the_upstream_session_contract() {
        let (endpoint, server) = mock_http_server(7).await;
        let client = reqwest::Client::new();
        let config = SelfHostedRealtimeConfig {
            endpoint: endpoint.clone(),
            api_key: "test-token".into(),
            model: "inclusionAI/Realtime-Venus/Realtime-Venus-Audio".into(),
            profile: SelfHostedRealtimeProfile::RealtimeVenus,
            instructions: "Use AHEAD for code work.".into(),
            on_barge_in: None,
        };

        let opened =
            open_session(&client, &config, &endpoint, "test-session", &config.model)
                .await
                .expect("open mock session");
        assert!(validate_session_capacity(&opened).is_ok());

        append_prefill(
            &client,
            &config,
            &endpoint,
            "test-session",
            7,
            "work-1",
            "Explain this code.",
            true,
        )
        .await
        .expect("append delegated result");

        let output = poll_output(
            &client,
            &config,
            &format!("{endpoint}/sessions/test-session/output"),
            7,
        )
        .await
        .expect("poll mock output")
        .expect("mock output step");
        assert_eq!(output["step_seq"], 1);

        let (audio_sender, audio_receiver) = mpsc::channel(2);
        audio_sender
            .send(AudioUpload {
                event_seq: 1,
                start_ms: 0,
                end_ms: 200,
                pcm16le: vec![1, 0, 2, 0],
            })
            .await
            .expect("queue first audio frame");
        audio_sender
            .send(AudioUpload {
                event_seq: 2,
                start_ms: 200,
                end_ms: 400,
                pcm16le: vec![3, 0, 4, 0],
            })
            .await
            .expect("queue second audio frame");
        drop(audio_sender);
        let input_muted = Arc::new(AtomicBool::new(true));
        let overflowed = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let (event_sender, _event_receiver) = std::sync::mpsc::sync_channel(8);
        upload_audio(
            client.clone(),
            config.clone(),
            endpoint.clone(),
            "test-session".into(),
            7,
            audio_receiver,
            input_muted,
            event_sender.clone(),
            overflowed.clone(),
            failed.clone(),
        )
        .await;
        assert!(!failed.load(Ordering::Acquire));

        let (ack_sender, ack_receiver) = mpsc::channel(1);
        ack_sender
            .send(VenusPlaybackAck {
                utterance_id: "foreground-test-session:1".into(),
                cumulative_played_chunks: 1,
            })
            .await
            .expect("queue playback acknowledgment");
        drop(ack_sender);
        send_playback_acks(
            client.clone(),
            config.clone(),
            endpoint.clone(),
            "test-session".into(),
            7,
            ack_receiver,
            event_sender,
            overflowed,
            failed.clone(),
        )
        .await;
        assert!(!failed.load(Ordering::Acquire));

        delete_session(&client, &config, &endpoint, "test-session", 7).await;
        let requests = server.await.expect("mock server completes");
        assert_eq!(requests.len(), 7);
        assert!(requests.iter().all(|request| request.lines().any(|line| {
            line.eq_ignore_ascii_case("authorization: bearer test-token")
        })));
        assert!(requests[0].starts_with("POST /sessions HTTP/1.1"));
        let session_open = request_body(&requests[0]);
        assert_eq!(session_open["session_id"], "test-session");
        assert_eq!(session_open["model"], config.model);
        assert!(session_open.get("protocol_version").is_none());
        assert!(
            requests[1].starts_with("POST /sessions/test-session/prefill HTTP/1.1")
        );
        let prefill = request_body(&requests[1]);
        assert_eq!(prefill["text_list"][0], "Explain this code.");
        assert_eq!(prefill["visibility"], "private");
        assert!(requests[2].starts_with("POST /sessions/test-session/output?"));
        assert!(requests[2].contains("incarnation=7"));
        for (request, expected_event_seq) in requests[3..5].iter().zip([1, 2]) {
            assert!(
                request.starts_with("POST /sessions/test-session/audio HTTP/1.1")
            );
            let body = request_body(request);
            assert_eq!(body["event_seq"], expected_event_seq);
            assert_eq!(body["format"], "pcm_s16le");
            let audio = STANDARD
                .decode(body["data"].as_str().expect("base64 audio"))
                .expect("decode audio");
            assert_eq!(audio.len(), 4);
            assert!(audio.iter().all(|sample| *sample == 0));
        }
        assert!(
            requests[5]
                .starts_with("POST /sessions/test-session/playback_ack HTTP/1.1")
        );
        let acknowledgment = request_body(&requests[5]);
        assert_eq!(acknowledgment["cumulative_played_chunks"], 1);
        assert!(requests[6].starts_with("DELETE /sessions/test-session?"));
        assert!(requests[6].contains("incarnation=7"));
    }

    #[test]
    fn full_audio_queue_fails_fast_without_blocking_capture() {
        let (sender, _receiver) = mpsc::channel(1);
        sender
            .try_send(AudioUpload {
                event_seq: 1,
                start_ms: 0,
                end_ms: 200,
                pcm16le: vec![0; 6_400],
            })
            .expect("fill bounded upload queue");
        let mut audio = SelfHostedAudioBuffer::default();
        audio
            .append_input(&[0; 9_600], SelfHostedRealtimeProfile::RealtimeVenus)
            .expect("prepare one Venus chunk");
        let mut event_seq = 1;
        let error = queue_audio_chunks(
            &mut audio,
            SelfHostedRealtimeProfile::RealtimeVenus,
            false,
            &sender,
            &mut event_seq,
        )
        .expect_err("full queue must stop the call instead of blocking capture");
        assert_eq!(error, "Realtime-Venus audio upload queue is full.");
        assert_eq!(event_seq, 2);
    }

    #[test]
    fn protocol_spans_become_private_delegations_not_spoken_text() {
        let mut decoder = make_decoder();
        let step = decoder
            .feed("foreground-1", &[15, 8, 12, 13, 14, 9, 15], false)
            .expect("decode mixed step");
        assert_eq!(step.text_delta, "Hello Hello");
        assert_eq!(step.delegations, ["Explain this code"]);
        assert!(step.saw_delegate);
    }

    #[test]
    fn unclosed_or_malformed_delegate_spans_are_discarded() {
        let mut decoder = make_decoder();
        let open = decoder
            .feed("foreground-1", &[8, 12, 13], false)
            .expect("decode partial delegate");
        assert!(open.delegations.is_empty());
        assert!(open.saw_delegate);
        let closed = decoder
            .feed("foreground-1", &[], true)
            .expect("discard after generation boundary");
        assert!(closed.delegations.is_empty());

        let mut decoder = make_decoder();
        let nested = decoder
            .feed("foreground-2", &[8, 12, 8, 14, 9], false)
            .expect("discard nested delegate");
        assert!(nested.delegations.is_empty());
    }

    #[test]
    fn text_and_delegate_state_reset_between_generations() {
        let mut decoder = make_decoder();
        let first = decoder
            .feed("foreground-1", &[15], false)
            .expect("decode first generation");
        assert_eq!(first.text_delta, "Hello");
        let second = decoder
            .feed("foreground-2", &[15], false)
            .expect("decode next generation");
        assert_eq!(second.text_delta, "Hello");
    }

    #[test]
    fn venus_url_requires_authenticated_remote_tls_and_rejects_url_credentials() {
        assert_eq!(
            base_url("https://voice.example.com:8000/").as_deref(),
            Ok("https://voice.example.com:8000")
        );
        assert_eq!(
            base_url("http://127.0.0.1:8000/").as_deref(),
            Ok("http://127.0.0.1:8000")
        );
        assert!(base_url("http://voice.example.com:8000/").is_err());
        assert!(base_url("https://user:secret@voice.example.com:8000").is_err());
    }

    #[test]
    fn session_opened_response_requires_the_upstream_single_session_limit() {
        assert!(validate_session_capacity(&json!({"max_sessions": 1})).is_ok());
        assert!(
            validate_session_capacity(&json!({"capabilities": {"max_sessions": 1}}))
                .is_ok()
        );
        assert!(validate_session_capacity(&json!({"max_sessions": 4})).is_err());
        assert!(validate_session_capacity(&json!({})).is_err());
    }

    #[test]
    fn muted_queued_audio_keeps_its_sequence_slot_without_sending_mic_samples() {
        let samples = vec![1, 2, 255, 128];
        let silent = audio_payload(samples.clone(), true);
        assert_eq!(silent.len(), samples.len());
        assert!(silent.iter().all(|sample| *sample == 0));
        assert_eq!(audio_payload(samples.clone(), false), samples);
    }
}
