//! DeliveryController unit tests.
//!
//! These tests exercise the single authority for the post-transcription
//! delivery lifecycle: direct inject, review show/fallback, confirm, cancel,
//! and clipboard restore on failure.

use crate::audio::MockAudioCapture;
use crate::clipboard::MockClipboard;
use crate::commands::MockEmitter;
use crate::commands::pipeline_state::PipelineState;
use crate::commands::recording_session::SessionPolicy;
use crate::commands::review_provider::{MockReviewProvider, ReviewProvider};
use crate::commands::window_controller::{NoopWindowController, WindowController};
use crate::config::{AppConfig, ConfigCache};
use crate::data_saving::SaveResult;
use crate::llm::MockCorrector;
use crate::perf::PerfHistory;
use crate::speech::mock::MockEngine;
use crate::state::{StateMachine, StateTag};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::commands::delivery_controller::{FocusOps, InjectError};

fn build_ps() -> (PipelineState, Arc<MockEmitter>) {
    build_ps_with_clipboard(Arc::new(MockClipboard::new()))
}

fn build_ps_with_clipboard(clipboard: Arc<MockClipboard>) -> (PipelineState, Arc<MockEmitter>) {
    build_ps_with_clipboard_and_review(clipboard, Arc::new(MockReviewProvider::new()))
}

