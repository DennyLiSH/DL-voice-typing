use crate::audio::rms;
use crate::clipboard::ClipboardProvider;
use crate::config::{AppConfig, Language};
use crate::data_saving::SaveResult;
use crate::hotkey::{HotkeyCallback, HotkeyEvent};
use crate::llm::{AnyCorrector, LLMClient, TextCorrector};
use crate::perf::PerfMetrics;
use crate::speech::SpeechEngine;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager, Position};
use tracing::{debug, info, warn};

use super::pipeline_state::PipelineState;
use super::review::{PendingReview, ReviewData};

/// Check silence and resample audio to 16kHz for Whisper.
/// Returns `None` if audio is near-silent (hallucination guard).
fn preprocess_audio(audio: &[f32], native_rate: u32) -> Option<Vec<f32>> {
    let rms_val = rms::calculate_rms(audio);
    if rms_val < 0.01 {
        debug!("Silent audio (rms={rms_val:.4}), skipping transcription");
        return None;
    }
    Some(crate::data_saving::resample(
        audio,
        native_rate,
        crate::data_saving::TARGET_SAMPLE_RATE,
    ))
}

/// Run the full transcription → LLM → injection pipeline asynchronously.
async fn run_pipeline(
    ps: PipelineState,
    audio_for_save: Vec<f32>,
    native_rate: u32,
    resampled: Vec<f32>,
    mut perf: PerfMetrics,
    t_press_for_e2e: Instant,
) {
    let config = AppConfig::read_cached(&ps.config_cache).unwrap_or_default();

    // -- Save audio and transcribe in parallel --
    let (save_result, transcription) =
        transcribe_and_save(&ps, audio_for_save, native_rate, resampled, &config).await;

    perf.transcription_ms = perf
        .transcription_ms
        .or(Some(Instant::now().elapsed().as_millis() as u64));

    // Skip if transcription is empty (all segments filtered by no_speech probability).
    if transcription.is_empty() {
        debug!("Empty transcription (all segments filtered), skipping injection");
        reset_to_idle(&ps);
        return;
    }

    // -- LLM Correction (optional) --
    perf.llm_enabled = config.llm_enabled;
    let final_text = if config.llm_enabled {
        resolve_llm_text(&ps, &config, &transcription, &mut perf).await
    } else {
        transcription.clone()
    };

    // Normalize Chinese punctuation when surrounded by CJK.
    let final_text = if config.language == Language::Zh {
        normalize_chinese_punctuation(&final_text)
    } else {
        final_text
    };

    // -- Injection --
    if config.review_before_paste {
        deliver_review(&ps, final_text, transcription, save_result, &config).await;
        return;
    }

    deliver_direct(
        &ps,
        final_text,
        transcription,
        save_result,
        &config,
        &mut perf,
        t_press_for_e2e,
    )
    .await;
}

/// Parallel save audio to disk + transcribe via speech engine.
/// Returns (save_result, transcription_text).
/// On transcription failure, emits error and returns empty string.
async fn transcribe_and_save(
    ps: &PipelineState,
    audio_for_save: Vec<f32>,
    native_rate: u32,
    resampled: Vec<f32>,
    config: &AppConfig,
) -> (Option<SaveResult>, String) {
    let save_config = config.clone();
    let sr_for_save = native_rate;
    let save_handle = tokio::task::spawn_blocking(move || {
        if save_config.data_saving_enabled && !save_config.data_saving_path.is_empty() {
            crate::data_saving::save_audio(&audio_for_save, sr_for_save, &save_config).ok()
        } else {
            None
        }
    });

    let engine_ref = ps.engine.clone();
    let transcribe_handle =
        tokio::task::spawn_blocking(
            move || match crate::util::lock_mutex(&engine_ref, "engine") {
                Some(e) => e.transcribe_sync(&resampled),
                None => Err(crate::error::AppError::Speech(
                    "engine lock poisoned".to_string(),
                )),
            },
        );

    let (save_result, transcription_result) = tokio::join!(save_handle, transcribe_handle);
    let save_result = save_result.unwrap_or(None);

    let transcription = match transcription_result {
        Ok(Ok(text)) => {
            let _ = ps.app.emit("transcription-complete", &text);
            if let Some(mut s) = crate::util::lock_mutex(&ps.sm, "state_machine") {
                let _ = s.add_partial_result(text.clone());
            }
            text
        }
        Ok(Err(e)) => {
            let _ = ps.app.emit("speech-error", e.to_string());
            reset_to_idle(ps);
            return (save_result, String::new());
        }
        Err(e) => {
            let _ = ps.app.emit("speech-error", e.to_string());
            reset_to_idle(ps);
            return (save_result, String::new());
        }
    };

    (save_result, transcription)
}

