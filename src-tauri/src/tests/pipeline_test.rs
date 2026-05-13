//! E2E pipeline tests exercising the full hotkey→transcription→injection
//! state machine using MockEngine, without Tauri/Win32/audio dependencies.

use crate::audio::rms;
use crate::config::{AppConfig, PipelineMode};
use crate::speech::SpeechEngine;
use crate::speech::mock::MockEngine;
use crate::state::{AppState, StateMachine};

#[test]
fn test_pipeline_direct_inject_no_llm() {
    let mut sm = StateMachine::new();
    let engine = MockEngine::new("hello world");

    // Simulate hotkey press → recording
    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 1600]).unwrap();
    sm.append_audio(&[0.3f32; 1600]).unwrap();
    let audio = sm.stop_recording().unwrap();

    // Silence guard: audio should not be silent
    let rms_val = rms::calculate_rms(&audio);
    assert!(
        rms_val >= 0.01,
        "audio should not be silent, got rms={rms_val}"
    );

    // Transcribe with mock engine
    let text = engine.transcribe_sync(&audio).unwrap();
    assert_eq!(text, "hello world");

    // Feed result into state machine
    sm.add_partial_result(text.clone()).unwrap();
    sm.transcribing_to_injecting(text.clone()).unwrap();
    sm.finish_injecting().unwrap();

    assert!(matches!(sm.state(), AppState::Idle));
}

#[test]
fn test_pipeline_silent_audio_skipped() {
    let mut sm = StateMachine::new();
    let _engine = MockEngine::new("should not be used");

    sm.start_recording().unwrap();
    sm.append_audio(&[0.0f32; 1600]).unwrap();
    let audio = sm.stop_recording().unwrap();

    // Silence detection
    let rms_val = rms::calculate_rms(&audio);
    assert!(
        rms_val < 0.01,
        "silence should be detected, got rms={rms_val}"
    );

    // Pipeline aborts — reset to Idle
    sm.reset();
    assert!(matches!(sm.state(), AppState::Idle));
}

#[test]
fn test_pipeline_with_llm_refining() {
    let mut sm = StateMachine::new();
    let engine = MockEngine::new("raw transcription");

    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 4800]).unwrap();
    let audio = sm.stop_recording().unwrap();

    // Transcribe
    let text = engine.transcribe_sync(&audio).unwrap();
    assert_eq!(text, "raw transcription");

    // LLM refining path
    sm.add_partial_result(text.clone()).unwrap();
    sm.start_llm_refining().unwrap();

    // Simulate LLM returning corrected text
    let corrected = "corrected transcription".to_string();
    sm.llm_to_injecting(corrected).unwrap();
    sm.finish_injecting().unwrap();

    assert!(matches!(sm.state(), AppState::Idle));
}

#[test]
fn test_pipeline_with_review_path() {
    let mut sm = StateMachine::new();
    let engine = MockEngine::new("review me");

    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 4800]).unwrap();
    let audio = sm.stop_recording().unwrap();

    let text = engine.transcribe_sync(&audio).unwrap();
    assert_eq!(text, "review me");

    // Transcribing → Reviewing
    sm.add_partial_result(text.clone()).unwrap();
    sm.transcribing_to_reviewing(text).unwrap();
    assert!(matches!(sm.state(), AppState::Reviewing { .. }));

    // User edits then confirms
    sm.reviewing_to_injecting("edited review me".to_string())
        .unwrap();
    sm.finish_injecting().unwrap();

    assert!(matches!(sm.state(), AppState::Idle));
}

#[test]
fn test_pipeline_with_llm_and_review() {
    let mut sm = StateMachine::new();
    let engine = MockEngine::new("raw text");

    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 4800]).unwrap();
    let audio = sm.stop_recording().unwrap();

    let text = engine.transcribe_sync(&audio).unwrap();
    assert_eq!(text, "raw text");

    // Transcribing → LLMRefining → Reviewing → Injecting
    sm.add_partial_result(text.clone()).unwrap();
    sm.start_llm_refining().unwrap();
    sm.llm_to_reviewing("llm corrected".to_string()).unwrap();
    assert!(matches!(sm.state(), AppState::Reviewing { .. }));

    // User further edits the LLM output
    sm.reviewing_to_injecting("user edited".to_string())
        .unwrap();
    sm.finish_injecting().unwrap();

    assert!(matches!(sm.state(), AppState::Idle));
}

#[test]
fn test_pipeline_cancel_review() {
    let mut sm = StateMachine::new();
    let engine = MockEngine::new("cancel me");

    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 4800]).unwrap();
    let audio = sm.stop_recording().unwrap();

    let text = engine.transcribe_sync(&audio).unwrap();
    assert_eq!(text, "cancel me");

    sm.add_partial_result(text.clone()).unwrap();
    sm.transcribing_to_reviewing(text).unwrap();
    assert!(matches!(sm.state(), AppState::Reviewing { .. }));

    // User cancels instead of confirming
    sm.cancel_reviewing().unwrap();
    assert!(matches!(sm.state(), AppState::Idle));
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
    sm.append_audio(&[0.5f32; 4800]).unwrap();

    // On release: stop_recording → Transcribing
    let _audio = sm.stop_recording().unwrap();

    // Simulate using accumulated text directly (no Whisper)
    let accumulated_text = "realtime transcription result".to_string();

    // Direct inject path (skip Whisper and LLM)
    sm.transcribing_to_injecting(accumulated_text).unwrap();
    sm.finish_injecting().unwrap();

    assert!(matches!(sm.state(), AppState::Idle));
}

#[test]
fn test_pipeline_realtime_direct_with_llm() {
    // RealtimeDirect + LLM: accumulated text → LLM → inject
    let mut sm = StateMachine::new();

    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 4800]).unwrap();
    let _audio = sm.stop_recording().unwrap();

    let _accumulated_text = "realtime text with homophones".to_string();

    // LLM refining path
    sm.start_llm_refining().unwrap();
    let llm_corrected = "realtime text with corrections".to_string();
    sm.llm_to_injecting(llm_corrected).unwrap();
    sm.finish_injecting().unwrap();

    assert!(matches!(sm.state(), AppState::Idle));
}

#[test]
fn test_pipeline_realtime_review_early_confirm() {
    // Simulate RealtimeReview: user confirms during recording (early confirm)
    let mut sm = StateMachine::new();

    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 4800]).unwrap();

    // User confirms early — reset state machine
    sm.reset();
    assert!(matches!(sm.state(), AppState::Idle));
}

#[test]
fn test_pipeline_realtime_review_early_cancel() {
    // Simulate RealtimeReview: user cancels during recording
    let mut sm = StateMachine::new();

    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 4800]).unwrap();

    // User cancels — reset
    sm.reset();
    assert!(matches!(sm.state(), AppState::Idle));
}
