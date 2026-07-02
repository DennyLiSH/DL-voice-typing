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
use crate::speech::mock::MockEngine;
use crate::state::{StateMachine, StateTag};
use std::sync::{Arc, Mutex};
use std::time::Instant;

fn build_ps() -> (PipelineState, Arc<MockEmitter>) {
    let sm = Arc::new(Mutex::new(StateMachine::new()));
    let ac = Arc::new(Mutex::new(MockAudioCapture::new()));
    let engine = Arc::new(MockEngine::new("test"));
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
async fn show_review_fallback_clears_context() {
    // Regression for M1 fix: fallback path must clear DeliveryContext to prevent
    // residual foreground_hwnd from polluting the next review cycle's focus
    // restoration (stale HWND → data delivery target confusion security channel).
    let (ps, _emitter) = build_ps();
    to_transcribing(&ps);
    let perf = crate::perf::PerfMetrics::new(0);
    let policy = build_policy();

    // Swap to a window controller that pretends the review window is missing,
    // forcing show_review into the fallback direct-injection branch.
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

    // After M1 fix, fallback path calls take_context() to clear residual state.
    // show_review's classic-review prefix (save_foreground + take_foreground +
    // store_foreground at delivery_controller.rs:243-246) runs before the
    // fallback branch, so without the fix foreground_hwnd would leak a sentinel
    // value (MockReviewProvider uses 42) into the next review cycle. This
    // indirectly guarantees stale HWND cannot hijack the next inject_text path.
    let (hwnd, data, _perf, t_press) = ps.delivery.take_context();
    assert!(
        hwnd.is_none(),
        "foreground_hwnd must be cleared after fallback (got {hwnd:?})"
    );
    assert!(data.is_none(), "review_data must be cleared after fallback");
    assert!(
        t_press.is_none(),
        "t_press_for_e2e must be cleared after fallback"
    );
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
    fn restore_foreground_hwnd(&self, _hwnd: isize) {}
}

// -----------------------------------------------------------------------------
// Task 2 regression: confirm_from_reviewing must restore foreground focus again
// when injection fails, so the user is not left in the review window.
// -----------------------------------------------------------------------------

/// Window controller that records restore_foreground_hwnd calls for verification.
struct RecordingWindowController {
    calls: Mutex<Vec<(String, Option<isize>)>>,
}

impl RecordingWindowController {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }

    fn record(&self, name: &str, hwnd: Option<isize>) {
        if let Ok(mut guard) = self.calls.lock() {
            guard.push((name.to_string(), hwnd));
        }
    }

    fn take_calls(&self) -> Vec<(String, Option<isize>)> {
        crate::util::lock_mutex(&self.calls, "RecordingWindowController::calls")
            .map(|mut guard| guard.drain(..).collect())
            .unwrap_or_default()
    }
}

impl WindowController for RecordingWindowController {
    fn show_floating_near_caret(&self) -> bool {
        true
    }
    fn hide_floating(&self) {}
    fn show_review_near_caret(&self) -> bool {
        true
    }
    fn hide_review(&self) {
        self.record("hide_review", None);
    }
    fn focus_review(&self) -> bool {
        true
    }
    fn eval_review_js(&self, _js: &str) -> bool {
        true
    }
    fn emit_review_show(&self) {}
    fn emit_review_final_text(&self, _text: &str) {}
    fn restore_foreground_hwnd(&self, hwnd: isize) {
        self.record("restore_foreground_hwnd", Some(hwnd));
    }
}

#[tokio::test]
async fn confirm_review_error_branch_restores_focus() {
    // Clipboard that succeeds at save() but fails at inject_text().
    let mut mock = MockClipboard::new();
    mock.inject_error = Some("inject failed".to_string());
    let failing_clipboard = Arc::new(Mutex::new(AnyClipboard::Mock(mock)));

    let recording = Arc::new(RecordingWindowController::new());

    let (base_ps, _emitter) = build_ps();
    let ps = PipelineState::new(
        base_ps.sm.clone(),
        base_ps.ac.clone(),
        base_ps.engine.clone(),
        failing_clipboard,
        base_ps.perf_history.clone(),
        base_ps.config_cache.clone(),
        base_ps.cached_llm.clone(),
        base_ps.realtime_transcriber.clone(),
        recording.clone(),
        base_ps.emitter.clone(),
        base_ps.review.clone(),
    );

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

    let result = ps
        .delivery
        .confirm_review(&ps, "confirmed text".to_string())
        .await;
    assert!(
        result.is_ok(),
        "confirm_review returns Ok even on inject failure"
    );

    let restore_calls: Vec<_> = recording
        .take_calls()
        .into_iter()
        .filter(|(name, _)| name == "restore_foreground_hwnd")
        .collect();

    assert_eq!(
        restore_calls.len(),
        2,
        "focus must be restored before inject and again on inject failure"
    );
    assert_eq!(restore_calls[0].1, Some(42));
    assert_eq!(restore_calls[1].1, Some(42));

    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
}

