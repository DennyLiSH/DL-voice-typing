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
    build_rig_inner(cfg, engine_text, corrector, Arc::new(NoopWindowController))
}

fn build_rig_inner(
    cfg: AppConfig,
    engine_text: &str,
    corrector: MockCorrector,
    wc: Arc<dyn crate::commands::window_controller::WindowController>,
) -> Rig {
    build_rig_full(cfg, engine_text, corrector, wc, true)
}

/// Full-control variant: `engine_ready=false` drives the on_press not-ready
/// branch (first-run dead-end routing tests).
fn build_rig_full(
    cfg: AppConfig,
    engine_text: &str,
    corrector: MockCorrector,
    wc: Arc<dyn crate::commands::window_controller::WindowController>,
    engine_ready: bool,
) -> Rig {
    let sm = Arc::new(Mutex::new(StateMachine::new()));
    let ac = Arc::new(Mutex::new(MockAudioCapture::new()));
    let mut engine = MockEngine::new(engine_text);
    engine.set_ready(engine_ready);
    let engine = Arc::new(engine);
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
        wc,
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

/// Window controller that records call names (not-ready visibility asserts).
struct CallLogWindowController {
    calls: Arc<Mutex<Vec<&'static str>>>,
}

impl crate::commands::window_controller::WindowController for CallLogWindowController {
    fn show_floating_near_caret(&self) -> bool {
        self.calls.lock().unwrap().push("show_floating");
        true
    }
    fn hide_floating(&self) {
        self.calls.lock().unwrap().push("hide_floating");
    }
    fn show_review_near_caret(&self) -> bool {
        true
    }
    fn hide_review(&self) {
        self.calls.lock().unwrap().push("hide_review");
    }
    fn focus_review(&self) -> bool {
        true
    }
    fn eval_review_js(&self, _js: &str) -> bool {
        true
    }
    fn emit_review_show(&self) {}
    fn emit_review_final_text(&self, _text: &str) {}
    fn restore_foreground_hwnd(&self, _hwnd: isize) {}
    fn show_floating_corner(&self) -> bool {
        self.calls.lock().unwrap().push("show_floating_corner");
        true
    }
    fn set_tray_tooltip(&self, _tooltip: &str) {}
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
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
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

// -----------------------------------------------------------------------------
// First-run dead-end: on_press not-ready branch (model routing + visibility)
// -----------------------------------------------------------------------------

#[test]
fn test_not_ready_message_routes_by_model_presence() {
    assert_eq!(
        crate::commands::recording_session::not_ready_message(true),
        "模型加载中，请稍候..."
    );
    assert_eq!(
        crate::commands::recording_session::not_ready_message(false),
        "模型未下载，请在 设置→模型 下载"
    );
}

#[test]
fn test_on_press_not_ready_missing_model_shows_actionable_error() {
    // A Custom model name that cannot exist on disk makes the exists() probe
    // deterministic without touching the real %APPDATA% models dir.
    let cfg = AppConfig {
        whisper_model: crate::config::WhisperModel::Custom(
            "no-such-model-for-test.bin".to_string(),
        ),
        ..config(false, false, false)
    };
    let calls = Arc::new(Mutex::new(Vec::new()));
    let wc = Arc::new(CallLogWindowController {
        calls: calls.clone(),
    });
    let rig = build_rig_full(cfg, "unused", MockCorrector::new("x"), wc, false);

    rig.session.on_press();

    // Routing: the missing-download message, not the misleading "loading".
    let events = rig.emitter.take_events();
    let speech_err = events
        .iter()
        .find(|(e, _)| e == "speech-error")
        .expect("speech-error emitted");
    assert_eq!(
        speech_err.1,
        serde_json::json!("模型未下载，请在 设置→模型 下载")
    );
    // Visibility: the floating window was never shown by the classic path
    // (it early-returns before show), so the branch must show it itself.
    let logged = calls.lock().unwrap().clone();
    assert!(logged.contains(&"show_floating"), "logged: {logged:?}");
    assert!(!logged.contains(&"show_floating_corner"));
    // State returns to Idle.
    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
}

// ---------------------------------------------------------------------------
// P1 Esc-cancel: cancel_active_pipeline state guard matrix
//
// Esc must only cancel when the pipeline is actually in a cancellable phase.
// (Transcribing / LLMRefining — both pre-delivery.) All other states
// (Idle / Recording / Injecting / Reviewing / RecordOnly) must pass through
// (return false, no state change, no event, no window hide).
// ---------------------------------------------------------------------------

#[test]
fn cancel_active_pipeline_only_succeeds_in_transcribing_or_llm_refining() {
    // Cases that MUST be cancellable.
    for start_state in [StateTag::Transcribing, StateTag::LLMRefining] {
        let rig = build_rig(config(false, false, false), "x");
        rig.sm.lock().unwrap().force_state_tag(start_state);
        // Pre-set a cancel token so we can verify it's flipped + taken.
        let token = Arc::new(std::sync::atomic::AtomicBool::new(false));
        rig.session.ps_ref().set_cancel_token(token.clone());

        let handled = rig.session.ps_ref().cancel_active_pipeline();
        assert!(handled, "{start_state:?} must be cancellable");
        // Token flipped.
        assert!(token.load(std::sync::atomic::Ordering::Relaxed));
        // State back to Idle.
        assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
        // Slot cleared.
        assert!(rig.session.ps_ref().take_cancel_token().is_none());
        // pipeline-cancelled event emitted.
        let events = rig.emitter.take_events();
        assert!(
            events.iter().any(|(n, _)| n == "pipeline-cancelled"),
            "{start_state:?} must emit pipeline-cancelled: {events:?}"
        );
    }

    // Cases that MUST NOT be cancellable — return false, no state change,
    // no event, no slot touch, token untouched.
    for start_state in [
        StateTag::Idle,
        StateTag::Recording,
        StateTag::Injecting,
        StateTag::Reviewing,
        StateTag::RecordOnly,
    ] {
        let rig = build_rig(config(false, false, false), "x");
        rig.sm.lock().unwrap().force_state_tag(start_state);
        let token = Arc::new(std::sync::atomic::AtomicBool::new(false));
        rig.session.ps_ref().set_cancel_token(token.clone());

        let handled = rig.session.ps_ref().cancel_active_pipeline();
        assert!(!handled, "{start_state:?} must NOT be cancellable");
        assert_eq!(rig.sm.lock().unwrap().state(), start_state);
        assert!(!token.load(std::sync::atomic::Ordering::Relaxed));
        let events = rig.emitter.take_events();
        assert!(
            !events.iter().any(|(n, _)| n == "pipeline-cancelled"),
            "{start_state:?} must NOT emit pipeline-cancelled: {events:?}"
        );
        // Slot untouched (token still set).
        assert!(rig.session.ps_ref().take_cancel_token().is_some());
    }
}

#[test]
fn cancel_active_pipeline_without_token_still_returns_true_and_resets_state() {
    let rig = build_rig(config(false, false, false), "x");
    rig.sm
        .lock()
        .unwrap()
        .force_state_tag(StateTag::Transcribing);

    let handled = rig.session.ps_ref().cancel_active_pipeline();
    assert!(handled);
    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
    let events = rig.emitter.take_events();
    assert!(events.iter().any(|(n, _)| n == "pipeline-cancelled"));
}

#[test]
fn cancel_active_pipeline_hides_floating_window() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let wc = Arc::new(CallLogWindowController {
        calls: calls.clone(),
    });
    let rig = build_rig_full(
        config(false, false, false),
        "x",
        MockCorrector::new("x"),
        wc,
        true,
    );
    rig.sm
        .lock()
        .unwrap()
        .force_state_tag(StateTag::Transcribing);

    let handled = rig.session.ps_ref().cancel_active_pipeline();
    assert!(handled);
    let logged = calls.lock().unwrap().clone();
    assert!(
        logged.contains(&"hide_floating"),
        "cancel must hide floating window: {logged:?}"
    );
}

#[test]
fn cancel_active_pipeline_hides_review_window_when_shown_on_press() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let wc = Arc::new(CallLogWindowController {
        calls: calls.clone(),
    });
    let rig = build_rig_full(
        config(false, false, false),
        "x",
        MockCorrector::new("x"),
        wc,
        true,
    );
    rig.sm
        .lock()
        .unwrap()
        .force_state_tag(StateTag::Transcribing);
    // Simulate a RealtimeReview session that showed the review window on press.
    rig.session.ps_ref().review().set_shown_on_press(true);

    let handled = rig.session.ps_ref().cancel_active_pipeline();

    assert!(handled);
    let logged = calls.lock().unwrap().clone();
    assert!(
        logged.contains(&"hide_review"),
        "Esc-cancel must clean up a review window shown on press: {logged:?}"
    );
    assert!(
        logged.contains(&"hide_floating"),
        "Esc-cancel must hide the floating window: {logged:?}"
    );
    assert!(
        !rig.session.ps_ref().review().was_shown_on_press(),
        "Esc-cancel must clear the shown_on_press flag"
    );
    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
}

