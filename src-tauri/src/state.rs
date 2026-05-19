//! Application state machine — pure state tag with transition guards.
//!
//! All resources (audio buffer, partial results, text, clipboard state)
//! have been extracted to external holders (AudioRingBuffer, PipelineState).
//! The state machine only validates which transitions are legal.
//!
//! ```text
//! Idle → Recording → Transcribing → LLMRefining → Injecting → Idle
//!                              ↘ Reviewing ↗         ↗
//!                                   ↓      (review disabled)
//!                              Injecting → Idle
//! * → Idle  (error/cancel)
//! ```

/// Lightweight state tag — no associated data.
///
/// Previous versions of `AppState` carried resources (audio_buffer,
/// partial_results, text, saved_clipboard). These have been extracted
/// to eliminate lock contention and simplify the state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateTag {
    /// No recording in progress.
    Idle,
    /// Hotkey pressed, audio is being captured.
    Recording,
    /// Hotkey released, Whisper is transcribing.
    Transcribing,
    /// LLM is refining the transcribed text.
    LLMRefining,
    /// Transcription done, waiting for user to review/edit text before injection.
    Reviewing,
    /// Text is ready, injecting via clipboard paste.
    Injecting,
}

/// Error for invalid state transitions.
#[derive(Debug, thiserror::Error)]
#[error("invalid state transition: {from} → {to}")]
pub struct TransitionError {
    pub from: String,
    pub to: String,
}

/// Wraps StateTag with controlled transitions.
///
/// All resources have been extracted; this struct only guards legal transitions.
pub struct StateMachine {
    tag: StateTag,
}

impl StateMachine {
    pub fn new() -> Self {
        Self {
            tag: StateTag::Idle,
        }
    }

    pub fn state(&self) -> StateTag {
        self.tag
    }

    /// Transition to Recording (from Idle only).
    pub fn start_recording(&mut self) -> Result<(), TransitionError> {
        match self.tag {
            StateTag::Idle => {
                self.tag = StateTag::Recording;
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Recording".to_string(),
            }),
        }
    }

    /// Transition to Transcribing (from Recording only).
    pub fn stop_recording(&mut self) -> Result<(), TransitionError> {
        match self.tag {
            StateTag::Recording => {
                self.tag = StateTag::Transcribing;
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Transcribing".to_string(),
            }),
        }
    }

    /// Transition from Transcribing to LLMRefining.
    pub fn start_llm_refining(&mut self) -> Result<(), TransitionError> {
        match self.tag {
            StateTag::Transcribing => {
                self.tag = StateTag::LLMRefining;
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "LLMRefining".to_string(),
            }),
        }
    }

    /// Transition from Transcribing to Injecting (LLM disabled path).
    pub fn transcribing_to_injecting(&mut self) -> Result<(), TransitionError> {
        match self.tag {
            StateTag::Transcribing => {
                self.tag = StateTag::Injecting;
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Injecting".to_string(),
            }),
        }
    }

    /// Transition from LLMRefining to Injecting.
    pub fn llm_to_injecting(&mut self) -> Result<(), TransitionError> {
        match self.tag {
            StateTag::LLMRefining => {
                self.tag = StateTag::Injecting;
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Injecting".to_string(),
            }),
        }
    }

    /// Transition from Transcribing to Reviewing.
    pub fn transcribing_to_reviewing(&mut self) -> Result<(), TransitionError> {
        match self.tag {
            StateTag::Transcribing => {
                self.tag = StateTag::Reviewing;
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Reviewing".to_string(),
            }),
        }
    }

    /// Transition from LLMRefining to Reviewing.
    pub fn llm_to_reviewing(&mut self) -> Result<(), TransitionError> {
        match self.tag {
            StateTag::LLMRefining => {
                self.tag = StateTag::Reviewing;
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Reviewing".to_string(),
            }),
        }
    }

    /// Transition from Reviewing to Injecting (user confirmed).
    pub fn reviewing_to_injecting(&mut self) -> Result<(), TransitionError> {
        match self.tag {
            StateTag::Reviewing => {
                self.tag = StateTag::Injecting;
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Injecting".to_string(),
            }),
        }
    }

    /// Cancel review and return to Idle (user cancelled).
    pub fn cancel_reviewing(&mut self) -> Result<(), TransitionError> {
        match self.tag {
            StateTag::Reviewing => {
                self.tag = StateTag::Idle;
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Idle".to_string(),
            }),
        }
    }

    /// Transition from Injecting to Idle.
    pub fn finish_injecting(&mut self) -> Result<(), TransitionError> {
        match self.tag {
            StateTag::Injecting => {
                self.tag = StateTag::Idle;
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Idle".to_string(),
            }),
        }
    }

    /// Reset to Idle from any state (error/cancel).
    pub fn reset(&mut self) {
        self.tag = StateTag::Idle;
    }

    pub(crate) fn state_name(&self) -> String {
        match self.tag {
            StateTag::Idle => "Idle".to_string(),
            StateTag::Recording => "Recording".to_string(),
            StateTag::Transcribing => "Transcribing".to_string(),
            StateTag::LLMRefining => "LLMRefining".to_string(),
            StateTag::Reviewing => "Reviewing".to_string(),
            StateTag::Injecting => "Injecting".to_string(),
        }
    }
}

