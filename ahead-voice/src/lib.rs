//! Native microphone capture and speaker playback for remote voice sessions.
#![allow(unsafe_code)] // The macOS audio backend is an explicit native FFI boundary.

pub mod live;

pub use live::{
    LiveCallEvent, LiveVoiceCall, OpenAiLiveConfig, SelfHostedRealtimeConfig,
    SelfHostedRealtimeProfile,
};

#[cfg(target_os = "macos")]
use std::{ffi::CStr, ptr::NonNull};
use std::{
    ffi::{c_char, c_void},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEvent {
    Ready,
    SpeechStarted { generation: u64 },
    SpeechEnded { generation: u64 },
    PlaybackStarted,
    PlaybackEnded,
    PlaybackChunkPlayed { generation: u64, frames: u64 },
    AudioFrame { sequence: u64, pcm16le: Vec<u8> },
    Error(String),
}

pub type BargeInHandler = Arc<dyn Fn() + Send + Sync>;

struct CallbackState {
    generation: AtomicU64,
    sequence: AtomicU64,
    output_playing: Arc<AtomicBool>,
    sender: SyncSender<VoiceEvent>,
    overflowed: AtomicBool,
}

/// Owns one native microphone and speaker session. Input is mono PCM16LE at
/// 24 kHz; capture is muted locally and queues fail visibly if the app stalls.
pub struct VoiceCapture {
    receiver: Receiver<VoiceEvent>,
    callback_state: Arc<CallbackState>,
    stopped: bool,
    #[cfg(target_os = "macos")]
    handle: NonNull<c_void>,
}

impl VoiceCapture {
    /// Requests microphone access and starts the native audio engine.
    pub fn start(output_playing: Arc<AtomicBool>) -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        {
            let (sender, receiver) = mpsc::sync_channel(48);
            let callback_state = Arc::new(CallbackState {
                generation: AtomicU64::new(1),
                sequence: AtomicU64::new(0),
                output_playing,
                sender,
                overflowed: AtomicBool::new(false),
            });
            let callback_context = Arc::into_raw(callback_state.clone());
            let handle = unsafe {
                ahead_voice_create(
                    voice_event_callback,
                    release_callback_context,
                    callback_context.cast_mut().cast(),
                )
            };
            let Some(handle) = NonNull::new(handle) else {
                unsafe {
                    drop(Arc::from_raw(callback_context));
                }
                return Err("Could not initialize native voice audio.".into());
            };
            unsafe { ahead_voice_start(handle.as_ptr()) };
            Ok(Self {
                receiver,
                callback_state,
                stopped: false,
                handle,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("Native voice calls are currently supported on macOS only.".into())
        }
    }

    pub fn take_events(&self) -> Vec<VoiceEvent> {
        let mut events = self.receiver.try_iter().collect::<Vec<_>>();
        if self.callback_state.overflowed.swap(false, Ordering::AcqRel) {
            events.push(VoiceEvent::Error(
                "Microphone audio could not keep up with the voice connection."
                    .into(),
            ));
        }
        events
    }

    /// Mutes samples at the device callback before they can reach the network.
    pub fn set_muted(&self, muted: bool) {
        #[cfg(target_os = "macos")]
        unsafe {
            ahead_voice_set_muted(self.handle.as_ptr(), muted);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = muted;
    }

    pub fn play_pcm16le(&self, audio: &[u8]) -> Result<(), String> {
        if !audio.len().is_multiple_of(2) {
            return Err("Speaker audio must contain complete PCM16 samples.".into());
        }
        if audio.len() > 96 * 1024 {
            return Err(
                "Speaker audio chunk exceeds the bounded playback queue.".into()
            );
        }
        #[cfg(target_os = "macos")]
        unsafe {
            ahead_voice_play(self.handle.as_ptr(), audio.as_ptr(), audio.len());
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = audio;
            Err("Native voice playback is currently supported on macOS only.".into())
        }
    }

    pub fn stop_playback(&self) -> u64 {
        #[cfg(target_os = "macos")]
        {
            unsafe { ahead_voice_stop_playback(self.handle.as_ptr()) }
        }
        #[cfg(not(target_os = "macos"))]
        {
            0
        }
    }

    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        #[cfg(target_os = "macos")]
        unsafe {
            ahead_voice_stop(self.handle.as_ptr());
        }
        self.stopped = true;
    }

    pub fn generation(&self) -> u64 {
        self.callback_state.generation.load(Ordering::Acquire)
    }
}

#[cfg(target_os = "macos")]
impl Drop for VoiceCapture {
    fn drop(&mut self) {
        self.stop();
        unsafe { ahead_voice_destroy(self.handle.as_ptr()) };
    }
}

#[cfg(target_os = "macos")]
type VoiceEventCallback =
    extern "C" fn(*mut c_void, i32, *const u8, usize, u64, *const c_char);

#[cfg(target_os = "macos")]
type ReleaseContextCallback = extern "C" fn(*const c_void);

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn ahead_voice_create(
        callback: VoiceEventCallback,
        release_context: ReleaseContextCallback,
        context: *mut c_void,
    ) -> *mut c_void;
    fn ahead_voice_start(handle: *mut c_void);
    fn ahead_voice_set_muted(handle: *mut c_void, muted: bool);
    fn ahead_voice_play(handle: *mut c_void, audio: *const u8, length: usize);
    fn ahead_voice_stop_playback(handle: *mut c_void) -> u64;
    fn ahead_voice_stop(handle: *mut c_void);
    fn ahead_voice_destroy(handle: *mut c_void);
}

#[cfg(target_os = "macos")]
extern "C" fn voice_event_callback(
    context: *mut c_void,
    event: i32,
    audio: *const u8,
    length: usize,
    value: u64,
    error: *const c_char,
) {
    if context.is_null() {
        return;
    }
    let callback_state = unsafe { &*(context.cast::<CallbackState>()) };
    match event {
        1 => send_voice_event(
            &callback_state.sender,
            &callback_state.overflowed,
            VoiceEvent::Ready,
        ),
        2 => {
            let generation = callback_state
                .generation
                .fetch_add(1, Ordering::AcqRel)
                .saturating_add(1);
            send_voice_event(
                &callback_state.sender,
                &callback_state.overflowed,
                VoiceEvent::SpeechStarted { generation },
            );
        }
        5 => {
            let generation = callback_state.generation.load(Ordering::Acquire);
            send_voice_event(
                &callback_state.sender,
                &callback_state.overflowed,
                VoiceEvent::SpeechEnded { generation },
            );
        }
        6 => {
            callback_state.output_playing.store(true, Ordering::Release);
            send_voice_event(
                &callback_state.sender,
                &callback_state.overflowed,
                VoiceEvent::PlaybackStarted,
            );
        }
        7 => {
            callback_state
                .output_playing
                .store(false, Ordering::Release);
            send_voice_event(
                &callback_state.sender,
                &callback_state.overflowed,
                VoiceEvent::PlaybackEnded,
            );
        }
        8 if length > 0 => send_voice_event(
            &callback_state.sender,
            &callback_state.overflowed,
            VoiceEvent::PlaybackChunkPlayed {
                generation: value,
                frames: length as u64,
            },
        ),
        3 if !audio.is_null() && length > 0 => {
            let bytes =
                unsafe { std::slice::from_raw_parts(audio, length) }.to_vec();
            let sequence = callback_state.sequence.fetch_add(1, Ordering::Relaxed);
            send_voice_event(
                &callback_state.sender,
                &callback_state.overflowed,
                VoiceEvent::AudioFrame {
                    sequence,
                    pcm16le: bytes,
                },
            );
        }
        4 => {
            let message = unsafe { c_string(error) };
            send_voice_event(
                &callback_state.sender,
                &callback_state.overflowed,
                VoiceEvent::Error(message),
            );
        }
        _ => {}
    }
}

#[cfg(target_os = "macos")]
fn send_voice_event(
    sender: &SyncSender<VoiceEvent>,
    overflowed: &AtomicBool,
    event: VoiceEvent,
) {
    if let Err(TrySendError::Full(_)) = sender.try_send(event) {
        overflowed.store(true, Ordering::Release);
    }
}

#[cfg(target_os = "macos")]
unsafe fn c_string(value: *const c_char) -> String {
    if value.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(target_os = "macos")]
extern "C" fn release_callback_context(context: *const c_void) {
    if !context.is_null() {
        unsafe { drop(Arc::from_raw(context.cast::<CallbackState>())) };
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::VoiceCapture;

    #[test]
    fn no_platform_capture_claim_is_made_without_a_native_backend() {
        assert!(VoiceCapture::start(Arc::new(AtomicBool::new(false))).is_err());
    }
}