// ---------------------------------------------------------------------------
// P1 Esc-cancel: orchestration gate tests
//
// Drive run_pipeline / run_realtime_fast_path with a cancelled token and
// verify the pipeline bails at each gate WITHOUT injecting text or emitting
// `speech-error`. Ownership contract: the hook thread (cancel_active_pipeline,
// simulated by esc_cancel) resets the state machine, hides windows, and
// drains the token slot BEFORE the gates fire — the gates themselves only
// log and return. The restarted-session tests below pin why: a gate-side
// reset would kill a NEW session started during the LLM/Whisper window.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancel_during_transcribe_skips_speech_error_and_no_injection() {
    // Gate 1: classic pipeline, transcribe_and_save bails before emitting
    // speech-error. Whisper is mocked and returns instantly, so we can't
    // flip the token mid-call; instead the Esc lands while Whisper is
    // "running" — cancel_active_pipeline fires BEFORE run_pipeline is
    // invoked, resetting state to Idle, flipping + draining the token, and
    // hiding the floating window. Gate 1 then log-and-returns without
    // touching anything (ownership: the hook thread did the cleanup).
    let rig = build_rig(config(false, false, false), "raw transcription");
    to_transcribing(&rig.sm);
    let perf = PerfMetrics::new(0);
    let policy = SessionPolicy::from_config(&config(false, false, false));
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Pre-arm the cancel slot so cancel_active_pipeline flips + drains this
    // exact token (the same Arc run_pipeline captures below).
    rig.session.ps_ref().set_cancel_token(cancel.clone());
    assert!(rig.session.ps_ref().cancel_active_pipeline());

    rig.session
        .run_pipeline(
            vec![],
            48000,
            vec![0.5f32; 1600],
            false,
            perf,
            Instant::now(),
            policy,
            cancel.clone(),
        )
        .await;

    // Gate 1 returns BEFORE Whisper's transcription_complete emit — early
    // return without any emit. Verify: state still Idle (reset by the hook
    // thread, not the gate), clipboard untouched, no injection, token slot
    // still drained.
    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
    assert!(
        rig.clipboard.injected().is_empty(),
        "gate1 must not inject; got: {:?}",
        rig.clipboard.injected()
    );
    assert!(
        rig.session.ps_ref().take_cancel_token().is_none(),
        "token slot must stay drained after gate1"
    );
}