/// Resolve LLM-corrected text. Handles cache lookup, client creation, and fallback.
async fn resolve_llm_text(
    ps: &PipelineState,
    config: &AppConfig,
    transcription: &str,
    perf: &mut PerfMetrics,
) -> String {
    if let Some(mut s) = crate::util::lock_mutex(&ps.sm, "state_machine") {
        let _ = s.start_llm_refining();
    }
    let _ = ps.app.emit("llm-refining", ());

    let t_llm = Instant::now();

    // Ensure the cached corrector matches config, creating a new one if needed.
    {
        let mut cached = crate::util::lock_mutex(&ps.cached_llm, "cached_llm")
            .expect("cached_llm lock poisoned");
        let needs_new = cached.as_ref().is_none_or(|c| {
            !c.matches_config(&config.llm_api_url, &config.llm_api_key, &config.llm_model)
        });
        if needs_new {
            *cached = Some(AnyCorrector::Live(LLMClient::new(
                config.llm_api_url.clone(),
                config.llm_api_key.clone(),
                config.llm_model.clone(),
            )));
        }
    }

    // Call correct_sync while re-acquiring the lock (holds lock for HTTP duration).
    let result = {
        let cached = crate::util::lock_mutex(&ps.cached_llm, "cached_llm")
            .expect("cached_llm lock poisoned");
        cached.as_ref().unwrap().correct_sync(transcription)
    };

    perf.llm_correction_ms = Some(t_llm.elapsed().as_millis() as u64);

    match result {
        Ok(corrected) => {
            let _ = ps.app.emit("llm-complete", &corrected);
            corrected
        }
        Err(e) => {
            let _ = ps.app.emit("llm-error", e.to_string());
            transcription.to_string()
        }
    }
}

/// Show review window with transcribed text for user editing.
async fn deliver_review(
    ps: &PipelineState,
    final_text: String,
    transcription: String,
    save_result: Option<SaveResult>,
    config: &AppConfig,
) {
    // Save clipboard before entering review state.
    if let Some(mut cb) = crate::util::lock_mutex(&ps.clipboard, "clipboard") {
        let _ = cb.save();
    }
    // Transition to Reviewing.
    if let Some(mut s) = crate::util::lock_mutex(&ps.sm, "state_machine") {
        if config.llm_enabled {
            let _ = s.llm_to_reviewing(final_text.clone());
        } else {
            let _ = s.transcribing_to_reviewing(final_text.clone());
        }
    }
    // Store text for the review window to fetch on load.
    if let Some(pending) = ps.app.try_state::<PendingReview>() {
        if let Some(mut guard) = crate::util::lock_mutex(&pending.text, "pending_text") {
            *guard = Some(final_text.clone());
            debug!("Review: stored pending text ({} chars)", final_text.len());
        }
    }

    // Show the pre-created review window near caret.
    let (cx, cy) = crate::win32::get_caret_screen_pos();
    let mut x = cx + 10.0;
    let mut y = cy + 20.0;
    let win_w = 420.0_f64;
    let win_h = 220.0_f64;
    if let Some((left, top, right, bottom)) =
        crate::win32::get_monitor_work_area(cx as i32, cy as i32)
    {
        x = x.min(right as f64 - win_w).max(left as f64);
        y = y.min(bottom as f64 - win_h).max(top as f64);
    }

    if let Some(win) = ps.app.get_webview_window("review") {
        if let Some(pending) = ps.app.try_state::<PendingReview>() {
            pending.save_foreground();
        }
        let _ = win.set_position(Position::Logical(tauri::LogicalPosition::new(x, y)));
        let _ = win.show();
        let _ = ps.app.emit("review-show", ());
        let _ = win.set_focus();
        debug!("Review: window shown, review-show emitted");

        // Store data-saving metadata for confirm/cancel to consume later.
        if let Some(sr) = save_result.as_ref() {
            if let Some(pending) = ps.app.try_state::<PendingReview>() {
                if let Some(mut guard) =
                    crate::util::lock_mutex(&pending.data_saving, "pending_data")
                {
                    *guard = Some(ReviewData {
                        json_path: sr.json_path.clone(),
                        raw_transcription: transcription,
                        llm_text: if config.llm_enabled {
                            Some(final_text)
                        } else {
                            None
                        },
                    });
                }
            }
        }
    } else {
        warn!("review window not found. Falling back to direct injection.");
        // Fallback: inject directly since review window is unavailable.
        let cb = ps.clipboard.clone();
        let text = final_text.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            if let Some(mut cb) = crate::util::lock_mutex(&cb, "clipboard") {
                let _ = cb.save();
                let _ = cb.inject_text(&text);
            }
        })
        .await;
        if let Some(mut s) = crate::util::lock_mutex(&ps.sm, "state_machine") {
            let _ = s.finish_injecting();
        }
        let _ = ps.app.emit("injection-complete", ());
        if let Some(win) = ps.app.get_webview_window("floating") {
            let _ = win.hide();
        }
    }
}

