use crate::audio::AudioCaptureProvider;
use crate::clipboard::AnyClipboard;
use crate::commands::EventEmitter;
use crate::commands::TauriEventEmitter;
use crate::commands::review_provider::{ReviewProvider, TauriReviewProvider};
use crate::commands::window_controller::window_controller_from_app;
use crate::config::ConfigCache;
use crate::llm::AnyCorrector;
use crate::perf::PerfHistory;
use crate::realtime::RealtimeTranscriber;
use crate::speech::AnyEngine;
use crate::state::StateMachine;
use std::sync::{Arc, Mutex};
use tauri::Manager;
use tracing::info;

/// Aggregated shared state for the hotkey pipeline.
/// Eliminates the need to pass 8 individual `Arc` references to `make_hotkey_callback`.
#[derive(Clone)]
pub(crate) struct PipelineState {
    pub sm: Arc<Mutex<StateMachine>>,
    pub ac: Arc<Mutex<dyn AudioCaptureProvider>>,
    pub engine: Arc<Mutex<AnyEngine>>,
    pub clipboard: Arc<Mutex<AnyClipboard>>,
    pub perf_history: Arc<PerfHistory>,
    pub config_cache: ConfigCache,
    pub cached_llm: Arc<Mutex<Option<AnyCorrector>>>,
    pub realtime_transcriber: Arc<Mutex<Option<RealtimeTranscriber>>>,
    pub window_controller: Arc<dyn crate::commands::window_controller::WindowController>,
    pub emitter: Arc<dyn EventEmitter>,
    pub review: Arc<dyn ReviewProvider>,
}

impl PipelineState {
    /// Direct constructor for testing. Each component is injectable.
    #[allow(dead_code, clippy::too_many_arguments)]
    pub fn new(
        sm: Arc<Mutex<StateMachine>>,
        ac: Arc<Mutex<dyn AudioCaptureProvider>>,
        engine: Arc<Mutex<AnyEngine>>,
        clipboard: Arc<Mutex<AnyClipboard>>,
        perf_history: Arc<PerfHistory>,
        config_cache: ConfigCache,
        cached_llm: Arc<Mutex<Option<AnyCorrector>>>,
        realtime_transcriber: Arc<Mutex<Option<RealtimeTranscriber>>>,
        window_controller: Arc<dyn crate::commands::window_controller::WindowController>,
        emitter: Arc<dyn EventEmitter>,
        review: Arc<dyn ReviewProvider>,
    ) -> Self {
        Self {
            sm,
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
        }
    }

    /// Extract all pipeline state from Tauri's managed state.
    pub fn from_app(app: &tauri::AppHandle) -> Self {
        Self {
            sm: app.state::<Arc<Mutex<StateMachine>>>().inner().clone(),
            ac: app
                .state::<Arc<Mutex<dyn AudioCaptureProvider>>>()
                .inner()
                .clone(),
            engine: app.state::<Arc<Mutex<AnyEngine>>>().inner().clone(),
            clipboard: app.state::<Arc<Mutex<AnyClipboard>>>().inner().clone(),
            perf_history: app.state::<Arc<PerfHistory>>().inner().clone(),
            config_cache: app.state::<ConfigCache>().inner().clone(),
            cached_llm: app
                .state::<Arc<Mutex<Option<AnyCorrector>>>>()
                .inner()
                .clone(),
            realtime_transcriber: app
                .state::<Arc<Mutex<Option<RealtimeTranscriber>>>>()
                .inner()
                .clone(),
            window_controller: window_controller_from_app(app),
            emitter: Arc::new(TauriEventEmitter::new(app.clone())),
            review: Arc::new(TauriReviewProvider::new(app.clone())),
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
}