#[tokio::test]
async fn cancel_after_llm_drops_result_no_injection_no_review() {
    // Gate 2: the hook-thread cancel runs on the `transcription-complete`
    // emit — the exact pipeline point between gate 1 (which has already
    // passed its load) and the gate 2 check. A preset-true token would exit
    // at gate 1 and make this test vacuous for gate 2 (review finding:
    // gates 2/3/3' were unreachable behind gate 1 with preset tokens).
    let rig = build_rig(config(false, false, true), "raw transcription");
    to_transcribing(&rig.sm);
    let perf = PerfMetrics::new(0);
    let policy = SessionPolicy::from_config(&config(false, false, true));
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Pre-arm the cancel slot so esc_cancel flips + drains this exact token.
    rig.session.ps_ref().set_cancel_token(cancel.clone());
    {
        let ps = rig.session.ps_ref().clone();
        rig.emitter.set_on_event(Box::new(move |event: &str| {
            if event == "transcription-complete" {
                esc_cancel(&ps);
            }
        }));
    }

    rig.session
        .run_pipeline(
            vec![],
            48000,
            vec![0.5f32; 1600],
            false,
            perf,
            Instant::now(),
            policy,
            cancel.clone(),
        )
        .await;

    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
    assert!(
        rig.clipboard.injected().is_empty(),
        "gate2 must drop LLM result; got: {:?}",
        rig.clipboard.injected()
    );
    assert!(rig.session.ps_ref().take_cancel_token().is_none());
    // Distinguishability: the pipeline reached the transcription-complete
    // emit (past gate 1) but never entered the LLM phase (gate 2 fires
    // BEFORE resolve_llm_text). If this test is moved behind gate 3 the
    // llm-refining assertion below goes red.
    let names = event_names(&rig.emitter);
    assert!(
        names.contains(&"transcription-complete".to_string()),
        "gate2 test must reach transcription-complete; events: {names:?}"
    );
    assert!(
        !names.contains(&"llm-refining".to_string()),
        "gate2 fires before LLM starts; llm-refining leaked through: {names:?}"
    );
}

