use crate::audio::AudioCapture;
use crate::clipboard::AnyClipboard;
use crate::config::ConfigCache;
use crate::llm::AnyCorrector;
use crate::perf::PerfHistory;
use crate::speech::AnyEngine;
use crate::state::StateMachine;
use std::sync::{Arc, Mutex};
use tauri::Manager;

/// Aggregated shared state for the hotkey pipeline.
/// Eliminates the need to pass 8 individual `Arc` references to `make_hotkey_callback`.
#[derive(Clone)]
pub(crate) struct PipelineState {
    pub sm: Arc<Mutex<StateMachine>>,
    pub ac: Arc<Mutex<AudioCapture>>,
    pub engine: Arc<Mutex<AnyEngine>>,
    pub clipboard: Arc<Mutex<AnyClipboard>>,
    pub perf_history: Arc<PerfHistory>,
    pub app: tauri::AppHandle,
    pub config_cache: ConfigCache,
    pub cached_llm: Arc<Mutex<Option<AnyCorrector>>>,
}

impl PipelineState {
    /// Extract all pipeline state from Tauri's managed state.
    pub fn from_app(app: &tauri::AppHandle) -> Self {
        Self {
            sm: app.state::<Arc<Mutex<StateMachine>>>().inner().clone(),
            ac: app.state::<Arc<Mutex<AudioCapture>>>().inner().clone(),
            engine: app.state::<Arc<Mutex<AnyEngine>>>().inner().clone(),
            clipboard: app.state::<Arc<Mutex<AnyClipboard>>>().inner().clone(),
            perf_history: app.state::<Arc<PerfHistory>>().inner().clone(),
            app: app.clone(),
            config_cache: app.state::<ConfigCache>().inner().clone(),
            cached_llm: app
                .state::<Arc<Mutex<Option<AnyCorrector>>>>()
                .inner()
                .clone(),
        }
    }
}
