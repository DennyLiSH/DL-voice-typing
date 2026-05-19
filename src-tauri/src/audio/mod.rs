pub mod rms;

use crate::error::AppError;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::sync::mpsc;
use tracing::error;

/// Target sample rate for Whisper (16 kHz).
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Stateful linear-interpolation resampler with internal buffer reuse.
///
/// Hot paths (e.g. the realtime transcription loop) should create one
/// `Resampler` and call `process()` each cycle to avoid repeated allocations.
/// Cold paths can use the [`resample`] convenience function.
pub struct Resampler {
    from_rate: u32,
    to_rate: u32,
    output_buf: Vec<f32>,
}

impl Resampler {
    /// Create a new resampler for the given rate conversion.
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        Self {
            from_rate,
            to_rate,
            output_buf: Vec::new(),
        }
    }

    /// Resample `input` and return a slice pointing to the internal buffer.
    ///
    /// The returned slice is only valid until the next call to `process` or
    /// `reset`. Clone it if you need to keep it longer.
    pub fn process(&mut self, input: &[f32]) -> &[f32] {
        if input.is_empty() {
            return &[];
        }
        if self.from_rate == self.to_rate {
            self.output_buf.clear();
            self.output_buf.extend_from_slice(input);
            return &self.output_buf;
        }
        let ratio = self.from_rate as f64 / self.to_rate as f64;
        let output_len = ((input.len() as f64) / ratio).round() as usize;
        self.output_buf.clear();
        self.output_buf.reserve(output_len);
        for i in 0..output_len {
            let src_pos = i as f64 * ratio;
            let src_idx = src_pos as usize;
            let frac = src_pos - src_idx as f64;
            let s0 = input[src_idx];
            let s1 = if src_idx + 1 < input.len() {
                input[src_idx + 1]
            } else {
                s0
            };
            self.output_buf
                .push((s0 as f64 + frac * (s1 as f64 - s0 as f64)) as f32);
        }
        &self.output_buf
    }

    /// Reset the internal state (does not deallocate the buffer).
    pub fn reset(&mut self) {
        self.output_buf.clear();
    }
}

/// One-shot convenience wrapper around [`Resampler`].
///
/// Creates a temporary `Resampler`, processes the samples, and returns an
/// owned `Vec`. Suitable for cold paths; hot paths should reuse a `Resampler`.
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    let mut r = Resampler::new(from_rate, to_rate);
    r.process(samples).to_vec()
}

/// Callback type for audio data: receives a slice of f32 samples.
pub type AudioCallback = Box<dyn Fn(&[f32]) + Send>;

/// Trait for audio capture, enabling test seams.
pub trait AudioCaptureProvider: Send {
    fn start(&mut self, on_data: AudioCallback) -> Result<(), AppError>;
    fn stop(&mut self);
    fn is_capturing(&self) -> bool;
    fn sample_rate(&self) -> Option<u32>;
}

/// Audio capture using cpal.
pub struct AudioCapture {
    stream: Option<Stream>,
    config: Option<StreamConfig>,
}

impl AudioCapture {
    pub fn new() -> Self {
        Self {
            stream: None,
            config: None,
        }
    }

    /// Start capturing with a channel-based callback.
    /// Returns a receiver that yields audio chunks.
    pub fn start_channel(&mut self) -> Result<mpsc::Receiver<Vec<f32>>, AppError> {
        let (tx, rx) = mpsc::channel::<Vec<f32>>();
        self.start(Box::new(move |data: &[f32]| {
            let _ = tx.send(data.to_vec());
        }))?;
        Ok(rx)
    }
}

impl AudioCaptureProvider for AudioCapture {
    fn start(&mut self, on_data: AudioCallback) -> Result<(), AppError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| AppError::Audio("no input device available".to_string()))?;

        let supported_config = device
            .supported_input_configs()?
            .find(|c| c.sample_format() == SampleFormat::F32)
            .or_else(|| {
                // Fallback: any available config
                device.supported_input_configs().ok()?.next()
            })
            .ok_or_else(|| AppError::Audio("no supported audio config".to_string()))?;

        let config = supported_config.with_max_sample_rate().config();

        self.config = Some(config.clone());

        let stream = device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                on_data(data);
            },
            |err| {
                error!("audio capture error: {err}");
            },
            None,
        )?;

        stream.play()?;
        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) {
        self.stream = None;
        self.config = None;
    }

    fn is_capturing(&self) -> bool {
        self.stream.is_some()
    }

    fn sample_rate(&self) -> Option<u32> {
        self.config.as_ref().map(|c| c.sample_rate.0)
    }
}

