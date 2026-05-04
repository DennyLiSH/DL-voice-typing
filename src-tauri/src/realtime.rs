/// Real-time transcription using a sliding window approach.
///
/// While the user holds the hotkey, a background thread periodically
/// extracts the last N seconds of audio, transcribes it, and emits
/// accumulated partial results to the frontend.
///
/// Text accumulation: consecutive sliding windows overlap by ~90%.
/// Each new transcription is diffed against the previous one to extract
/// only the new content, which is appended to a running accumulated string.
use crate::audio::rms;
use crate::speech::{AnyEngine, SpeechEngine};
use crate::state::StateMachine;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tauri::Emitter;
use tracing::{debug, warn};

/// Sleep for `total_ms`, but wake early if `running` becomes false.
fn sleep_or_stop(running: &AtomicBool, total_ms: u64) {
    let steps = total_ms.div_ceil(STOP_POLL_MS);
    for _ in 0..steps {
        if !running.load(Ordering::Relaxed) {
            return;
        }
        thread::sleep(Duration::from_millis(STOP_POLL_MS));
    }
}

/// Interval between transcription attempts (milliseconds).
const STEP_MS: u64 = 500;

/// Shorter sleep when audio buffer is still empty or very short (milliseconds).
const SHORT_STEP_MS: u64 = 100;

/// Sleep interval for polling the stop flag (milliseconds).
const STOP_POLL_MS: u64 = 50;

/// Audio window size for each transcription (seconds).
const WINDOW_SECS: u32 = 5;

/// RMS threshold below which audio is considered silent.
const VAD_THRESHOLD: f32 = 0.02;

/// Target sample rate for Whisper (16 kHz).
const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Minimum overlap ratio to consider two consecutive partials as continuous speech.
/// Below this threshold, we treat the new partial as a fresh segment.
const MIN_OVERLAP_RATIO: f32 = 0.5;

/// Frame size for speech energy detection (100ms at 16kHz = 1600 samples).
const ENERGY_FRAME_SAMPLES: usize = 1600;

/// Per-frame RMS threshold for speech energy detection.
const ENERGY_FRAME_THRESHOLD: f32 = 0.04;

/// Minimum number of high-energy frames required to consider audio as containing speech.
const ENERGY_MIN_FRAMES: usize = 5;

/// Check whether the audio contains sustained speech energy.
/// Splits the audio into 100ms frames and requires at least `ENERGY_MIN_FRAMES` frames
/// to exceed `ENERGY_FRAME_THRESHOLD` RMS. This distinguishes real speech (clear energy
/// peaks from syllables) from ambient noise (uniform low energy).
fn has_speech_energy(resampled: &[f32]) -> bool {
    if resampled.len() < ENERGY_FRAME_SAMPLES {
        return false;
    }
    let mut high_energy_frames = 0;
    for frame in resampled.chunks(ENERGY_FRAME_SAMPLES) {
        if frame.len() < ENERGY_FRAME_SAMPLES {
            break;
        }
        let frame_rms = rms::calculate_rms(frame);
        if frame_rms > ENERGY_FRAME_THRESHOLD {
            high_energy_frames += 1;
        }
    }
    high_energy_frames >= ENERGY_MIN_FRAMES
}

pub struct RealtimeTranscriber {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    /// Accumulated text from all partial transcriptions (deduplicated).
    accumulated: Arc<Mutex<String>>,
}

/// Whether a character is punctuation or whitespace that should be ignored
/// during overlap comparison (Whisper output varies in punctuation between windows).
fn is_punct(c: char) -> bool {
    matches!(
        c,
        ',' | '.'
            | '!'
            | '?'
            | ';'
            | ':'
            | '-'
            | '—'
            | '…'
            | '\u{201C}'
            | '\u{201D}'
            | '\''
            | '，'
            | '。'
            | '！'
            | '？'
            | '；'
            | '：'
            | '、'
    ) || c.is_whitespace()
}

