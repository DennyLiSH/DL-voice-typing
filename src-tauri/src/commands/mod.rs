pub mod config_cmd;
pub mod data_management_cmd;
pub mod delivery_controller;
pub mod download;
pub mod error_history;
pub mod hotkey_pipeline;
pub mod llm_cmd;
pub mod log_cmd;
pub(crate) mod open_window_hotkey;
pub mod perf_cmd;
pub(crate) mod pipeline_state;
pub(crate) mod record_only_session;
pub(crate) mod recording_session;
pub(crate) use open_window_hotkey::{WindowKind, make_open_window_callback};
pub mod review;
pub(crate) mod review_provider;
pub mod transcribe_cmd;
pub mod window_controller;

use tauri::Emitter as TauriEmitter;

/// Trait for emitting events to the frontend.
/// Abstracts `tauri::AppHandle.emit()` for testability.
/// Uses `serde_json::Value` for trait-object safety.
pub trait EventEmitter: Send + Sync {
    fn emit(&self, event: &str, payload: serde_json::Value);
}

/// Tauri-based event emitter for production use.
pub(crate) struct TauriEventEmitter {
    app: tauri::AppHandle,
}

impl TauriEventEmitter {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl EventEmitter for TauriEventEmitter {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = TauriEmitter::emit(&self.app, event, payload);
    }
}

type EventHook = Box<dyn Fn(&str) + Send + Sync>;

/// Mock event emitter for testing. Records all emitted events.
pub struct MockEmitter {
    events: std::sync::Mutex<Vec<(String, serde_json::Value)>>,
    /// Optional test hook fired on every emit AFTER the event is recorded.
    /// Used by cancel-gate tests to flip the cancel token at a precise
    /// pipeline point (e.g. on `transcription-complete`, which is emitted
    /// between gate 1 and gate 2 — preset-true tokens exit at gate 1 and
    /// make every downstream gate test vacuous).
    on_event: std::sync::Mutex<Option<EventHook>>,
}

impl Default for MockEmitter {
    fn default() -> Self {
        Self::new()
    }
}

impl MockEmitter {
    /// Create a new mock emitter with an empty event log.
    pub fn new() -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
            on_event: std::sync::Mutex::new(None),
        }
    }

    /// Install a callback fired on every `emit` (after recording). The
    /// callback receives the event name; filter inside it.
    pub fn set_on_event(&self, cb: EventHook) {
        if let Some(mut guard) =
            crate::util::lock_mutex(&self.on_event, "MockEmitter::set_on_event")
        {
            *guard = Some(cb);
        }
    }

    /// Drain and return all recorded (event, payload) pairs.
    pub fn take_events(&self) -> Vec<(String, serde_json::Value)> {
        crate::util::lock_mutex(&self.events, "MockEmitter::take_events")
            .map(|mut guard| guard.drain(..).collect())
            .unwrap_or_default()
    }
}

impl EventEmitter for MockEmitter {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        if let Some(mut guard) = crate::util::lock_mutex(&self.events, "MockEmitter::emit") {
            guard.push((event.to_string(), payload));
        }
        // Fired while holding the on_event lock (a DIFFERENT lock than
        // `events`, so the callback may call take_events without
        // deadlocking). Callbacks must not re-enter set_on_event.
        if let Some(guard) = crate::util::lock_mutex(&self.on_event, "MockEmitter::emit_on_event")
            && let Some(cb) = guard.as_ref()
        {
            cb(event);
        }
    }
}

// Type re-exports used by lib.rs / tests + the hotkey callback ctor.
// Command functions are always referenced by full path in
// generate_handler; do not re-add function re-exports.
pub use download::{DownloadState, ModelsResponse};
pub(crate) use hotkey_pipeline::make_hotkey_callback;
pub use review::PendingReview;
pub use window_controller::WindowController;