fn build_ps_with_clipboard_and_review(
    clipboard: Arc<MockClipboard>,
    review: Arc<MockReviewProvider>,
) -> (PipelineState, Arc<MockEmitter>) {
    let sm = Arc::new(Mutex::new(StateMachine::new()));
    let ac = Arc::new(Mutex::new(MockAudioCapture::new()));
    let engine = Arc::new(MockEngine::new("test"));
    let emitter = Arc::new(MockEmitter::new());
    let ps = PipelineState::new(
        sm,
        ac,
        engine,
        clipboard,
        Arc::new(PerfHistory::new()),
        ConfigCache::new(AppConfig::default()),
        Arc::new(Mutex::new(Some(Box::new(MockCorrector::new("corrected"))))),
        Arc::new(Mutex::new(None)),
        Arc::new(NoopWindowController),
        emitter.clone(),
        review,
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
    // inject_direct's entry transition moves Transcribing -> Injecting, so the
    // state must be Transcribing (not pre-advanced to Injecting) when called.
    to_transcribing(&ps);
    let mut perf = crate::perf::PerfMetrics::new(0);
    let policy = build_policy();

    ps.delivery()
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

    ps.delivery()
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
    let c = ps.test_components();
    let ps = PipelineState::new(
        c.sm,
        c.ac,
        c.engine,
        c.clipboard,
        c.perf_history,
        c.config_cache,
        c.cached_llm,
        c.realtime_transcriber,
        Arc::new(HiddenReviewWindowController),
        c.emitter,
        c.review,
    );

    ps.delivery()
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
    let c = ps.test_components();
    let ps = PipelineState::new(
        c.sm,
        c.ac,
        c.engine,
        c.clipboard,
        c.perf_history,
        c.config_cache,
        c.cached_llm,
        c.realtime_transcriber,
        Arc::new(HiddenReviewWindowController),
        c.emitter,
        c.review,
    );

    ps.delivery()
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
    let (hwnd, data, _perf, t_press) = ps.delivery().take_context();
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
        .delivery()
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

    let result = ps.delivery().confirm_review(&ps, "text".to_string()).await;
    assert!(result.is_err());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
}

#[tokio::test]
async fn test_confirm_review_from_recording_returns_error_and_resets() {
    let (ps, _emitter) = build_ps();
    ps.sm_start_recording();

    let result = ps.delivery().confirm_review(&ps, "text".to_string()).await;
    assert!(result.is_err());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
}

#[tokio::test]
async fn test_cancel_review_returns_to_idle() {
    let (ps, _emitter) = build_ps();
    to_reviewing(&ps);

    let cancel_result = ps.delivery().cancel_review(&ps).await;
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

    let cancel_result = ps.delivery().cancel_review(&ps).await;
    assert!(cancel_result.is_err());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
}

#[tokio::test]
async fn test_cancel_review_clipboard_restore_failure_still_returns_idle() {
    let (ps, _emitter) = build_ps();
    to_reviewing(&ps);

    let failing_clipboard = Arc::new(MockClipboard::new().with_restore_error("restore failed"));
    let c = ps.test_components();
    let ps_with_failing = PipelineState::new(
        c.sm,
        c.ac,
        c.engine,
        failing_clipboard,
        c.perf_history,
        c.config_cache,
        c.cached_llm,
        c.realtime_transcriber,
        c.window_controller,
        c.emitter,
        c.review,
    );

    let cancel_result = ps_with_failing
        .delivery()
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
    // inject_direct's entry transition expects Transcribing, not Injecting.
    to_transcribing(&ps);

    let mock = Arc::new(MockClipboard::new().with_inject_error("inject failed"));
    let failing_clipboard = mock.clone();
    let c = ps.test_components();
    let ps_with_failing = PipelineState::new(
        c.sm,
        c.ac,
        c.engine,
        failing_clipboard,
        c.perf_history,
        c.config_cache,
        c.cached_llm,
        c.realtime_transcriber,
        c.window_controller,
        c.emitter,
        c.review,
    );

    let mut perf = crate::perf::PerfMetrics::new(0);
    let policy = build_policy();
    ps_with_failing
        .delivery()
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
    assert!(
        mock.restored(),
        "clipboard should be restored on inject failure"
    );
}

/// Window controller that simulates a missing review window so the fallback
/// direct-injection path in `show_review` is exercised.
/// Window controller that records every call name (cleanup-ordering asserts).
struct CallRecordingWindowController {
    calls: Arc<Mutex<Vec<&'static str>>>,
}

impl WindowController for CallRecordingWindowController {
    fn show_floating_near_caret(&self) -> bool {
        if let Some(mut c) = crate::util::lock_mutex(&self.calls, "rw_controller") {
            c.push("show_floating");
        }
        true
    }
    fn hide_floating(&self) {
        if let Some(mut c) = crate::util::lock_mutex(&self.calls, "rw_controller") {
            c.push("hide_floating");
        }
    }
    fn show_review_near_caret(&self) -> bool {
        if let Some(mut c) = crate::util::lock_mutex(&self.calls, "rw_controller") {
            c.push("show_review");
        }
        true
    }
    fn hide_review(&self) {
        if let Some(mut c) = crate::util::lock_mutex(&self.calls, "rw_controller") {
            c.push("hide_review");
        }
    }
    fn focus_review(&self) -> bool {
        if let Some(mut c) = crate::util::lock_mutex(&self.calls, "rw_controller") {
            c.push("focus_review");
        }
        true
    }
    fn eval_review_js(&self, _js: &str) -> bool {
        if let Some(mut c) = crate::util::lock_mutex(&self.calls, "rw_controller") {
            c.push("eval_review_js");
        }
        true
    }
    fn emit_review_show(&self) {
        if let Some(mut c) = crate::util::lock_mutex(&self.calls, "rw_controller") {
            c.push("emit_review_show");
        }
    }
    fn emit_review_final_text(&self, _text: &str) {
        if let Some(mut c) = crate::util::lock_mutex(&self.calls, "rw_controller") {
            c.push("emit_review_final_text");
        }
    }
    fn restore_foreground_hwnd(&self, _hwnd: isize) {
        if let Some(mut c) = crate::util::lock_mutex(&self.calls, "rw_controller") {
            c.push("restore_foreground");
        }
    }
}

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
    let failing_clipboard = Arc::new(MockClipboard::new().with_inject_error("inject failed"));

    let recording = Arc::new(RecordingWindowController::new());

    let (base_ps, _emitter) = build_ps();
    let c = base_ps.test_components();
    let ps = PipelineState::new(
        c.sm,
        c.ac,
        c.engine,
        failing_clipboard,
        c.perf_history,
        c.config_cache,
        c.cached_llm,
        c.realtime_transcriber,
        recording.clone(),
        c.emitter,
        c.review,
    );

    to_transcribing(&ps);
    let perf = crate::perf::PerfMetrics::new(0);
    let policy = build_policy();

    ps.delivery()
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
        .delivery()
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
    ps.delivery()
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
    let (hwnd, _data, _perf, _t_press) = ps.delivery().take_context();
    assert_eq!(
        hwnd,
        Some(42),
        "test setup invariant: show_review must populate foreground_hwnd before disturbance"
    );
    // Re-populate because the sanity check above just took it.
    to_transcribing(ps);
    let perf = crate::perf::PerfMetrics::new(0);
    ps.delivery()
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

    let result = ps.delivery().confirm_review(&ps, "text".to_string()).await;
    assert!(result.is_err());

    let (hwnd, data, _perf, t_press) = ps.delivery().take_context();
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

    let result = ps.delivery().confirm_review(&ps, "text".to_string()).await;
    assert!(result.is_err());

    let (hwnd, data, _perf, t_press) = ps.delivery().take_context();
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

    let result = ps.delivery().cancel_review(&ps).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, "STATE");
    assert_eq!(err.message, "cancel_reviewing failed");

    // The failure branch must clear the leaked DeliveryContext.
    let (hwnd, data, _perf, t_press) = ps.delivery().take_context();
    assert!(
        hwnd.is_none(),
        "foreground_hwnd must be cleared after sm_cancel_reviewing failure (got {hwnd:?})"
    );
    assert!(data.is_none(), "review_data must be cleared");
    assert!(t_press.is_none(), "t_press_for_e2e must be cleared");

    // Demonstrate the clear_forced_sm_state hygiene helper. After clearing,
    // sm_state() must reflect the real tag (Idle, set above by force_state_tag)
    // rather than the previously-forced Reviewing override.
    ps.clear_forced_sm_state();
    assert_eq!(
        ps.sm_state(),
        Some(StateTag::Idle),
        "clear_forced_sm_state must drop the override so sm_state reads the real tag"
    );
}