/// Direct clipboard injection path (review disabled).
async fn deliver_direct(
    ps: &PipelineState,
    final_text: String,
    transcription: String,
    save_result: Option<SaveResult>,
    config: &AppConfig,
    perf: &mut PerfMetrics,
    t_press_for_e2e: Instant,
) {
    if let Some(mut s) = crate::util::lock_mutex(&ps.sm, "state_machine") {
        if config.llm_enabled {
            let _ = s.llm_to_injecting(final_text.clone());
        } else {
            let _ = s.transcribing_to_injecting(final_text.clone());
        }
    }

    let t_inject = Instant::now();
    {
        let cb_for_inject = ps.clipboard.clone();
        let text_for_inject = final_text.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            let mut cb = cb_for_inject.lock().map_err(|e| e.to_string())?;
            cb.save().map_err(|e| e.to_string())?;
            cb.inject_text(&text_for_inject).map_err(|e| e.to_string())
        })
        .await
        {
            Ok(Err(e)) => {
                let _ = ps.app.emit("injection-error", e);
            }
            Err(e) => {
                let _ = ps.app.emit("injection-error", e.to_string());
            }
            _ => {}
        }
    }
    perf.injection_ms = Some(t_inject.elapsed().as_millis() as u64);
    perf.end_to_end_ms = Some(t_press_for_e2e.elapsed().as_millis() as u64);
    perf.text_length = final_text.len();

    if let Some(mut s) = crate::util::lock_mutex(&ps.sm, "state_machine") {
        let _ = s.finish_injecting();
    }
    let _ = ps.app.emit("injection-complete", ());

    if let Some(win) = ps.app.get_webview_window("floating") {
        let _ = win.hide();
    }

    // Update data_saving JSON with transcription.
    if let Some(sr) = save_result {
        let llm_text = if config.llm_enabled {
            Some(final_text.as_str())
        } else {
            None
        };
        let _ = crate::data_saving::update_json_with_text(
            &sr.json_path,
            &transcription,
            llm_text,
            Some(&final_text),
        );
    }

    ps.perf_history.record(perf.clone());
    let _ = ps.app.emit("perf-metrics", &perf);
    info!("{}", perf.summary());
}

/// Reset state machine to Idle and hide floating window.
fn reset_to_idle(ps: &PipelineState) {
    if let Some(mut s) = crate::util::lock_mutex(&ps.sm, "state_machine") {
        s.reset();
    }
    if let Some(win) = ps.app.get_webview_window("floating") {
        let _ = win.hide();
    }
}

