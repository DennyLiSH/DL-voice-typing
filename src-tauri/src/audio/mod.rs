pub mod rms;

use crate::error::AppError;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::sync::mpsc;

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
                eprintln!("audio capture error: {}", err);
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
}