#[tokio::test]
async fn cancel_before_delivery_skips_both_review_and_inject() {
    // Gate 3: classic, review=true + llm=true. The hook-thread cancel runs
    // on the `llm-refining` emit (inside resolve_llm_text, i.e. AFTER gate
    // 2 has passed its load and LLM work begins) so the next check — gate
    // 3, after normalize, before show_review — is the one that trips.
    let rig = build_rig(config(false, true, true), "review me");
    to_transcribing(&rig.sm);
    let perf = PerfMetrics::new(0);
    let policy = SessionPolicy::from_config(&config(false, true, true));
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Pre-arm the cancel slot so esc_cancel flips + drains this exact token.
    rig.session.ps_ref().set_cancel_token(cancel.clone());
    {
        let ps = rig.session.ps_ref().clone();
        rig.emitter.set_on_event(Box::new(move |event: &str| {
            if event == "llm-refining" {
                esc_cancel(&ps);
            }
        }));
    }

    rig.session
        .run_pipeline(
            vec![],
            48000,
            vec![0.5f32; 1600],
            true,
            perf,
            Instant::now(),
            policy,
            cancel.clone(),
        )
        .await;

    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
    assert!(
        rig.clipboard.injected().is_empty(),
        "gate3 must skip inject"
    );
    let names = event_names(&rig.emitter);
    // No review window opened — show_review would emit "review-shown"
    // via DeliveryController. We assert nothing about review here beyond
    // clipboard emptiness (the mock window controller is a no-op).
    assert!(!names.contains(&"injection-complete".to_string()));
    // Distinguishability from gate 2: LLM actually ran (llm-refining was
    // emitted), so this test proves the post-LLM gate trips.
    assert!(
        names.contains(&"llm-refining".to_string()),
        "gate3 test must reach the LLM phase; events: {names:?}"
    );
    assert!(rig.session.ps_ref().take_cancel_token().is_none());
}

#[tokio::test]
async fn cancel_during_fast_path_skips_injection() {
    // Gate 2' (fast path): RealtimeDirect + LLM. The hook-thread cancel
    // runs on the `llm-refining` emit (resolve_llm_text starts AFTER the
    // fast path's earlier steps) so the fast-path cancel gates — checked
    // after LLM, before delivery — are what trip. A preset-true token
    // would exit at gate 2' indistinguishably, hiding coverage gaps.
    //
    // COVERAGE SEMANTICS: gates 2' and 3' are adjacent, externally
    // indistinguishable defences (same no-inject effect; the only code
    // between them is the save await, which emits no event). This test
    // therefore locks the JOINT coverage "gate2' OR gate3' exists":
    // disabling either one alone stays green (the other catches it —
    // verified manually), disabling BOTH goes red (the cancel reaches
    // inject_direct). Splitting them requires a test hook inside the save
    // spawn_blocking closure; tracked in _Project/TODO.md.
    let rig = build_rig(config(true, false, true), "ignored");
    to_transcribing(&rig.sm);
    let perf = PerfMetrics::new(0);
    let policy = SessionPolicy::from_config(&config(true, false, true));
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Pre-arm the cancel slot so esc_cancel flips + drains this exact token.
    rig.session.ps_ref().set_cancel_token(cancel.clone());
    {
        let ps = rig.session.ps_ref().clone();
        rig.emitter.set_on_event(Box::new(move |event: &str| {
            if event == "llm-refining" {
                esc_cancel(&ps);
            }
        }));
    }

    rig.session
        .run_realtime_fast_path(
            "你好".to_string(),
            vec![],
            48000,
            perf,
            Instant::now(),
            policy,
            cancel.clone(),
        )
        .await;

    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Idle);
    assert!(
        rig.clipboard.injected().is_empty(),
        "fast-path cancel must skip inject"
    );
    let names = event_names(&rig.emitter);
    assert!(
        names.contains(&"llm-refining".to_string()),
        "gate2' test must reach the LLM phase; events: {names:?}"
    );
    assert!(!names.contains(&"injection-complete".to_string()));
    assert!(rig.session.ps_ref().take_cancel_token().is_none());
}

