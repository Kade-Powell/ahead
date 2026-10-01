//! Local microphone capture with private, on-device speech recognition.
#![allow(unsafe_code)] // The macOS backend is an explicit native FFI boundary.

#[cfg(target_os = "macos")]
use std::{ffi::CStr, ptr::NonNull};
use std::{
    ffi::{c_char, c_void},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
};

use ahead_rpc::ahead::VoiceTranscriptUpdate;

/// Events from an active local microphone session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEvent {
    /// The microphone detected the start of a spoken utterance.
    SpeechStarted { generation: u64 },
    /// A partial or finalized transcription.
    Transcript(VoiceTranscriptUpdate),
    /// Capture or local recognition could not continue.
    Error(String),
}

pub type BargeInHandler = Arc<dyn Fn() + Send + Sync>;

struct CallbackState {
    voice_session_id: String,
    generation: AtomicU64,
    sender: Sender<VoiceEvent>,
    on_barge_in: Option<BargeInHandler>,
}

/// Owns one live microphone capture session.
pub struct VoiceCapture {
    receiver: Receiver<VoiceEvent>,
    callback_state: Arc<CallbackState>,
    stopped: bool,
    #[cfg(target_os = "macos")]
    handle: NonNull<c_void>,
}

impl VoiceCapture {
    /// Starts capture only after the user explicitly enables voice input.
    ///
    /// On macOS, recognition is required to run on-device. The service fails
    /// closed if the current system language cannot be recognized locally.
    pub fn start(on_barge_in: Option<BargeInHandler>) -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        {
            let (sender, receiver) = mpsc::channel();
            let callback_state = Arc::new(CallbackState {
                voice_session_id: uuid::Uuid::new_v4().to_string(),
                generation: AtomicU64::new(1),
                sender,
                on_barge_in,
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
                return Err("Could not initialize local speech capture.".to_string());
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
            let _ = on_barge_in;
            Err("Local microphone transcription is currently supported on macOS only.".to_string())
        }
    }

    /// Returns all events received since the previous poll.
    pub fn take_events(&self) -> Vec<VoiceEvent> {
        self.receiver.try_iter().collect()
    }

    /// Mutes capture without stopping the active editor/agent session.
    pub fn set_muted(&self, muted: bool) {
        #[cfg(target_os = "macos")]
        unsafe {
            ahead_voice_set_muted(self.handle.as_ptr(), muted);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = muted;
    }

    /// Stops capture while leaving pending final transcript events readable.
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

    /// Returns the identity shared by transcript events from this capture.
    pub fn voice_session_id(&self) -> &str {
        &self.callback_state.voice_session_id
    }

    /// Returns the active speech generation used to reject stale transcripts.
    pub fn generation(&self) -> u64 {
        self.callback_state.generation.load(Ordering::SeqCst)
    }
}

#[cfg(target_os = "macos")]
impl Drop for VoiceCapture {
    fn drop(&mut self) {
        self.stop();
        unsafe {
            ahead_voice_destroy(self.handle.as_ptr());
        }
    }
}

#[cfg(target_os = "macos")]
type VoiceEventCallback =
    extern "C" fn(*mut c_void, i32, *const c_char, *const c_char);

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
    fn ahead_voice_stop(handle: *mut c_void);
    fn ahead_voice_destroy(handle: *mut c_void);
}

#[cfg(target_os = "macos")]
extern "C" fn voice_event_callback(
    context: *mut c_void,
    event: i32,
    text: *const c_char,
    error: *const c_char,
) {
    if context.is_null() {
        return;
    }
    let callback_state = unsafe { &*(context.cast::<CallbackState>()) };
    match event {
        1 => {
            let text = unsafe { c_string(text) };
            let generation = callback_state.generation.load(Ordering::SeqCst);
            let update = VoiceTranscriptUpdate {
                voice_session_id: callback_state.voice_session_id.clone(),
                epoch: 1,
                generation,
                text,
                is_final: false,
                speaker_id: "human".to_string(),
            };
            send_voice_event(&callback_state.sender, VoiceEvent::Transcript(update));
        }
        2 => {
            let generation = callback_state
                .generation
                .fetch_add(1, Ordering::SeqCst)
                .saturating_add(1);
            if let Some(handler) = callback_state.on_barge_in.as_ref() {
                handler();
            }
            send_voice_event(
                &callback_state.sender,
                VoiceEvent::SpeechStarted { generation },
            );
        }
        3 => {
            let text = unsafe { c_string(text) };
            let generation = callback_state.generation.load(Ordering::SeqCst);
            let update = VoiceTranscriptUpdate {
                voice_session_id: callback_state.voice_session_id.clone(),
                epoch: 1,
                generation,
                text,
                is_final: true,
                speaker_id: "human".to_string(),
            };
            send_voice_event(&callback_state.sender, VoiceEvent::Transcript(update));
        }
        4 => {
            let message = unsafe { c_string(error) };
            send_voice_event(&callback_state.sender, VoiceEvent::Error(message));
        }
        _ => {}
    }
}

#[cfg(target_os = "macos")]
fn send_voice_event(sender: &Sender<VoiceEvent>, event: VoiceEvent) {
    if sender.send(event).is_err() {
        tracing::warn!("AHEAD voice event receiver was dropped");
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
        unsafe {
            drop(Arc::from_raw(context.cast::<CallbackState>()));
        }
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::VoiceCapture;

    #[test]
    fn no_platform_capture_claim_is_made_without_a_native_backend() {
        #[cfg(not(target_os = "macos"))]
        assert!(VoiceCapture::start(None).is_err());
    }
}
