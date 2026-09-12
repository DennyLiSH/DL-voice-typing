use crate::audio::{AudioCaptureProvider, AudioRingBuffer};
use crate::clipboard::ClipboardProvider;
use crate::commands::EventEmitter;
use crate::commands::TauriEventEmitter;
use crate::commands::delivery_controller::DeliveryController;
use crate::commands::review_provider::{ReviewProvider, TauriReviewProvider};
use crate::commands::window_controller::WindowController;
use crate::commands::window_controller::window_controller_from_app;
use crate::config::ConfigCache;
use crate::llm::TextCorrector;
use crate::perf::PerfHistory;
use crate::realtime::RealtimeTranscriber;
use crate::speech::SpeechEngine;
use crate::state::{StateMachine, StateTag, TransitionError};
use std::sync::{Arc, Mutex};
use tauri::Manager;
use tracing::{debug, info, warn};

/// Aggregated shared state for the hotkey pipeline.
/// Eliminates the need to pass 8 individual `Arc` references to `make_hotkey_callback`.
/// Test-only component snapshot for building variant PipelineStates.
/// Note: delivery / record_only / audio_ring_buffer are NOT here — they are
/// not `PipelineState::new` parameters; delivery is rebuilt by `new()`, the
/// other two are fresh per-instance aggregates, so variant rebuilds simply
/// get new ones (no test currently needs to swap them).
#[cfg(test)]
pub(crate) struct TestComponents {
    pub(crate) sm: Arc<Mutex<StateMachine>>,
    pub(crate) ac: Arc<Mutex<dyn AudioCaptureProvider>>,
    pub(crate) engine: Arc<dyn SpeechEngine>,
    pub(crate) clipboard: Arc<dyn ClipboardProvider>,
    pub(crate) config_cache: ConfigCache,
    pub(crate) perf_history: Arc<crate::perf::PerfHistory>,
    pub(crate) cached_llm: Arc<Mutex<Option<Box<dyn TextCorrector>>>>,
    pub(crate) realtime_transcriber: Arc<Mutex<Option<RealtimeTranscriber>>>,
    pub(crate) window_controller: Arc<dyn WindowController>,
    pub(crate) emitter: Arc<dyn EventEmitter>,
    pub(crate) review: Arc<dyn ReviewProvider>,
}

#[cfg(test)]
impl TestComponents {
    /// Swap the window controller, keeping the rest (variant rebuilds).
    pub(crate) fn with_window_controller(mut self, wc: Arc<dyn WindowController>) -> Self {
        self.window_controller = wc;
        self
    }

    /// Swap the clipboard provider, keeping the rest.
    pub(crate) fn with_clipboard(mut self, cb: Arc<dyn ClipboardProvider>) -> Self {
        self.clipboard = cb;
        self
    }

    /// Assemble the variant PipelineState. The ONLY `PipelineState::new` call
    /// site on the variant-rebuild path — adding/removing/reordering `new()`
    /// parameters now requires updating exactly one place instead of 7.
    pub(crate) fn build(self) -> PipelineState {
        PipelineState::new(
            self.sm,
            self.ac,
            self.engine,
            self.clipboard,
            self.perf_history,
            self.config_cache,
            self.cached_llm,
            self.realtime_transcriber,
            self.window_controller,
            self.emitter,
            self.review,
        )
    }
}

#[derive(Clone)]
pub struct PipelineState {
    sm: Arc<Mutex<StateMachine>>,
    #[cfg(test)]
    forced_sm_state: Arc<Mutex<Option<StateTag>>>,
    ac: Arc<Mutex<dyn AudioCaptureProvider>>,
    engine: Arc<dyn SpeechEngine>,
    clipboard: Arc<dyn ClipboardProvider>,
    perf_history: Arc<PerfHistory>,
    config_cache: ConfigCache,
    cached_llm: Arc<Mutex<Option<Box<dyn TextCorrector>>>>,
    realtime_transcriber: Arc<Mutex<Option<RealtimeTranscriber>>>,
    window_controller: Arc<dyn WindowController>,
    emitter: Arc<dyn EventEmitter>,
    review: Arc<dyn ReviewProvider>,
    /// Single authority for post-transcription delivery lifecycle.
    delivery: Arc<DeliveryController>,
    /// Decoupled audio buffer for lock-free realtime reads.
    /// Capacity: 60 seconds @ 48kHz = 2,880,000 samples.
    audio_ring_buffer: Arc<Mutex<AudioRingBuffer>>,
    /// Active record-only session (Some only while a record-only session is
    /// capturing). Private; access goes through set/take_record_only_session.
    record_only: Arc<Mutex<Option<crate::commands::record_only_session::ActiveRecordOnly>>>,
    /// Cancellation token of the in-flight classic/fast-path delivery (Some
    /// between `sm_stop_recording` and delivery completion). Set by the
    /// release path, flipped by the Esc-cancel hook slot, consumed at every
    /// pipeline exit. Private; access goes through set/take/snapshot.
    cancel_token: Arc<Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>>,
}

impl PipelineState {
    /// Direct constructor for testing. Each component is injectable.
    /// Pre-allocated audio buffer capacity: 60 seconds at 48 kHz.
    const AUDIO_BUFFER_CAPACITY: usize = 48_000 * 60;

