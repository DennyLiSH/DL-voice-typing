//! Tests for the RecordingSession deep module — the orchestration test surface
//! that did not exist before this refactor.
//!
//! A. `decide_release` pure function (mode × accumulated → kind).
//! B. Future bodies (`run_pipeline` / `run_realtime_fast_path`) driven directly.
//! C. `recover()` panic-recovery (conditional restore).

use crate::audio::MockAudioCapture;
use crate::clipboard::{ClipboardProvider, MockClipboard};
use crate::commands::MockEmitter;
use crate::commands::recording_session::{
    RecordingSession, ReleaseAction, ReleaseActionKind, SessionPolicy, decide_release,
};
use crate::commands::review_provider::MockReviewProvider;
use crate::commands::window_controller::NoopWindowController;
use crate::config::{AppConfig, PipelineMode};
use crate::llm::MockCorrector;
use crate::perf::PerfMetrics;
use crate::speech::mock::MockEngine;
use crate::state::{StateMachine, StateTag};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Test rig: owns the session plus handles to the shared mock components
/// (same `Arc`s the session cloned into its `PipelineState`).
struct Rig {
    session: RecordingSession,
    sm: Arc<Mutex<StateMachine>>,
    emitter: Arc<MockEmitter>,
    clipboard: Arc<MockClipboard>,
}

fn config(realtime: bool, review: bool, llm: bool) -> AppConfig {
    AppConfig {
        realtime_transcription: realtime,
        review_before_paste: review,
        llm_enabled: llm,
        ..Default::default()
    }
}

fn build_rig(cfg: AppConfig, engine_text: &str) -> Rig {
    build_rig_with_corrector(cfg, engine_text, MockCorrector::new("corrected"))
}

/// `build_rig` with an injectable corrector — the default wrapper keeps the
/// existing 9 call sites unchanged; LLM-failure tests pass `MockCorrector::failing()`.
fn build_rig_with_corrector(cfg: AppConfig, engine_text: &str, corrector: MockCorrector) -> Rig {
    let sm = Arc::new(Mutex::new(StateMachine::new()));
    let ac = Arc::new(Mutex::new(MockAudioCapture::new()));
    let engine = Arc::new(MockEngine::new(engine_text));
    let clipboard = Arc::new(MockClipboard::new());
    let emitter = Arc::new(MockEmitter::new());
    let ps = crate::commands::pipeline_state::PipelineState::new(
        sm.clone(),
        ac,
        engine,
        clipboard.clone(),
        Arc::new(crate::perf::PerfHistory::new()),
        crate::config::ConfigCache::new(cfg),
        Arc::new(Mutex::new(Some(Box::new(corrector)))),
        Arc::new(Mutex::new(None)),
        Arc::new(NoopWindowController),
        emitter.clone(),
        Arc::new(MockReviewProvider::new()),
    );
    Rig {
        session: RecordingSession::new(ps),
        sm,
        emitter,
        clipboard,
    }
}

/// Drive the state machine into `Transcribing` (the precondition for the
/// delivery paths).
fn to_transcribing(sm: &Arc<Mutex<StateMachine>>) {
    let mut s = sm.lock().unwrap();
    s.start_recording().unwrap();
    s.stop_recording().unwrap();
}