/// Walk through `s` and return the byte offset right after the `n`-th content
/// (non-punctuation) character. Returns `s.len()` if fewer than `n` content
/// characters exist. This preserves any punctuation between the overlap and
/// new content (e.g., the space in "is John").
fn content_char_offset(s: &str, n: usize) -> usize {
    let mut count = 0;
    for (byte_idx, ch) in s.char_indices() {
        if !is_punct(ch) {
            count += 1;
            if count == n {
                return byte_idx + ch.len_utf8();
            }
        }
    }
    s.len()
}

/// Find the character position where `prev_partial` overlaps with `new_partial`.
/// Returns the byte offset in `new_partial` where new content begins.
/// Returns 0 if no meaningful overlap is found.
/// Comparison ignores punctuation/whitespace to handle Whisper output variation.
fn find_overlap_end(prev_partial: &str, new_partial: &str) -> usize {
    if prev_partial.is_empty() || new_partial.is_empty() {
        return 0;
    }

    let prev_content: Vec<char> = prev_partial.chars().filter(|c| !is_punct(*c)).collect();
    let new_content: Vec<char> = new_partial.chars().filter(|c| !is_punct(*c)).collect();
    let min_overlap = ((prev_content.len() as f32) * MIN_OVERLAP_RATIO) as usize;

    // Try longest overlap first: find the longest suffix of prev that matches
    // a prefix of new, comparing content characters only (ignoring punctuation).
    let max_check = prev_content.len().min(new_content.len());
    for len in (min_overlap.max(1)..=max_check).rev() {
        if prev_content[prev_content.len() - len..] == new_content[..len] {
            // Found overlap of `len` content chars.
            // Map back to byte offset in original new_partial.
            return content_char_offset(new_partial, len);
        }
    }

    0
}

/// Accumulate a new partial transcription into the running text.
/// Uses suffix-matching to detect sliding-window overlap and extract only new content.
fn accumulate(accumulated: &str, prev_partial: &str, new_partial: &str) -> String {
    if accumulated.is_empty() {
        return new_partial.to_string();
    }

    let overlap_end = find_overlap_end(prev_partial, new_partial);
    if overlap_end > 0 {
        // Found overlap — append only the non-overlapping part.
        let new_content = &new_partial[overlap_end..];
        let mut result = String::with_capacity(accumulated.len() + new_content.len());
        result.push_str(accumulated);
        result.push_str(new_content);
        result
    } else {
        // No overlap — new topic or Whisper rephrased. Append as separate segment.
        let mut result = String::with_capacity(accumulated.len() + 1 + new_partial.len());
        result.push_str(accumulated);
        result.push(' ');
        result.push_str(new_partial);
        result
    }
}