    /// Direct constructor for testing. Each component is injectable.
    #[allow(dead_code, clippy::too_many_arguments)]
    pub(crate) fn new(
        sm: Arc<Mutex<StateMachine>>,
        ac: Arc<Mutex<dyn AudioCaptureProvider>>,
        engine: Arc<dyn SpeechEngine>,
        clipboard: Arc<dyn ClipboardProvider>,
        perf_history: Arc<PerfHistory>,
        config_cache: ConfigCache,
        cached_llm: Arc<Mutex<Option<Box<dyn TextCorrector>>>>,
        realtime_transcriber: Arc<Mutex<Option<RealtimeTranscriber>>>,
        window_controller: Arc<dyn WindowController>,
        emitter: Arc<dyn EventEmitter>,
        review: Arc<dyn ReviewProvider>,
    ) -> Self {
        let delivery = Arc::new(DeliveryController::new(
            emitter.clone(),
            window_controller.clone(),
            clipboard.clone(),
            perf_history.clone(),
        ));
        Self {
            sm,
            #[cfg(test)]
            forced_sm_state: Arc::new(Mutex::new(None)),
            ac,
            engine,
            clipboard,
            perf_history,
            config_cache,
            cached_llm,
            realtime_transcriber,
            window_controller,
            emitter,
            review,
            delivery,
            audio_ring_buffer: Arc::new(Mutex::new(AudioRingBuffer::new(
                Self::AUDIO_BUFFER_CAPACITY,
            ))),
            record_only: Arc::new(Mutex::new(None)),
            cancel_token: Arc::new(Mutex::new(None)),
        }
    }

    /// Extract all pipeline state from Tauri's managed state.
    pub fn from_app(app: &tauri::AppHandle) -> Self {
        let window_controller = window_controller_from_app(app);
        let emitter: Arc<dyn EventEmitter> = Arc::new(TauriEventEmitter::new(app.clone()));
        // Wrap the emitter with the error-history recorder when available.
        // Assembled HERE (not in lib.rs) so config_cmd's rebuild path gets
        // the decorator too — it creates its own TauriEventEmitter via this
        // same function. The miss fallback (with warn) is a second line of
        // defense; lib.rs manages ErrorHistory before the first from_app.
        let emitter: Arc<dyn EventEmitter> =
            match app.try_state::<Arc<crate::commands::error_history::ErrorHistory>>() {
                Some(history) => Arc::new(crate::commands::error_history::RecordingEmitter::new(
                    emitter,
                    history.inner().clone(),
                )),
                None => {
                    tracing::warn!(
                        target: "error_history",
                        "ErrorHistory not managed; emitter unrecorded"
                    );
                    emitter
                }
            };
        let review: Arc<dyn ReviewProvider> = Arc::new(TauriReviewProvider::new(app.clone()));
        let clipboard = app.state::<Arc<dyn ClipboardProvider>>().inner().clone();
        let perf_history = app.state::<Arc<PerfHistory>>().inner().clone();
        let delivery = Arc::new(DeliveryController::new(
            emitter.clone(),
            window_controller.clone(),
            clipboard.clone(),
            perf_history.clone(),
        ));
        Self {
            sm: app.state::<Arc<Mutex<StateMachine>>>().inner().clone(),
            #[cfg(test)]
            forced_sm_state: Arc::new(Mutex::new(None)),
            ac: app
                .state::<Arc<Mutex<dyn AudioCaptureProvider>>>()
                .inner()
                .clone(),
            engine: app.state::<Arc<dyn SpeechEngine>>().inner().clone(),
            clipboard,
            perf_history,
            config_cache: app.state::<ConfigCache>().inner().clone(),
            cached_llm: app
                .state::<Arc<Mutex<Option<Box<dyn TextCorrector>>>>>()
                .inner()
                .clone(),
            realtime_transcriber: app
                .state::<Arc<Mutex<Option<RealtimeTranscriber>>>>()
                .inner()
                .clone(),
            window_controller,
            emitter,
            review,
            delivery,
            audio_ring_buffer: Arc::new(Mutex::new(AudioRingBuffer::new(
                Self::AUDIO_BUFFER_CAPACITY,
            ))),
            record_only: Arc::new(Mutex::new(None)),
            cancel_token: Arc::new(Mutex::new(None)),
        }
    }

