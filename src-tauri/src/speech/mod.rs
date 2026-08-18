#[cfg(test)]
pub mod mock;
#[cfg(not(feature = "whisper"))]
pub mod noop;
#[cfg(feature = "whisper")]
pub mod whisper;
#[cfg(feature = "whisper")]
pub mod whisper_factory;

use crate::error::AppError;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A transcribed text segment with absolute timestamps in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Segment {
    pub text: String,
    pub start_ms: u32,
    pub end_ms: u32,
}

impl Segment {
    /// Build a segment from whisper.cpp timestamps, which are reported in
    /// centiseconds (10s of milliseconds). Single conversion entry point —
    /// never multiply by 10 anywhere else.
    pub fn from_whisper(text: String, start_cs: i64, end_cs: i64) -> Self {
        Self {
            text,
            start_ms: centiseconds_to_ms(start_cs),
            end_ms: centiseconds_to_ms(end_cs),
        }
    }
}

/// centiseconds → milliseconds, clamped to the u32 range and ≥ 0.
fn centiseconds_to_ms(cs: i64) -> u32 {
    u32::try_from(cs.saturating_mul(10).max(0)).unwrap_or(u32::MAX)
}

/// Error message used when a transcription is cancelled via its token.
pub(crate) const CANCELLED_MESSAGE: &str = "transcription cancelled";

/// Trait for speech-to-text engines.
///
/// Allows swapping backends (Whisper.cpp, cloud APIs) without changing consumers.
/// Dyn-compatible: uses only `&self` methods, no generic parameters, no async methods.
pub trait SpeechEngine: Send + Sync + 'static {
    /// Synchronous transcription (blocking). Use via `spawn_blocking` from async contexts.
    fn transcribe_sync(&self, samples: &[f32]) -> Result<String, AppError>;

    /// Check if the engine's model is loaded and ready.
    fn is_ready(&self) -> bool;

    /// Get the engine name for display.
    fn name(&self) -> &str;

    /// Return the compute mode badge string for the UI.
    /// Default is "unloaded"; WhisperEngine returns "gpu", "cpu", or "unloaded".
    fn compute_mode(&self) -> &'static str {
        "unloaded"
    }

    /// Transcribe with prior context for stabilizing overlapping regions.
    /// Default implementation ignores context and delegates to `transcribe_sync`.
    fn transcribe_sync_with_context(
        &self,
        samples: &[f32],
        _context: Option<&str>,
    ) -> Result<String, AppError> {
        self.transcribe_sync(samples)
    }

    /// Transcribe long-form audio into timestamped segments, with
    /// cancellation and progress reporting. Samples are 16kHz mono f32.
    ///
    /// Default implementation delegates to `transcribe_sync` and maps the
    /// result to a single segment spanning the whole clip (Mock/Noop
    /// behavior; WhisperEngine overrides with real segment timestamps).
    fn transcribe_with_segments_sync(
        &self,
        samples: &[f32],
        cancel: Arc<AtomicBool>,
        progress: Box<dyn Fn(u8) + Send + Sync>,
    ) -> Result<Vec<Segment>, AppError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(AppError::Speech(CANCELLED_MESSAGE.to_string()));
        }
        let text = self.transcribe_sync(samples)?;
        progress(100);
        let duration_ms = (samples.len() as u64 * 1000 / crate::audio::TARGET_SAMPLE_RATE as u64)
            .min(u32::MAX as u64) as u32;
        Ok(vec![Segment {
            text,
            start_ms: 0,
            end_ms: duration_ms,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::mock::MockEngine;

    #[test]
    fn test_from_whisper_converts_centiseconds() {
        let seg = Segment::from_whisper("你好".to_string(), 0, 150);
        assert_eq!(seg.start_ms, 0);
        assert_eq!(seg.end_ms, 1500);
    }

    #[test]
    fn test_from_whisper_clamps_negative() {
        let seg = Segment::from_whisper("x".to_string(), -5, -1);
        assert_eq!(seg.start_ms, 0);
        assert_eq!(seg.end_ms, 0);
    }

    #[test]
    fn test_from_whisper_large_values_saturate() {
        let seg = Segment::from_whisper("x".to_string(), i64::MAX, i64::MAX / 2);
        assert_eq!(seg.start_ms, u32::MAX);
        assert!(seg.end_ms > 0);
    }

    #[test]
    fn test_segment_serde_roundtrip() {
        let seg = Segment {
            text: "文本".to_string(),
            start_ms: 1000,
            end_ms: 2500,
        };
        let json = serde_json::to_string(&seg);
        assert!(json.is_ok());
        let Ok(json) = json else { return };
        let parsed: Result<Segment, _> = serde_json::from_str(&json);
        assert!(parsed.is_ok());
        if let Ok(p) = parsed {
            assert_eq!(p, seg);
        }
    }

    #[test]
    fn test_default_segments_impl_single_segment() {
        let engine = MockEngine::new("hello world");
        // 1 second of 16kHz audio.
        let samples = vec![0.0f32; 16000];
        let progress_calls = std::sync::Mutex::new(Vec::new());
        let result = engine.transcribe_with_segments_sync(
            &samples,
            Arc::new(AtomicBool::new(false)),
            Box::new(move |p| {
                if let Ok(mut g) = progress_calls.lock() {
                    g.push(p);
                }
            }),
        );
        assert!(result.is_ok());
        let Ok(segments) = result else { return };
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "hello world");
        assert_eq!(segments[0].start_ms, 0);
        assert_eq!(segments[0].end_ms, 1000);
    }

    #[test]
    fn test_default_segments_impl_cancelled() {
        let engine = MockEngine::new("hello");
        let result = engine.transcribe_with_segments_sync(
            &[0.0f32; 100],
            Arc::new(AtomicBool::new(true)),
            Box::new(|_| {}),
        );
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains(CANCELLED_MESSAGE));
        }
    }

    #[test]
    fn test_default_segments_impl_duration_from_samples() {
        let engine = MockEngine::new("x");
        // 90 seconds of 16kHz audio.
        let samples = vec![0.0f32; 16000 * 90];
        let result = engine.transcribe_with_segments_sync(
            &samples,
            Arc::new(AtomicBool::new(false)),
            Box::new(|_| {}),
        );
        assert!(result.is_ok());
        let Ok(segments) = result else { return };
        assert_eq!(segments[0].end_ms, 90_000);
    }
}