impl Default for StateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_happy_path_with_llm() {
        let mut sm = StateMachine::new();
        assert_eq!(sm.state(), StateTag::Idle);

        sm.start_recording().unwrap();
        assert_eq!(sm.state(), StateTag::Recording);

        sm.stop_recording().unwrap();
        assert_eq!(sm.state(), StateTag::Transcribing);

        sm.start_llm_refining().unwrap();
        assert_eq!(sm.state(), StateTag::LLMRefining);

        sm.llm_to_injecting().unwrap();
        assert_eq!(sm.state(), StateTag::Injecting);

        sm.finish_injecting().unwrap();
        assert_eq!(sm.state(), StateTag::Idle);
    }

    #[test]
    fn test_happy_path_without_llm() {
        let mut sm = StateMachine::new();
        sm.start_recording().unwrap();
        sm.stop_recording().unwrap();
        sm.transcribing_to_injecting().unwrap();
        sm.finish_injecting().unwrap();
        assert_eq!(sm.state(), StateTag::Idle);
    }

    #[test]
    fn test_reset_from_any_state() {
        let mut sm = StateMachine::new();
        sm.start_recording().unwrap();
        sm.reset();
        assert_eq!(sm.state(), StateTag::Idle);

        sm.start_recording().unwrap();
        sm.stop_recording().unwrap();
        sm.reset();
        assert_eq!(sm.state(), StateTag::Idle);
    }

    #[test]
    fn test_invalid_start_recording() {
        let mut sm = StateMachine::new();
        sm.start_recording().unwrap();
        assert!(sm.start_recording().is_err());
    }

    #[test]
    fn test_invalid_stop_recording() {
        let mut sm = StateMachine::new();
        assert!(sm.stop_recording().is_err());
    }

    #[test]
    fn test_invalid_transitions() {
        let mut sm = StateMachine::new();
        assert!(sm.start_llm_refining().is_err());
        assert!(sm.transcribing_to_injecting().is_err());
        assert!(sm.llm_to_injecting().is_err());
        assert!(sm.finish_injecting().is_err());
    }

    #[test]
    fn test_transition_error_message() {
        let mut sm = StateMachine::new();
        let err = sm.stop_recording().unwrap_err();
        assert!(err.to_string().contains("Idle"));
        assert!(err.to_string().contains("Transcribing"));
    }

    #[test]
    fn test_happy_path_with_review() {
        let mut sm = StateMachine::new();
        sm.start_recording().unwrap();
        sm.stop_recording().unwrap();
        sm.transcribing_to_reviewing().unwrap();
        assert_eq!(sm.state(), StateTag::Reviewing);

        sm.reviewing_to_injecting().unwrap();
        assert_eq!(sm.state(), StateTag::Injecting);

        sm.finish_injecting().unwrap();
        assert_eq!(sm.state(), StateTag::Idle);
    }

    #[test]
    fn test_happy_path_with_llm_and_review() {
        let mut sm = StateMachine::new();
        sm.start_recording().unwrap();
        sm.stop_recording().unwrap();
        sm.start_llm_refining().unwrap();

        sm.llm_to_reviewing().unwrap();
        assert_eq!(sm.state(), StateTag::Reviewing);

        sm.reviewing_to_injecting().unwrap();
        sm.finish_injecting().unwrap();
        assert_eq!(sm.state(), StateTag::Idle);
    }

    #[test]
    fn test_review_cancel() {
        let mut sm = StateMachine::new();
        sm.start_recording().unwrap();
        sm.stop_recording().unwrap();
        sm.transcribing_to_reviewing().unwrap();

        sm.cancel_reviewing().unwrap();
        assert_eq!(sm.state(), StateTag::Idle);
    }

    #[test]
    fn test_review_reset() {
        let mut sm = StateMachine::new();
        sm.start_recording().unwrap();
        sm.stop_recording().unwrap();
        sm.transcribing_to_reviewing().unwrap();

        sm.reset();
        assert_eq!(sm.state(), StateTag::Idle);
    }

    #[test]
    fn test_invalid_review_transitions() {
        let mut sm = StateMachine::new();
        // Cannot review from Idle
        assert!(sm.transcribing_to_reviewing().is_err());
        assert!(sm.llm_to_reviewing().is_err());
        assert!(sm.reviewing_to_injecting().is_err());
        assert!(sm.cancel_reviewing().is_err());
    }
}