    /// Stop audio capture and realtime transcriber (non-blocking).
    /// Returns accumulated realtime text if available.
    /// Must be called from non-blocking contexts (e.g., Windows hook thread)
    /// — uses `stop()` (detach) not `stop_and_wait()`.
    pub fn stop_recording_resources(&self) -> Option<String> {
        if let Some(mut ac_guard) = crate::util::lock_mutex(&self.ac, "audio_capture") {
            ac_guard.stop();
        }

        if let Some(mut rt_guard) =
            crate::util::lock_mutex(&self.realtime_transcriber, "realtime_transcriber")
        {
            if let Some(ref mut rt) = *rt_guard {
                rt.stop();
                let text = rt.take_accumulated();
                rt_guard.take();
                info!(
                    "stop_recording_resources: realtime accumulated={} chars",
                    text.len()
                );
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        None
    }

    /// Stop audio capture and realtime transcriber (blocking, graceful).
    /// For use in async contexts where blocking is acceptable (review commands).
    /// Uses `stop_and_wait()` for clean thread shutdown.
    pub fn stop_recording_resources_graceful(&self) {
        if let Some(mut ac_guard) = crate::util::lock_mutex(&self.ac, "audio_capture") {
            ac_guard.stop();
        }

        if let Some(mut rt_guard) =
            crate::util::lock_mutex(&self.realtime_transcriber, "realtime_transcriber")
        {
            if let Some(mut rt) = rt_guard.take() {
                info!("stop_recording_resources_graceful: stopping realtime thread");
                rt.stop_and_wait();
            }
        }
    }

    // ========================================================================
    // Component accessors
    //
    // Fields are private to PipelineState (aggregation per ADR-0004);
    // collaborators reach components through these methods only.
    // ========================================================================

    pub(crate) fn emitter(&self) -> Arc<dyn EventEmitter> {
        self.emitter.clone()
    }

    pub(crate) fn window_controller(&self) -> Arc<dyn WindowController> {
        self.window_controller.clone()
    }

    pub(crate) fn review(&self) -> Arc<dyn ReviewProvider> {
        self.review.clone()
    }

    pub(crate) fn engine(&self) -> Arc<dyn SpeechEngine> {
        self.engine.clone()
    }

    /// Run `f` against the cached-LLM slot under its lock. Lock poisoning
    /// surfaces as None — callers translate that to their existing
    /// "lock poisoned" errors. This is the ONLY way across this seam now:
    /// the lock protocol (util::lock_mutex + slot name) lives with the
    /// slot's owner instead of in 3 callers' heads. Lock is held for the
    /// duration of `f`; HTTP calls (correct_sync via block_in_place) happen
    /// inside the closure, matching the prior open-coded pattern.
    pub(crate) fn with_cached_llm<R>(
        &self,
        f: impl FnOnce(&mut Option<Box<dyn TextCorrector>>) -> R,
    ) -> Option<R> {
        crate::util::lock_mutex(&self.cached_llm, "cached_llm").map(|mut guard| f(&mut guard))
    }

    pub(crate) fn audio_capture(&self) -> Arc<Mutex<dyn AudioCaptureProvider>> {
        self.ac.clone()
    }

    pub(crate) fn config_cache(&self) -> ConfigCache {
        self.config_cache.clone()
    }

    pub(crate) fn delivery(&self) -> Arc<DeliveryController> {
        self.delivery.clone()
    }

    /// Clear the ring buffer (new recording session).
    pub(crate) fn clear_ring(&self) {
        if let Some(mut buf) = crate::util::lock_mutex(&self.audio_ring_buffer, "audio_ring_buffer")
        {
            buf.clear();
        }
    }

    /// Take all accumulated audio samples from the ring buffer.
    pub(crate) fn take_ring_samples(&self) -> Vec<f32> {
        crate::util::lock_mutex(&self.audio_ring_buffer, "audio_ring_buffer")
            .map(|mut buf| buf.take_all())
            .unwrap_or_default()
    }

    /// Next perf cycle id (monotonic). Replaces the perf_history()
    /// accessor — callers only ever used this one operation.
    pub(crate) fn next_perf_cycle_id(&self) -> u64 {
        self.perf_history.next_cycle_id()
    }

    /// Shared ring-buffer handle. **Only** for the two call sites that
    /// genuinely need the shared Arc (cpal callback lock+push, and the
    /// `AudioRingBufferSource` adapter construction) — same shared-handle
    /// shape as C2's PushHandle (ADR-0015). Pure operations go through
    /// `clear_ring` / `take_ring_samples` instead.
    pub(crate) fn ring_buffer(&self) -> Arc<Mutex<AudioRingBuffer>> {
        self.audio_ring_buffer.clone()
    }

    // ========================================================================
    // Record-only resource management
    // ========================================================================

    /// Install the active record-only session. Must only be called after
    /// `sm_start_record_only()` succeeded (transition is the atomic gate).
    pub(crate) fn set_record_only_session(
        &self,
        session: crate::commands::record_only_session::ActiveRecordOnly,
    ) {
        if let Some(mut guard) = crate::util::lock_mutex(&self.record_only, "record_only") {
            *guard = Some(session);
        }
    }

    /// Take the active record-only session (release / reset paths).
    pub(crate) fn take_record_only_session(
        &self,
    ) -> Option<crate::commands::record_only_session::ActiveRecordOnly> {
        crate::util::lock_mutex(&self.record_only, "record_only").and_then(|mut g| g.take())
    }

    // ========================================================================
    // Cancellation-token slot (Esc cancel)
    //
    // Lifecycle: installed by the release path right after `sm_stop_recording`
    // succeeds, flipped to `true` by `cancel_active_pipeline` (hook thread),
    // consumed (`take`) at every pipeline exit so a stale token can never leak
    // into the next session.
    // ========================================================================

    /// Install the cancellation token for the in-flight delivery.
    pub(crate) fn set_cancel_token(&self, token: Arc<std::sync::atomic::AtomicBool>) {
        if let Some(mut guard) = crate::util::lock_mutex(&self.cancel_token, "cancel_token") {
            *guard = Some(token);
        }
    }

    /// Take the cancellation token out of the slot (take-once).
    pub(crate) fn take_cancel_token(&self) -> Option<Arc<std::sync::atomic::AtomicBool>> {
        crate::util::lock_mutex(&self.cancel_token, "cancel_token").and_then(|mut g| g.take())
    }

    /// Read-only handle on the current token without consuming the slot.
    /// An empty slot yields a fresh, un-cancelled token (defensive fallback)
    /// so pipeline code never has to branch on `Option`.
    pub(crate) fn cancel_token_snapshot(&self) -> Arc<std::sync::atomic::AtomicBool> {
        crate::util::lock_mutex(&self.cancel_token, "cancel_token")
            .and_then(|g| g.clone())
            .unwrap_or_else(|| Arc::new(std::sync::atomic::AtomicBool::new(false)))
    }

    // ========================================================================
    // Realtime-transcriber slot
    //
    // Lifecycle: installed at session start (`set_realtime_transcriber`,
    // on_press realtime branch), stopped and taken at release via
    // `stop_recording_resources` (which also takes the accumulated text),
    // or via `stop_recording_resources_graceful` on review-cancel/error
    // exits (blocking stop, no text); `stop_realtime_leftover`
    // at the next session start is the defensive sweep for a previous
    // session that ended abnormally.
    // ========================================================================

    /// Install the realtime transcriber (set semantics, mirrors the
    /// record_only / cancel_token slot verbs).
    pub(crate) fn set_realtime_transcriber(&self, rt: crate::realtime::RealtimeTranscriber) {
        if let Some(mut guard) =
            crate::util::lock_mutex(&self.realtime_transcriber, "realtime_transcriber")
        {
            *guard = Some(rt);
        }
    }

    /// Session-start guard: stop and drop any leftover realtime
    /// transcriber from an abnormally-ended previous session (e.g. hook
    /// release event not delivered by Windows).
    pub(crate) fn stop_realtime_leftover(&self) {
        if let Some(mut rt_guard) =
            crate::util::lock_mutex(&self.realtime_transcriber, "realtime_transcriber")
        {
            if let Some(ref mut rt) = *rt_guard {
                rt.stop();
                rt_guard.take();
            }
        }
    }

    // ========================================================================
    // Composite verbs (session teardown & Esc-cancel)
    //
    // The ordering knowledge — "reset must hide overlays, review cleanup is
    // conditional on shown_on_press" — lives HERE, with the state owner,
    // instead of in every caller's orchestration (architecture review
    // 2026-09-06 candidate 1).
    // ========================================================================

    /// Hide the floating window and, if the review window was shown on
    /// press, hide it and clear the flag. The review half is conditional:
    /// `shown_on_press` is only set for RealtimeReview sessions.
    pub(crate) fn hide_overlays(&self) {
        self.window_controller.hide_floating();
        if self.review.was_shown_on_press() {
            self.window_controller.hide_review();
            self.review.set_shown_on_press(false);
        }
    }

    /// Reset the state machine to Idle and hide all overlays. The
    /// cancel-token slot is NOT drained here — the `CancelGuard` in the
    /// delivery future owns that (see its doc comment); callers outside
    /// any guard are safe by construction (the slot is empty before
    /// `sm_stop_recording` succeeds).
    pub(crate) fn reset_to_idle(&self) {
        self.sm_reset();
        self.hide_overlays();
    }

    /// Esc-cancel entrypoint: atomically check-and-transition from
    /// Transcribing/LLMRefining to Idle, flip the in-flight cancel token,
    /// hide the overlays, and emit `pipeline-cancelled`.
    /// Returns `false` when not in a cancellable phase — the caller then
    /// passes the Esc keypress through to the focused app.
    ///
    /// Atomicity: the state-machine check-and-transition is done under a
    /// single lock acquisition (`sm_cancel_transcribing_or_llm`), eliminating
    /// the read-then-transition TOCTOU window where the pipeline could
    /// advance to `Injecting` between the guard check and the reset.
    pub(crate) fn cancel_active_pipeline(&self) -> bool {
        if !self.sm_cancel_transcribing_or_llm() {
            // Not in a cancellable phase (Idle / Recording / Injecting /
            // Reviewing / RecordOnly). Pass the Esc through so the focused app
            // receives it — including the case where the pipeline raced ahead
            // and finished delivery between the user's intent and our check.
            return false;
        }
        // Flip the token so the in-flight Whisper call aborts. If the slot
        // is empty (no active pipeline body yet — possible if Esc lands
        // during the microsecond gap between sm_stop_recording and the
        // future spawn), there's nothing to abort — the state-reset above
        // already cancelled.
        if let Some(token) = self.take_cancel_token() {
            token.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.hide_overlays();
        self.emitter
            .emit("pipeline-cancelled", serde_json::Value::Null);
        info!(target: "cancel", "pipeline cancelled via Esc");
        true
    }

    /// Restore the clipboard only if it was saved earlier this cycle.
    /// Returns whether a restore actually ran. Recover-path use only —
    /// normal delivery self-restores inside the `save_and_inject` unit.
    /// Namesake: DeliveryController has a private method of the same name
    /// (returns (), warns on failure with target "delivery") — this one is
    /// the recover-path gate and stays silent on restore failure, matching
    /// the pre-existing recover semantics.
    pub(crate) fn restore_clipboard_if_saved(&self) -> bool {
        if self.clipboard.was_saved() {
            self.clipboard.restore().is_ok()
        } else {
            false
        }
    }

    // ========================================================================
    // State-machine verb layer
    //
    // Each verb encapsulates lock_mutex of sm + the StateMachine method + warn
    // on failure, so callers no longer need to reach past PipelineState's public
    // API to lock the state machine. Verbs return bool (true = transition
    // succeeded); lock-poisoned and transition-rejected both collapse to false
    // with a warn trail. Constraint: verb bodies hold no await, callbacks, or
    // further lock acquisitions — they only call the matching StateMachine
    // method under the sm lock (re-entrancy safe; panic=abort compatible since
    // verb bodies never panic on lock failure — they match and return false).
    // ========================================================================

    /// Shared template for the sm_* transition verbs: lock, run the
    /// transition, warn on rejection. Lock-poisoned and transition-rejected
    /// both collapse to `false` with a warn trail.
    fn sm_call(
        &self,
        name: &'static str,
        transition: impl FnOnce(&mut StateMachine) -> Result<(), TransitionError>,
    ) -> bool {
        let Some(mut s) = crate::util::lock_mutex(&self.sm, "state_machine") else {
            return false;
        };
        match transition(&mut s) {
            Ok(()) => true,
            Err(e) => {
                warn!("{name} failed: {e}");
                false
            }
        }
    }

    /// Current state-machine tag, or None if the lock is poisoned.
    /// Query surface for production state guards (DeliveryController
    /// confirm/cancel, record-only release/recover/monitor) and the
    /// test audit view of the verb layer.
    pub(crate) fn sm_state(&self) -> Option<StateTag> {
        #[cfg(test)]
        {
            if let Some(guard) = crate::util::lock_mutex(&self.forced_sm_state, "forced_sm_state") {
                if let Some(tag) = *guard {
                    return Some(tag);
                }
            }
        }
        crate::util::lock_mutex(&self.sm, "state_machine").map(|s| s.state())
    }

    pub(crate) fn sm_start_recording(&self) -> bool {
        self.sm_call("sm_start_recording", StateMachine::start_recording)
    }

    pub(crate) fn sm_stop_recording(&self) -> bool {
        self.sm_call("sm_stop_recording", StateMachine::stop_recording)
    }

    pub(crate) fn sm_start_llm_refining(&self) -> bool {
        self.sm_call("sm_start_llm_refining", StateMachine::start_llm_refining)
    }

    pub(crate) fn sm_transcribing_to_injecting(&self) -> bool {
        self.sm_call(
            "sm_transcribing_to_injecting",
            StateMachine::transcribing_to_injecting,
        )
    }

    pub(crate) fn sm_llm_to_injecting(&self) -> bool {
        self.sm_call("sm_llm_to_injecting", StateMachine::llm_to_injecting)
    }

    pub(crate) fn sm_transcribing_to_reviewing(&self) -> bool {
        self.sm_call(
            "sm_transcribing_to_reviewing",
            StateMachine::transcribing_to_reviewing,
        )
    }

    pub(crate) fn sm_llm_to_reviewing(&self) -> bool {
        self.sm_call("sm_llm_to_reviewing", StateMachine::llm_to_reviewing)
    }

    /// Review → Injecting transition. Called by DeliveryController::confirm_from_reviewing.
    pub(crate) fn sm_reviewing_to_injecting(&self) -> bool {
        self.sm_call(
            "sm_reviewing_to_injecting",
            StateMachine::reviewing_to_injecting,
        )
    }

    /// Review → Idle (cancel) transition. Called by DeliveryController::cancel_review.
    pub(crate) fn sm_cancel_reviewing(&self) -> bool {
        self.sm_call("sm_cancel_reviewing", StateMachine::cancel_reviewing)
    }

    pub(crate) fn sm_finish_injecting(&self) -> bool {
        self.sm_call("sm_finish_injecting", StateMachine::finish_injecting)
    }

    /// Esc-cancel entrypoint verb: atomically check-and-transition from
    /// Transcribing/LLMRefining to Idle. Returns true iff the state was
    /// actually cancellable (caller may then flip the cancel token, hide
    /// the floating window, emit `pipeline-cancelled`).
    pub(crate) fn sm_cancel_transcribing_or_llm(&self) -> bool {
        let Some(mut s) = crate::util::lock_mutex(&self.sm, "state_machine") else {
            return false;
        };
        match s.cancel_transcribing_or_llm() {
            Ok(()) => true,
            Err(e) => {
                debug!("sm_cancel_transcribing_or_llm declined: {e}");
                false
            }
        }
    }

    pub(crate) fn sm_start_record_only(&self) -> bool {
        self.sm_call("sm_start_record_only", StateMachine::start_record_only)
    }

    pub(crate) fn sm_finish_record_only(&self) -> bool {
        self.sm_call("sm_finish_record_only", StateMachine::finish_record_only)
    }

    /// Reset to Idle unconditionally (infallible — `reset()` itself cannot fail).
    /// Lock-poisoned case is silently dropped (already warn!'d by `lock_mutex`).
    pub(crate) fn sm_reset(&self) {
        if let Some(mut s) = crate::util::lock_mutex(&self.sm, "state_machine") {
            s.reset();
        }
    }
}

/// RAII owner of pipeline-exit token hygiene (deep-module move: the
/// "every exit path must drain" invariant leaves caller discipline and
/// becomes type-enforced). Create at the top of a delivery future body;
/// `Drop` drains the slot so ANY exit — return, early gate, panic
/// unwind — clears the token. Replaces the hand-placed
/// `let _ = ps.take_cancel_token()` drains.
///
/// Invariant (7f0cbf0): this guard NEVER touches the state machine or
/// windows — that cleanup belongs solely to `cancel_active_pipeline`
/// on the hook thread. Draining the slot is slot hygiene, not cleanup.
pub(crate) struct CancelGuard<'a> {
    ps: &'a PipelineState,
}

impl<'a> CancelGuard<'a> {
    pub(crate) fn new(ps: &'a PipelineState) -> Self {
        Self { ps }
    }
}

impl Drop for CancelGuard<'_> {
    fn drop(&mut self) {
        let _ = self.ps.take_cancel_token();
    }
}

#[cfg(test)]
impl PipelineState {
    /// Test-only: snapshot of all components for building variant
    /// PipelineStates (swap one component, keep the rest).
    pub(crate) fn test_components(&self) -> TestComponents {
        TestComponents {
            sm: self.sm.clone(),
            ac: self.ac.clone(),
            engine: self.engine.clone(),
            clipboard: self.clipboard.clone(),
            perf_history: self.perf_history.clone(),
            config_cache: self.config_cache.clone(),
            cached_llm: self.cached_llm.clone(),
            realtime_transcriber: self.realtime_transcriber.clone(),
            window_controller: self.window_controller.clone(),
            emitter: self.emitter.clone(),
            review: self.review.clone(),
        }
    }

