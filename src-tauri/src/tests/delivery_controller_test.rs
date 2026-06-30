//! DeliveryController unit tests.
//!
//! These tests exercise the single authority for the post-transcription
//! delivery lifecycle: direct inject, review show/fallback, confirm, cancel,
//! and clipboard restore on failure.

use crate::audio::MockAudioCapture;
use crate::clipboard::{AnyClipboard, MockClipboard};
use crate::commands::MockEmitter;
use crate::commands::pipeline_state::PipelineState;
use crate::commands::recording_session::SessionPolicy;
use crate::commands::review_provider::MockReviewProvider;
use crate::commands::window_controller::{NoopWindowController, WindowController};
use crate::config::{AppConfig, ConfigCache};
use crate::llm::{AnyCorrector, MockCorrector};
use crate::perf::PerfHistory;
use crate::speech::{AnyEngine, mock::MockEngine};
use crate::state::{StateMachine, StateTag};
use std::sync::{Arc, Mutex};
use std::time::Instant;

fn build_ps() -> (PipelineState, Arc<MockEmitter>) {
    let sm = Arc::new(Mutex::new(StateMachine::new()));
    let ac = Arc::new(Mutex::new(MockAudioCapture::new()));
    let engine = Arc::new(AnyEngine::Mock(MockEngine::new("test")));
    let clipboard = Arc::new(Mutex::new(AnyClipboard::Mock(MockClipboard::new())));
    let emitter = Arc::new(MockEmitter::new());
    let ps = PipelineState::new(
        sm,
        ac,
        engine,
        clipboard,
        Arc::new(PerfHistory::new()),
        ConfigCache::new(AppConfig::default()),
        Arc::new(Mutex::new(Some(AnyCorrector::Mock(MockCorrector::new(
            "corrected",
        ))))),
        Arc::new(Mutex::new(None)),
        Arc::new(NoopWindowController),
        emitter.clone(),
        Arc::new(MockReviewProvider::new()),
    );
    (ps, emitter)
}

fn build_policy() -> SessionPolicy {
    SessionPolicy::from_config(&AppConfig::default())
}

fn to_reviewing(ps: &PipelineState) {
    ps.sm_start_recording();
    ps.sm_stop_recording();
    ps.sm_transcribing_to_reviewing();
}

fn to_transcribing(ps: &PipelineState) {
    ps.sm_start_recording();
    ps.sm_stop_recording();
}

fn to_injecting(ps: &PipelineState) {
    ps.sm_start_recording();
    ps.sm_stop_recording();
    ps.sm_transcribing_to_injecting();
}

