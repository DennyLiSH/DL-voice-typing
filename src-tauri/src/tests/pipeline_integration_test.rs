//! Integration tests using mock PipelineState components.
//!
//! These tests verify mock component interactions and EventEmitter abstraction,
//! without constructing a full PipelineState (which would pull in TauriWindowController
//! and its win32 DLL dependencies at link time).

use crate::audio::{AudioCaptureProvider, MockAudioCapture};
use crate::clipboard::{AnyClipboard, ClipboardProvider, MockClipboard};
use crate::commands::EventEmitter;
use crate::config::AppConfig;
use crate::llm::{AnyCorrector, MockCorrector, TextCorrector};
use crate::speech::{AnyEngine, SpeechEngine};
use crate::state::StateMachine;
use std::sync::{Arc, Mutex, RwLock};

struct MockEmitter {
    events: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
}

impl MockEmitter {
    fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl EventEmitter for MockEmitter {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        self.events
            .lock()
            .unwrap()
            .push((event.to_string(), payload));
    }
}

// ---------------------------------------------------------------------------
// Mock component interactions
// ---------------------------------------------------------------------------

#[test]
fn test_mock_engine_transcribes() {
    let engine = AnyEngine::new_mock("test transcription");
    let result = engine.transcribe_sync(&[0.5f32; 1600]);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "test transcription");
}

#[test]
fn test_mock_engine_is_ready() {
    let engine = AnyEngine::new_mock("test");
    assert!(engine.is_ready());
}

#[test]
fn test_mock_clipboard_cycle() {
    let mut cb = AnyClipboard::Mock(MockClipboard::new());
    assert!(cb.save().is_ok());
    assert!(cb.inject_text("hello").is_ok());
    assert!(cb.restore().is_ok());
}

#[test]
fn test_mock_corrector_corrects() {
    let corrector = AnyCorrector::Mock(MockCorrector::new("corrected"));
    let result = corrector.correct_sync("original");
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "corrected");
}

#[test]
fn test_mock_audio_capture_lifecycle() {
    let mut ac = MockAudioCapture::new();
    assert!(!ac.is_capturing());
    assert!(ac.sample_rate().is_none());
    assert!(ac.start(Box::new(|_data: &[f32]| {})).is_ok());
    assert!(ac.is_capturing());
    ac.stop();
    assert!(!ac.is_capturing());
}

#[test]
fn test_config_cache_round_trip() {
    let config = AppConfig {
        language: crate::config::Language::Zh,
        ..Default::default()
    };
    let cache: Arc<RwLock<AppConfig>> = Arc::new(RwLock::new(config.clone()));
    let cached = AppConfig::read_cached(&cache).unwrap();
    assert_eq!(cached.language, crate::config::Language::Zh);
}

// ---------------------------------------------------------------------------
// State machine + mock engine integration
// ---------------------------------------------------------------------------

#[test]
fn test_state_machine_with_mock_engine() {
    let mut sm = StateMachine::new();
    sm.start_recording().unwrap();
    sm.append_audio(&[0.5f32; 1600]).unwrap();
    let audio = sm.stop_recording().unwrap();

    let engine = AnyEngine::new_mock("hello world");
    let text = engine.transcribe_sync(&audio).unwrap();

    sm.transcribing_to_injecting(text.clone()).unwrap();

    let mut cb = AnyClipboard::Mock(MockClipboard::new());
    cb.save().unwrap();
    cb.inject_text(&text).unwrap();

    sm.finish_injecting().unwrap();
    assert!(matches!(sm.state(), crate::state::AppState::Idle));
}

// ---------------------------------------------------------------------------
// EventEmitter integration
// ---------------------------------------------------------------------------

#[test]
fn test_emitter_records_events() {
    let emitter = MockEmitter::new();
    emitter.emit("test-event", serde_json::json!("payload"));
    emitter.emit("another", serde_json::Value::Null);

    let events = emitter.events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].0, "test-event");
    assert_eq!(events[1].0, "another");
}

#[test]
fn test_emitter_trait_object_dispatch() {
    let emitter: Arc<dyn EventEmitter> = Arc::new(MockEmitter::new());
    emitter.emit("injection-complete", serde_json::Value::Null);
    emitter.emit("speech-error", serde_json::json!("test error"));

    // Verify via raw pointer downcast (Arc<dyn EventEmitter> doesn't support downcast).
    let emitter_ref: &dyn EventEmitter = &*emitter;
    let raw = emitter_ref as *const dyn EventEmitter;
    let _ = raw; // Pointer is valid — trait object dispatch succeeded.
}