impl Default for AudioCapture {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: cpal::Stream is not Send/Sync due to platform-specific internals,
// but AudioCapture is only ever accessed through `Arc<Mutex<AudioCapture>>`.
// The Mutex guarantees exclusive access — Stream is started/stopped exclusively
// within the Mutex guard scope. See lib.rs for the Arc<Mutex<>> wrapper.
unsafe impl Send for AudioCapture {}
unsafe impl Sync for AudioCapture {}

/// Ring buffer for audio samples with fixed capacity.
/// Used to decouple audio data from the state machine, eliminating
/// lock contention between cpal callback, realtime thread, and hotkey release.
pub struct AudioRingBuffer {
    buffer: Vec<f32>,
    write_pos: usize,
}

impl AudioRingBuffer {
    /// Create a new ring buffer with the given capacity (in samples).
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: vec![0.0; capacity],
            write_pos: 0,
        }
    }

    /// Capacity in samples.
    pub fn capacity(&self) -> usize {
        self.buffer.len()
    }

    /// Push samples into the ring buffer. Overwrites oldest data when full.
    pub fn push(&mut self, samples: &[f32]) {
        for (i, &sample) in samples.iter().enumerate() {
            let idx = (self.write_pos + i) % self.buffer.len();
            self.buffer[idx] = sample;
        }
        self.write_pos += samples.len();
    }

    /// Total samples ever written (may exceed capacity).
    pub fn total_written(&self) -> usize {
        self.write_pos
    }

    /// Current logical length (capped at capacity).
    pub fn len(&self) -> usize {
        self.write_pos.min(self.buffer.len())
    }

    pub fn is_empty(&self) -> bool {
        self.write_pos == 0
    }

    /// Clear all data.
    pub fn clear(&mut self) {
        self.write_pos = 0;
    }

    /// Copy the most recent `max_samples` into a new Vec.
    pub fn snapshot_recent(&self, max_samples: usize) -> Vec<f32> {
        let available = self.len();
        let to_take = max_samples.min(available);
        let mut result = Vec::with_capacity(to_take);
        let start = self.write_pos.saturating_sub(to_take);
        for i in 0..to_take {
            let idx = (start + i) % self.buffer.len();
            result.push(self.buffer[idx]);
        }
        result
    }

    /// Take all samples as a contiguous Vec and clear the buffer.
    pub fn take_all(&mut self) -> Vec<f32> {
        let len = self.len();
        let mut result = Vec::with_capacity(len);
        let start = self.write_pos.saturating_sub(len);
        for i in 0..len {
            let idx = (start + i) % self.buffer.len();
            result.push(self.buffer[idx]);
        }
        self.write_pos = 0;
        result
    }
}

/// Mock audio capture for testing.
pub struct MockAudioCapture {
    capturing: bool,
    sample_rate_val: Option<u32>,
}

impl MockAudioCapture {
    pub fn new() -> Self {
        Self {
            capturing: false,
            sample_rate_val: None,
        }
    }
}

impl AudioCaptureProvider for MockAudioCapture {
    fn start(&mut self, _on_data: AudioCallback) -> Result<(), AppError> {
        self.capturing = true;
        self.sample_rate_val = Some(48000);
        Ok(())
    }

    fn stop(&mut self) {
        self.capturing = false;
        self.sample_rate_val = None;
    }

    fn is_capturing(&self) -> bool {
        self.capturing
    }

    fn sample_rate(&self) -> Option<u32> {
        self.sample_rate_val
    }
}

impl Default for MockAudioCapture {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_capture() {
        let capture = AudioCapture::new();
        assert!(!capture.is_capturing());
        assert!(capture.sample_rate().is_none());
    }

    #[test]
    fn test_stop_without_start() {
        let mut capture = AudioCapture::new();
        capture.stop(); // Should not panic
        assert!(!capture.is_capturing());
    }