// -----------------------------------------------------------------------------
// M2 regression: confirm_review / cancel_review early-exception paths must
// clear DeliveryContext so residual foreground_hwnd cannot hijack the next
// cycle's restore_foreground_hwnd (same root cause as M1 commit 72cb57e).
// -----------------------------------------------------------------------------

/// Drive show_review through the classic-review path to populate
/// DeliveryContext.foreground_hwnd with the MockReviewProvider sentinel (42),
/// plus perf and t_press_for_e2e.
async fn populate_context_via_show_review(ps: &PipelineState) {
    to_transcribing(ps);
    let perf = crate::perf::PerfMetrics::new(0);
    let policy = build_policy();
    ps.delivery
        .show_review(
            ps,
            "review me".to_string(),
            "review me".to_string(),
            None,
            &policy,
            perf,
            Instant::now(),
            false,
        )
        .await;
    // Sanity: show_review entered Reviewing and populated context.
    assert_eq!(ps.sm_state(), Some(StateTag::Reviewing));
    let (hwnd, _data, _perf, _t_press) = ps.delivery.take_context();
    assert_eq!(
        hwnd,
        Some(42),
        "test setup invariant: show_review must populate foreground_hwnd before disturbance"
    );
    // Re-populate because the sanity check above just took it.
    to_transcribing(ps);
    let perf = crate::perf::PerfMetrics::new(0);
    ps.delivery
        .show_review(
            ps,
            "review me".to_string(),
            "review me".to_string(),
            None,
            &policy,
            perf,
            Instant::now(),
            false,
        )
        .await;
}

#[tokio::test]
async fn confirm_review_recording_branch_clears_context() {
    let (ps, _emitter) = build_ps();
    populate_context_via_show_review(&ps).await;

    // Disturb state machine back to Recording (e.g., user pressed hotkey again
    // while review window was visible).
    ps.sm_reset();
    ps.sm_start_recording();
    assert_eq!(ps.sm_state(), Some(StateTag::Recording));

    let result = ps.delivery.confirm_review(&ps, "text".to_string()).await;
    assert!(result.is_err());

    let (hwnd, data, _perf, t_press) = ps.delivery.take_context();
    assert!(
        hwnd.is_none(),
        "foreground_hwnd must be cleared after Recording branch (got {hwnd:?})"
    );
    assert!(data.is_none(), "review_data must be cleared");
    assert!(t_press.is_none(), "t_press_for_e2e must be cleared");
}

#[tokio::test]
async fn confirm_review_catchall_branch_clears_context() {
    let (ps, _emitter) = build_ps();
    populate_context_via_show_review(&ps).await;

    // Reset to Idle so confirm_review falls through to the catch-all arm.
    ps.sm_reset();
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));

    let result = ps.delivery.confirm_review(&ps, "text".to_string()).await;
    assert!(result.is_err());

    let (hwnd, data, _perf, t_press) = ps.delivery.take_context();
    assert!(
        hwnd.is_none(),
        "foreground_hwnd must be cleared after catch-all branch (got {hwnd:?})"
    );
    assert!(data.is_none(), "review_data must be cleared");
    assert!(t_press.is_none(), "t_press_for_e2e must be cleared");
}

#[tokio::test]
async fn cancel_review_sm_cancel_failure_branch_clears_context() {
    let (ps, _emitter) = build_ps();
    populate_context_via_show_review(&ps).await;

    // Simulate the TOCTOU outcome: sm_state() observes Reviewing, but the
    // real state has already left Reviewing, so sm_cancel_reviewing() fails.
    ps.force_state_tag(StateTag::Idle); // real tag
    ps.force_sm_state(StateTag::Reviewing); // reported tag

    assert_eq!(ps.sm_state(), Some(StateTag::Reviewing));

    let result = ps.delivery.cancel_review(&ps).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, "STATE");
    assert_eq!(err.message, "cancel_reviewing failed");

    // The failure branch must clear the leaked DeliveryContext.
    let (hwnd, data, _perf, t_press) = ps.delivery.take_context();
    assert!(
        hwnd.is_none(),
        "foreground_hwnd must be cleared after sm_cancel_reviewing failure (got {hwnd:?})"
    );
    assert!(data.is_none(), "review_data must be cleared");
    assert!(t_press.is_none(), "t_press_for_e2e must be cleared");
}

#[tokio::test]
async fn cancel_review_catchall_branch_clears_context() {
    let (ps, _emitter) = build_ps();
    populate_context_via_show_review(&ps).await;

    // Reset to Idle so cancel_review falls through to the catch-all arm.
    ps.sm_reset();
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));

    let result = ps.delivery.cancel_review(&ps).await;
    assert!(result.is_err());

    let (hwnd, data, _perf, t_press) = ps.delivery.take_context();
    assert!(
        hwnd.is_none(),
        "foreground_hwnd must be cleared after catch-all branch (got {hwnd:?})"
    );
    assert!(data.is_none(), "review_data must be cleared");
    assert!(t_press.is_none(), "t_press_for_e2e must be cleared");
}