/// Build the hotkey callback that starts/stops recording and runs the full pipeline.
pub(crate) fn make_hotkey_callback(ps: PipelineState) -> HotkeyCallback {
    // Shared perf state between Pressed and Released invocations of the same cycle.
    let perf_slot: Arc<Mutex<Option<PerfMetrics>>> = Arc::new(Mutex::new(None));

    Box::new(move |event| {
        match event {
            HotkeyEvent::Pressed => {
                let t_press = Instant::now();
                let cycle_id = ps.perf_history.next_cycle_id();

                let can_record = crate::util::lock_mutex(&ps.sm, "state_machine")
                    .map(|mut s| s.start_recording().is_ok())
                    .unwrap_or(false);

                if can_record {
                    let engine_ready = crate::util::lock_mutex(&ps.engine, "engine")
                        .map(|e| e.is_ready())
                        .unwrap_or(false);
                    if !engine_ready {
                        reset_to_idle(&ps);
                        let _ = ps.app.emit("speech-error", "模型加载中，请稍候...");
                        return;
                    }
                }

                if can_record {
                    // Show floating window near text caret.
                    if let Some(win) = ps.app.get_webview_window("floating") {
                        let (cx, cy) = crate::win32::get_caret_screen_pos();
                        let win_half = 90.0;
                        let offset = 40.0;
                        let x = cx - offset - win_half;
                        let y = cy - offset - win_half;
                        let _ =
                            win.set_position(Position::Logical(tauri::LogicalPosition::new(x, y)));
                        let _ = win.show();
                    }
                    let _ = ps.app.emit("recording-start", ());

                    // Start audio capture with RMS-emitting callback (~30 fps).
                    let last_rms_emit = Arc::new(Mutex::new(Instant::now()));
                    let config = AppConfig::read_cached(&ps.config_cache).unwrap_or_default();
                    if let Some(mut ac_guard) = crate::util::lock_mutex(&ps.ac, "audio_capture") {
                        let sm_for_audio = Arc::clone(&ps.sm);
                        let app_for_rms = ps.app.clone();
                        let last_rms_for_cb = Arc::clone(&last_rms_emit);
                        let _ = ac_guard.start(Box::new(move |data: &[f32]| {
                            if let Some(mut s) =
                                crate::util::lock_mutex(&sm_for_audio, "state_machine")
                            {
                                let _ = s.append_audio(data);
                            }
                            let rms_val = rms::calculate_rms(data);
                            if let Some(mut last) =
                                crate::util::lock_mutex(&last_rms_for_cb, "last_rms_emit")
                            {
                                if last.elapsed() >= Duration::from_millis(33) {
                                    *last = Instant::now();
                                    let _ = app_for_rms.emit("audio-rms", rms_val);
                                }
                            }
                        }));

                        // Start real-time transcription if enabled.
                        if config.realtime_transcription {
                            if let Some(sr) = ac_guard.sample_rate() {
                                let rt = crate::realtime::RealtimeTranscriber::start(
                                    ps.engine.clone(),
                                    ps.sm.clone(),
                                    ps.app.clone(),
                                    sr,
                                );
                                if let Some(mut rt_guard) = crate::util::lock_mutex(
                                    &ps.realtime_transcriber,
                                    "realtime_transcriber",
                                ) {
                                    *rt_guard = Some(rt);
                                }
                            }
                        }
                    }

                    let press_latency = t_press.elapsed().as_millis() as u64;
                    let mut perf = PerfMetrics::new(cycle_id);
                    perf.press_latency_ms = Some(press_latency);
                    if let Some(mut slot) = crate::util::lock_mutex(&perf_slot, "perf") {
                        *slot = Some(perf);
                    }
                }
            }
            HotkeyEvent::Released => {
                let t_release = Instant::now();

                let mut perf = crate::util::lock_mutex(&perf_slot, "perf")
                    .and_then(|mut s| s.take())
                    .unwrap_or_else(|| PerfMetrics::new(ps.perf_history.next_cycle_id()));

                perf.audio_duration_ms = Some(t_release.elapsed().as_millis() as u64);

                let sample_rate =
                    crate::util::lock_mutex(&ps.ac, "audio_capture").and_then(|a| a.sample_rate());
                if let Some(mut ac_guard) = crate::util::lock_mutex(&ps.ac, "audio_capture") {
                    ac_guard.stop();
                }

                // Stop real-time transcription.
                if let Some(mut rt_guard) =
                    crate::util::lock_mutex(&ps.realtime_transcriber, "realtime_transcriber")
                {
                    rt_guard.take();
                }

                if let Some(mut sm_guard) = crate::util::lock_mutex(&ps.sm, "state_machine") {
                    if let Ok(audio_data) = sm_guard.stop_recording() {
                        perf.release_latency_ms = Some(t_release.elapsed().as_millis() as u64);
                        perf.audio_samples = audio_data.len();
                        perf.audio_sample_rate = sample_rate.unwrap_or(48000);

                        let native_rate = sample_rate.unwrap_or(48000);
                        let resampled = match preprocess_audio(&audio_data, native_rate) {
                            Some(r) => r,
                            None => {
                                sm_guard.reset();
                                if let Some(win) = ps.app.get_webview_window("floating") {
                                    let _ = win.hide();
                                }
                                return;
                            }
                        };

                        let audio_for_save = audio_data;
                        let t_press_for_e2e =
                            t_release - Duration::from_millis(perf.audio_duration_ms.unwrap_or(0));

                        tauri::async_runtime::spawn(run_pipeline(
                            ps.clone(),
                            audio_for_save,
                            native_rate,
                            resampled,
                            perf,
                            t_press_for_e2e,
                        ));
                    }
                }
            }
        }
    })
}