fn event_names(emitter: &MockEmitter) -> Vec<String> {
    emitter
        .take_events()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

// ---------------------------------------------------------------------------
// A. decide_release — pure function, covers the mode × accumulated table.
// ---------------------------------------------------------------------------

#[test]
fn decide_classic_direct_none() {
    assert_eq!(
        decide_release(PipelineMode::ClassicDirect, None),
        ReleaseActionKind::DeliverFull
    );
}

#[test]
fn decide_classic_review_none() {
    assert_eq!(
        decide_release(PipelineMode::ClassicReview, None),
        ReleaseActionKind::DeliverFull
    );
}

#[test]
fn decide_realtime_direct_some() {
    assert_eq!(
        decide_release(PipelineMode::RealtimeDirect, Some("你好")),
        ReleaseActionKind::DeliverFast
    );
}

#[test]
fn decide_realtime_direct_none_fallthrough() {
    assert_eq!(
        decide_release(PipelineMode::RealtimeDirect, None),
        ReleaseActionKind::DeliverFull
    );
}

#[test]
fn decide_realtime_review_some_handoff() {
    assert_eq!(
        decide_release(PipelineMode::RealtimeReview, Some("你好")),
        ReleaseActionKind::Done
    );
}

#[test]
fn decide_realtime_review_none_fallthrough() {
    assert_eq!(
        decide_release(PipelineMode::RealtimeReview, None),
        ReleaseActionKind::DeliverFull
    );
}

// ---------------------------------------------------------------------------
// B. Future bodies driven directly with constructed inputs.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_pipeline_classic_direct_injects() {
    let rig = build_rig(config(false, false, false), "hello world");
    to_transcribing(&rig.sm);
    let perf = PerfMetrics::new(0);
    let policy = SessionPolicy::from_config(&config(false, false, false));
    rig.session
        .run_pipeline(
            vec![],
            48000,
            vec![0.5f32; 1600],
            false,
            perf,
            Instant::now(),
            policy,
        )
        .await;

    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
    let names = event_names(&rig.emitter);
    assert!(names.contains(&"transcription-complete".to_string()));
    assert!(names.contains(&"injection-complete".to_string()));
    assert!(rig.clipboard.saved(), "clipboard should be saved");
    assert!(!rig.clipboard.injected().is_empty(), "text injected");
}

#[tokio::test]
async fn run_pipeline_classic_review_enters_reviewing() {
    let rig = build_rig(config(false, true, false), "review me");
    to_transcribing(&rig.sm);
    let perf = PerfMetrics::new(0);
    let policy = SessionPolicy::from_config(&config(false, true, false));
    rig.session
        .run_pipeline(
            vec![],
            48000,
            vec![0.5f32; 1600],
            true,
            perf,
            Instant::now(),
            policy,
        )
        .await;

    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Reviewing);
    let names = event_names(&rig.emitter);
    assert!(names.contains(&"transcription-complete".to_string()));
    assert!(
        !names.contains(&"injection-complete".to_string()),
        "review path must not inject"
    );
    assert!(rig.clipboard.saved(), "clipboard saved before review");
}

#[tokio::test]
async fn run_realtime_fast_path_injects_accumulated() {
    let rig = build_rig(config(true, false, false), "ignored");
    to_transcribing(&rig.sm);
    let perf = PerfMetrics::new(0);
    let policy = SessionPolicy::from_config(&config(true, false, false));
    rig.session
        .run_realtime_fast_path(
            "你好".to_string(),
            vec![],
            48000,
            perf,
            Instant::now(),
            policy,
        )
        .await;

    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
    let names = event_names(&rig.emitter);
    assert!(names.contains(&"injection-complete".to_string()));
    assert!(
        !names.contains(&"transcription-complete".to_string()),
        "fast path skips Whisper"
    );
    assert!(!rig.clipboard.injected().is_empty());
}

#[tokio::test]
async fn run_pipeline_empty_transcription_resets() {
    let rig = build_rig(config(false, false, false), "");
    to_transcribing(&rig.sm);
    let perf = PerfMetrics::new(0);
    let policy = SessionPolicy::from_config(&config(false, false, false));
    rig.session
        .run_pipeline(
            vec![],
            48000,
            vec![0.5f32; 1600],
            false,
            perf,
            Instant::now(),
            policy,
        )
        .await;

    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
    let names = event_names(&rig.emitter);
    assert!(
        !names.contains(&"injection-complete".to_string()),
        "empty transcription must not inject"
    );
}

#[tokio::test]
async fn run_pipeline_llm_corrects_then_injects() {
    // llm_enabled with default (empty) config: MockCorrector matches and is
    // reused — no real HTTP.
    let rig = build_rig(config(false, false, true), "raw transcription");
    to_transcribing(&rig.sm);
    let perf = PerfMetrics::new(0);
    let policy = SessionPolicy::from_config(&config(false, false, true));
    rig.session
        .run_pipeline(
            vec![],
            48000,
            vec![0.5f32; 1600],
            false,
            perf,
            Instant::now(),
            policy,
        )
        .await;

    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
    let names = event_names(&rig.emitter);
    assert!(names.contains(&"llm-refining".to_string()));
    assert!(names.contains(&"llm-complete".to_string()));
    assert!(names.contains(&"injection-complete".to_string()));
}