fn event_names(emitter: &MockEmitter) -> Vec<String> {
    emitter
        .take_events()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

#[tokio::test]
async fn test_inject_direct_succeeds() {
    let (ps, emitter) = build_ps();
    to_injecting(&ps);
    let mut perf = crate::perf::PerfMetrics::new(0);
    let policy = build_policy();

    ps.delivery
        .inject_direct(
            &ps,
            "hello world".to_string(),
            "hello world".to_string(),
            None,
            &policy,
            &mut perf,
            Instant::now(),
            false,
        )
        .await;

    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
    let names = event_names(&emitter);
    assert!(names.contains(&"injection-complete".to_string()));
}

#[tokio::test]
async fn test_show_review_enters_reviewing() {
    let (ps, emitter) = build_ps();
    to_transcribing(&ps);
    let perf = crate::perf::PerfMetrics::new(0);
    let policy = build_policy();

    ps.delivery
        .show_review(
            &ps,
            "review me".to_string(),
            "review me".to_string(),
            None,
            &policy,
            perf,
            Instant::now(),
            false,
        )
        .await;

    assert_eq!(ps.sm_state(), Some(StateTag::Reviewing));
    let names = event_names(&emitter);
    assert!(!names.contains(&"injection-complete".to_string()));
}

#[tokio::test]
async fn test_show_review_fallback_injects_when_window_missing() {
    // NoopWindowController::show_review_near_caret always returns false, so
    // show_review must fall back to direct injection.
    let (ps, emitter) = build_ps();
    to_transcribing(&ps);
    let perf = crate::perf::PerfMetrics::new(0);
    let policy = build_policy();

    // Swap to a window controller that pretends the review window is missing.
    let ps = PipelineState::new(
        ps.sm.clone(),
        ps.ac.clone(),
        ps.engine.clone(),
        ps.clipboard.clone(),
        ps.perf_history.clone(),
        ps.config_cache.clone(),
        ps.cached_llm.clone(),
        ps.realtime_transcriber.clone(),
        Arc::new(HiddenReviewWindowController),
        ps.emitter.clone(),
        ps.review.clone(),
    );

    ps.delivery
        .show_review(
            &ps,
            "fallback text".to_string(),
            "fallback text".to_string(),
            None,
            &policy,
            perf,
            Instant::now(),
            false,
        )
        .await;

    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
    let names = event_names(&emitter);
    assert!(names.contains(&"injection-complete".to_string()));
}

#[tokio::test]
async fn test_confirm_review_injects_and_returns_idle() {
    let (ps, emitter) = build_ps();
    to_reviewing(&ps);

    let confirm_result = ps
        .delivery
        .confirm_review(&ps, "confirmed text".to_string())
        .await;
    assert!(
        confirm_result.is_ok(),
        "confirm from Reviewing should succeed"
    );

    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
    let names = event_names(&emitter);
    assert!(names.contains(&"injection-complete".to_string()));
}

#[tokio::test]
async fn test_confirm_review_from_idle_returns_error() {
    let (ps, _emitter) = build_ps();

    let result = ps.delivery.confirm_review(&ps, "text".to_string()).await;
    assert!(result.is_err());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
}

#[tokio::test]
async fn test_confirm_review_from_recording_returns_error_and_resets() {
    let (ps, _emitter) = build_ps();
    ps.sm_start_recording();

    let result = ps.delivery.confirm_review(&ps, "text".to_string()).await;
    assert!(result.is_err());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
}

#[tokio::test]
async fn test_cancel_review_returns_to_idle() {
    let (ps, _emitter) = build_ps();
    to_reviewing(&ps);

    let cancel_result = ps.delivery.cancel_review(&ps).await;
    assert!(
        cancel_result.is_ok(),
        "cancel from Reviewing should succeed"
    );

    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
}

#[tokio::test]
async fn test_cancel_review_from_idle_returns_error() {
    let (ps, _emitter) = build_ps();
    // No state setup: starts in Idle.

    let cancel_result = ps.delivery.cancel_review(&ps).await;
    assert!(cancel_result.is_err());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
}

#[tokio::test]
async fn test_cancel_review_clipboard_restore_failure_still_returns_idle() {
    let (ps, _emitter) = build_ps();
    to_reviewing(&ps);

    let mut mock = MockClipboard::new();
    mock.restore_error = Some("restore failed".to_string());
    let failing_clipboard = Arc::new(Mutex::new(AnyClipboard::Mock(mock)));
    let ps_with_failing = PipelineState::new(
        ps.sm.clone(),
        ps.ac.clone(),
        ps.engine.clone(),
        failing_clipboard,
        ps.perf_history.clone(),
        ps.config_cache.clone(),
        ps.cached_llm.clone(),
        ps.realtime_transcriber.clone(),
        ps.window_controller.clone(),
        ps.emitter.clone(),
        ps.review.clone(),
    );

    let cancel_result = ps_with_failing
        .delivery
        .cancel_review(&ps_with_failing)
        .await;
    assert!(
        cancel_result.is_ok(),
        "cancel must return Ok even when clipboard restore fails"
    );
    assert_eq!(ps_with_failing.sm_state(), Some(StateTag::Idle));
}

#[tokio::test]
async fn test_clipboard_restore_on_inject_failure() {
    let (ps, _emitter) = build_ps();
    to_injecting(&ps);

    let mut mock = MockClipboard::new();
    mock.inject_error = Some("inject failed".to_string());
    let failing_clipboard = Arc::new(Mutex::new(AnyClipboard::Mock(mock)));
    let ps_with_failing = PipelineState::new(
        ps.sm.clone(),
        ps.ac.clone(),
        ps.engine.clone(),
        failing_clipboard.clone(),
        ps.perf_history.clone(),
        ps.config_cache.clone(),
        ps.cached_llm.clone(),
        ps.realtime_transcriber.clone(),
        ps.window_controller.clone(),
        ps.emitter.clone(),
        ps.review.clone(),
    );

    let mut perf = crate::perf::PerfMetrics::new(0);
    let policy = build_policy();
    ps_with_failing
        .delivery
        .inject_direct(
            &ps_with_failing,
            "hello".to_string(),
            "hello".to_string(),
            None,
            &policy,
            &mut perf,
            Instant::now(),
            false,
        )
        .await;

    assert_eq!(ps_with_failing.sm_state(), Some(StateTag::Idle));
    let restored = match failing_clipboard.lock() {
        Ok(guard) => match &*guard {
            AnyClipboard::Mock(m) => m.restored,
            _ => panic!("expected mock clipboard"),
        },
        Err(_) => panic!("clipboard lock poisoned"),
    };
    assert!(restored, "clipboard should be restored on inject failure");
}

/// Window controller that simulates a missing review window so the fallback
/// direct-injection path in `show_review` is exercised.
struct HiddenReviewWindowController;

impl WindowController for HiddenReviewWindowController {
    fn show_floating_near_caret(&self) -> bool {
        true
    }
    fn hide_floating(&self) {}
    fn show_review_near_caret(&self) -> bool {
        false
    }
    fn hide_review(&self) {}
    fn focus_review(&self) -> bool {
        true
    }
    fn eval_review_js(&self, _js: &str) -> bool {
        true
    }
    fn emit_review_show(&self) {}
    fn emit_review_final_text(&self, _text: &str) {}
}
