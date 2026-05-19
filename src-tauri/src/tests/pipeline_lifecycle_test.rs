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
use crate::state::{StateMachine, StateTag};
use std::sync::{Arc, Mutex};

fn build_ps() -> PipelineState {
    let sm = Arc::new(Mutex::new(StateMachine::new()));
    let ac = Arc::new(Mutex::new(MockAudioCapture::new()));
    let engine = Arc::new(AnyEngine::Mock(MockEngine::new(
        "test transcription",
    )));
    let clipboard = Arc::new(Mutex::new(AnyClipboard::Mock(MockClipboard::new())));
    let emitter: Arc<dyn EventEmitter> = Arc::new(MockEmitter::new());

    PipelineState::new(
        sm,
        ac,
        engine,
        clipboard,
        Arc::new(PerfHistory::new()),
        crate::config::ConfigCache::new(AppConfig::default()),
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
    assert_eq!(sm.state(), StateTag::Recording);

    // Recording → Transcribing
    sm.stop_recording().unwrap();
    assert_eq!(sm.state(), StateTag::Transcribing);

    // Transcribing → Injecting
    sm.transcribing_to_injecting().unwrap();
    assert_eq!(sm.state(), StateTag::Injecting);

    // Injecting → Idle
    sm.finish_injecting().unwrap();
    assert_eq!(sm.state(), StateTag::Idle);
}

#[test]
fn test_classic_review_lifecycle() {
    let ps = build_ps();
    let mut sm = ps.sm.lock().unwrap();

    // Idle → Recording → Transcribing
    sm.start_recording().unwrap();
    sm.stop_recording().unwrap();

    // Transcribing → Reviewing
    sm.transcribing_to_reviewing().unwrap();
    assert_eq!(sm.state(), StateTag::Reviewing);

    // Reviewing → Injecting
    sm.reviewing_to_injecting().unwrap();
    assert_eq!(sm.state(), StateTag::Injecting);

    // Injecting → Idle
    sm.finish_injecting().unwrap();
    assert_eq!(sm.state(), StateTag::Idle);
}

#[test]
fn test_llm_lifecycle() {
    let ps = build_ps();
    let mut sm = ps.sm.lock().unwrap();

    // Idle → Recording → Transcribing
    sm.start_recording().unwrap();
    sm.stop_recording().unwrap();

    // Transcribing → LLMRefining
    sm.start_llm_refining().unwrap();
    assert_eq!(sm.state(), StateTag::LLMRefining);

    // LLMRefining → Injecting
    sm.llm_to_injecting().unwrap();
    assert_eq!(sm.state(), StateTag::Injecting);

    // Injecting → Idle
    sm.finish_injecting().unwrap();
    assert_eq!(sm.state(), StateTag::Idle);
}

#[test]
fn test_cancel_during_recording() {
    let ps = build_ps();
    let mut sm = ps.sm.lock().unwrap();

    sm.start_recording().unwrap();
    assert_eq!(sm.state(), StateTag::Recording);

    // Cancel: reset to Idle
    sm.reset();
    assert_eq!(sm.state(), StateTag::Idle);
}

#[test]
fn test_cancel_during_reviewing() {
    let ps = build_ps();
    let mut sm = ps.sm.lock().unwrap();

    sm.start_recording().unwrap();
    sm.stop_recording().unwrap();
    sm.transcribing_to_reviewing().unwrap();

    // Cancel from reviewing
    sm.cancel_reviewing().unwrap();
    assert_eq!(sm.state(), StateTag::Idle);
}

#[test]
fn test_realtime_review_lifecycle() {
    let ps = build_ps();
    let mut sm = ps.sm.lock().unwrap();

    // Idle → Recording (realtime starts)
    sm.start_recording().unwrap();

    // Recording → Transcribing → Reviewing (realtime accumulated text)
    sm.stop_recording().unwrap();
    sm.transcribing_to_reviewing().unwrap();
    assert_eq!(sm.state(), StateTag::Reviewing);

    // Confirm from reviewing
    sm.reviewing_to_injecting().unwrap();
    sm.finish_injecting().unwrap();
    assert_eq!(sm.state(), StateTag::Idle);
}

#[test]
fn test_reset_from_any_state() {
    let ps = build_ps();
    let mut sm = ps.sm.lock().unwrap();

    // Test reset from Recording
    sm.start_recording().unwrap();
    sm.reset();
    assert_eq!(sm.state(), StateTag::Idle);

    // Test reset from Transcribing
    sm.start_recording().unwrap();
    sm.stop_recording().unwrap();
    sm.reset();
    assert_eq!(sm.state(), StateTag::Idle);

    // Test reset from Injecting
    sm.start_recording().unwrap();
    sm.stop_recording().unwrap();
    sm.transcribing_to_injecting().unwrap();
    sm.reset();
    assert_eq!(sm.state(), StateTag::Idle);
}
