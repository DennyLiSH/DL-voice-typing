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
            let _span = tracing::info_span!("realtime_transcriber").entered();
            while running_clone.load(Ordering::Relaxed) {
                // Only clone the last WINDOW_SECS of audio to minimize
                // lock hold time and memory usage (max ~240K floats vs ~2.88M).
                let window = {
                    let samples_needed = (sample_rate * WINDOW_SECS) as usize;
                    match state_machine.lock() {
                        Ok(sm) => match sm.get_audio_buffer() {
                            Some(buf) if !buf.is_empty() => {
                                let start = buf.len().saturating_sub(samples_needed);
                                buf[start..].to_vec()
                            }
                            Some(_) => {
                                thread::sleep(Duration::from_millis(STEP_MS));
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

                if rms_val < VAD_THRESHOLD {
                    debug!("realtime VAD: silent (rms={rms_val:.4}), skipping");
                    thread::sleep(Duration::from_millis(STEP_MS));
                    continue;
                }

                let text = {
                    let t_lock = std::time::Instant::now();
                    let guard = match engine.lock() {
                        Ok(g) => g,
                        Err(_) => {
                            warn!("engine lock poisoned, exiting realtime loop");
                            break;
                        }
                    };
                    let wait_ms = t_lock.elapsed().as_millis();
                    if wait_ms > 100 {
                        warn!("realtime: engine lock wait {wait_ms}ms");
                    }
                    match guard.transcribe_sync(&resampled) {
                        Ok(t) => t,
                        Err(err) => {
                            warn!("realtime transcription error: {err}");
                            thread::sleep(Duration::from_millis(STEP_MS));
                            continue;
                        }
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

    /// Signal the background loop to stop and detach the thread.
    ///
    /// The thread will exit on its next iteration after seeing `running=false`.
    /// We detach instead of join to avoid blocking the caller (which may be
    /// the Windows keyboard hook thread — a blocking join there can cause
    /// Windows to remove the hook).
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            // Drop without joining — the thread checks `running` each loop
            // iteration and exits promptly after transcription completes.
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
        let samples = vec![1.0f32; 48_000 * 10]; // 10 seconds at 48kHz
        let samples_needed = 48_000 * 5;
        let start = samples.len().saturating_sub(samples_needed);
        let actual = samples[start..].to_vec();
        assert_eq!(actual.len(), samples_needed);
        assert!(actual.iter().all(|&v| v == 1.0));
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
        assert!(rms >= VAD_THRESHOLD, "rms={rms} should be above threshold");
    }

    #[test]
    fn test_vad_rms_detects_silence() {
        let silent = vec![0.0f32; 16000];
        let rms = rms::calculate_rms(&silent);
        assert!(rms < VAD_THRESHOLD, "rms={rms} should be below threshold");
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
        };
        rt.stop();
        assert!(!rt.running.load(Ordering::Relaxed));
        assert!(rt.handle.is_none());
    }

    #[test]
    fn test_drop_signals_stop() {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let handle = thread::spawn(move || {
            while running_clone.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
            }
        });
        let rt = RealtimeTranscriber {
            running,
            handle: Some(handle),
        };
        drop(rt);
        // Thread should exit — if this hangs, the drop didn't signal stop.
    }
}