#[tokio::test]
async fn cancel_review_catchall_branch_clears_context() {
    let (ps, _emitter) = build_ps();
    populate_context_via_show_review(&ps).await;

    // Reset to Idle so cancel_review falls through to the catch-all arm.
    ps.sm_reset();
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));

    let result = ps.delivery().cancel_review(&ps).await;
    assert!(result.is_err());

    let (hwnd, data, _perf, t_press) = ps.delivery().take_context();
    assert!(
        hwnd.is_none(),
        "foreground_hwnd must be cleared after catch-all branch (got {hwnd:?})"
    );
    assert!(data.is_none(), "review_data must be cleared");
    assert!(t_press.is_none(), "t_press_for_e2e must be cleared");
}

// P1-1 regression: inject_direct / show_review must abort cleanly when the
// entry state-machine transition returns false (TOCTOU — real state already
// moved away from Transcribing/LLMRefining, e.g. watchdog reset or concurrent
// cancel_review won the lock). Pre-fix: bool return was dropped, inject
// proceeded anyway and the subsequent sm_finish_injecting also failed
// silently, leaving the state machine tag inconsistent with reality.

#[tokio::test]
async fn inject_direct_aborts_when_entry_transition_fails() {
    let (ps, emitter) = build_ps();
    // Real state stays Idle (default) — sm_transcribing_to_injecting() will
    // return false because the state machine is not in Transcribing.
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));

    let mut perf = crate::perf::PerfMetrics::new(0);
    let policy = build_policy();

    ps.delivery()
        .inject_direct(
            &ps,
            "hello".to_string(),
            "hello".to_string(),
            None,
            &policy,
            &mut perf,
            Instant::now(),
            false,
        )
        .await;

    let names = event_names(&emitter);
    assert!(
        !names.contains(&"injection-complete".to_string()),
        "inject_direct must not emit injection-complete when entry transition fails (got {names:?})"
    );
    assert_eq!(
        ps.sm_state(),
        Some(StateTag::Idle),
        "inject_direct must leave state at Idle after guard fires"
    );
}

#[tokio::test]
async fn show_review_aborts_when_entry_transition_fails() {
    let mock_cb = Arc::new(MockClipboard::new());
    let (ps, emitter) = build_ps_with_clipboard(mock_cb.clone());
    // Real state stays Idle — sm_transcribing_to_reviewing() will return false.
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));

    let perf = crate::perf::PerfMetrics::new(0);
    let policy = build_policy();

    ps.delivery()
        .show_review(
            &ps,
            "hello".to_string(),
            "hello".to_string(),
            None,
            &policy,
            perf,
            Instant::now(),
            false,
        )
        .await;

    let names = event_names(&emitter);
    assert!(
        !names.contains(&"injection-complete".to_string()),
        "show_review must not fall through to inject on entry transition failure (got {names:?})"
    );
    assert_eq!(
        ps.sm_state(),
        Some(StateTag::Idle),
        "show_review must leave state at Idle after guard fires"
    );

    // Clipboard must have been saved (pre-transition) then restored (guard
    // recovery). Assert through the concrete MockClipboard handle.
    assert!(
        mock_cb.saved(),
        "clipboard must be saved before attempting transition"
    );
    assert!(
        mock_cb.restored(),
        "clipboard must be restored when entry transition fails (else user's clipboard is silently held)"
    );
}