// ---------------------------------------------------------------------------
// P0 regression (2026-09-06 review): Esc resets the pipeline on the hook
// thread (cancel_active_pipeline → Idle), so the user can re-press and start
// a NEW Recording session while the old pipeline body (LLM HTTP / Whisper)
// is still in flight. When that body later hits a cancel gate it must NOT
// touch the state machine, windows, or token slot again — those now belong
// to the new session. A gate-side reset silently kills the second recording.
// ---------------------------------------------------------------------------

/// What the hook thread does on Esc (cancel_active_pipeline), minus the
/// `pipeline-cancelled` emit — MockEmitter fires test hooks while holding
/// its `on_event` lock, so re-entering `emit` from inside would deadlock.
/// The emit is the ONLY divergence: guard, token flip, and hide_overlays
/// mirror the production method exactly.
fn esc_cancel(ps: &crate::commands::pipeline_state::PipelineState) {
    assert!(
        ps.sm_cancel_transcribing_or_llm(),
        "hook-thread cancel must succeed (pipeline is in a cancellable phase)"
    );
    if let Some(token) = ps.take_cancel_token() {
        token.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    ps.hide_overlays();
}

/// `esc_cancel` plus the user immediately re-pressing the hotkey
/// (Idle → Recording) — the P0 race precondition.
fn esc_then_repress(
    ps: &crate::commands::pipeline_state::PipelineState,
    sm: &Arc<Mutex<StateMachine>>,
) {
    esc_cancel(ps);
    sm.lock().unwrap().start_recording().unwrap();
}

fn hide_floating_count(calls: &Arc<Mutex<Vec<&'static str>>>) -> usize {
    calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| **c == "hide_floating")
        .count()
}

#[tokio::test]
async fn cancel_gate_leaves_restarted_session_intact_classic() {
    // Gate 3 (classic): token trips during the LLM phase, gate fires after
    // normalize with a new Recording session already running.
    let calls: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let wc = Arc::new(CallLogWindowController {
        calls: calls.clone(),
    });
    let rig = build_rig_inner(
        config(false, false, true),
        "raw",
        MockCorrector::new("corrected"),
        wc,
    );
    to_transcribing(&rig.sm);
    let ps = rig.session.ps_ref().clone();
    let sm = rig.sm.clone();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    rig.session.ps_ref().set_cancel_token(cancel.clone());
    {
        let ps = ps.clone();
        let sm = sm.clone();
        rig.emitter.set_on_event(Box::new(move |event: &str| {
            if event == "llm-refining" {
                esc_then_repress(&ps, &sm);
            }
        }));
    }

    rig.session
        .run_pipeline(
            vec![],
            48000,
            vec![0.5f32; 1600],
            false,
            PerfMetrics::new(0),
            Instant::now(),
            SessionPolicy::from_config(&config(false, false, true)),
            cancel.clone(),
        )
        .await;

    assert_eq!(
        rig.sm.lock().unwrap().state(),
        StateTag::Recording,
        "gate must not reset a restarted session's state"
    );
    assert!(
        rig.clipboard.injected().is_empty(),
        "cancelled pipeline must not inject"
    );
    assert_eq!(
        hide_floating_count(&calls),
        1,
        "only cancel_active_pipeline may hide the floating window; a second hide would kill the new session's UI"
    );
}