impl RealtimeTranscriber {
    /// Start the background transcription loop.
    pub fn start(
        engine: Arc<Mutex<AnyEngine>>,
        state_machine: Arc<Mutex<StateMachine>>,
        app: tauri::AppHandle,
        sample_rate: u32,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let accumulated: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let accumulated_clone = accumulated.clone();

        let handle = thread::spawn(move || {
            let _span = tracing::info_span!("realtime_transcriber").entered();
            let mut prev_partial = String::new();

            while running_clone.load(Ordering::Relaxed) {
                let window = {
                    let samples_needed = (sample_rate * WINDOW_SECS) as usize;
                    match state_machine.lock() {
                        Ok(sm) => match sm.get_audio_buffer() {
                            Some(buf) if !buf.is_empty() => {
                                let start = buf.len().saturating_sub(samples_needed);
                                buf[start..].to_vec()
                            }
                            Some(_) => {
                                thread::sleep(Duration::from_millis(SHORT_STEP_MS));
                                continue;
                            }
                            None => break,
                        },
                        Err(_) => {
                            warn!("state machine lock poisoned, exiting realtime loop");
                            break;
                        }
                    }
                };

                let resampled =
                    crate::data_saving::resample(&window, sample_rate, TARGET_SAMPLE_RATE);
                let rms_val = rms::calculate_rms(&resampled);

                if rms_val < VAD_THRESHOLD || !has_speech_energy(&resampled) {
                    debug!("realtime VAD: silent (rms={rms_val:.4}), skipping");
                    sleep_or_stop(&running_clone, STEP_MS);
                    continue;
                }

                let text = {
                    let guard = match engine.try_lock() {
                        Ok(g) => g,
                        Err(std::sync::TryLockError::WouldBlock) => {
                            debug!("realtime: engine busy, skipping cycle");
                            sleep_or_stop(&running_clone, STEP_MS);
                            continue;
                        }
                        Err(std::sync::TryLockError::Poisoned(_)) => {
                            warn!("engine lock poisoned, exiting realtime loop");
                            break;
                        }
                    };
                    match guard.transcribe_sync(&resampled) {
                        Ok(t) => t,
                        Err(err) => {
                            warn!("realtime transcription error: {err}");
                            sleep_or_stop(&running_clone, STEP_MS);
                            continue;
                        }
                    }
                };

                if !text.is_empty() {
                    // Accumulate: diff against previous partial to extract new content.
                    let new_accumulated = {
                        let mut acc_guard = match accumulated_clone.lock() {
                            Ok(g) => g,
                            Err(_) => break,
                        };
                        let new_acc = accumulate(&acc_guard, &prev_partial, &text);
                        *acc_guard = new_acc.clone();
                        new_acc
                    };
                    prev_partial = text;

                    // Only emit if still running — avoid late events after stop signal.
                    if running_clone.load(Ordering::Relaxed) {
                        let _ = app.emit("transcription-partial", &new_accumulated);
                    }
                }

                sleep_or_stop(&running_clone, STEP_MS);
            }

            debug!("realtime transcriber loop exited");
        });

        Self {
            running,
            handle: Some(handle),
            accumulated,
        }
    }

    /// Take the accumulated text, clearing the internal buffer.
    /// Returns the deduplicated concatenated text from all partial transcriptions.
    pub fn take_accumulated(&self) -> String {
        match self.accumulated.lock() {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(_) => String::new(),
        }
    }

    /// Signal the background loop to stop and detach the thread.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            drop(handle);
        }
    }

    /// Signal the background loop to stop and wait up to 300ms for the thread
    /// to finish before detaching.
    pub fn stop_and_wait(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
            while std::time::Instant::now() < deadline && !handle.is_finished() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if !handle.is_finished() {
                warn!("realtime transcriber thread did not exit within 300ms, detaching");
            }
            drop(handle);
        }
    }
}

impl Drop for RealtimeTranscriber {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_last_5s_from_buffer() {
        let samples = vec![1.0f32; 48_000 * 10];
        let samples_needed = 48_000 * 5;
        let start = samples.len().saturating_sub(samples_needed);
        let actual = samples[start..].to_vec();
        assert_eq!(actual.len(), samples_needed);
    }

    #[test]
    fn test_extract_short_buffer_returns_all() {
        let samples = vec![0.5f32; 1000];
        let samples_needed = 48_000 * 5;
        let start = samples.len().saturating_sub(samples_needed);
        let actual = samples[start..].to_vec();
        assert_eq!(actual.len(), 1000);
    }

    #[test]
    fn test_vad_rms_detects_speech() {
        let loud = vec![0.5f32; 16000];
        let rms = rms::calculate_rms(&loud);
        assert!(rms >= VAD_THRESHOLD);
    }

    #[test]
    fn test_vad_rms_detects_silence() {
        let silent = vec![0.0f32; 16000];
        let rms = rms::calculate_rms(&silent);
        assert!(rms < VAD_THRESHOLD);
    }

    #[test]
    fn test_has_speech_energy_pure_silence() {
        // All zeros → no speech energy
        let silent = vec![0.0f32; 80_000]; // 5 seconds at 16kHz
        assert!(!has_speech_energy(&silent));
    }

    #[test]
    fn test_has_speech_energy_ambient_noise() {
        // Low uniform noise below frame threshold (RMS ≈ 0.01) → no speech energy
        let noise = vec![0.01f32; 80_000];
        assert!(!has_speech_energy(&noise));
    }

