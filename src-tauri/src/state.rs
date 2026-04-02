/// Application state machine.
///
/// All app behavior is driven by transitions between these states.
/// Invalid transitions return an error.
///
/// ```text
/// Idle → Recording → Transcribing → LLMRefining → Injecting → Idle
///                              ↘ Reviewing ↗         ↗
///                                   ↓      (review disabled)
///                              Injecting → Idle
/// * → Idle  (error/cancel)
/// ```
#[derive(Debug)]
pub enum AppState {
    /// No recording in progress.
    Idle,

    /// Hotkey pressed, audio is being captured.
    Recording { audio_buffer: Vec<f32> },

    /// Hotkey released, Whisper is transcribing.
    Transcribing { partial_results: Vec<String> },

    /// LLM is refining the transcribed text.
    LLMRefining { original_text: String },

    /// Transcription done, waiting for user to review/edit text before injection.
    Reviewing { text: String },

    /// Text is ready, injecting via clipboard paste.
    Injecting {
        text: String,
        saved_clipboard: Option<String>,
    },
}

/// Error for invalid state transitions.
#[derive(Debug, thiserror::Error)]
#[error("invalid state transition: {from} → {to}")]
pub struct TransitionError {
    pub from: String,
    pub to: String,
}

/// Wraps AppState with controlled transitions.
pub struct StateMachine {
    state: AppState,
}

impl StateMachine {
    pub fn new() -> Self {
        Self {
            state: AppState::Idle,
        }
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Transition to Recording (from Idle only).
    pub fn start_recording(&mut self) -> Result<(), TransitionError> {
        match &self.state {
            AppState::Idle => {
                self.state = AppState::Recording {
                    audio_buffer: Vec::new(),
                };
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Recording".to_string(),
            }),
        }
    }

    /// Transition to Transcribing (from Recording only).
    pub fn stop_recording(&mut self) -> Result<Vec<f32>, TransitionError> {
        match std::mem::replace(&mut self.state, AppState::Idle) {
            AppState::Recording { audio_buffer, .. } => {
                self.state = AppState::Transcribing {
                    partial_results: Vec::new(),
                };
                Ok(audio_buffer)
            }
            other => {
                self.state = other;
                Err(TransitionError {
                    from: self.state_name(),
                    to: "Transcribing".to_string(),
                })
            }
        }
    }

    /// Transition from Transcribing to LLMRefining.
    pub fn start_llm_refining(&mut self, original_text: String) -> Result<(), TransitionError> {
        match std::mem::replace(&mut self.state, AppState::Idle) {
            AppState::Transcribing { .. } => {
                self.state = AppState::LLMRefining { original_text };
                Ok(())
            }
            other => {
                self.state = other;
                Err(TransitionError {
                    from: self.state_name(),
                    to: "LLMRefining".to_string(),
                })
            }
        }
    }

    /// Transition from Transcribing to Injecting (LLM disabled path).
    pub fn transcribing_to_injecting(&mut self, text: String) -> Result<(), TransitionError> {
        match std::mem::replace(&mut self.state, AppState::Idle) {
            AppState::Transcribing { .. } => {
                self.state = AppState::Injecting {
                    text,
                    saved_clipboard: None,
                };
                Ok(())
            }
            other => {
                self.state = other;
                Err(TransitionError {
                    from: self.state_name(),
                    to: "Injecting".to_string(),
                })
            }
        }
    }

    /// Transition from LLMRefining to Injecting.
    pub fn llm_to_injecting(&mut self, text: String) -> Result<(), TransitionError> {
        match std::mem::replace(&mut self.state, AppState::Idle) {
            AppState::LLMRefining { .. } => {
                self.state = AppState::Injecting {
                    text,
                    saved_clipboard: None,
                };
                Ok(())
            }
            other => {
                self.state = other;
                Err(TransitionError {
                    from: self.state_name(),
                    to: "Injecting".to_string(),
                })
            }
        }
    }

    /// Transition from Transcribing to Reviewing.
    pub fn transcribing_to_reviewing(&mut self, text: String) -> Result<(), TransitionError> {
        match std::mem::replace(&mut self.state, AppState::Idle) {
            AppState::Transcribing { .. } => {
                self.state = AppState::Reviewing { text };
                Ok(())
            }
            other => {
                self.state = other;
                Err(TransitionError {
                    from: self.state_name(),
                    to: "Reviewing".to_string(),
                })
            }
        }
    }

    /// Transition from LLMRefining to Reviewing.
    pub fn llm_to_reviewing(&mut self, text: String) -> Result<(), TransitionError> {
        match std::mem::replace(&mut self.state, AppState::Idle) {
            AppState::LLMRefining { .. } => {
                self.state = AppState::Reviewing { text };
                Ok(())
            }
            other => {
                self.state = other;
                Err(TransitionError {
                    from: self.state_name(),
                    to: "Reviewing".to_string(),
                })
            }
        }
    }

    /// Transition from Reviewing to Injecting (user confirmed).
    pub fn reviewing_to_injecting(&mut self, text: String) -> Result<(), TransitionError> {
        match std::mem::replace(&mut self.state, AppState::Idle) {
            AppState::Reviewing { .. } => {
                self.state = AppState::Injecting {
                    text,
                    saved_clipboard: None,
                };
                Ok(())
            }
            other => {
                self.state = other;
                Err(TransitionError {
                    from: self.state_name(),
                    to: "Injecting".to_string(),
                })
            }
        }
    }