    #[test]
    fn test_audio_capture_is_send_sync_via_mutex() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<std::sync::Mutex<AudioCapture>>();
    }

    #[test]
    fn test_resample_48k_to_16k() {
        let samples: Vec<f32> = (0..48_000).map(|i| i as f32).collect();
        let resampled = resample(&samples, 48_000, 16_000);
        assert_eq!(resampled.len(), 16_000);
    }

    #[test]
    fn test_resample_identity() {
        let samples = vec![1.0f32, 2.0, 3.0, 4.0];
        let resampled = resample(&samples, 16_000, 16_000);
        assert_eq!(resampled, samples);
    }

    #[test]
    fn test_mock_capture_lifecycle() {
        let mut cap = MockAudioCapture::new();
        assert!(!cap.is_capturing());
        assert!(cap.sample_rate().is_none());

        cap.start(Box::new(|_| {})).unwrap();
        assert!(cap.is_capturing());
        assert_eq!(cap.sample_rate(), Some(48000));

        cap.stop();
        assert!(!cap.is_capturing());
        assert!(cap.sample_rate().is_none());
    }

    #[test]
    fn test_mock_capture_default() {
        let cap = MockAudioCapture::default();
        assert!(!cap.is_capturing());
    }

    // -----------------------------------------------------------------------
    // AudioRingBuffer tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_ring_buffer_empty() {
        let mut rb = AudioRingBuffer::new(100);
        assert!(rb.is_empty());
        assert_eq!(rb.len(), 0);
        assert_eq!(rb.snapshot_recent(10), Vec::<f32>::new());
        assert_eq!(rb.take_all(), Vec::<f32>::new());
    }

    #[test]
    fn test_ring_buffer_push_and_take() {
        let mut rb = AudioRingBuffer::new(100);
        rb.push(&[1.0, 2.0, 3.0]);
        assert_eq!(rb.len(), 3);
        assert_eq!(rb.take_all(), vec![1.0, 2.0, 3.0]);
        assert!(rb.is_empty());
    }

    #[test]
    fn test_ring_buffer_snapshot_recent() {
        let mut rb = AudioRingBuffer::new(100);
        rb.push(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(rb.snapshot_recent(3), vec![3.0, 4.0, 5.0]);
        assert_eq!(rb.snapshot_recent(10), vec![1.0, 2.0, 3.0, 4.0, 5.0]);
        // Buffer unchanged after snapshot.
        assert_eq!(rb.len(), 5);
    }

    #[test]
    fn test_ring_buffer_wraparound() {
        let mut rb = AudioRingBuffer::new(4);
        rb.push(&[1.0, 2.0, 3.0, 4.0]);
        rb.push(&[5.0, 6.0]); // Overwrites 1.0, 2.0
        assert_eq!(rb.len(), 4);
        assert_eq!(rb.take_all(), vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn test_ring_buffer_wraparound_snapshot() {
        let mut rb = AudioRingBuffer::new(4);
        rb.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        // Buffer now contains [5.0, 6.0, 7.0, 4.0] physically,
        // but logically [4.0, 5.0, 6.0, 7.0].
        assert_eq!(rb.snapshot_recent(4), vec![4.0, 5.0, 6.0, 7.0]);
        assert_eq!(rb.snapshot_recent(2), vec![6.0, 7.0]);
    }

    #[test]
    fn test_ring_buffer_clear() {
        let mut rb = AudioRingBuffer::new(100);
        rb.push(&[1.0, 2.0, 3.0]);
        rb.clear();
        assert!(rb.is_empty());
        assert_eq!(rb.take_all(), Vec::<f32>::new());
    }

    #[test]
    fn test_ring_buffer_capacity_unchanged() {
        let mut rb = AudioRingBuffer::new(100);
        rb.push(&[1.0; 50]);
        assert_eq!(rb.capacity(), 100);
        rb.take_all();
        assert_eq!(rb.capacity(), 100);
    }

    // -----------------------------------------------------------------------
    // Resampler tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resampler_identity() {
        let mut r = Resampler::new(16_000, 16_000);
        let samples = vec![1.0f32, 2.0, 3.0, 4.0];
        let out = r.process(&samples);
        assert_eq!(out, &samples);
    }

    #[test]
    fn test_resampler_48k_to_16k() {
        let mut r = Resampler::new(48_000, 16_000);
        let samples: Vec<f32> = (0..48_000).map(|i| i as f32).collect();
        let out = r.process(&samples);
        assert_eq!(out.len(), 16_000);
    }

    #[test]
    fn test_resampler_buffer_reuse() {
        let mut r = Resampler::new(48_000, 16_000);
        let first = r.process(&[0.5f32; 48_000]);
        let first_cap = first.len();
        let second = r.process(&[0.3f32; 48_000]);
        // Same capacity reused, same output length.
        assert_eq!(second.len(), first_cap);
    }

    #[test]
    fn test_resampler_empty_input() {
        let mut r = Resampler::new(48_000, 16_000);
        assert!(r.process(&[]).is_empty());
    }

    #[test]
    fn test_resampler_reset() {
        let mut r = Resampler::new(48_000, 16_000);
        let first = r.process(&[0.5f32; 1000]).to_vec();
        assert!(!first.is_empty());
        r.reset();
        let second = r.process(&[0.3f32; 500]).to_vec();
        assert_eq!(second.len(), 167); // 500 / 3 ≈ 166.67 → rounds to 167
    }
}