    #[test]
    fn test_has_speech_energy_with_speech() {
        // 5 frames of high amplitude (speech-like) + rest silence → speech energy detected
        let mut audio = vec![0.0f32; 80_000];
        // Fill 5 frames (each 1600 samples) with high-amplitude signal
        for i in 0..5 {
            let start = i * ENERGY_FRAME_SAMPLES;
            for j in 0..ENERGY_FRAME_SAMPLES {
                audio[start + j] = 0.2; // RMS = 0.2, well above 0.04
            }
        }
        assert!(has_speech_energy(&audio));
    }

    #[test]
    fn test_has_speech_energy_too_few_frames() {
        // Only 4 high-energy frames (< ENERGY_MIN_FRAMES=5) → no speech energy
        let mut audio = vec![0.0f32; 80_000];
        for i in 0..4 {
            let start = i * ENERGY_FRAME_SAMPLES;
            for j in 0..ENERGY_FRAME_SAMPLES {
                audio[start + j] = 0.2;
            }
        }
        assert!(!has_speech_energy(&audio));
    }

    #[test]
    fn test_has_speech_energy_short_buffer() {
        // Buffer shorter than one frame → no speech energy
        let short = vec![0.5f32; 100];
        assert!(!has_speech_energy(&short));
    }

    #[test]
    fn test_accumulate_first_partial() {
        let result = accumulate("", "", "Hello world");
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn test_accumulate_overlapping() {
        // Typical sliding window: new result extends previous
        let result = accumulate(
            "Hello, my name is",
            "Hello, my name is",
            "Hello, my name is John",
        );
        assert_eq!(result, "Hello, my name is John");
    }

    #[test]
    fn test_accumulate_chinese_overlapping() {
        let result = accumulate("你好，我是", "你好，我是", "你好，我是小明");
        assert_eq!(result, "你好，我是小明");
    }

    #[test]
    fn test_accumulate_no_overlap() {
        // User paused and started a new topic
        let result = accumulate("Hello world", "Hello world", "Nice to meet you");
        assert_eq!(result, "Hello world Nice to meet you");
    }

    #[test]
    fn test_accumulate_punctuation_variation() {
        // Whisper adds comma in second window — overlap detected despite punctuation diff.
        // Accumulated text keeps first version's punctuation; only new content is appended.
        let result = accumulate("你好我是小明", "你好我是小明", "你好，我是小明今年二十岁");
        assert_eq!(result, "你好我是小明今年二十岁");
    }

    #[test]
    fn test_accumulate_punctuation_variation_english() {
        // Whisper adds commas in second window — overlap detected, first version's
        // punctuation preserved, new content appended.
        let result = accumulate(
            "Hello my name is John",
            "Hello my name is John",
            "Hello, my name is John, and I live in NYC",
        );
        assert_eq!(result, "Hello my name is John, and I live in NYC");
    }

    #[test]
    fn test_accumulate_chinese_multi_sentence() {
        let mut acc = String::new();
        let mut prev = String::new();

        // Window 1
        acc = accumulate(&acc, &prev, "你好，我是小明");
        prev = "你好，我是小明".to_string();

        // Window 2 (extends)
        acc = accumulate(&acc, &prev, "你好，我是小明，今年二十岁");
        prev = "你好，我是小明，今年二十岁".to_string();

        // Window 3 (extends)
        acc = accumulate(&acc, &prev, "我是小明，今年二十岁，住在上海");

        assert_eq!(acc, "你好，我是小明，今年二十岁，住在上海");
    }

    #[test]
    fn test_stop_clears_running_flag() {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let handle = thread::spawn(move || {
            while running_clone.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
            }
        });
        let mut rt = RealtimeTranscriber {
            running,
            handle: Some(handle),
            accumulated: Arc::new(Mutex::new(String::new())),
        };
        rt.stop();
        assert!(!rt.running.load(Ordering::Relaxed));
    }

    #[test]
    fn test_take_accumulated() {
        let rt = RealtimeTranscriber {
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
            accumulated: Arc::new(Mutex::new("test text".to_string())),
        };
        assert_eq!(rt.take_accumulated(), "test text");
        assert_eq!(rt.take_accumulated(), "");
    }
}