    /// Cancel review and return to Idle (user cancelled).
    pub fn cancel_reviewing(&mut self) -> Result<(), TransitionError> {
        match &self.state {
            AppState::Reviewing { .. } => {
                self.state = AppState::Idle;
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
        match &self.state {
            AppState::Injecting { .. } => {
                self.state = AppState::Idle;
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
        self.state = AppState::Idle;
    }

    /// Append audio samples to the recording buffer.
    pub fn append_audio(&mut self, samples: &[f32]) -> Result<(), TransitionError> {
        match &mut self.state {
            AppState::Recording { audio_buffer, .. } => {
                audio_buffer.extend_from_slice(samples);
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Recording (append)".to_string(),
            }),
        }
    }

    /// Add a partial transcription result.
    pub fn add_partial_result(&mut self, text: String) -> Result<(), TransitionError> {
        match &mut self.state {
            AppState::Transcribing { partial_results } => {
                partial_results.push(text);
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Transcribing (add_partial)".to_string(),
            }),
        }
    }

    fn state_name(&self) -> String {
        match self.state {
            AppState::Idle => "Idle".to_string(),
            AppState::Recording { .. } => "Recording".to_string(),
            AppState::Transcribing { .. } => "Transcribing".to_string(),
            AppState::LLMRefining { .. } => "LLMRefining".to_string(),
            AppState::Reviewing { .. } => "Reviewing".to_string(),
            AppState::Injecting { .. } => "Injecting".to_string(),
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
        assert!(matches!(sm.state(), AppState::Idle));

        sm.start_recording().unwrap();
        assert!(matches!(sm.state(), AppState::Recording { .. }));

        sm.append_audio(&[0.1, 0.2, 0.3]).unwrap();

        let audio = sm.stop_recording().unwrap();
        assert_eq!(audio, vec![0.1, 0.2, 0.3]);
        assert!(matches!(sm.state(), AppState::Transcribing { .. }));

        sm.add_partial_result("hello".to_string()).unwrap();
        sm.start_llm_refining("hello world".to_string()).unwrap();
        assert!(matches!(sm.state(), AppState::LLMRefining { .. }));

        sm.llm_to_injecting("hello world refined".to_string())
            .unwrap();
        assert!(matches!(sm.state(), AppState::Injecting { .. }));

        sm.finish_injecting().unwrap();
        assert!(matches!(sm.state(), AppState::Idle));
    }

    #[test]
    fn test_happy_path_without_llm() {
        let mut sm = StateMachine::new();
        sm.start_recording().unwrap();
        let _ = sm.stop_recording().unwrap();
        sm.transcribing_to_injecting("hello".to_string()).unwrap();
        sm.finish_injecting().unwrap();
        assert!(matches!(sm.state(), AppState::Idle));
    }

    #[test]
    fn test_reset_from_any_state() {
        let mut sm = StateMachine::new();
        sm.start_recording().unwrap();
        sm.reset();
        assert!(matches!(sm.state(), AppState::Idle));

        sm.start_recording().unwrap();
        let _ = sm.stop_recording().unwrap();
        sm.reset();
        assert!(matches!(sm.state(), AppState::Idle));
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
        assert!(sm.start_llm_refining("text".to_string()).is_err());
        assert!(sm.transcribing_to_injecting("text".to_string()).is_err());
        assert!(sm.llm_to_injecting("text".to_string()).is_err());
        assert!(sm.finish_injecting().is_err());
        assert!(sm.append_audio(&[]).is_err());
        assert!(sm.add_partial_result("text".to_string()).is_err());
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
        let _ = sm.stop_recording().unwrap();
        sm.transcribing_to_reviewing("hello".to_string()).unwrap();
        assert!(matches!(sm.state(), AppState::Reviewing { .. }));

        sm.reviewing_to_injecting("hello edited".to_string())
            .unwrap();
        assert!(matches!(sm.state(), AppState::Injecting { .. }));

        sm.finish_injecting().unwrap();
        assert!(matches!(sm.state(), AppState::Idle));
    }

    #[test]
    fn test_happy_path_with_llm_and_review() {
        let mut sm = StateMachine::new();
        sm.start_recording().unwrap();
        let _ = sm.stop_recording().unwrap();
        sm.add_partial_result("hello".to_string()).unwrap();
        sm.start_llm_refining("hello world".to_string()).unwrap();

        sm.llm_to_reviewing("hello world refined".to_string())
            .unwrap();
        assert!(matches!(sm.state(), AppState::Reviewing { .. }));

        sm.reviewing_to_injecting("hello world refined".to_string())
            .unwrap();
        sm.finish_injecting().unwrap();
        assert!(matches!(sm.state(), AppState::Idle));
    }

    #[test]
    fn test_review_cancel() {
        let mut sm = StateMachine::new();
        sm.start_recording().unwrap();
        let _ = sm.stop_recording().unwrap();
        sm.transcribing_to_reviewing("hello".to_string()).unwrap();

        sm.cancel_reviewing().unwrap();
        assert!(matches!(sm.state(), AppState::Idle));
    }

    #[test]
    fn test_review_reset() {
        let mut sm = StateMachine::new();
        sm.start_recording().unwrap();
        let _ = sm.stop_recording().unwrap();
        sm.transcribing_to_reviewing("hello".to_string()).unwrap();

        sm.reset();
        assert!(matches!(sm.state(), AppState::Idle));
    }

    #[test]
    fn test_invalid_review_transitions() {
        let mut sm = StateMachine::new();
        // Cannot review from Idle
        assert!(sm.transcribing_to_reviewing("text".to_string()).is_err());
        assert!(sm.llm_to_reviewing("text".to_string()).is_err());
        assert!(sm.reviewing_to_injecting("text".to_string()).is_err());
        assert!(sm.cancel_reviewing().is_err());
    }
}
