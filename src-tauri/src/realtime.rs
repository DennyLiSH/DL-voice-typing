/// Real-time transcription using a sliding window approach.
///
/// While the user holds the hotkey, a background thread periodically
/// extracts the last N seconds of audio, transcribes it, and emits
/// accumulated partial results to the frontend.
///
/// Text accumulation: consecutive sliding windows overlap by ~90%.
/// Each new transcription is diffed against the previous one to extract
/// only the new content, which is appended to a running accumulated string.
use crate::audio::{Resampler, TARGET_SAMPLE_RATE, rms};
use crate::speech::SpeechEngine;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tracing::{debug, warn};

/// Abstract source of audio samples for real-time transcription.
/// Decouples the transcriber from the concrete state machine.
pub trait AudioSource: Send + Sync {
    /// Returns the most recent `max_samples` audio samples, or `None` if
    /// the source is unavailable (e.g. not recording).
    fn get_recent_samples(&self, max_samples: usize) -> Option<Vec<f32>>;
}

/// Adapter: exposes `AudioRingBuffer` as an `AudioSource`.
/// Eliminates StateMachine lock contention on the realtime path.
pub struct AudioRingBufferSource {
    buffer: Arc<Mutex<crate::audio::AudioRingBuffer>>,
}

impl AudioRingBufferSource {
    pub fn new(buffer: Arc<Mutex<crate::audio::AudioRingBuffer>>) -> Self {
        Self { buffer }
    }
}

impl AudioSource for AudioRingBufferSource {
    fn get_recent_samples(&self, max_samples: usize) -> Option<Vec<f32>> {
        let buf = self.buffer.lock().ok()?;
        if buf.is_empty() {
            return Some(Vec::new());
        }
        Some(buf.snapshot_recent(max_samples))
    }
}