#[tokio::test]
async fn realtime_review_handoff_saves_clipboard_and_advances_state() {
    let mock_cb = Arc::new(MockClipboard::new());
    let review = Arc::new(MockReviewProvider::new());
    let (ps, _emitter) = build_ps_with_clipboard_and_review(mock_cb.clone(), review.clone());

    // Pre-seed review_provider.foreground via save_foreground() (MockReviewProvider
    // hardcodes sentinel 42 in MockReviewProvider::save_foreground). realtime_review_handoff will
    // call take_foreground() to migrate this hwnd into delivery context.
    review.save_foreground();

    // Drive state to Recording; realtime_review_handoff internally runs
    // sm_stop_recording -> sm_transcribing_to_reviewing, so the test enters from
    // the production path.
    ps.sm_start_recording();

    // Call realtime_review_handoff with some accumulated text:
    ps.delivery()
        .realtime_review_handoff(&ps, Some("accumulated".to_string()));

    // Assertions on current documented behavior:
    assert!(mock_cb.saved(), "clipboard must be saved");
    assert_eq!(
        mock_cb.injected(),
        Vec::<String>::new(),
        "realtime_review_handoff must NOT inject (no paste path)"
    );
    assert_eq!(ps.sm_state(), Some(StateTag::Reviewing));
    // store_text was called with "accumulated":
    assert_eq!(review.get_text(), Some("accumulated".to_string()));
    // foreground was migrated from review_provider.take_foreground() to delivery context.
    // DeliveryContext.context is private — use pub(crate) take_context() to inspect.
    let (foreground_hwnd, _data_saving, _perf, _t_press) = ps.delivery().take_context();
    assert_eq!(
        foreground_hwnd,
        Some(42),
        "sentinel from MockReviewProvider::save_foreground"
    );
}

#[tokio::test]
async fn show_review_was_shown_on_press_branch_stores_context_no_inject() {
    let mock_cb = Arc::new(MockClipboard::new());
    let mock_review = Arc::new(MockReviewProvider::new());
    let (ps, _emitter) = build_ps_with_clipboard_and_review(mock_cb.clone(), mock_review.clone());

    // Pre-seed via the existing ReviewProvider::set_shown_on_press trait method.
    mock_review.set_shown_on_press(true);

    // Drive state to Transcribing:
    ps.sm_start_recording();
    ps.sm_stop_recording();

    let policy = SessionPolicy::from_config(&AppConfig::default());
    let perf = crate::perf::PerfMetrics::new(0);

    let save_result = Some(SaveResult {
        wav_path: PathBuf::from("test.wav"),
        json_path: PathBuf::from("test.json"),
    });

    ps.delivery()
        .show_review(
            &ps,
            "final text".to_string(),
            "raw transcription".to_string(),
            save_result,
            &policy,
            perf,
            Instant::now(),
            false,
        )
        .await;

    // Assertions on current documented behavior:
    assert_eq!(ps.sm_state(), Some(StateTag::Reviewing));
    assert_eq!(
        mock_cb.injected(),
        Vec::<String>::new(),
        "was_shown_on_press path must NOT inject"
    );
    // show_review UNCONDITIONALLY calls clipboard.save() before the
    // was_shown_on_press branch — saved() returns true.
    assert!(
        mock_cb.saved(),
        "show_review always calls clipboard.save before branching"
    );
    // store_text was called unconditionally with the final text.
    assert_eq!(
        mock_review.get_text(),
        Some("final text".to_string()),
        "show_review must store final text in review provider"
    );
    // store_context populated. DeliveryContext.context is private; field name is
    // `data_saving` (not `review_data`). Use take_context() to inspect.
    // show_review builds ReviewData from save_result via Option::map, so
    // with save_result=Some(...) the stored data_saving must be Some(...).
    let (_foreground_hwnd, data_saving, _perf, _t_press) = ps.delivery().take_context();
    assert!(
        data_saving.is_some(),
        "data_saving must be stored for later confirm"
    );

    let rd = data_saving.unwrap();
    assert_eq!(rd.json_path, PathBuf::from("test.json"));
    assert_eq!(rd.raw_transcription, "raw transcription");
    // AppConfig::default().llm_enabled is false, so llm_text is None per
    // ReviewData construction in the was_shown_on_press branch.
    assert!(
        rd.llm_text.is_none(),
        "llm_text must be None when llm_enabled is false"
    );
}