    /// Test-only: force the real state-machine tag.
    pub(crate) fn force_state_tag(&self, tag: StateTag) {
        if let Some(mut guard) = crate::util::lock_mutex(&self.sm, "state_machine") {
            guard.force_state_tag(tag);
        }
    }

    /// Test-only: override the value returned by `sm_state()` without changing
    /// the real state machine. Used to deterministically simulate TOCTOU races.
    pub(crate) fn force_sm_state(&self, tag: StateTag) {
        if let Some(mut guard) = crate::util::lock_mutex(&self.forced_sm_state, "forced_sm_state") {
            *guard = Some(tag);
        }
    }

    /// Test-only: clear a previously set `force_sm_state` override. Call this
    /// in afterEach / drop guards to prevent state leakage between tests that
    /// share a PipelineState. Failing to clear turns the override into a
    /// silent test-time bomb — later tests' sm_state() reads return the stale
    /// forced value instead of the real tag.
    pub(crate) fn clear_forced_sm_state(&self) {
        if let Some(mut guard) = crate::util::lock_mutex(&self.forced_sm_state, "forced_sm_state") {
            *guard = None;
        }
    }

    /// Test-only: dropped-block counter of the active record-only session.
    pub(crate) fn test_record_only_dropped_counter(
        &self,
    ) -> Option<std::sync::Arc<std::sync::atomic::AtomicU64>> {
        crate::util::lock_mutex(&self.record_only, "record_only")
            .and_then(|g| g.as_ref().map(|s| s.dropped_counter()))
    }
}

#[cfg(test)]
mod sm_verb_tests {
    use super::*;
    use crate::audio::MockAudioCapture;
    use crate::clipboard::MockClipboard;
    use crate::commands::MockEmitter;
    use crate::commands::review_provider::MockReviewProvider;
    use crate::commands::window_controller::NoopWindowController;
    use crate::config::{AppConfig, ConfigCache};
    use crate::llm::MockCorrector;
    use crate::perf::PerfHistory;
    use crate::speech::mock::MockEngine;
    use crate::state::StateTag;

