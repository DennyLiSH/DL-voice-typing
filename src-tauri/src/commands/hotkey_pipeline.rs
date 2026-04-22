use crate::audio::AudioCapture;
use crate::audio::rms;
use crate::config::{AppConfig, Language};
use crate::hotkey::{HotkeyCallback, HotkeyEvent};
use crate::llm::LLMClient;
use crate::perf::{PerfHistory, PerfMetrics};
use crate::speech::AnyEngine;
use crate::speech::SpeechEngine;
use crate::state::StateMachine;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager, Position};
use tracing::{debug, info, warn};

use super::review::{PendingReview, ReviewData};

/// Build the hotkey callback that starts/stops recording and runs the full pipeline.
#[allow(clippy::too_many_arguments)]
pub fn make_hotkey_callback(
    sm: Arc<Mutex<StateMachine>>,
    ac: Arc<Mutex<AudioCapture>>,
    engine: Arc<Mutex<AnyEngine>>,
    clipboard: Arc<Mutex<crate::clipboard::ClipboardManager>>,
    perf_history: Arc<PerfHistory>,
    app: tauri::AppHandle,
    config_cache: crate::config::ConfigCache,
    cached_llm: Arc<Mutex<Option<LLMClient>>>,
) -> HotkeyCallback {
    // Shared perf state between Pressed and Released invocations of the same cycle.
    // Pressed creates it, Released takes it and moves into the async task.
    let perf_slot: Arc<Mutex<Option<PerfMetrics>>> = Arc::new(Mutex::new(None));

    Box::new(move |event| {
        match event {
            HotkeyEvent::Pressed => {
                let t_press = Instant::now();
                let cycle_id = perf_history.next_cycle_id();

                let can_record = crate::util::lock_mutex(&sm, "state_machine")
                    .map(|mut s| s.start_recording().is_ok())
                    .unwrap_or(false);

                // Block recording if the model is still loading in the background.
                if can_record {
                    let engine_ready = crate::util::lock_mutex(&engine, "engine")
                        .map(|e| e.is_ready())
                        .unwrap_or(false);
                    if !engine_ready {
                        if let Some(mut s) = crate::util::lock_mutex(&sm, "state_machine") {
                            s.reset();
                        }
                        let _ = app.emit("speech-error", "模型加载中，请稍候...");
                        return;
                    }
                }
                if can_record {
                    // Show floating window near text caret (upper-left 45°).
                    if let Some(win) = app.get_webview_window("floating") {
                        let (cx, cy) = crate::win32::get_caret_screen_pos();
                        // Window is 180x180, indicator is centered.
                        // Place indicator center ~40px upper-left of caret.
                        let win_half = 90.0;
                        let offset = 40.0;
                        let x = cx - offset - win_half;
                        let y = cy - offset - win_half;
                        let _ =
                            win.set_position(Position::Logical(tauri::LogicalPosition::new(x, y)));
                        let _ = win.show();
                    }
                    let _ = app.emit("recording-start", ());

                    // Start audio capture with RMS-emitting callback (throttled to ~30 fps).
                    let last_rms_emit = Arc::new(Mutex::new(Instant::now()));
                    if let Some(mut ac_guard) = crate::util::lock_mutex(&ac, "audio_capture") {
                        let sm_for_audio = Arc::clone(&sm);
                        let app_for_rms = app.clone();
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
                    }

                    // Store perf metrics with press timing.
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

                // Take the perf metrics created by Pressed.
                let mut perf = crate::util::lock_mutex(&perf_slot, "perf")
                    .and_then(|mut s| s.take())
                    .unwrap_or_else(|| PerfMetrics::new(perf_history.next_cycle_id()));

                perf.audio_duration_ms = Some(t_release.elapsed().as_millis() as u64);

                // Read sample rate BEFORE stop (stop clears config).
                let sample_rate =
                    crate::util::lock_mutex(&ac, "audio_capture").and_then(|a| a.sample_rate());
                if let Some(mut ac_guard) = crate::util::lock_mutex(&ac, "audio_capture") {
                    ac_guard.stop();
                }
                if let Some(mut sm_guard) = crate::util::lock_mutex(&sm, "state_machine") {
                    if let Ok(audio_data) = sm_guard.stop_recording() {
                        perf.release_latency_ms = Some(t_release.elapsed().as_millis() as u64);
                        perf.audio_samples = audio_data.len();
                        perf.audio_sample_rate = sample_rate.unwrap_or(48000);

                        // Skip transcription if audio is near-silent (hallucination guard).
                        let rms = rms::calculate_rms(&audio_data);
                        if rms < 0.01 {
                            debug!("Silent audio (rms={rms:.4}), skipping transcription");
                            sm_guard.reset();
                            if let Some(win) = app.get_webview_window("floating") {
                                let _ = win.hide();
                            }
                            return;
                        }

                        // Resample to 16kHz for Whisper.
                        let native_rate = sample_rate.unwrap_or(48000);
                        let resampled = crate::data_saving::resample(
                            &audio_data,
                            native_rate,
                            crate::data_saving::TARGET_SAMPLE_RATE,
                        );

                        // Capture state for async task.
                        let sr_for_save = native_rate;
                        let audio_for_save = audio_data; // Move ownership into async task

                        // Clone references for the async task.
                        let engine_clone = engine.clone();
                        let sm_clone = sm.clone();
                        let cb_clone = clipboard.clone();
                        let app_clone = app.clone();
                        let ph_clone = perf_history.clone();
                        let cached_llm_clone = cached_llm.clone();
                        let cc_clone = config_cache.clone();
                        let t_press_for_e2e =
                            t_release - Duration::from_millis(perf.audio_duration_ms.unwrap_or(0));

                        tauri::async_runtime::spawn(async move {
                            // Read config once for all subsequent operations.
                            let config = AppConfig::read_cached(&cc_clone).unwrap_or_default();

                            // -- Save audio and transcribe in parallel --
                            let save_config = config.clone();
                            let save_handle = tokio::task::spawn_blocking(move || {
                                if save_config.data_saving_enabled
                                    && !save_config.data_saving_path.is_empty()
                                {
                                    crate::data_saving::save_audio(
                                        &audio_for_save,
                                        sr_for_save,
                                        &save_config,
                                    )
                                    .ok()
                                } else {
                                    None
                                }
                            });

                            let engine_ref = engine_clone.clone();
                            let transcribe_handle =
                                tokio::task::spawn_blocking(move || match crate::util::lock_mutex(
                                    &engine_ref,
                                    "engine",
                                ) {
                                    Some(e) => e.transcribe_sync(&resampled),
                                    None => Err(crate::error::AppError::Speech(
                                        "engine lock poisoned".to_string(),
                                    )),
                                });

                            let t_transcribe = Instant::now();
                            let (save_result, transcription_result) =
                                tokio::join!(save_handle, transcribe_handle);
                            let save_result = save_result.unwrap_or(None);

                            let transcription = match transcription_result {
                                Ok(Ok(text)) => {
                                    let _ = app_clone.emit("transcription-complete", &text);
                                    {
                                        if let Some(mut s) =
                                            crate::util::lock_mutex(&sm_clone, "state_machine")
                                        {
                                            let _ = s.add_partial_result(text.clone());
                                        }
                                    }
                                    text
                                }
                                Ok(Err(e)) => {
                                    let _ = app_clone.emit("speech-error", e.to_string());
                                    if let Some(mut s) =
                                        crate::util::lock_mutex(&sm_clone, "state_machine")
                                    {
                                        s.reset();
                                    }
                                    if let Some(win) = app_clone.get_webview_window("floating") {
                                        let _ = win.hide();
                                    }
                                    return;
                                }
                                Err(e) => {
                                    let _ = app_clone.emit("speech-error", e.to_string());
                                    if let Some(mut s) =
                                        crate::util::lock_mutex(&sm_clone, "state_machine")
                                    {
                                        s.reset();
                                    }
                                    if let Some(win) = app_clone.get_webview_window("floating") {
                                        let _ = win.hide();
                                    }
                                    return;
                                }
                            };
                            perf.transcription_ms = Some(t_transcribe.elapsed().as_millis() as u64);

                            // Skip LLM + injection if transcription is empty (all segments
                            // filtered by no_speech probability).
                            if transcription.is_empty() {
                                debug!(
                                    "Empty transcription (all segments filtered), skipping injection"
                                );
                                if let Some(mut s) =
                                    crate::util::lock_mutex(&sm_clone, "state_machine")
                                {
                                    s.reset();
                                }
                                if let Some(win) = app_clone.get_webview_window("floating") {
                                    let _ = win.hide();
                                }
                                return;
                            }

                            // -- LLM Correction (optional) --
                            perf.llm_enabled = config.llm_enabled;
                            let final_text = if config.llm_enabled {
                                if let Some(mut s) =
                                    crate::util::lock_mutex(&sm_clone, "state_machine")
                                {
                                    let _ = s.start_llm_refining(transcription.clone());
                                }
                                let _ = app_clone.emit("llm-refining", ());

                                let t_llm = Instant::now();
                                let llm = {
                                    let mut cached =
                                        crate::util::lock_mutex(&cached_llm_clone, "cached_llm")
                                            .expect("cached_llm lock poisoned");
                                    let needs_new = cached.as_ref().is_none_or(|c| {
                                        !c.matches_config(
                                            &config.llm_api_url,
                                            &config.llm_api_key,
                                            &config.llm_model,
                                        )
                                    });
                                    if needs_new {
                                        let new_client = LLMClient::new(
                                            config.llm_api_url.clone(),
                                            config.llm_api_key.clone(),
                                            config.llm_model.clone(),
                                        );
                                        *cached = Some(new_client.clone());
                                        new_client
                                    } else {
                                        cached.as_ref().unwrap().clone()
                                    }
                                };
                                let result = match llm.correct(&transcription).await {
                                    Ok(corrected) => {
                                        let _ = app_clone.emit("llm-complete", &corrected);
                                        corrected
                                    }
                                    Err(e) => {
                                        let _ = app_clone.emit("llm-error", e.to_string());
                                        transcription.clone()
                                    }
                                };
                                perf.llm_correction_ms = Some(t_llm.elapsed().as_millis() as u64);
                                result
                            } else {
                                transcription.clone()
                            };

                            // Normalize Chinese punctuation (half-width → full-width) when surrounded by CJK.
                            let final_text = if config.language == Language::Zh {
                                normalize_chinese_punctuation(&final_text)
                            } else {
                                final_text
                            };

                            // -- Injection --
                            if config.review_before_paste {
                                // Save clipboard before entering review state.
                                if let Some(mut cb) =
                                    crate::util::lock_mutex(&cb_clone, "clipboard")
                                {
                                    let _ = cb.save();
                                }
                                // Transition to Reviewing.
                                if let Some(mut s) =
                                    crate::util::lock_mutex(&sm_clone, "state_machine")
                                {
                                    if config.llm_enabled {
                                        let _ = s.llm_to_reviewing(final_text.clone());
                                    } else {
                                        let _ = s.transcribing_to_reviewing(final_text.clone());
                                    }
                                }
                                // Store text for the review window to fetch on load.
                                if let Some(pending) = app_clone.try_state::<PendingReview>() {
                                    if let Some(mut guard) =
                                        crate::util::lock_mutex(&pending.text, "pending_text")
                                    {
                                        *guard = Some(final_text.clone());
                                        debug!(
                                            "Review: stored pending text ({} chars)",
                                            final_text.len()
                                        );
                                    }
                                }

                                // Show the pre-created review window near caret.
                                let (cx, cy) = crate::win32::get_caret_screen_pos();
                                let mut x = cx + 10.0;
                                let mut y = cy + 20.0;
                                let win_w = 420.0_f64;
                                let win_h = 220.0_f64;
                                // Clamp to the monitor the caret is on (works with multi-monitor).
                                if let Some((left, top, right, bottom)) =
                                    crate::win32::get_monitor_work_area(cx as i32, cy as i32)
                                {
                                    x = x.min(right as f64 - win_w).max(left as f64);
                                    y = y.min(bottom as f64 - win_h).max(top as f64);
                                }

                                if let Some(win) = app_clone.get_webview_window("review") {
                                    // Save foreground HWND before showing review window,
                                    // so we can restore focus after confirm/cancel.
                                    if let Some(pending) = app_clone.try_state::<PendingReview>() {
                                        pending.save_foreground();
                                    }
                                    let _ = win.set_position(Position::Logical(
                                        tauri::LogicalPosition::new(x, y),
                                    ));
                                    let _ = win.show();
                                    let _ = app_clone.emit("review-show", ());
                                    let _ = win.set_focus();
                                    debug!("Review: window shown, review-show emitted");

                                    // Store data-saving metadata for confirm/cancel to consume later.
                                    if let Some(sr) = save_result.as_ref() {
                                        if let Some(pending) =
                                            app_clone.try_state::<PendingReview>()
                                        {
                                            if let Some(mut guard) = crate::util::lock_mutex(
                                                &pending.data_saving,
                                                "pending_data",
                                            ) {
                                                *guard = Some(ReviewData {
                                                    json_path: sr.json_path.clone(),
                                                    raw_transcription: transcription.clone(),
                                                    llm_text: if config.llm_enabled {
                                                        Some(final_text.clone())
                                                    } else {
                                                        None
                                                    },
                                                });
                                            }
                                        }
                                    }
                                } else {
                                    warn!(
                                        "review window not found. Falling back to direct injection."
                                    );
                                    if let Some(mut s) =
                                        crate::util::lock_mutex(&sm_clone, "state_machine")
                                    {
                                        s.reset();
                                    }
                                    let cb_fb = cb_clone.clone();
                                    let text_fb = final_text.clone();
                                    let _ = tauri::async_runtime::spawn_blocking(move || {
                                        if let Some(mut cb) =
                                            crate::util::lock_mutex(&cb_fb, "clipboard")
                                        {
                                            let _ = cb.save();
                                            let _ = cb.inject_text(&text_fb);
                                        }
                                    })
                                    .await;
                                    if let Some(mut s) =
                                        crate::util::lock_mutex(&sm_clone, "state_machine")
                                    {
                                        let _ = s.finish_injecting();
                                    }
                                    let _ = app_clone.emit("injection-complete", ());
                                    if let Some(win) = app_clone.get_webview_window("floating") {
                                        let _ = win.hide();
                                    }
                                }

                                return; // Stop here — wait for confirm_inject or cancel_review.
                            }

                            // Direct injection path (review disabled).
                            if let Some(mut s) = crate::util::lock_mutex(&sm_clone, "state_machine")
                            {
                                if config.llm_enabled {
                                    let _ = s.llm_to_injecting(final_text.clone());
                                } else {
                                    let _ = s.transcribing_to_injecting(final_text.clone());
                                }
                            }

                            let t_inject = Instant::now();
                            {
                                let cb_for_inject = cb_clone.clone();
                                let text_for_inject = final_text.clone();
                                match tauri::async_runtime::spawn_blocking(move || {
                                    let mut cb = cb_for_inject.lock().map_err(|e| e.to_string())?;
                                    cb.save().map_err(|e| e.to_string())?;
                                    cb.inject_text(&text_for_inject).map_err(|e| e.to_string())
                                })
                                .await
                                {
                                    Ok(Err(e)) => {
                                        let _ = app_clone.emit("injection-error", e);
                                    }
                                    Err(e) => {
                                        let _ = app_clone.emit("injection-error", e.to_string());
                                    }
                                    _ => {}
                                }
                            }
                            perf.injection_ms = Some(t_inject.elapsed().as_millis() as u64);
                            perf.end_to_end_ms = Some(t_press_for_e2e.elapsed().as_millis() as u64);
                            perf.text_length = final_text.len();

                            if let Some(mut s) = crate::util::lock_mutex(&sm_clone, "state_machine")
                            {
                                let _ = s.finish_injecting();
                            }
                            let _ = app_clone.emit("injection-complete", ());

                            // Hide floating window.
                            if let Some(win) = app_clone.get_webview_window("floating") {
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

                            // Record and report performance metrics.
                            ph_clone.record(perf.clone());
                            let _ = app_clone.emit("perf-metrics", &perf);
                            info!("{}", perf.summary());
                        });
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
}
