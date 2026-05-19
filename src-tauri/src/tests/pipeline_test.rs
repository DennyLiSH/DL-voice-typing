//! E2E pipeline tests exercising the full hotkey→transcription→injection
//! state machine using MockEngine, without Tauri/Win32/audio dependencies.

use crate::config::{AppConfig, PipelineMode};
use crate::speech::SpeechEngine;
use crate::speech::mock::MockEngine;
use crate::state::{StateMachine, StateTag};

#[test]
fn test_pipeline_direct_inject_no_llm() {
    let mut sm = StateMachine::new();
    let engine = MockEngine::new("hello world");

    // Simulate hotkey press → recording
    sm.start_recording().unwrap();
    sm.stop_recording().unwrap();

    // Transcribe with mock engine (audio would come from AudioRingBuffer)
    let audio = vec![0.5f32; 4800];
    let text = engine.transcribe_sync(&audio).unwrap();
    assert_eq!(text, "hello world");

    // Feed result into state machine
    sm.transcribing_to_injecting().unwrap();
    sm.finish_injecting().unwrap();

    assert_eq!(sm.state(), StateTag::Idle);
}

#[test]
fn test_pipeline_silent_audio_skipped() {
    let mut sm = StateMachine::new();

    sm.start_recording().unwrap();
    sm.stop_recording().unwrap();

    // Pipeline aborts — reset to Idle
    sm.reset();
    assert_eq!(sm.state(), StateTag::Idle);
}

#[test]
fn test_pipeline_with_llm_refining() {
    let mut sm = StateMachine::new();
    let engine = MockEngine::new("raw transcription");

    sm.start_recording().unwrap();
    sm.stop_recording().unwrap();

    // Transcribe (audio from ring buffer)
    let audio = vec![0.5f32; 4800];
    let text = engine.transcribe_sync(&audio).unwrap();
    assert_eq!(text, "raw transcription");

    // LLM refining path
    sm.start_llm_refining().unwrap();

    // Simulate LLM returning corrected text
    sm.llm_to_injecting().unwrap();
    sm.finish_injecting().unwrap();

    assert_eq!(sm.state(), StateTag::Idle);
}

#[test]
fn test_pipeline_with_review_path() {
    let mut sm = StateMachine::new();
    let engine = MockEngine::new("review me");

    sm.start_recording().unwrap();
    sm.stop_recording().unwrap();

    let audio = vec![0.5f32; 4800];
    let text = engine.transcribe_sync(&audio).unwrap();
    assert_eq!(text, "review me");

    // Transcribing → Reviewing
    sm.transcribing_to_reviewing().unwrap();
    assert_eq!(sm.state(), StateTag::Reviewing);

    // User edits then confirms
    sm.reviewing_to_injecting().unwrap();
    sm.finish_injecting().unwrap();

    assert_eq!(sm.state(), StateTag::Idle);
}

#[test]
fn test_pipeline_with_llm_and_review() {
    let mut sm = StateMachine::new();
    let engine = MockEngine::new("raw text");

    sm.start_recording().unwrap();
    sm.stop_recording().unwrap();

    let audio = vec![0.5f32; 4800];
    let text = engine.transcribe_sync(&audio).unwrap();
    assert_eq!(text, "raw text");

    // Transcribing → LLMRefining → Reviewing → Injecting
    sm.start_llm_refining().unwrap();
    sm.llm_to_reviewing().unwrap();
    assert_eq!(sm.state(), StateTag::Reviewing);

    // User further edits the LLM output
    sm.reviewing_to_injecting().unwrap();
    sm.finish_injecting().unwrap();

    assert_eq!(sm.state(), StateTag::Idle);
}

#[test]
fn test_pipeline_cancel_review() {
    let mut sm = StateMachine::new();
    let engine = MockEngine::new("cancel me");

    sm.start_recording().unwrap();
    sm.stop_recording().unwrap();

    let audio = vec![0.5f32; 4800];
    let text = engine.transcribe_sync(&audio).unwrap();
    assert_eq!(text, "cancel me");

    sm.transcribing_to_reviewing().unwrap();
    assert_eq!(sm.state(), StateTag::Reviewing);

    // User cancels instead of confirming
    sm.cancel_reviewing().unwrap();
    assert_eq!(sm.state(), StateTag::Idle);
}

// --- PipelineMode tests ---

#[test]
fn test_pipeline_mode_derivation() {
    let mut config = AppConfig::default();
    assert_eq!(config.pipeline_mode(), PipelineMode::ClassicDirect);

    config.review_before_paste = true;
    assert_eq!(config.pipeline_mode(), PipelineMode::ClassicReview);

    config.review_before_paste = false;
    config.realtime_transcription = true;
    assert_eq!(config.pipeline_mode(), PipelineMode::RealtimeDirect);

    config.review_before_paste = true;
    assert_eq!(config.pipeline_mode(), PipelineMode::RealtimeReview);
}

// --- RealtimeDirect (Path C) state machine tests ---

#[test]
fn test_pipeline_realtime_direct_with_accumulated_text() {
    // Simulate RealtimeDirect: recording → realtime accumulated text → inject
    // (skip Whisper entirely, just use accumulated text directly)
    let mut sm = StateMachine::new();

    // Recording phase
    sm.start_recording().unwrap();

    // On release: stop_recording → Transcribing
    sm.stop_recording().unwrap();

    // Direct inject path (skip Whisper and LLM)
    sm.transcribing_to_injecting().unwrap();
    sm.finish_injecting().unwrap();

    assert_eq!(sm.state(), StateTag::Idle);
}

#[test]
fn test_pipeline_realtime_direct_with_llm() {
    // RealtimeDirect + LLM: accumulated text → LLM → inject
    let mut sm = StateMachine::new();

    sm.start_recording().unwrap();
    sm.stop_recording().unwrap();

    // LLM refining path
    sm.start_llm_refining().unwrap();
    sm.llm_to_injecting().unwrap();
    sm.finish_injecting().unwrap();

    assert_eq!(sm.state(), StateTag::Idle);
}

#[test]
fn test_pipeline_realtime_review_early_confirm() {
    // Simulate RealtimeReview: user confirms during recording (early confirm)
    let mut sm = StateMachine::new();

    sm.start_recording().unwrap();

    // User confirms early — reset state machine
    sm.reset();
    assert_eq!(sm.state(), StateTag::Idle);
}

#[test]
fn test_pipeline_realtime_review_early_cancel() {
    // Simulate RealtimeReview: user cancels during recording
    let mut sm = StateMachine::new();

    sm.start_recording().unwrap();

    // User cancels — reset
    sm.reset();
    assert_eq!(sm.state(), StateTag::Idle);
}