    /// Minimal PipelineState for state-machine verb tests.
    /// All collaborators are mocks — only sm behaviour is exercised.
    fn build_test_ps() -> PipelineState {
        let emitter: Arc<dyn EventEmitter> = Arc::new(MockEmitter::new());
        PipelineState::new(
            Arc::new(Mutex::new(StateMachine::new())),
            Arc::new(Mutex::new(MockAudioCapture::new())),
            Arc::new(MockEngine::new("test")),
            Arc::new(MockClipboard::new()),
            Arc::new(PerfHistory::new()),
            ConfigCache::new(AppConfig::default()),
            Arc::new(Mutex::new(Some(Box::new(MockCorrector::new("corrected"))))),
            Arc::new(Mutex::new(None)),
            Arc::new(NoopWindowController),
            emitter,
            Arc::new(MockReviewProvider::new()),
        )
    }

    /// Test-only WindowController recording method calls by name.
    struct LogWindowController {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    impl crate::commands::window_controller::WindowController for LogWindowController {
        fn show_floating_near_caret(&self) -> bool {
            false
        }
        fn hide_floating(&self) {
            self.calls.lock().unwrap().push("hide_floating");
        }
        fn show_review_near_caret(&self) -> bool {
            false
        }
        fn hide_review(&self) {
            self.calls.lock().unwrap().push("hide_review");
        }
        fn focus_review(&self) -> bool {
            false
        }
        fn eval_review_js(&self, _js: &str) -> bool {
            false
        }
        fn emit_review_show(&self) {}
        fn emit_review_final_text(&self, _text: &str) {}
        fn restore_foreground_hwnd(&self, _hwnd: isize) {}
        fn show_floating_corner(&self) -> bool {
            false
        }
        fn set_tray_tooltip(&self, _tooltip: &str) {}
    }