// --- inject_to_hwnd (transcribe window detached delivery, ADR-0014) ---

const DEAD_WINDOW_OPS: FocusOps = FocusOps {
    is_valid: |_| false,
    focus: |_| false,
};

#[tokio::test]
async fn test_inject_to_hwnd_success() {
    let clipboard = Arc::new(MockClipboard::new());
    let (ps, _emitter) = build_ps_with_clipboard(clipboard.clone());
    let ops = FocusOps {
        is_valid: |_| true,
        focus: |_| true,
    };

    let result = ps.delivery().inject_to_hwnd(7, "成功文本", &ops).await;

    assert!(result.is_ok());
    assert!(clipboard.saved());
    assert_eq!(clipboard.injected(), vec!["成功文本".to_string()]);
    // Success goes through the atomic save_and_inject only — no fallback set_text.
    assert!(clipboard.set_texts().is_empty());
}

#[tokio::test]
async fn test_inject_to_hwnd_window_gone_leaves_text() {
    let clipboard = Arc::new(MockClipboard::new());
    let (ps, _emitter) = build_ps_with_clipboard(clipboard.clone());

    let result = ps
        .delivery()
        .inject_to_hwnd(0, "留底文本", &DEAD_WINDOW_OPS)
        .await;

    assert!(matches!(result, Err(InjectError::WindowGone)));
    // Fallback: text left in the clipboard for manual paste, no paste simulated.
    assert_eq!(clipboard.set_texts(), vec!["留底文本".to_string()]);
    assert!(clipboard.injected().is_empty());
    // New order: window validity is checked before any clipboard op.
    assert!(!clipboard.saved());
}

#[tokio::test]
async fn test_inject_to_hwnd_focus_failed_leaves_text_no_save() {
    let clipboard = Arc::new(MockClipboard::new());
    let (ps, _emitter) = build_ps_with_clipboard(clipboard.clone());
    let ops = FocusOps {
        is_valid: |_| true,
        focus: |_| false,
    };

    let result = ps.delivery().inject_to_hwnd(1, "聚焦失败文本", &ops).await;

    assert!(matches!(result, Err(InjectError::FocusFailed)));
    assert_eq!(clipboard.set_texts(), vec!["聚焦失败文本".to_string()]);
    assert!(clipboard.injected().is_empty());
    // Behavior change vs old do_inject: focus runs before save, so the failed
    // path no longer performs a wasted clipboard save (ADR-0014).
    assert!(!clipboard.saved());
    assert!(!clipboard.restored());
}

#[tokio::test]
async fn test_inject_to_hwnd_inject_failed_restores_clipboard() {
    let clipboard = Arc::new(MockClipboard::new().with_inject_error("paste failed"));
    let (ps, _emitter) = build_ps_with_clipboard(clipboard.clone());
    let ops = FocusOps {
        is_valid: |_| true,
        focus: |_| true,
    };

    let result = ps.delivery().inject_to_hwnd(1, "注入失败文本", &ops).await;

    assert!(matches!(result, Err(InjectError::Clipboard(_))));
    assert!(clipboard.saved());
    assert!(clipboard.injected().is_empty());
    // Paste attempted and failed → saved clipboard content is restored.
    assert!(clipboard.restored());
}

#[tokio::test]
async fn test_inject_to_hwnd_save_failure_no_restore() {
    let clipboard = Arc::new(MockClipboard::new().with_save_error("clipboard busy"));
    let (ps, _emitter) = build_ps_with_clipboard(clipboard.clone());
    let ops = FocusOps {
        is_valid: |_| true,
        focus: |_| true,
    };

    let result = ps.delivery().inject_to_hwnd(1, "保存失败文本", &ops).await;

    assert!(matches!(result, Err(InjectError::Clipboard(_))));
    // Save failed → nothing saved: no paste, no restore (was_saved()=false),
    // and no fallback set_text (the paste was never the issue).
    assert!(!clipboard.saved());
    assert!(clipboard.injected().is_empty());
    assert!(!clipboard.restored());
    assert!(clipboard.set_texts().is_empty());
}

