//! AHEAD Real-time Full-Duplex Voice Runtime Probe
//!
//! Grounded in Section 8.2 of `ahead-editor-mvp.md`.
//! Decouples microphone capture, incremental transcription, audio playback,
//! and background coding tasks.
//! Implements strict barge-in generation tracking and separates speech interruption
//! from task cancellation.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};
use parking_lot::Mutex;
use lapce_rpc::ahead::{Id, VoiceControl};

#[derive(Debug, Clone)]
pub struct QueuedAudioFrame {
    pub generation: u64,
    pub sequence: u64,
    pub payload_bytes: Vec<u8>,
    pub enqueued_at: Instant,
}

pub struct VoiceSession {
    pub session_id: Id,
    pub voice_session_id: Id,
    epoch: AtomicU64,
    current_generation: AtomicU64,
    mic_muted: AtomicBool,
    speaker_muted: AtomicBool,
    audio_output_queue: Arc<Mutex<VecDeque<QueuedAudioFrame>>>,
    active_coding_tasks: Arc<Mutex<Vec<Id>>>,
}

impl VoiceSession {
    pub fn new(session_id: Id, voice_session_id: Id) -> Self {
        Self {
            session_id,
            voice_session_id,
            epoch: AtomicU64::new(1),
            current_generation: AtomicU64::new(1),
            mic_muted: AtomicBool::new(false),
            speaker_muted: AtomicBool::new(false),
            audio_output_queue: Arc::new(Mutex::new(VecDeque::new())),
            active_coding_tasks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn current_generation(&self) -> u64 {
        self.current_generation.load(Ordering::SeqCst)
    }

    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    pub fn register_coding_task(&self, task_id: Id) {
        let mut tasks = self.active_coding_tasks.lock();
        tasks.push(task_id);
    }

    pub fn is_task_active(&self, task_id: &str) -> bool {
        let tasks = self.active_coding_tasks.lock();
        tasks.iter().any(|t| t == task_id)
    }

    /// Enqueues an audio frame for output playback
    pub fn enqueue_audio(&self, frame: QueuedAudioFrame) -> bool {
        if self.speaker_muted.load(Ordering::SeqCst) {
            return false;
        }
        if frame.generation != self.current_generation() {
            // Drop stale generation frame immediately
            return false;
        }
        let mut q = self.audio_output_queue.lock();
        q.push_back(frame);
        true
    }

    /// Handles voice control messages
    pub fn handle_control(&self, control: VoiceControl) {
        match control {
            VoiceControl::InterruptPlayback { generation } => {
                let current = self.current_generation();
                if generation >= current {
                    let new_gen = generation + 1;
                    self.current_generation.store(new_gen, Ordering::SeqCst);
                    // Barge-in: immediately drain the audio queue
                    let mut q = self.audio_output_queue.lock();
                    q.clear();
                }
            }
            VoiceControl::SetMicMuted { muted } => {
                self.mic_muted.store(muted, Ordering::SeqCst);
            }
            VoiceControl::SetSpeakerMuted { muted } => {
                self.speaker_muted.store(muted, Ordering::SeqCst);
                if muted {
                    let mut q = self.audio_output_queue.lock();
                    q.clear();
                }
            }
            VoiceControl::CancelCodingTask { task_id } => {
                let mut tasks = self.active_coding_tasks.lock();
                tasks.retain(|t| t != &task_id);
            }
        }
    }

    pub fn pending_frames_count(&self) -> usize {
        let q = self.audio_output_queue.lock();
        q.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_barge_in_clears_queued_audio_instantly_without_aborting_coding_task() {
        let voice = VoiceSession::new("sess-1".to_string(), "voice-1".to_string());
        voice.register_coding_task("task-long-code-123".to_string());
        assert!(voice.is_task_active("task-long-code-123"));

        let gen1 = voice.current_generation();

        // Enqueue several audio frames for generation 1
        for i in 0..5 {
            voice.enqueue_audio(QueuedAudioFrame {
                generation: gen1,
                sequence: i,
                payload_bytes: vec![0u8; 320], // 20ms frame
                enqueued_at: Instant::now(),
            });
        }
        assert_eq!(voice.pending_frames_count(), 5);

        // Human barges in (interrupt playback)
        let start = Instant::now();
        voice.handle_control(VoiceControl::InterruptPlayback { generation: gen1 });
        let elapsed = start.elapsed();

        // Must clear queue in well under 150ms (latency budget)
        assert!(elapsed < Duration::from_millis(50));
        assert_eq!(voice.pending_frames_count(), 0);

        // Crucial invariant: The background coding task is NOT cancelled by conversational barge-in!
        assert!(voice.is_task_active("task-long-code-123"));

        // Stale generation frames arriving late are rejected
        let late_frame = QueuedAudioFrame {
            generation: gen1,
            sequence: 6,
            payload_bytes: vec![0u8; 320],
            enqueued_at: Instant::now(),
        };
        let accepted = voice.enqueue_audio(late_frame);
        assert!(!accepted);
        assert_eq!(voice.pending_frames_count(), 0);
    }

    #[test]
    fn test_explicit_task_cancellation_cancels_task() {
        let voice = VoiceSession::new("sess-1".to_string(), "voice-1".to_string());
        voice.register_coding_task("task-456".to_string());
        assert!(voice.is_task_active("task-456"));

        voice.handle_control(VoiceControl::CancelCodingTask {
            task_id: "task-456".to_string(),
        });
        assert!(!voice.is_task_active("task-456"));
    }
}