    #[test]
    fn test_sm_verbs_cover_all_transitions() {
        let ps = build_test_ps();
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));

        // Path 1: Idle to Recording to Transcribing to Injecting to Idle (no LLM, no review).
        assert!(ps.sm_start_recording());
        assert_eq!(ps.sm_state(), Some(StateTag::Recording));
        assert!(ps.sm_stop_recording());
        assert_eq!(ps.sm_state(), Some(StateTag::Transcribing));
        assert!(ps.sm_transcribing_to_injecting());
        assert_eq!(ps.sm_state(), Some(StateTag::Injecting));
        assert!(ps.sm_finish_injecting());
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));

        // Path 2: Idle through LLMRefining to Injecting to Idle.
        ps.sm_start_recording();
        ps.sm_stop_recording();
        assert!(ps.sm_start_llm_refining());
        assert_eq!(ps.sm_state(), Some(StateTag::LLMRefining));
        assert!(ps.sm_llm_to_injecting());
        assert!(ps.sm_finish_injecting());

        // Path 3: review path (transcribing then reviewing then injecting).
        ps.sm_start_recording();
        ps.sm_stop_recording();
        assert!(ps.sm_transcribing_to_reviewing());
        assert_eq!(ps.sm_state(), Some(StateTag::Reviewing));
        assert!(ps.sm_reviewing_to_injecting());
        assert!(ps.sm_finish_injecting());

        // Path 4: LLM then reviewing then injecting.
        ps.sm_start_recording();
        ps.sm_stop_recording();
        ps.sm_start_llm_refining();
        assert!(ps.sm_llm_to_reviewing());
        assert!(ps.sm_reviewing_to_injecting());
        assert!(ps.sm_finish_injecting());

        // Path 5: cancel review.
        ps.sm_start_recording();
        ps.sm_stop_recording();
        ps.sm_transcribing_to_reviewing();
        assert!(ps.sm_cancel_reviewing());
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));

        // Path 6: reset from any state back to Idle.
        ps.sm_start_recording();
        ps.sm_reset();
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));

        // Path 7: illegal transition returns false (no panic, no state change).
        assert!(!ps.sm_stop_recording()); // Idle cannot go to Transcribing
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));
    }

    #[test]
    fn test_sm_verbs_control_flow_equivalence() {
        // Regression for can_record/stop_ok control flow: pre-migration code used
        // `lock_mutex(...).map(|s| s.start_recording().is_ok()).unwrap_or(false)`.
        // Post-migration `ps.sm_xxx()` must return the same bool for both the
        // legal-transition and lock-poisoned cases. This test exercises happy path.
        let ps = build_test_ps();
        let can_record = ps.sm_start_recording();
        assert!(can_record);
        let stop_ok = ps.sm_stop_recording();
        assert!(stop_ok);
    }

    #[test]
    fn test_sm_record_only_verbs() {
        let ps = build_test_ps();
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));

        // Happy path: Idle → RecordOnly → Idle.
        assert!(ps.sm_start_record_only());
        assert_eq!(ps.sm_state(), Some(StateTag::RecordOnly));
        assert!(ps.sm_finish_record_only());
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));

        // Illegal: finish from Idle; start while Recording.
        assert!(!ps.sm_finish_record_only());
        assert!(ps.sm_start_recording());
        assert!(!ps.sm_start_record_only());
        ps.sm_reset();
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));
    }

    #[test]
    fn cancel_token_slot_set_take_roundtrip() {
        use std::sync::atomic::AtomicBool;
        let ps = build_test_ps();
        assert!(ps.take_cancel_token().is_none(), "slot starts empty");

        let token = Arc::new(AtomicBool::new(false));
        ps.set_cancel_token(token.clone());
        let taken = ps.take_cancel_token();
        assert!(taken.is_some());
        let Some(taken) = taken else { return };
        assert!(Arc::ptr_eq(&taken, &token), "same token comes back out");
        assert!(ps.take_cancel_token().is_none(), "take-once semantics");
    }

    #[test]
    fn cancel_guard_drop_drains_token_slot() {
        use std::sync::atomic::AtomicBool;
        let ps = build_test_ps();
        ps.set_cancel_token(Arc::new(AtomicBool::new(false)));
        {
            let _guard = CancelGuard::new(&ps);
            // Slot still holds the token while the guard is alive.
            assert!(
                !ps.cancel_token_snapshot()
                    .load(std::sync::atomic::Ordering::Relaxed)
            );
        }
        // Drop ran — slot drained without any hand-placed take.
        assert!(
            ps.take_cancel_token().is_none(),
            "guard drop must drain the slot"
        );
    }

    #[test]
    fn cancel_guard_drop_drains_token_slot_on_panic_unwind() {
        use std::sync::atomic::AtomicBool;
        // Central guarantee of the RAII guard: the slot must be drained on
        // ANY exit path — normal scope exit, early return, AND panic unwind.
        // Without this, a panic inside a delivery future would leak a stale
        // cancel token that the next session's `set_cancel_token` does NOT
        // overwrite until recording starts (sm_stop_recording returns),
        // giving a stale token a window where it could cancel unrelated work.
        let ps = build_test_ps();
        ps.set_cancel_token(Arc::new(AtomicBool::new(false)));

        // AssertUnwindSafe: CancelGuard holds `&PipelineState`, and the
        // closure's panic would otherwise be rejected as carrying a
        // !UnwindSafe borrow. The raw pointer dance is the standard idiom.
        let ps_for_panic: *const PipelineState = &ps;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = CancelGuard::new(unsafe { &*ps_for_panic });
            panic!("test panic — guard drop must still drain the slot");
        }));
        assert!(
            result.is_err(),
            "panic must have propagated out of catch_unwind"
        );
        // The guard's Drop ran during stack unwinding — slot must be drained.
        assert!(
            ps.take_cancel_token().is_none(),
            "guard drop must drain on panic unwind"
        );
    }

    #[test]
    fn cancel_token_snapshot_is_fresh_false_when_slot_empty() {
        use std::sync::atomic::Ordering;
        let ps = build_test_ps();
        // Defensive fallback: an empty slot yields a fresh, un-cancelled token
        // so callers never have to branch on Option.
        let snap = ps.cancel_token_snapshot();
        assert!(!snap.load(Ordering::Relaxed));
        // With a token installed, the snapshot aliases it.
        let token = Arc::new(std::sync::atomic::AtomicBool::new(false));
        ps.set_cancel_token(token.clone());
        let snap = ps.cancel_token_snapshot();
        assert!(Arc::ptr_eq(&snap, &token));
        token.store(true, Ordering::SeqCst);
        assert!(snap.load(Ordering::Relaxed));
        // Snapshot does NOT consume the slot.
        assert!(ps.take_cancel_token().is_some());
    }

    #[test]
    fn test_record_only_session_slot_roundtrip() {
        let ps = build_test_ps();
        // No session installed → take returns None.
        assert!(ps.take_record_only_session().is_none());

        let dir = std::env::temp_dir().join("dl-vt-ps-record-only");
        let _ = std::fs::remove_dir_all(&dir);
        let rec = crate::streaming_recorder::StreamingRecorder::start(
            &dir,
            crate::audio::TARGET_SAMPLE_RATE,
        );
        assert!(rec.is_ok());
        let Some(rec) = rec.ok() else { return };
        let session = crate::commands::record_only_session::ActiveRecordOnly::new_for_test(
            rec,
            crate::commands::record_only_session::RecordOnlyPolicy {
                data_saving_path: dir.to_string_lossy().to_string(),
                language: crate::config::Language::Zh,
                whisper_model: crate::config::WhisperModel::default(),
            },
        );
        ps.set_record_only_session(session);
        let taken = ps.take_record_only_session();
        assert!(taken.is_some());
        // Finalize so the writer thread exits cleanly.
        if let Some(s) = taken {
            assert!(s.stop_and_wait(std::time::Duration::from_secs(5)).is_ok());
        }
        // Slot is empty after take.
        assert!(ps.take_record_only_session().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_cached_llm_runs_closure_under_slot_lock() {
        // Verb contract: closure runs with &mut Option<…>; the returned value
        // is funneled back to the caller via Some(...). Lock-poisoned case
        // collapses to None (callers translate to their own error variant).
        let ps = build_test_ps();
        // build_test_ps seeds the slot with Some(MockCorrector("corrected"))
        // — the predicate reports Some(true) because is_none() is false.
        assert_eq!(ps.with_cached_llm(|c| c.is_none()), Some(false));

        // Seeding via the verb works the same way as direct lock mutation.
        let ps_empty = PipelineState::new(
            Arc::new(Mutex::new(StateMachine::new())),
            Arc::new(Mutex::new(MockAudioCapture::new())),
            Arc::new(MockEngine::new("test")),
            Arc::new(MockClipboard::new()),
            Arc::new(PerfHistory::new()),
            ConfigCache::new(AppConfig::default()),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
            Arc::new(NoopWindowController),
            Arc::new(MockEmitter::new()),
            Arc::new(MockReviewProvider::new()),
        );
        assert_eq!(
            ps_empty.with_cached_llm(|c| c.is_none()),
            Some(true),
            "fresh slot is None"
        );
        // The verb can also mutate the slot (seed for downstream tests).
        ps_empty.with_cached_llm(|c| {
            *c = Some(Box::new(MockCorrector::new("seeded")));
        });
        assert_eq!(
            ps_empty.with_cached_llm(|c| c.is_some()),
            Some(true),
            "mutation visible on next verb call"
        );
    }

    #[test]
    fn public_surface_ratchet() {
        // Interface ratchet: counts the PipelineState pub-fn surface.
        // Round 4 (2026-09-12): deliberate rebalance per architecture
        // review 2026-09-06 candidate 1 — removed 3 object-graph accessors
        // (clipboard / perf_history / realtime_transcriber, all call sites
        // absorbed), added 7 composed verbs carrying ordering knowledge
        // (hide_overlays, reset_to_idle, cancel_active_pipeline,
        // restore_clipboard_if_saved, next_perf_cycle_id,
        // set_realtime_transcriber, stop_realtime_leftover). Real count
        // 47 → 51; the interface grew but its NATURE changed: exported
        // object graph → composed operations. Growth still requires
        // editing this number DELIBERATELY (and justifying it).
        //
        // Calibration: the ratchet's two `src.matches(...)` literal strings
        // ALSO appear in the test body itself (one for each pattern), so the
        // measured count is real count + 2.
        let src = include_str!("pipeline_state.rs");
        let count = src.matches("    pub(crate) fn ").count() + src.matches("    pub fn ").count();
        assert_eq!(
            count, 53,
            "PipelineState pub-fn surface changed; update the ratchet deliberately \
             (real count = measured - 2, to subtract the test body's two self-references)"
        );
    }

    #[test]
    fn hide_overlays_hides_floating_and_conditional_review() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let review = Arc::new(MockReviewProvider::new());
        let ps = PipelineState::new(
            Arc::new(Mutex::new(StateMachine::new())),
            Arc::new(Mutex::new(MockAudioCapture::new())),
            Arc::new(MockEngine::new("test")),
            Arc::new(MockClipboard::new()),
            Arc::new(PerfHistory::new()),
            ConfigCache::new(AppConfig::default()),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
            Arc::new(LogWindowController {
                calls: calls.clone(),
            }),
            Arc::new(MockEmitter::new()),
            review.clone(),
        );

        // Case 1: review was NOT shown on press — floating only, flag untouched.
        ps.hide_overlays();
        assert_eq!(*calls.lock().unwrap(), vec!["hide_floating"]);

        // Case 2: review shown on press — both hidden, flag cleared.
        review.set_shown_on_press(true);
        ps.hide_overlays();
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["hide_floating", "hide_floating", "hide_review"]
        );
        assert!(!review.was_shown_on_press());
    }

    #[test]
    fn reset_to_idle_resets_state_and_hides_overlays() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let review = Arc::new(MockReviewProvider::new());
        let ps = PipelineState::new(
            Arc::new(Mutex::new(StateMachine::new())),
            Arc::new(Mutex::new(MockAudioCapture::new())),
            Arc::new(MockEngine::new("test")),
            Arc::new(MockClipboard::new()),
            Arc::new(PerfHistory::new()),
            ConfigCache::new(AppConfig::default()),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
            Arc::new(LogWindowController {
                calls: calls.clone(),
            }),
            Arc::new(MockEmitter::new()),
            review.clone(),
        );

        ps.sm_start_recording();
        review.set_shown_on_press(true);
        ps.reset_to_idle();
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));
        assert_eq!(*calls.lock().unwrap(), vec!["hide_floating", "hide_review"]);
        assert!(!review.was_shown_on_press());
    }

    #[test]
    fn next_perf_cycle_id_monotonic() {
        let ps = build_test_ps();
        let a = ps.next_perf_cycle_id();
        let b = ps.next_perf_cycle_id();
        assert!(b > a, "cycle ids must be monotonically increasing");
    }

    #[test]
    fn realtime_transcriber_slot_set_and_leftover_stop() {
        let ps = build_test_ps();
        // Smoke-only: the slot is private and RealtimeTranscriber exposes no
        // handle back out, so no-panic + idempotence across the set → stop →
        // stop sequence IS the entire observable surface of these two verbs
        // here. Deep behaviour (thread actually stops, accumulated text
        // taken) is covered by stop_recording_resources integration tests.
        ps.stop_realtime_leftover();
        ps.set_realtime_transcriber(crate::realtime::RealtimeTranscriber::start(
            Arc::new(crate::realtime::AudioRingBufferSource::new(
                ps.ring_buffer(),
            )),
            Arc::new(MockEngine::new("test")),
            Arc::new(MockEmitter::new()),
            16_000,
            crate::realtime::RealtimePolicy {
                language: crate::config::Language::Zh,
            },
        ));
        ps.stop_realtime_leftover();
        ps.stop_realtime_leftover(); // idempotent
    }

    #[test]
    fn restore_clipboard_if_saved_gates_on_was_saved() {
        let ps = build_test_ps();
        // MockClipboard starts unsaved → no restore ran.
        // 正分支（was_saved=true → restore 执行）由既有 recover 集成测试
        // `recover_with_saved_restores_clipboard`（recording_session_test.rs）
        // 间接覆盖——MockClipboard 的 saved 态由注入文本周期驱动，本单元
        // 测试只锁 gate 语义（unsaved 不触发）。
        assert!(!ps.restore_clipboard_if_saved());
    }
}