#[tokio::test]
async fn run_pipeline_llm_failure_falls_back_to_raw_and_emits_summary() {
    let rig = build_rig_with_corrector(
        config(false, false, true),
        "raw transcription",
        MockCorrector::failing(),
    );
    to_transcribing(&rig.sm);
    let perf = PerfMetrics::new(0);
    let policy = SessionPolicy::from_config(&config(false, false, true));
    rig.session
        .run_pipeline(
            vec![],
            48000,
            vec![0.5f32; 1600],
            false,
            perf,
            Instant::now(),
            policy,
        )
        .await;

    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
    let events = rig.emitter.take_events();
    let names: Vec<String> = events.iter().map(|(n, _)| n.clone()).collect();
    assert!(names.contains(&"llm-refining".to_string()));
    assert!(
        !names.contains(&"llm-complete".to_string()),
        "failed correction must not emit llm-complete"
    );
    // Payload is the fixed Chinese summary as a bare JSON string — not the
    // English technical detail, not an object.
    let llm_error = events
        .iter()
        .find(|(n, _)| n == "llm-error")
        .map(|(_, v)| v.clone())
        .expect("llm-error must be emitted on failure");
    assert_eq!(
        llm_error,
        serde_json::Value::String(
            crate::commands::recording_session::LLM_ERROR_USER_MSG.to_string()
        )
    );
    // Fallback path: raw transcription is still injected.
    assert!(names.contains(&"injection-complete".to_string()));
    assert_eq!(
        rig.clipboard.injected(),
        vec!["raw transcription".to_string()]
    );
}

// ---------------------------------------------------------------------------
// C. recover() — panic recovery (conditional clipboard restore).
// ---------------------------------------------------------------------------

#[test]
fn recover_with_saved_restores_clipboard() {
    let rig = build_rig(config(false, false, false), "x");
    {
        let mut s = rig.sm.lock().unwrap();
        s.start_recording().unwrap();
        s.stop_recording().unwrap();
        s.transcribing_to_injecting().unwrap();
        // Clipboard was saved this cycle.
        rig.clipboard.save().unwrap();
    }

    rig.session.recover();

    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
    assert!(
        rig.clipboard.restored(),
        "saved clipboard should be restored"
    );
    let names = event_names(&rig.emitter);
    assert!(names.contains(&"speech-error".to_string()));
}

#[test]
fn recover_without_save_does_not_restore() {
    let rig = build_rig(config(false, false, false), "x");
    {
        let mut s = rig.sm.lock().unwrap();
        s.start_recording().unwrap();
        s.stop_recording().unwrap();
    }
    // Clipboard never saved.

    rig.session.recover();

    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
    assert!(
        !rig.clipboard.restored(),
        "unsaved clipboard must not be restored"
    );
    assert!(event_names(&rig.emitter).contains(&"speech-error".to_string()));
}

#[test]
fn recover_is_idempotent() {
    let rig = build_rig(config(false, false, false), "x");
    rig.session.recover();
    rig.session.recover();
    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
}

// ---------------------------------------------------------------------------
// on_release routing — verifies ReleaseAction variants without spawning
// (Deliver futures are inspectable, not detached).
// ---------------------------------------------------------------------------

#[test]
fn on_release_silent_audio_is_done() {
    // No audio captured (empty ring buffer) → preprocess_audio returns None →
    // silent → Done.
    let rig = build_rig(config(false, false, false), "x");
    // Simulate a prior press so on_release has a recording state to stop.
    {
        let mut s = rig.sm.lock().unwrap();
        s.start_recording().unwrap();
    }
    let action = rig.session.on_release();
    assert!(matches!(action, ReleaseAction::Done));
}