#[tokio::test]
async fn cancel_gate_leaves_restarted_session_intact_fast_path() {
    // Gates 2'/3' (fast path): token trips during the LLM phase of
    // run_realtime_fast_path, gate fires with a new Recording session
    // already running.
    let calls: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let wc = Arc::new(CallLogWindowController {
        calls: calls.clone(),
    });
    let rig = build_rig_inner(
        config(true, false, true),
        "ignored",
        MockCorrector::new("corrected"),
        wc,
    );
    to_transcribing(&rig.sm);
    let ps = rig.session.ps_ref().clone();
    let sm = rig.sm.clone();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    rig.session.ps_ref().set_cancel_token(cancel.clone());
    {
        let ps = ps.clone();
        let sm = sm.clone();
        rig.emitter.set_on_event(Box::new(move |event: &str| {
            if event == "llm-refining" {
                esc_then_repress(&ps, &sm);
            }
        }));
    }

    rig.session
        .run_realtime_fast_path(
            "你好".to_string(),
            vec![],
            48000,
            PerfMetrics::new(0),
            Instant::now(),
            SessionPolicy::from_config(&config(true, false, true)),
            cancel.clone(),
        )
        .await;

    assert_eq!(
        rig.sm.lock().unwrap().state(),
        StateTag::Recording,
        "gate must not reset a restarted session's state"
    );
    assert!(
        rig.clipboard.injected().is_empty(),
        "cancelled fast path must not inject"
    );
    assert_eq!(
        hide_floating_count(&calls),
        1,
        "only cancel_active_pipeline may hide the floating window"
    );
}

#[tokio::test]
async fn cancel_before_whisper_returns_leaves_restarted_session_intact() {
    // Gate 1 + the empty-transcription branch: Esc lands while Whisper is
    // still running (hook thread cancels + resets state), the user
    // re-presses, and only THEN does the pipeline body observe the token.
    // Gate 1 returns an empty transcription, and the empty-transcription
    // branch must not "clean up" a state machine that no longer belongs to
    // this pipeline.
    let calls: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let wc = Arc::new(CallLogWindowController {
        calls: calls.clone(),
    });
    let rig = build_rig_inner(
        config(false, false, false),
        "raw transcription",
        MockCorrector::new("corrected"),
        wc,
    );
    to_transcribing(&rig.sm);
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    rig.session.ps_ref().set_cancel_token(cancel.clone());

    assert!(rig.session.ps_ref().cancel_active_pipeline());
    // User immediately re-presses before the pipeline body runs.
    rig.sm.lock().unwrap().start_recording().unwrap();

    rig.session
        .run_pipeline(
            vec![],
            48000,
            vec![0.5f32; 1600],
            false,
            PerfMetrics::new(0),
            Instant::now(),
            SessionPolicy::from_config(&config(false, false, false)),
            cancel.clone(),
        )
        .await;

    assert_eq!(
        rig.sm.lock().unwrap().state(),
        StateTag::Recording,
        "gate 1 + empty-transcription branch must not reset a restarted session"
    );
    assert!(
        rig.clipboard.injected().is_empty(),
        "cancelled pipeline must not inject"
    );
    assert_eq!(hide_floating_count(&calls), 1);
}

#[tokio::test]
async fn happy_path_clears_cancel_token_slot() {
    // No cancel: token slot must still be cleared on the success exit
    // (preventing vacuous test passes from a leaked flag).
    let rig = build_rig(config(false, false, false), "raw transcription");
    to_transcribing(&rig.sm);
    let perf = PerfMetrics::new(0);
    let policy = SessionPolicy::from_config(&config(false, false, false));
    // Pre-arm the slot with a fresh token; pipeline must clear it.
    let pre_token = Arc::new(std::sync::atomic::AtomicBool::new(false));
    rig.session.ps_ref().set_cancel_token(pre_token.clone());

    rig.session
        .run_pipeline(
            vec![],
            48000,
            vec![0.5f32; 1600],
            false,
            perf,
            Instant::now(),
            policy,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;

    // Successful inject happened.
    assert!(!rig.clipboard.injected().is_empty());
    // Slot cleared — proving the post-delivery take_cancel_token runs.
    assert!(
        rig.session.ps_ref().take_cancel_token().is_none(),
        "happy path must clear cancel slot"
    );
}

#[tokio::test]
async fn review_happy_path_clears_cancel_token_slot() {
    let rig = build_rig(config(false, true, false), "review me");
    to_transcribing(&rig.sm);
    let perf = PerfMetrics::new(0);
    let policy = SessionPolicy::from_config(&config(false, true, false));
    rig.session
        .ps_ref()
        .set_cancel_token(Arc::new(std::sync::atomic::AtomicBool::new(false)));

    rig.session
        .run_pipeline(
            vec![],
            48000,
            vec![0.5f32; 1600],
            true,
            perf,
            Instant::now(),
            policy,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;

    assert_eq!(rig.sm.lock().unwrap().state(), StateTag::Reviewing);
    assert!(
        rig.session.ps_ref().take_cancel_token().is_none(),
        "review path must clear cancel slot"
    );
}