/// Sleep for `total_ms`, but wake early if `running` becomes false.
fn sleep_or_stop(running: &AtomicBool, total_ms: u64, stop_poll_ms: u64) {
    let steps = total_ms.div_ceil(stop_poll_ms);
    for _ in 0..steps {
        if !running.load(Ordering::Relaxed) {
            return;
        }
        thread::sleep(Duration::from_millis(stop_poll_ms));
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

/// Minimum overlap ratio to consider two consecutive partials as continuous speech.
/// Below this threshold, we treat the new partial as a fresh segment.
const MIN_OVERLAP_RATIO: f32 = 0.5;

/// Edit distance tolerance for fuzzy overlap matching (10% = 1 edit per 10 chars).
const EDIT_DISTANCE_RATIO: f32 = 0.1;

/// Fuzzy matching only tries the top-N longest candidates (performance guard).
const FUZZY_TOP_N: usize = 3;

/// Frame size for speech energy detection (100ms at 16kHz = 1600 samples).
const ENERGY_FRAME_SAMPLES: usize = 1600;

/// Per-frame RMS threshold for speech energy detection.
const ENERGY_FRAME_THRESHOLD: f32 = 0.04;

/// Minimum number of high-energy frames required to consider audio as containing speech.
const ENERGY_MIN_FRAMES: usize = 5;

/// Tunable configuration for the realtime transcription loop.
///
/// Groups loop-policy (timing/window) and VAD-tuning constants into a single
/// struct so the loop body reads as `config.step_ms` etc. instead of bare
/// module constants. Values are sourced from the module-level `const`s by
/// `DEFAULT` to keep a single source of truth.
///
/// Text-overlap constants (`MIN_OVERLAP_RATIO` / `EDIT_DISTANCE_RATIO` /
/// `FUZZY_TOP_N`) are intentionally NOT in this struct — they are intrinsic
/// to `TextAccumulator`'s algorithm and stay as module-level `const`.
pub(crate) struct RealtimeLoopConfig {
    // Loop-policy fields wired up in T2 (loop body migration). `#[expect]`
    // marks them as intentionally-unread at the T1 introduction boundary so
    // clippy does not fire `dead_code` for the partial rollout. Once T2 routes
    // the loop body through `config.*`, drop the attributes.
    #[expect(dead_code, reason = "wired up in T2 loop-body migration")]
    pub step_ms: u64,
    #[expect(dead_code, reason = "wired up in T2 loop-body migration")]
    pub short_step_ms: u64,
    #[expect(dead_code, reason = "wired up in T2 loop-body migration")]
    pub stop_poll_ms: u64,
    #[expect(dead_code, reason = "wired up in T2 loop-body migration")]
    pub window_secs: u32,
    #[expect(dead_code, reason = "wired up in T2 loop-body migration")]
    pub vad_threshold: f32,
    pub energy_frame_samples: usize,
    pub energy_frame_threshold: f32,
    pub energy_min_frames: usize,
}

impl RealtimeLoopConfig {
    pub(crate) const DEFAULT: Self = Self {
        step_ms: STEP_MS,
        short_step_ms: SHORT_STEP_MS,
        stop_poll_ms: STOP_POLL_MS,
        window_secs: WINDOW_SECS,
        vad_threshold: VAD_THRESHOLD,
        energy_frame_samples: ENERGY_FRAME_SAMPLES,
        energy_frame_threshold: ENERGY_FRAME_THRESHOLD,
        energy_min_frames: ENERGY_MIN_FRAMES,
    };
}

/// Check whether the audio contains sustained speech energy.
/// Splits the audio into 100ms frames and requires at least `ENERGY_MIN_FRAMES` frames
/// to exceed `ENERGY_FRAME_THRESHOLD` RMS. This distinguishes real speech (clear energy
/// peaks from syllables) from ambient noise (uniform low energy).
fn has_speech_energy(resampled: &[f32], config: &RealtimeLoopConfig) -> bool {
    if resampled.len() < config.energy_frame_samples {
        return false;
    }
    let mut high_energy_frames = 0;
    for frame in resampled.chunks(config.energy_frame_samples) {
        if frame.len() < config.energy_frame_samples {
            break;
        }
        let frame_rms = rms::calculate_rms(frame);
        if frame_rms > config.energy_frame_threshold {
            high_energy_frames += 1;
        }
    }
    high_energy_frames >= config.energy_min_frames
}

/// Outcome of pulling one audio window from the source.
enum WindowPull {
    /// Non-empty window ready for processing.
    Window(Vec<f32>),
    /// Source returned an empty buffer (still recording but no samples yet).
    Empty,
    /// Source returned None (e.g., recording stopped). Caller should terminate.
    SourceGone,
}

/// Pull the most recent audio window from the source.
///
/// Encapsulates the three-way match on `get_recent_samples` so the loop body
/// reads as `match pull_window(...) { Window(w) => ..., Empty => ..., SourceGone => break }`.
fn pull_window(audio: &dyn AudioSource, samples_needed: usize) -> WindowPull {
    match audio.get_recent_samples(samples_needed) {
        Some(buf) if !buf.is_empty() => WindowPull::Window(buf),
        Some(_) => WindowPull::Empty,
        None => WindowPull::SourceGone,
    }
}

/// Outcome of reading accumulated context text under the mutex.
enum ContextRead {
    /// Accumulator held non-empty confirmed text — use as Whisper initial_prompt.
    Context(String),
    /// Accumulator was empty (first iteration or after take()).
    Empty,
    /// Mutex poisoned — caller should terminate loop.
    Poisoned,
}

/// Read the accumulated context text for use as Whisper initial_prompt.
///
/// Extracts the lock + empty-check logic so the loop body does not embed
/// lock handling inline. Poison path maps to ContextRead::Poisoned which
/// the caller converts to `break` (preserving original `Err(_) => break`).
fn read_context(accumulator: &Mutex<TextAccumulator>) -> ContextRead {
    match accumulator.lock() {
        Ok(guard) => {
            let t = guard.text();
            if t.is_empty() {
                ContextRead::Empty
            } else {
                ContextRead::Context(t.to_string())
            }
        }
        Err(_) => ContextRead::Poisoned,
    }
}

/// Outcome of pushing new text and emitting the partial event.
enum PushOutcome {
    /// Push + emit completed normally.
    Ok,
    /// Accumulator mutex poisoned — caller should terminate loop.
    Poisoned,
}

/// Push transcribed text into the accumulator and emit the partial event.
///
/// Combines the lock-push-text-emit sequence so the loop body reads as a
/// single named call. Late-event guard (running check before emit) preserved
/// to avoid late events after stop signal. Poison path returns PushOutcome::Poisoned
/// which the caller converts to `break` (preserving original `Err(_) => break`).
fn push_and_emit(
    text: &str,
    accumulator: &Mutex<TextAccumulator>,
    emitter: &dyn crate::commands::EventEmitter,
    running: &AtomicBool,
) -> PushOutcome {
    let new_accumulated = match accumulator.lock() {
        Ok(mut guard) => {
            guard.push(text);
            guard.text().to_string()
        }
        Err(_) => return PushOutcome::Poisoned,
    };

    if running.load(Ordering::Relaxed) {
        emitter.emit(
            "transcription-partial",
            serde_json::Value::String(new_accumulated),
        );
    }
    PushOutcome::Ok
}

/// Background transcriber that runs a sliding-window loop on a dedicated thread.
///
/// Owns the thread handle, stop flag, and incremental text accumulator.
/// Call [`start`](Self::start) to spawn the loop, [`take_accumulated`](Self::take_accumulated)
/// to collect results, and [`stop`](Self::stop) / [`stop_and_wait`](Self::stop_and_wait) to shut down.
pub struct RealtimeTranscriber {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    /// Incremental text accumulator with buffer reuse for overlap detection.
    accumulated: Arc<Mutex<TextAccumulator>>,
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

/// Incremental text accumulator with buffer reuse for real-time transcription overlap detection.
/// Maintains internal state across pushes to avoid repeated allocations and re-parsing.
pub struct TextAccumulator {
    /// Confirmed text that will no longer change.
    confirmed: String,
    /// Content characters (punctuation-stripped) of the last partial, reused via scratch buffer.
    last_content: Vec<char>,
    /// Raw text of the last partial (preserves original formatting).
    last_raw: String,
    /// Scratch buffer for parsing new partial content without allocation.
    scratch: Vec<char>,
}

impl TextAccumulator {
    pub fn new() -> Self {
        Self {
            confirmed: String::new(),
            last_content: Vec::new(),
            last_raw: String::new(),
            scratch: Vec::new(),
        }
    }

    /// Push a new partial transcription and return the updated accumulated text.
    pub fn push(&mut self, new_partial: &str) -> &str {
        if self.confirmed.is_empty() {
            self.confirmed = new_partial.to_string();
            self.last_raw = new_partial.to_string();
            self.update_last_content(new_partial);
            return &self.confirmed;
        }

        if new_partial.is_empty() {
            return &self.confirmed;
        }

        // Parse new partial into scratch buffer (reuses capacity).
        self.scratch.clear();
        self.scratch
            .extend(new_partial.chars().filter(|c| !is_punct(*c)));

        let overlap_len = find_longest_suffix_prefix(&self.last_content, &self.scratch);

        if overlap_len > 0 {
            let byte_offset = content_char_offset(new_partial, overlap_len);
            self.confirmed.push_str(&new_partial[byte_offset..]);
            self.last_raw = new_partial.to_string();
            std::mem::swap(&mut self.last_content, &mut self.scratch);
        } else {
            // No overlap: Whisper re-transcribed differently. Don't append to prevent explosion.
            self.last_raw = new_partial.to_string();
            std::mem::swap(&mut self.last_content, &mut self.scratch);
        }

        &self.confirmed
    }

    /// Get the current accumulated text.
    pub fn text(&self) -> &str {
        &self.confirmed
    }

    /// Take the accumulated text, clearing the internal state.
    pub fn take(&mut self) -> String {
        self.last_content.clear();
        self.last_raw.clear();
        self.scratch.clear();
        std::mem::take(&mut self.confirmed)
    }

    /// Clear all state.
    pub fn clear(&mut self) {
        self.confirmed.clear();
        self.last_content.clear();
        self.last_raw.clear();
        self.scratch.clear();
    }

    fn update_last_content(&mut self, raw: &str) {
        self.last_content.clear();
        self.last_content
            .extend(raw.chars().filter(|c| !is_punct(*c)));
    }
}

impl Default for TextAccumulator {
    fn default() -> Self {
        Self::new()
    }
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

/// Compute Levenshtein edit distance between two char slices.
/// Returns early with `usize::MAX` if the distance exceeds `max_dist`.
fn bounded_edit_distance(a: &[char], b: &[char], max_dist: usize) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev = vec![0usize; b.len() + 1];
    let mut curr = vec![0usize; b.len() + 1];
    for (j, val) in prev.iter_mut().enumerate() {
        *val = j;
    }
    for i in 1..=a.len() {
        curr[0] = i;
        let mut row_min = curr[0];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
            row_min = row_min.min(curr[j]);
        }
        if row_min > max_dist {
            return usize::MAX;
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Find the longest suffix of `a` that matches a prefix of `b`.
/// Returns the match length in characters (not bytes).
/// Both slices are already punctuation-stripped content characters.
///
/// Two-phase matching: first tries exact match (fast), then fuzzy match
/// with edit distance tolerance (handles homophone drift).
fn find_longest_suffix_prefix(a: &[char], b: &[char]) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }

    let min_overlap = ((a.len() as f32) * MIN_OVERLAP_RATIO) as usize;
    let max_check = a.len().min(b.len());

    // Phase 1: exact match (fast path).
    for len in (min_overlap.max(1)..=max_check).rev() {
        if a[a.len() - len..] == b[..len] {
            return len;
        }
    }

    // Phase 2: fuzzy match — only try top-N longest candidates.
    for len in (min_overlap.max(1)..=max_check).rev().take(FUZZY_TOP_N) {
        let max_edits = ((len as f32) * EDIT_DISTANCE_RATIO).max(1.0) as usize;
        let dist = bounded_edit_distance(&a[a.len() - len..], &b[..len], max_edits);
        if dist <= max_edits {
            return len;
        }
    }

    0
}

/// Accumulate a new partial transcription into the running text.
/// Uses suffix-matching to detect sliding-window overlap and extract only new content.
///
/// This is a stateless convenience wrapper around `TextAccumulator` for testing.
/// The hot path (`RealtimeTranscriber`) uses `TextAccumulator` directly for buffer reuse.
#[cfg(test)]
fn accumulate(accumulated: &str, prev_partial: &str, new_partial: &str) -> String {
    let mut acc = TextAccumulator {
        confirmed: accumulated.to_string(),
        last_raw: prev_partial.to_string(),
        last_content: prev_partial.chars().filter(|c| !is_punct(*c)).collect(),
        scratch: Vec::new(),
    };
    acc.push(new_partial);
    acc.take()
}

impl RealtimeTranscriber {
    /// Start the background transcription loop.
    ///
    /// The `engine` parameter uses `Arc<dyn SpeechEngine>` trait-object dispatch,
    /// consistent with the project's other trait-object abstractions
    /// (`ClipboardProvider`, `TextCorrector`, `AudioCaptureProvider`). Tests use `MockEngine::new()`.
    pub fn start(
        audio: Arc<dyn AudioSource + Send + Sync>,
        engine: Arc<dyn SpeechEngine>,
        emitter: Arc<dyn crate::commands::EventEmitter + Send + Sync>,
        sample_rate: u32,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let accumulated: Arc<Mutex<TextAccumulator>> = Arc::new(Mutex::new(TextAccumulator::new()));
        let accumulated_clone = accumulated.clone();

        let handle = thread::spawn(move || {
            let _span = tracing::info_span!("realtime_transcriber").entered();
            let mut resampler = Resampler::new(sample_rate, TARGET_SAMPLE_RATE);
            let config = RealtimeLoopConfig::DEFAULT;

            while running_clone.load(Ordering::Relaxed) {
                let samples_needed = (sample_rate * WINDOW_SECS) as usize;
                let window = match pull_window(audio.as_ref(), samples_needed) {
                    WindowPull::Window(w) => w,
                    WindowPull::Empty => {
                        thread::sleep(Duration::from_millis(SHORT_STEP_MS));
                        continue;
                    }
                    WindowPull::SourceGone => break,
                };

                let resampled = resampler.process(&window);
                let rms_val = rms::calculate_rms(resampled);

                let speech_energy = has_speech_energy(resampled, &config);
                debug!(
                    "realtime VAD: rms={rms_val:.4} energy={speech_energy} samples={}",
                    resampled.len()
                );
                if rms_val < VAD_THRESHOLD || !speech_energy {
                    debug!("realtime VAD: silent, skipping");
                    sleep_or_stop(&running_clone, STEP_MS, STOP_POLL_MS);
                    continue;
                }

                let ctx_owned: Option<String> = match read_context(&accumulated_clone) {
                    ContextRead::Context(s) => Some(s),
                    ContextRead::Empty => None,
                    ContextRead::Poisoned => break,
                };

                let text =
                    match engine.transcribe_sync_with_context(resampled, ctx_owned.as_deref()) {
                        Ok(t) => t,
                        Err(err) => {
                            warn!("realtime transcription error: {err}");
                            sleep_or_stop(&running_clone, STEP_MS, STOP_POLL_MS);
                            continue;
                        }
                    };

                debug!("realtime transcription: {text:?}");

                if !text.is_empty() {
                    match push_and_emit(&text, &accumulated_clone, emitter.as_ref(), &running_clone)
                    {
                        PushOutcome::Ok => {}
                        PushOutcome::Poisoned => break,
                    }
                }

                sleep_or_stop(&running_clone, STEP_MS, STOP_POLL_MS);
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
            Ok(mut guard) => guard.take(),
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
    use crate::speech::mock::MockEngine;

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
        assert!(!has_speech_energy(&silent, &RealtimeLoopConfig::DEFAULT));
    }

    #[test]
    fn test_has_speech_energy_ambient_noise() {
        // Low uniform noise below frame threshold (RMS ≈ 0.01) → no speech energy
        let noise = vec![0.01f32; 80_000];
        assert!(!has_speech_energy(&noise, &RealtimeLoopConfig::DEFAULT));
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
        assert!(has_speech_energy(&audio, &RealtimeLoopConfig::DEFAULT));
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
        assert!(!has_speech_energy(&audio, &RealtimeLoopConfig::DEFAULT));
    }

    #[test]
    fn test_has_speech_energy_short_buffer() {
        // Buffer shorter than one frame → no speech energy
        let short = vec![0.5f32; 100];
        assert!(!has_speech_energy(&short, &RealtimeLoopConfig::DEFAULT));
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
        // Whisper re-transcribed same audio differently (homophone variation).
        // Should NOT append — return accumulated as-is to prevent text explosion.
        let result = accumulate("Hello world", "Hello world", "Nice to meet you");
        assert_eq!(result, "Hello world");
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
            accumulated: Arc::new(Mutex::new(TextAccumulator::new())),
        };
        rt.stop();
        assert!(!rt.running.load(Ordering::Relaxed));
    }

    #[test]
    fn test_take_accumulated() {
        let mut acc = TextAccumulator::new();
        acc.push("test text");
        let rt = RealtimeTranscriber {
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
            accumulated: Arc::new(Mutex::new(acc)),
        };
        assert_eq!(rt.take_accumulated(), "test text");
        assert_eq!(rt.take_accumulated(), "");
    }

    // -----------------------------------------------------------------------
    // Mock implementations for testing the realtime thread loop
    // -----------------------------------------------------------------------

    struct MockAudioSource {
        samples: Vec<f32>,
    }

    impl MockAudioSource {
        fn new(samples: Vec<f32>) -> Self {
            Self { samples }
        }
    }

    impl AudioSource for MockAudioSource {
        fn get_recent_samples(&self, _max_samples: usize) -> Option<Vec<f32>> {
            Some(self.samples.clone())
        }
    }

    struct MockEventEmitter {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl MockEventEmitter {
        fn new() -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl crate::commands::EventEmitter for MockEventEmitter {
        fn emit(&self, event: &str, payload: serde_json::Value) {
            // Only capture partial-transcription events with string payloads.
            if event == "transcription-partial" {
                if let serde_json::Value::String(text) = payload {
                    if let Ok(mut events) = self.events.lock() {
                        events.push(text);
                    }
                }
            }
        }
    }

    /// Build loud audio (5s @ 16kHz) that passes both RMS and frame-based VAD.
    fn loud_audio_5s() -> Vec<f32> {
        vec![0.3f32; 16_000 * 5]
    }

    #[test]
    fn test_realtime_loop_emits_partial_events() {
        let audio = Arc::new(MockAudioSource::new(loud_audio_5s()));
        let engine = Arc::new(MockEngine::new("Hello world"));
        let emitter = Arc::new(MockEventEmitter::new());

        let mut rt = RealtimeTranscriber::start(audio, engine, emitter.clone(), 16_000);

        // Wait long enough for one transcription cycle (STEP_MS=500 + processing).
        thread::sleep(Duration::from_millis(800));
        rt.stop_and_wait();

        let last = emitter.events.lock().map_or(String::new(), |events| {
            events.last().cloned().unwrap_or_default()
        });
        assert!(!last.is_empty(), "expected at least one partial event");
        assert_eq!(last, "Hello world");
    }

    #[test]
    fn test_realtime_loop_silent_audio_no_events() {
        // All zeros → RMS = 0, below VAD_THRESHOLD → no transcription
        let audio = Arc::new(MockAudioSource::new(vec![0.0f32; 16_000 * 5]));
        let engine = Arc::new(MockEngine::new("should not emit"));
        let emitter = Arc::new(MockEventEmitter::new());

        let mut rt = RealtimeTranscriber::start(audio, engine, emitter.clone(), 16_000);

        thread::sleep(Duration::from_millis(800));
        rt.stop_and_wait();

        let is_empty = emitter
            .events
            .lock()
            .map_or(true, |events| events.is_empty());
        assert!(is_empty, "silent audio should not produce events");
    }

    #[test]
    fn test_realtime_loop_accumulates_multiple_partials() {
        // Engine returns incrementally longer text on each call.
        // We simulate this by using a single response; the accumulate logic
        // deduplicates identical consecutive partials, so we only see one event.
        let audio = Arc::new(MockAudioSource::new(loud_audio_5s()));
        let engine = Arc::new(MockEngine::new("First"));
        let emitter = Arc::new(MockEventEmitter::new());

        let mut rt = RealtimeTranscriber::start(audio, engine, emitter.clone(), 16_000);

        // Two cycles → same text emitted twice (accumulate dedups identical).
        thread::sleep(Duration::from_millis(1_200));
        rt.stop_and_wait();

        let events = emitter
            .events
            .lock()
            .map_or(Vec::new(), |events| events.clone());
        // First cycle emits "First", second cycle sees overlap and emits "First" again
        // because the engine always returns the same text.
        assert!(!events.is_empty());
        assert!(events.iter().all(|e| e == "First"));
    }

    // -----------------------------------------------------------------------
    // Fuzzy overlap detection tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_fuzzy_overlap_single_homophone() {
        // Chinese: 他 → 她 (single character difference)
        let result = accumulate("你好他是小明", "你好他是小明", "你好她是小明今年二十岁");
        assert!(
            result.contains("今年二十岁"),
            "expected fuzzy overlap to detect 他/她 difference, got: {result}"
        );
    }

    #[test]
    fn test_fuzzy_overlap_english_minor_diff() {
        // Suffix-prefix model: old="the cat", new="the bat sat down"
        // suffix of old "thecat" vs prefix of new "thebat": 1 edit (c→b), within tolerance.
        let result = accumulate("the cat", "the cat", "the bat sat down");
        assert!(
            result.contains("sat down"),
            "expected fuzzy overlap to detect cat/bat difference, got: {result}"
        );
    }

    #[test]
    fn test_fuzzy_overlap_within_tolerance() {
        // Exactly 1 edit in 10 chars = 10% = within EDIT_DISTANCE_RATIO
        // "ABCDEFGHIJ" → "ABCXEFGHIJ" (1 substitution)
        let result = accumulate(
            "ABCDEFGHIJ hello",
            "ABCDEFGHIJ hello",
            "ABCXEFGHIJ hello world",
        );
        assert!(
            result.contains("world"),
            "expected fuzzy overlap within tolerance, got: {result}"
        );
    }

    #[test]
    fn test_fuzzy_overlap_exceeds_tolerance() {
        // 3 edits in 10 chars = 30% = exceeds EDIT_DISTANCE_RATIO (10%)
        // "ABCDEFGHIJ" → "XXXDEFGHIJ" (3 substitutions)
        let result = accumulate(
            "ABCDEFGHIJ hello",
            "ABCDEFGHIJ hello",
            "XXXDEFGHIJ hello world",
        );
        assert_eq!(
            result, "ABCDEFGHIJ hello",
            "expected freeze when edit distance exceeds tolerance, got: {result}"
        );
    }

    #[test]
    fn test_bounded_edit_distance_identical() {
        let a: Vec<char> = "hello".chars().collect();
        let b: Vec<char> = "hello".chars().collect();
        assert_eq!(bounded_edit_distance(&a, &b, 0), 0);
    }

    #[test]
    fn test_bounded_edit_distance_one_sub() {
        let a: Vec<char> = "hallo".chars().collect();
        let b: Vec<char> = "hello".chars().collect();
        assert_eq!(bounded_edit_distance(&a, &b, 1), 1);
    }

    #[test]
    fn test_bounded_edit_distance_early_exit() {
        let a: Vec<char> = "abcdefghij".chars().collect();
        let b: Vec<char> = "xxxxxxxxxx".chars().collect();
        // 10 edits, max_dist=1 → should return usize::MAX via early exit
        assert_eq!(bounded_edit_distance(&a, &b, 1), usize::MAX);
    }

    #[test]
    fn test_bounded_edit_distance_empty() {
        let a: Vec<char> = "".chars().collect();
        let b: Vec<char> = "abc".chars().collect();
        assert_eq!(bounded_edit_distance(&a, &b, 10), 3);
        assert_eq!(bounded_edit_distance(&b, &a, 10), 3);
    }

    // -----------------------------------------------------------------------
    // last_n_chars tests
    // -----------------------------------------------------------------------

    #[cfg(feature = "whisper")]
    mod last_n_chars_tests {
        #[test]
        fn test_last_n_chars_shorter_than_n() {
            let result = crate::speech::whisper::WhisperEngine::last_n_chars("你好", 50);
            assert_eq!(result, "你好");
        }

        #[test]
        fn test_last_n_chars_truncation() {
            let result =
                crate::speech::whisper::WhisperEngine::last_n_chars("你好我是小明今年二十岁", 5);
            assert_eq!(result, "今年二十岁");
        }

        #[test]
        fn test_last_n_chars_exact() {
            let result = crate::speech::whisper::WhisperEngine::last_n_chars("一二三四五", 5);
            assert_eq!(result, "一二三四五");
        }
    }

    // -----------------------------------------------------------------------
    // transcribe_sync_with_context tests (via MockEngine)
    // -----------------------------------------------------------------------

    #[test]
    fn test_transcribe_with_context_mock_ignores_context() {
        let engine = MockEngine::new("hello");
        let result = engine.transcribe_sync_with_context(&[0.5f32; 100], Some("ignored context"));
        assert_eq!(result.map_or(String::new(), |s| s), "hello");
    }

    #[test]
    fn test_transcribe_with_context_none_works() {
        let engine = MockEngine::new("test");
        let result = engine.transcribe_sync_with_context(&[0.5f32; 100], None);
        assert_eq!(result.map_or(String::new(), |s| s), "test");
    }
}
