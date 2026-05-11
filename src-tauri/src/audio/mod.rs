pub mod rms;

use crate::error::AppError;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::sync::mpsc;
use tracing::error;

/// Target sample rate for Whisper (16 kHz).
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Linear interpolation resampling.
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = ((samples.len() as f64) / ratio).round() as usize;
    let mut output = Vec::with_capacity(output_len);
    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let src_idx = src_pos as usize;
        let frac = src_pos - src_idx as f64;
        let s0 = samples[src_idx];
        let s1 = if src_idx + 1 < samples.len() {
            samples[src_idx + 1]
        } else {
            s0
        };
        output.push((s0 as f64 + frac * (s1 as f64 - s0 as f64)) as f32);
    }
    output
}

/// Callback type for audio data: receives a slice of f32 samples.
pub type AudioCallback = Box<dyn Fn(&[f32]) + Send>;

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

    /// Start capturing audio from the default input device.
    /// Calls `on_data` for each chunk of audio samples.
    pub fn start(&mut self, on_data: AudioCallback) -> Result<(), AppError> {
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

    /// Start capturing with a channel-based callback.
    /// Returns a receiver that yields audio chunks.
    pub fn start_channel(&mut self) -> Result<mpsc::Receiver<Vec<f32>>, AppError> {
        let (tx, rx) = mpsc::channel::<Vec<f32>>();
        self.start(Box::new(move |data: &[f32]| {
            let _ = tx.send(data.to_vec());
        }))?;
        Ok(rx)
    }

    /// Stop capturing audio.
    pub fn stop(&mut self) {
        self.stream = None;
        self.config = None;
    }

    /// Check if currently capturing.
    pub fn is_capturing(&self) -> bool {
        self.stream.is_some()
    }

    /// Get the sample rate if capturing.
    pub fn sample_rate(&self) -> Option<u32> {
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
}