/// Replace ASCII comma/period with full-width Chinese equivalents
/// when surrounded by CJK Unified Ideographs, or at end-of-string preceded by CJK.
fn normalize_chinese_punctuation(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len());

    fn is_cjk(ch: char) -> bool {
        matches!(ch, '\u{4E00}'..='\u{9FFF}')
    }

    for (i, &ch) in chars.iter().enumerate() {
        if ch == ',' || ch == '.' {
            let prev_cjk = i > 0 && is_cjk(chars[i - 1]);
            let next_cjk = i + 1 < chars.len() && is_cjk(chars[i + 1]);
            let at_end = i + 1 == chars.len();
            if prev_cjk && (next_cjk || at_end) {
                result.push(if ch == ',' { '，' } else { '。' });
                continue;
            }
        }
        result.push(ch);
    }
    result
}

#[cfg(test)]
mod tests_normalize_chinese_punctuation {
    use super::*;

    #[test]
    fn test_between_chinese() {
        assert_eq!(normalize_chinese_punctuation("你好,世界"), "你好，世界");
        assert_eq!(normalize_chinese_punctuation("今天.明天"), "今天。明天");
    }

    #[test]
    fn test_mixed_language_unchanged() {
        assert_eq!(normalize_chinese_punctuation("Hello,世界"), "Hello,世界");
        assert_eq!(normalize_chinese_punctuation("你好, world"), "你好, world");
    }

    #[test]
    fn test_decimal_unchanged() {
        assert_eq!(normalize_chinese_punctuation("版本3.5"), "版本3.5");
        assert_eq!(normalize_chinese_punctuation("3.14"), "3.14");
    }

    #[test]
    fn test_already_fullwidth_unchanged() {
        assert_eq!(normalize_chinese_punctuation("你好，世界"), "你好，世界");
        assert_eq!(normalize_chinese_punctuation("今天。明天"), "今天。明天");
    }

    #[test]
    fn test_multiple_replacements() {
        assert_eq!(
            normalize_chinese_punctuation("你好,今天天气不错.我们去玩吧"),
            "你好，今天天气不错。我们去玩吧"
        );
    }

    #[test]
    fn test_boundary_cases() {
        assert_eq!(normalize_chinese_punctuation(","), ",");
        assert_eq!(normalize_chinese_punctuation("你好."), "你好。");
        assert_eq!(normalize_chinese_punctuation("你好,"), "你好，");
        assert_eq!(normalize_chinese_punctuation(",你好"), ",你好");
    }

    #[test]
    fn test_end_of_string_not_cjk() {
        assert_eq!(normalize_chinese_punctuation("3."), "3.");
        assert_eq!(normalize_chinese_punctuation("hello."), "hello.");
    }

    #[test]
    fn test_preprocess_audio_silent() {
        let silent = vec![0.0f32; 4800];
        assert!(preprocess_audio(&silent, 48000).is_none());
    }

    #[test]
    fn test_preprocess_audio_loud() {
        let loud = vec![0.5f32; 4800];
        let result = preprocess_audio(&loud, 48000);
        assert!(result.is_some());
        // Resampled from 48000 to 16000 = 1/3 the samples.
        let resampled = result.unwrap();
        assert!(resampled.len() < 4800);
    }
}
