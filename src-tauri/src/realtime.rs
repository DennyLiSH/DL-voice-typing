/// Real-time transcription using a sliding window approach.
///
/// While the user holds the hotkey, a background thread periodically
/// extracts the last N seconds of audio, transcribes it, and emits
/// partial results to the frontend.
use crate::audio::rms;
use crate::speech::{AnyEngine, SpeechEngine};
use crate::state::StateMachine;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tauri::Emitter;
use tracing::{debug, warn};

/// Interval between transcription attempts (milliseconds).
const STEP_MS: u64 = 800;

/// Audio window size for each transcription (seconds).
const WINDOW_SECS: u32 = 5;

/// RMS threshold below which audio is considered silent.
const VAD_THRESHOLD: f32 = 0.01;

/// Target sample rate for Whisper (16 kHz).
const TARGET_SAMPLE_RATE: u32 = 16_000;

pub struct RealtimeTranscriber {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl RealtimeTranscriber {
    /// Start the background transcription loop.
    ///
    /// The loop runs in a dedicated thread and periodically:
    /// 1. Reads the current audio buffer from the state machine.
    /// 2. Extracts the last `WINDOW_SECS` of audio.
    /// 3. Resamples to 16 kHz.
    /// 4. Skips if VAD (RMS) indicates silence.
    /// 5. Transcribes via the speech engine.
    /// 6. Emits the result via `transcription-partial` event.
    pub fn start(
        engine: Arc<Mutex<AnyEngine>>,
        state_machine: Arc<Mutex<StateMachine>>,
        app: tauri::AppHandle,
        sample_rate: u32,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let handle = thread::spawn(move || {
            while running_clone.load(Ordering::Relaxed) {
                let audio = match state_machine.lock() {
                    Ok(sm) => sm.get_audio_buffer().map(|buf| buf.to_vec()),
                    Err(_) => {
                        warn!("state machine lock poisoned, exiting realtime loop");
                        break;
                    }
                };

                let Some(audio) = audio else {
                    break;
                };

                if audio.is_empty() {
                    thread::sleep(Duration::from_millis(STEP_MS));
                    continue;
                }

                let samples_needed = (sample_rate * WINDOW_SECS) as usize;
                let window = if audio.len() >= samples_needed {
                    audio[audio.len() - samples_needed..].to_vec()
                } else {
                    audio
                };

                let resampled =
                    crate::data_saving::resample(&window, sample_rate, TARGET_SAMPLE_RATE);
                let rms_val = rms::calculate_rms(&resampled);

                if rms_val < VAD_THRESHOLD {
                    debug!("realtime VAD: silent (rms={rms_val:.4}), skipping");
                    thread::sleep(Duration::from_millis(STEP_MS));
                    continue;
                }

                let text = match engine.lock() {
                    Ok(e) => match e.transcribe_sync(&resampled) {
                        Ok(t) => t,
                        Err(err) => {
                            warn!("realtime transcription error: {err}");
                            thread::sleep(Duration::from_millis(STEP_MS));
                            continue;
                        }
                    },
                    Err(_) => {
                        warn!("engine lock poisoned, exiting realtime loop");
                        break;
                    }
                };

                if !text.is_empty() {
                    let _ = app.emit("transcription-partial", &text);
                }

                thread::sleep(Duration::from_millis(STEP_MS));
            }

            debug!("realtime transcriber loop exited");
        });

        Self {
            running,
            handle: Some(handle),
        }
    }

    /// Signal the background loop to stop and wait for it to finish.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
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
        let samples = vec![1.0f32; 48_000 * 10]; // 10 seconds at 48kHz
        let expected_len = 48_000 * 5; // last 5 seconds
        let actual = if samples.len() >= expected_len {
            samples[samples.len() - expected_len..].to_vec()
        } else {
            samples.clone()
        };
        assert_eq!(actual.len(), expected_len);
        assert!(actual.iter().all(|&v| v == 1.0));
    }

    #[test]
    fn test_extract_short_buffer_returns_all() {
        let samples = vec![0.5f32; 1000];
        let expected_len = 48_000 * 5;
        let actual = if samples.len() >= expected_len {
            samples[samples.len() - expected_len..].to_vec()
        } else {
            samples.clone()
        };
        assert_eq!(actual.len(), 1000);
    }

    #[test]
    fn test_vad_rms_detects_speech() {
        let loud = vec![0.5f32; 16000];
        let rms = rms::calculate_rms(&loud);
        assert!(rms >= VAD_THRESHOLD, "rms={rms} should be above threshold");
    }

    #[test]
    fn test_vad_rms_detects_silence() {
        let silent = vec![0.0f32; 16000];
        let rms = rms::calculate_rms(&silent);
        assert!(rms < VAD_THRESHOLD, "rms={rms} should be below threshold");
    }
}
