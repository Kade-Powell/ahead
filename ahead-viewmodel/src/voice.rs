//! Voice input/output state mirrored into the editor UI.

/// Active microphone, listener, speaker, mute, and interruption state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VoiceIntent {
    pub active: bool,
    pub listening: bool,
    pub speaking: bool,
    pub mic_muted: bool,
    pub generation: u64,
}

impl VoiceIntent {
    pub fn new() -> Self {
        Self {
            generation: 1,
            ..Default::default()
        }
    }
}

/// Toggles intent. Turning off also clears speaking.
pub fn toggle(intent: &mut VoiceIntent) {
    intent.active = !intent.active;
    intent.listening = intent.active;
    if !intent.active {
        intent.speaking = false;
    }
}

/// Barge-in: bumps the output generation and stops speaking locally.
/// Input capture and coding tasks continue; stale generations are dropped.
pub fn barge_in(intent: &mut VoiceIntent) -> u64 {
    intent.generation += 1;
    intent.speaking = false;
    intent.generation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_tracks_intent() {
        let mut v = VoiceIntent::new();
        toggle(&mut v);
        assert!(v.active && v.listening);
        toggle(&mut v);
        assert!(!v.active && !v.listening && !v.speaking);
    }

    #[test]
    fn barge_in_bumps_generation() {
        let mut v = VoiceIntent::new();
        v.speaking = true;
        let g = barge_in(&mut v);
        assert_eq!(g, 2);
        assert!(!v.speaking);
    }
}