#[tokio::test]
async fn catchall_confirm_from_injecting_resets_clears_context() {
    let (ps, _) = build_ps();
    ps.force_state_tag(StateTag::Injecting);
    let result = ps.delivery().confirm_review(&ps, "text".to_string()).await;
    assert!(result.is_err());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
    let (hwnd, data, _, _) = ps.delivery().take_context();
    assert!(hwnd.is_none());
    assert!(data.is_none());
}

#[tokio::test]
async fn catchall_confirm_from_llm_refining_resets_clears_context() {
    let (ps, _) = build_ps();
    ps.force_state_tag(StateTag::LLMRefining);
    let result = ps.delivery().confirm_review(&ps, "text".to_string()).await;
    assert!(result.is_err());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
    let (hwnd, data, _, _) = ps.delivery().take_context();
    assert!(hwnd.is_none());
    assert!(data.is_none());
}

#[tokio::test]
async fn catchall_cancel_from_injecting_hides_windows_and_clears_context() {
    let (ps, _) = build_ps();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let c = ps.test_components();
    let ps = PipelineState::new(
        c.sm,
        c.ac,
        c.engine,
        c.clipboard,
        c.perf_history,
        c.config_cache,
        c.cached_llm,
        c.realtime_transcriber,
        Arc::new(CallRecordingWindowController {
            calls: calls.clone(),
        }),
        c.emitter,
        c.review,
    );
    ps.force_state_tag(StateTag::Injecting);
    let result = ps.delivery().cancel_review(&ps).await;
    assert!(result.is_err());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
    let (hwnd, data, _, _) = ps.delivery().take_context();
    assert!(hwnd.is_none());
    assert!(data.is_none());
    let recorded = crate::util::lock_mutex(&calls, "rw_controller")
        .map(|c| c.clone())
        .unwrap_or_default();
    assert!(recorded.contains(&"hide_review"));
    assert!(recorded.contains(&"hide_floating"));
}

#[tokio::test]
async fn catchall_cancel_from_llm_refining_hides_windows_and_clears_context() {
    let (ps, _) = build_ps();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let c = ps.test_components();
    let ps = PipelineState::new(
        c.sm,
        c.ac,
        c.engine,
        c.clipboard,
        c.perf_history,
        c.config_cache,
        c.cached_llm,
        c.realtime_transcriber,
        Arc::new(CallRecordingWindowController {
            calls: calls.clone(),
        }),
        c.emitter,
        c.review,
    );
    ps.force_state_tag(StateTag::LLMRefining);
    let result = ps.delivery().cancel_review(&ps).await;
    assert!(result.is_err());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));
    let (hwnd, data, _, _) = ps.delivery().take_context();
    assert!(hwnd.is_none());
    assert!(data.is_none());
    let recorded = crate::util::lock_mutex(&calls, "rw_controller")
        .map(|c| c.clone())
        .unwrap_or_default();
    assert!(recorded.contains(&"hide_review"));
    assert!(recorded.contains(&"hide_floating"));
}

/// was_shown_on_press=true path: show_review stores context with the
/// migrated foreground handle; confirm consumes it end-to-end — context
/// cleared, review UI hidden, state Idle, delivery events emitted, and the
/// shown-on-press flag reset for the next cycle.
#[tokio::test]
async fn was_shown_on_press_confirm_clears_context_and_resets_flag() {
    let (ps, emitter) = build_ps();
    // show_review's entry transition moves Transcribing -> Reviewing; starting
    // from Reviewing would fail the transition and reset to Idle.
    to_transcribing(&ps);
    let perf = crate::perf::PerfMetrics::new(0);
    let policy = build_policy();
    ps.review().set_shown_on_press(true);

    ps.delivery()
        .show_review(
            &ps,
            "text".to_string(),
            "text".to_string(),
            None,
            &policy,
            perf,
            Instant::now(),
            false,
        )
        .await;
    assert_eq!(ps.sm_state(), Some(StateTag::Reviewing));

    let result = ps
        .delivery()
        .confirm_review(&ps, "confirmed".to_string())
        .await;
    assert!(result.is_ok());
    assert_eq!(ps.sm_state(), Some(StateTag::Idle));

    let (hwnd, data, _, _) = ps.delivery().take_context();
    assert!(hwnd.is_none());
    assert!(data.is_none());
    assert!(!ps.review().was_shown_on_press());
    let names = event_names(&emitter);
    assert!(names.contains(&"injection-complete".to_string()));
}
