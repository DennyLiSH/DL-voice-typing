use crate::audio::{AudioCaptureProvider, AudioRingBuffer};
use crate::clipboard::ClipboardProvider;
use crate::commands::EventEmitter;
use crate::commands::TauriEventEmitter;
use crate::commands::delivery_controller::DeliveryController;
use crate::commands::review_provider::{ReviewProvider, TauriReviewProvider};
use crate::commands::window_controller::window_controller_from_app;
use crate::config::ConfigCache;
use crate::llm::AnyCorrector;
use crate::perf::PerfHistory;
use crate::realtime::RealtimeTranscriber;
use crate::speech::SpeechEngine;
use crate::state::{StateMachine, StateTag};
use std::sync::{Arc, Mutex};
use tauri::Manager;
use tracing::{info, warn};

/// Aggregated shared state for the hotkey pipeline.
/// Eliminates the need to pass 8 individual `Arc` references to `make_hotkey_callback`.
#[derive(Clone)]
pub struct PipelineState {
    pub(crate) sm: Arc<Mutex<StateMachine>>,
    #[cfg(test)]
    forced_sm_state: Arc<Mutex<Option<StateTag>>>,
    pub(crate) ac: Arc<Mutex<dyn AudioCaptureProvider>>,
    pub(crate) engine: Arc<dyn SpeechEngine>,
    pub(crate) clipboard: Arc<dyn ClipboardProvider>,
    pub(crate) perf_history: Arc<PerfHistory>,
    pub(crate) config_cache: ConfigCache,
    pub(crate) cached_llm: Arc<Mutex<Option<AnyCorrector>>>,
    pub(crate) realtime_transcriber: Arc<Mutex<Option<RealtimeTranscriber>>>,
    pub(crate) window_controller: Arc<dyn crate::commands::window_controller::WindowController>,
    pub(crate) emitter: Arc<dyn EventEmitter>,
    pub(crate) review: Arc<dyn ReviewProvider>,
    /// Single authority for post-transcription delivery lifecycle.
    pub(crate) delivery: Arc<DeliveryController>,
    /// Decoupled audio buffer for lock-free realtime reads.
    /// Capacity: 60 seconds @ 48kHz = 2,880,000 samples.
    pub(crate) audio_ring_buffer: Arc<Mutex<AudioRingBuffer>>,
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
        cached_llm: Arc<Mutex<Option<AnyCorrector>>>,
        realtime_transcriber: Arc<Mutex<Option<RealtimeTranscriber>>>,
        window_controller: Arc<dyn crate::commands::window_controller::WindowController>,
        emitter: Arc<dyn EventEmitter>,
        review: Arc<dyn ReviewProvider>,
    ) -> Self {
        let delivery = Arc::new(DeliveryController::new(
            emitter.clone(),
            window_controller.clone(),
            clipboard.clone(),
            review.clone(),
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
        }
    }

    /// Extract all pipeline state from Tauri's managed state.
    pub fn from_app(app: &tauri::AppHandle) -> Self {
        let window_controller = window_controller_from_app(app);
        let emitter: Arc<dyn EventEmitter> = Arc::new(TauriEventEmitter::new(app.clone()));
        let review: Arc<dyn ReviewProvider> = Arc::new(TauriReviewProvider::new(app.clone()));
        let clipboard = app.state::<Arc<dyn ClipboardProvider>>().inner().clone();
        let perf_history = app.state::<Arc<PerfHistory>>().inner().clone();
        let delivery = Arc::new(DeliveryController::new(
            emitter.clone(),
            window_controller.clone(),
            clipboard.clone(),
            review.clone(),
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
                .state::<Arc<Mutex<Option<AnyCorrector>>>>()
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

    /// Current state-machine tag, or None if the lock is poisoned.
    /// Used by sm_verb_tests; kept for future query-style callers.
    #[allow(dead_code)]
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
        let Some(mut s) = crate::util::lock_mutex(&self.sm, "state_machine") else {
            return false;
        };
        match s.start_recording() {
            Ok(()) => true,
            Err(e) => {
                warn!("sm_start_recording failed: {e}");
                false
            }
        }
    }

    pub(crate) fn sm_stop_recording(&self) -> bool {
        let Some(mut s) = crate::util::lock_mutex(&self.sm, "state_machine") else {
            return false;
        };
        match s.stop_recording() {
            Ok(()) => true,
            Err(e) => {
                warn!("sm_stop_recording failed: {e}");
                false
            }
        }
    }

    pub(crate) fn sm_start_llm_refining(&self) -> bool {
        let Some(mut s) = crate::util::lock_mutex(&self.sm, "state_machine") else {
            return false;
        };
        match s.start_llm_refining() {
            Ok(()) => true,
            Err(e) => {
                warn!("sm_start_llm_refining failed: {e}");
                false
            }
        }
    }

    pub(crate) fn sm_transcribing_to_injecting(&self) -> bool {
        let Some(mut s) = crate::util::lock_mutex(&self.sm, "state_machine") else {
            return false;
        };
        match s.transcribing_to_injecting() {
            Ok(()) => true,
            Err(e) => {
                warn!("sm_transcribing_to_injecting failed: {e}");
                false
            }
        }
    }

    pub(crate) fn sm_llm_to_injecting(&self) -> bool {
        let Some(mut s) = crate::util::lock_mutex(&self.sm, "state_machine") else {
            return false;
        };
        match s.llm_to_injecting() {
            Ok(()) => true,
            Err(e) => {
                warn!("sm_llm_to_injecting failed: {e}");
                false
            }
        }
    }

    pub(crate) fn sm_transcribing_to_reviewing(&self) -> bool {
        let Some(mut s) = crate::util::lock_mutex(&self.sm, "state_machine") else {
            return false;
        };
        match s.transcribing_to_reviewing() {
            Ok(()) => true,
            Err(e) => {
                warn!("sm_transcribing_to_reviewing failed: {e}");
                false
            }
        }
    }

    pub(crate) fn sm_llm_to_reviewing(&self) -> bool {
        let Some(mut s) = crate::util::lock_mutex(&self.sm, "state_machine") else {
            return false;
        };
        match s.llm_to_reviewing() {
            Ok(()) => true,
            Err(e) => {
                warn!("sm_llm_to_reviewing failed: {e}");
                false
            }
        }
    }

    /// Review → Injecting transition. Called by DeliveryController::confirm_from_reviewing.
    pub(crate) fn sm_reviewing_to_injecting(&self) -> bool {
        let Some(mut s) = crate::util::lock_mutex(&self.sm, "state_machine") else {
            return false;
        };
        match s.reviewing_to_injecting() {
            Ok(()) => true,
            Err(e) => {
                warn!("sm_reviewing_to_injecting failed: {e}");
                false
            }
        }
    }

    /// Review → Idle (cancel) transition. Called by DeliveryController::cancel_review.
    pub(crate) fn sm_cancel_reviewing(&self) -> bool {
        let Some(mut s) = crate::util::lock_mutex(&self.sm, "state_machine") else {
            return false;
        };
        match s.cancel_reviewing() {
            Ok(()) => true,
            Err(e) => {
                warn!("sm_cancel_reviewing failed: {e}");
                false
            }
        }
    }

    pub(crate) fn sm_finish_injecting(&self) -> bool {
        let Some(mut s) = crate::util::lock_mutex(&self.sm, "state_machine") else {
            return false;
        };
        match s.finish_injecting() {
            Ok(()) => true,
            Err(e) => {
                warn!("sm_finish_injecting failed: {e}");
                false
            }
        }
    }

    /// Reset to Idle unconditionally (infallible — `reset()` itself cannot fail).
    /// Lock-poisoned case is silently dropped (already warn!'d by `lock_mutex`).
    pub(crate) fn sm_reset(&self) {
        if let Some(mut s) = crate::util::lock_mutex(&self.sm, "state_machine") {
            s.reset();
        }
    }
}

#[cfg(test)]
impl PipelineState {
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
    use crate::llm::{AnyCorrector, MockCorrector};
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
}
