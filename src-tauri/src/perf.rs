use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Performance metrics for a single voice-typing cycle (hotkey press → injection complete).
#[derive(Debug, Clone, Serialize)]
pub struct PerfMetrics {
    /// Monotonically increasing cycle ID.
    pub cycle_id: u64,

    // Phase durations in milliseconds. `None` means the phase was skipped or the cycle
    // errored before reaching that phase.
    /// Time from hotkey press callback entry to audio capture started.
    pub press_latency_ms: Option<u64>,
    /// How long the user held the hotkey (press → release).
    pub audio_duration_ms: Option<u64>,
    /// Sync work on hook thread: stop audio + extract buffer + resample.
    pub release_latency_ms: Option<u64>,
    /// Whisper.cpp inference time.
    pub transcription_ms: Option<u64>,
    /// LLM API round-trip (None if disabled).
    pub llm_correction_ms: Option<u64>,
    /// Clipboard save + write + Ctrl+V + 200ms sleep + restore.
    pub injection_ms: Option<u64>,
    /// Total wall-clock: press → injection complete.
    pub end_to_end_ms: Option<u64>,

    // Metadata
    /// Raw audio sample count before resample.
    pub audio_samples: usize,
    /// Capture sample rate (typically 48000).
    pub audio_sample_rate: u32,
    /// Final text character count.
    pub text_length: usize,
    /// Whether LLM correction was enabled for this cycle.
    pub llm_enabled: bool,
}

impl PerfMetrics {
    pub fn new(cycle_id: u64) -> Self {
        Self {
            cycle_id,
            press_latency_ms: None,
            audio_duration_ms: None,
            release_latency_ms: None,
            transcription_ms: None,
            llm_correction_ms: None,
            injection_ms: None,
            end_to_end_ms: None,
            audio_samples: 0,
            audio_sample_rate: 0,
            text_length: 0,
            llm_enabled: false,
        }
    }

    /// Format a single-line summary for console output.
    pub fn summary(&self) -> String {
        let fmt_ms = |v: Option<u64>| -> String {
            match v {
                Some(ms) if ms >= 1000 => {
                    let secs = ms as f64 / 1000.0;
                    format!("{secs:.2}s")
                }
                Some(ms) => format!("{ms}ms"),
                None => "-".to_string(),
            }
        };

        format!(
            "[perf #{}] e2e={} | capture={} | release={} | whisper={} | llm={} | inject={} | {}ch",
            self.cycle_id,
            fmt_ms(self.end_to_end_ms),
            fmt_ms(self.audio_duration_ms),
            fmt_ms(self.release_latency_ms),
            fmt_ms(self.transcription_ms),
            fmt_ms(self.llm_correction_ms),
            fmt_ms(self.injection_ms),
            self.text_length,
        )
    }
}

/// Bounded ring buffer holding the last `CAPACITY` cycle metrics.
pub struct PerfHistory {
    counter: AtomicU64,
    buffer: Mutex<VecDeque<PerfMetrics>>,
}

impl PerfHistory {
    pub const CAPACITY: usize = 64;

    pub fn new() -> Self {
        Self {
            counter: AtomicU64::new(1),
            buffer: Mutex::new(VecDeque::with_capacity(Self::CAPACITY)),
        }
    }

    /// Allocate the next monotonically increasing cycle ID.
    pub fn next_cycle_id(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Record a completed cycle's metrics.
    pub fn record(&self, metrics: PerfMetrics) {
        if let Some(mut buf) = crate::util::lock_mutex(&self.buffer, "perf_history") {
            if buf.len() >= Self::CAPACITY {
                buf.pop_front();
            }
            buf.push_back(metrics);
        }
    }

    /// Return the last `n` recorded metrics (most recent last).
    pub fn recent(&self, n: usize) -> Vec<PerfMetrics> {
        if let Some(buf) = crate::util::lock_mutex(&self.buffer, "perf_history") {
            buf.iter()
                .rev()
                .take(n)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        } else {
            Vec::new()
        }
    }
}

impl Default for PerfHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_metrics_all_none() {
        let m = PerfMetrics::new(1);
        assert_eq!(m.cycle_id, 1);
        assert!(m.press_latency_ms.is_none());
        assert!(m.end_to_end_ms.is_none());
    }

    #[test]
    fn test_summary_format() {
        let m = PerfMetrics {
            cycle_id: 42,
            end_to_end_ms: Some(2340),
            audio_duration_ms: Some(1200),
            release_latency_ms: Some(12),
            transcription_ms: Some(890),
            llm_correction_ms: None,
            injection_ms: Some(215),
            text_length: 48,
            ..PerfMetrics::new(42)
        };
        let s = m.summary();
        assert!(s.contains("[perf #42]"));
        assert!(s.contains("e2e=2.34s"));
        assert!(s.contains("capture=1.20s"));
        assert!(s.contains("release=12ms"));
        assert!(s.contains("whisper=890ms"));
        assert!(s.contains("inject=215ms"));
        assert!(s.contains("48ch"));
    }

    #[test]
    fn test_history_record_and_recent() {
        let h = PerfHistory::new();
        let id1 = h.next_cycle_id();
        let id2 = h.next_cycle_id();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);

        h.record(PerfMetrics::new(id1));
        h.record(PerfMetrics::new(id2));

        let recent = h.recent(10);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].cycle_id, 1);
        assert_eq!(recent[1].cycle_id, 2);
    }

    #[test]
    fn test_history_caps_at_capacity() {
        let h = PerfHistory::new();
        for _ in 0..(PerfHistory::CAPACITY + 10) {
            let id = h.next_cycle_id();
            h.record(PerfMetrics::new(id));
        }
        let recent = h.recent(100);
        assert_eq!(recent.len(), PerfHistory::CAPACITY);
        // Oldest should have been evicted; newest is last.
        assert_eq!(
            recent.last().unwrap().cycle_id,
            PerfHistory::CAPACITY as u64 + 10
        );
    }
}
