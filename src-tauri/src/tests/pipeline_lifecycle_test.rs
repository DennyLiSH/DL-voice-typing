//! Full lifecycle integration tests using all mock components.
//! Exercises the pipeline functions through complete state machine transitions.

use crate::audio::MockAudioCapture;
use crate::clipboard::{AnyClipboard, MockClipboard};
use crate::commands::pipeline_state::PipelineState;
use crate::commands::review_provider::MockReviewProvider;
use crate::commands::window_controller::NoopWindowController;
use crate::commands::{EventEmitter, MockEmitter};
use crate::config::AppConfig;
use crate::llm::{AnyCorrector, MockCorrector};
use crate::perf::PerfHistory;
use crate::speech::{AnyEngine, mock::MockEngine};
use crate::state::{AppState, StateMachine};
use std::sync::{Arc, Mutex, RwLock};

fn build_ps() -> PipelineState {
    let sm = Arc::new(Mutex::new(StateMachine::new()));
    let ac = Arc::new(Mutex::new(MockAudioCapture::new()));
    let engine = Arc::new(Mutex::new(AnyEngine::Mock(MockEngine::new("test transcription"))));
    let clipboard = Arc::new(Mutex::new(AnyClipboard::Mock(MockClipboard::new())));
    let emitter: Arc<dyn EventEmitter> = Arc::new(MockEmitter::new());

    PipelineState::new(
        sm,
        ac,
        engine,
        clipboard,
        Arc::new(PerfHistory::new()),
        Arc::new(RwLock::new(AppConfig::default())),
        Arc::new(Mutex::new(Some(AnyCorrector::Mock(MockCorrector::new(
            "corrected",
        ))))),
        Arc::new(Mutex::new(None)),
        Arc::new(NoopWindowController),
        emitter,
        Arc::new(MockReviewProvider::new()),
    )
}

#[test]
fn test_classic_direct_lifecycle() {
    let ps = build_ps();
    let mut sm = ps.sm.lock().unwrap();

    // Idle → Recording
    sm.start_recording().unwrap();
    assert!(matches!(sm.state(), AppState::Recording { .. }));

    // Recording → Transcribing
    sm.append_audio(&[0.5f32; 4800]).unwrap();
    let audio = sm.stop_recording().unwrap();
    assert!(matches!(sm.state(), AppState::Transcribing { .. }));

    // Transcribing → Injecting
    sm.add_partial_result("test transcription".to_string()).unwrap();
    sm.transcribing_to_injecting("test transcription".to_string()).unwrap();
    assert!(matches!(sm.state(), AppState::Injecting { .. }));

    // Injecting → Idle
    sm.finish_injecting().unwrap();
    assert!(matches!(sm.state(), AppState::Idle));
}

#[test]
fn test_classic_review_lifecycle() {
    let ps = build_ps();
    let mut sm = ps.sm.lock().unwrap();

    // Idle → Recording → Transcribing
    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 4800]).unwrap();
    sm.stop_recording().unwrap();

    // Transcribing → Reviewing
    sm.add_partial_result("test".to_string()).unwrap();
    sm.transcribing_to_reviewing("test".to_string()).unwrap();
    assert!(matches!(sm.state(), AppState::Reviewing { .. }));

    // Reviewing → Injecting
    sm.reviewing_to_injecting("edited text".to_string()).unwrap();
    assert!(matches!(sm.state(), AppState::Injecting { .. }));

    // Injecting → Idle
    sm.finish_injecting().unwrap();
    assert!(matches!(sm.state(), AppState::Idle));
}

#[test]
fn test_llm_lifecycle() {
    let ps = build_ps();
    let mut sm = ps.sm.lock().unwrap();

    // Idle → Recording → Transcribing
    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 4800]).unwrap();
    sm.stop_recording().unwrap();

    // Transcribing → LLMRefining
    sm.add_partial_result("raw".to_string()).unwrap();
    sm.start_llm_refining().unwrap();
    assert!(matches!(sm.state(), AppState::LLMRefining { .. }));

    // LLMRefining → Injecting
    sm.llm_to_injecting("corrected".to_string()).unwrap();
    assert!(matches!(sm.state(), AppState::Injecting { .. }));

    // Injecting → Idle
    sm.finish_injecting().unwrap();
    assert!(matches!(sm.state(), AppState::Idle));
}

#[test]
fn test_cancel_during_recording() {
    let ps = build_ps();
    let mut sm = ps.sm.lock().unwrap();

    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 4800]).unwrap();
    assert!(matches!(sm.state(), AppState::Recording { .. }));

    // Cancel: reset to Idle
    sm.reset();
    assert!(matches!(sm.state(), AppState::Idle));
}

#[test]
fn test_cancel_during_reviewing() {
    let ps = build_ps();
    let mut sm = ps.sm.lock().unwrap();

    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 4800]).unwrap();
    sm.stop_recording().unwrap();
    sm.add_partial_result("test".to_string()).unwrap();
    sm.transcribing_to_reviewing("test".to_string()).unwrap();

    // Cancel from reviewing
    sm.cancel_reviewing().unwrap();
    assert!(matches!(sm.state(), AppState::Idle));
}

#[test]
fn test_realtime_review_lifecycle() {
    let ps = build_ps();
    let mut sm = ps.sm.lock().unwrap();

    // Idle → Recording (realtime starts)
    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 4800]).unwrap();

    // Recording → Transcribing → Reviewing (realtime accumulated text)
    sm.stop_recording().unwrap();
    sm.transcribing_to_reviewing("accumulated text".to_string()).unwrap();
    assert!(matches!(sm.state(), AppState::Reviewing { .. }));

    // Confirm from reviewing
    sm.reviewing_to_injecting("accumulated text".to_string()).unwrap();
    sm.finish_injecting().unwrap();
    assert!(matches!(sm.state(), AppState::Idle));
}

#[test]
fn test_reset_from_any_state() {
    let ps = build_ps();
    let mut sm = ps.sm.lock().unwrap();

    // Test reset from Recording
    sm.start_recording().unwrap();
    sm.reset();
    assert!(matches!(sm.state(), AppState::Idle));

    // Test reset from Transcribing
    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 100]).unwrap();
    sm.stop_recording().unwrap();
    sm.reset();
    assert!(matches!(sm.state(), AppState::Idle));

    // Test reset from Injecting
    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 100]).unwrap();
    sm.stop_recording().unwrap();
    sm.add_partial_result("test".to_string()).unwrap();
    sm.transcribing_to_injecting("test".to_string()).unwrap();
    sm.reset();
    assert!(matches!(sm.state(), AppState::Idle));
}
