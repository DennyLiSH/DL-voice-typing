//! Full lifecycle integration tests using all mock components.
//! Exercises the pipeline functions through complete state machine transitions.

use crate::audio::MockAudioCapture;
use crate::clipboard::MockClipboard;
use crate::commands::pipeline_state::PipelineState;
use crate::commands::review_provider::MockReviewProvider;
use crate::commands::window_controller::NoopWindowController;
use crate::commands::{EventEmitter, MockEmitter};
use crate::config::AppConfig;
use crate::llm::MockCorrector;
use crate::perf::PerfHistory;
use crate::speech::mock::MockEngine;
use crate::state::{StateMachine, StateTag};
use std::sync::{Arc, Mutex};

fn build_ps() -> PipelineState {
    let sm = Arc::new(Mutex::new(StateMachine::new()));
    let ac = Arc::new(Mutex::new(MockAudioCapture::new()));
    let engine = Arc::new(MockEngine::new("test transcription"));
    let clipboard = Arc::new(MockClipboard::new());
    let emitter: Arc<dyn EventEmitter> = Arc::new(MockEmitter::new());

    PipelineState::new(
        sm,
        ac,
        engine,
        clipboard,
        Arc::new(PerfHistory::new()),
        crate::config::ConfigCache::new(AppConfig::default()),
        Arc::new(Mutex::new(Some(Box::new(MockCorrector::new("corrected"))))),
        Arc::new(Mutex::new(None)),
        Arc::new(NoopWindowController),
        emitter,
        Arc::new(MockReviewProvider::new()),
    )
}

#[test]
fn test_classic_direct_lifecycle() {
    let ps = build_ps();

    // Idle → Recording
    assert!(ps.sm_start_recording());
    assert_eq!(ps.sm_state(), Some(StateTag::Recording));

    // Recording → Transcribing
    assert!(ps.sm_stop_recording());
    assert_eq!(ps.sm_state(), Some(StateTag::Transcribing));

    // Transcribing → Injecting
    assert!(ps.sm_transcribing_to_injecting());
    assert_eq!(ps.sm_state(), Some(StateTag::Injecting));

    // Injecting → Idle
    assert!(ps.sm_finish_injecting());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
}

#[test]
fn test_classic_review_lifecycle() {
    let ps = build_ps();

    // Idle → Recording → Transcribing
    assert!(ps.sm_start_recording());
    assert!(ps.sm_stop_recording());

    // Transcribing → Reviewing
    assert!(ps.sm_transcribing_to_reviewing());
    assert_eq!(ps.sm_state(), Some(StateTag::Reviewing));

    // Reviewing → Injecting
    assert!(ps.sm_reviewing_to_injecting());
    assert_eq!(ps.sm_state(), Some(StateTag::Injecting));

    // Injecting → Idle
    assert!(ps.sm_finish_injecting());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
}

#[test]
fn test_llm_lifecycle() {
    let ps = build_ps();

    // Idle → Recording → Transcribing
    assert!(ps.sm_start_recording());
    assert!(ps.sm_stop_recording());

    // Transcribing → LLMRefining
    assert!(ps.sm_start_llm_refining());
    assert_eq!(ps.sm_state(), Some(StateTag::LLMRefining));

    // LLMRefining → Injecting
    assert!(ps.sm_llm_to_injecting());
    assert_eq!(ps.sm_state(), Some(StateTag::Injecting));

    // Injecting → Idle
    assert!(ps.sm_finish_injecting());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
}

#[test]
fn test_cancel_during_recording() {
    let ps = build_ps();

    assert!(ps.sm_start_recording());
    assert_eq!(ps.sm_state(), Some(StateTag::Recording));

    // Cancel: reset to Idle
    ps.sm_reset();
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
}

#[test]
fn test_cancel_during_reviewing() {
    let ps = build_ps();

    assert!(ps.sm_start_recording());
    assert!(ps.sm_stop_recording());
    assert!(ps.sm_transcribing_to_reviewing());

    // Cancel from reviewing
    assert!(ps.sm_cancel_reviewing());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
}

#[test]
fn test_realtime_review_lifecycle() {
    let ps = build_ps();

    // Idle → Recording (realtime starts)
    assert!(ps.sm_start_recording());

    // Recording → Transcribing → Reviewing (realtime accumulated text)
    assert!(ps.sm_stop_recording());
    assert!(ps.sm_transcribing_to_reviewing());
    assert_eq!(ps.sm_state(), Some(StateTag::Reviewing));

    // Confirm from reviewing
    assert!(ps.sm_reviewing_to_injecting());
    assert!(ps.sm_finish_injecting());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
}

#[test]
fn test_reset_from_any_state() {
    let ps = build_ps();

    // Test reset from Recording
    assert!(ps.sm_start_recording());
    ps.sm_reset();
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));

    // Test reset from Transcribing
    assert!(ps.sm_start_recording());
    assert!(ps.sm_stop_recording());
    ps.sm_reset();
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));

    // Test reset from Injecting
    assert!(ps.sm_start_recording());
    assert!(ps.sm_stop_recording());
    assert!(ps.sm_transcribing_to_injecting());
    ps.sm_reset();
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
}
