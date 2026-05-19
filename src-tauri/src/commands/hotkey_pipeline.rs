use crate::audio::{TARGET_SAMPLE_RATE, resample, rms};
use crate::clipboard::ClipboardProvider;
use crate::config::{AppConfig, Language};
use crate::data_saving::{SaveConfig, SaveResult};
use crate::hotkey::{HotkeyCallback, HotkeyEvent};
use crate::llm::{AnyCorrector, LLMClient, TextCorrector};
use crate::perf::PerfMetrics;
use crate::speech::SpeechEngine;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::pipeline_state::PipelineState;
use super::review::ReviewData;

/// Adapter: bridges `commands::EventEmitter` → `realtime::EventEmitter`.
struct RealtimeEmitterAdapter(Arc<dyn crate::commands::EventEmitter>);

impl crate::realtime::EventEmitter for RealtimeEmitterAdapter {
    fn emit_partial(&self, text: &str) {
        self.0.emit(
            "transcription-partial",
            serde_json::to_value(text).unwrap_or_default(),
        );
    }
}

/// Check silence and resample audio to 16kHz for Whisper.
/// Returns `None` if audio is near-silent (hallucination guard).
fn preprocess_audio(audio: &[f32], native_rate: u32) -> Option<Vec<f32>> {
    if audio.is_empty() {
        warn!("preprocess_audio: empty audio buffer, skipping transcription");
        return None;
    }
    let rms_val = rms::calculate_rms(audio);
    if rms_val < 0.01 {
        debug!("Silent audio (rms={rms_val:.4}), skipping transcription");
        return None;
    }
    Some(resample(audio, native_rate, TARGET_SAMPLE_RATE))
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
    let config = ps.config_cache.read_cached();
    info!(
        "Pipeline: starting (review={}, llm={}, samples={})",
        config.review_before_paste,
        config.llm_enabled,
        resampled.len()
    );

    // -- Save audio and transcribe in parallel --
    let (save_result, transcription) =
        transcribe_and_save(&ps, audio_for_save, native_rate, resampled, &config).await;

    perf.transcription_ms = perf
        .transcription_ms
        .or(Some(Instant::now().elapsed().as_millis() as u64));

    // Skip if transcription is empty (all segments filtered by no_speech probability).
    if transcription.is_empty() {
        info!("run_pipeline: empty transcription, resetting to idle");
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
        info!(
            "run_pipeline: handing off to deliver_review ({} chars)",
            final_text.len()
        );
        deliver_review(&ps, final_text, transcription, save_result, &config).await;
        return;
    }

    info!(
        "run_pipeline: handing off to deliver_direct ({} chars)",
        final_text.len()
    );
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
    let save_config = SaveConfig::from_app_config(config);
    let sr_for_save = native_rate;
    let save_handle = tokio::task::spawn_blocking(move || {
        if save_config.enabled && !save_config.path.is_empty() {
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
            ps.emitter.emit(
                "transcription-complete",
                serde_json::to_value(&text).unwrap_or_default(),
            );
            if let Some(mut s) = crate::util::lock_mutex(&ps.sm, "state_machine") {
                let _ = s.add_partial_result(text.clone());
            }
            text
        }
        Ok(Err(e)) => {
            ps.emitter.emit(
                "speech-error",
                serde_json::to_value(e.to_string()).unwrap_or_default(),
            );
            reset_to_idle(ps);
            return (save_result, String::new());
        }
        Err(e) => {
            ps.emitter.emit(
                "speech-error",
                serde_json::to_value(e.to_string()).unwrap_or_default(),
            );
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
    ps.emitter.emit("llm-refining", serde_json::Value::Null);

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
            ps.emitter.emit(
                "llm-complete",
                serde_json::to_value(&corrected).unwrap_or_default(),
            );
            corrected
        }
        Err(e) => {
            ps.emitter.emit(
                "llm-error",
                serde_json::to_value(e.to_string()).unwrap_or_default(),
            );
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
    info!(
        "deliver_review: ENTER ({} chars, llm={})",
        final_text.len(),
        config.llm_enabled
    );

    // Save clipboard before entering review state.
    if let Some(mut cb) = crate::util::lock_mutex(&ps.clipboard, "clipboard") {
        let _ = cb.save();
    } else {
        warn!("deliver_review: clipboard lock returned None (poisoned?)");
    }
    // Transition to Reviewing.
    if let Some(mut s) = crate::util::lock_mutex(&ps.sm, "state_machine") {
        if config.llm_enabled {
            let _ = s.llm_to_reviewing(final_text.clone());
        } else {
            let _ = s.transcribing_to_reviewing(final_text.clone());
        }
    } else {
        warn!("deliver_review: state_machine lock returned None (poisoned?)");
    }
    // Store text for the review window to fetch on load.
    ps.review.store_text(final_text.clone());
    debug!("Review: stored pending text ({} chars)", final_text.len());

    // Check if review window was already shown on press (realtime + review mode).
    let was_shown_on_press = ps.review.was_shown_on_press();

    if was_shown_on_press {
        // Review window already visible — update the text.
        info!(
            "Review: deliver_review() called, was_shown_on_press=true, final_text={} chars",
            final_text.len()
        );
        // Primary mechanism: eval() directly sets textarea via JS, bypassing event/command IPC.
        let json_text = serde_json::to_string(&final_text).unwrap_or_default();
        let js = format!(
            "(function(){{\
                 var t=document.getElementById('review-text');\
                 if(t){{t.value={json_text};t.selectionStart=t.selectionEnd=t.value.length;t.scrollTop=t.scrollHeight;}}\
                 var p=document.getElementById('preview');\
                 if(p){{p.textContent='';p.classList.remove('visible');}}\
                 var b=document.getElementById('btn-confirm');\
                 if(b){{b.disabled=false;}}\
             }})()"
        );
        if ps.window_controller.eval_review_js(&js) {
            info!(
                "Review: set final text via eval OK ({} chars)",
                final_text.len()
            );
        } else {
            warn!("Review: get_webview_window('review') returned None");
        }

        // Secondary: emit event (diagnostic / frontend polling may consume it).
        ps.window_controller.emit_review_final_text(&final_text);
        info!(
            "Review: emitted review-final-text OK ({} chars)",
            final_text.len()
        );

        // Store data-saving metadata for confirm/cancel to consume later.
        if let Some(sr) = save_result.as_ref() {
            ps.review.store_review_data(ReviewData {
                json_path: sr.json_path.clone(),
                raw_transcription: transcription,
                llm_text: if config.llm_enabled {
                    Some(final_text)
                } else {
                    None
                },
            });
        }
        return;
    }

    ps.review.save_foreground();
    if ps.window_controller.show_review_near_caret() {
        debug!("Review: window shown, review-show emitted");

        // Store data-saving metadata for confirm/cancel to consume later.
        if let Some(sr) = save_result.as_ref() {
            ps.review.store_review_data(ReviewData {
                json_path: sr.json_path.clone(),
                raw_transcription: transcription,
                llm_text: if config.llm_enabled {
                    Some(final_text)
                } else {
                    None
                },
            });
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
        ps.emitter
            .emit("injection-complete", serde_json::Value::Null);
        ps.window_controller.hide_floating();
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

    let mut ctx = super::text_injector::InjectionContext {
        text: final_text,
        transcription,
        save_result,
        config,
        perf,
        t_press_for_e2e,
    };
    super::text_injector::inject_text(ps, &mut ctx).await;
}

/// Fast path for RealtimeDirect mode (RT=on, REVIEW=off).
/// Uses accumulated realtime text directly, optionally runs LLM, then injects.
/// Skips Whisper transcription entirely when accumulated text is non-empty.
async fn run_realtime_fast_path(
    ps: PipelineState,
    accumulated: String,
    audio_data: Vec<f32>,
    native_rate: u32,
    mut perf: PerfMetrics,
    t_press_for_e2e: Instant,
) {
    let config = ps.config_cache.read_cached();
    info!(
        "RealtimeFastPath: starting (llm={}, accumulated={} chars)",
        config.llm_enabled,
        accumulated.len()
    );

    // Save audio in background for training data.
    let save_config = SaveConfig::from_app_config(&config);
    let sr_for_save = native_rate;
    let audio_for_save = audio_data.clone();
    let save_handle = tokio::task::spawn_blocking(move || {
        if save_config.enabled && !save_config.path.is_empty() {
            crate::data_saving::save_audio(&audio_for_save, sr_for_save, &save_config).ok()
        } else {
            None
        }
    });

    // State: Recording → Transcribing (brief, for state consistency).
    // Note: stop_recording() was already called in the release handler before
    // spawning this task, so the state machine should already be in Transcribing.
    // We only need to proceed from here.

    // Optionally run LLM on the accumulated text.
    let transcription = accumulated.clone();
    perf.llm_enabled = config.llm_enabled;
    let final_text = if config.llm_enabled {
        resolve_llm_text(&ps, &config, &transcription, &mut perf).await
    } else {
        transcription.clone()
    };

    // Normalize Chinese punctuation.
    let final_text = if config.language == Language::Zh {
        normalize_chinese_punctuation(&final_text)
    } else {
        final_text
    };

    // State: Transcribing → Injecting.
    if let Some(mut s) = crate::util::lock_mutex(&ps.sm, "state_machine") {
        if config.llm_enabled {
            let _ = s.llm_to_injecting(final_text.clone());
        } else {
            let _ = s.transcribing_to_injecting(final_text.clone());
        }
    }

    let save_result = save_handle.await.unwrap_or(None);
    let mut ctx = super::text_injector::InjectionContext {
        text: final_text,
        transcription,
        save_result,
        config: &config,
        perf: &mut perf,
        t_press_for_e2e,
    };
    super::text_injector::inject_text(&ps, &mut ctx).await;
}

/// Reset state machine to Idle and hide floating window.
fn reset_to_idle(ps: &PipelineState) {
    if let Some(mut s) = crate::util::lock_mutex(&ps.sm, "state_machine") {
        s.reset();
    }
    ps.window_controller.hide_floating();
    // Hide review window if it was shown on press (realtime+review mode).
    if ps.review.was_shown_on_press() {
        ps.window_controller.hide_review();
        ps.review.set_shown_on_press(false);
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
                    .map(|mut s| {
                        let result = s.start_recording();
                        if let Err(ref _e) = result {
                            warn!(
                                "hotkey press: start_recording failed: state={}",
                                s.state_name()
                            );
                        }
                        result.is_ok()
                    })
                    .unwrap_or_else(|| {
                        warn!("hotkey press: state_machine lock returned None (poisoned?)");
                        false
                    });

                if can_record {
                    let engine_ready = crate::util::lock_mutex(&ps.engine, "engine")
                        .map(|e| e.is_ready())
                        .unwrap_or(false);
                    if !engine_ready {
                        reset_to_idle(&ps);
                        ps.emitter.emit(
                            "speech-error",
                            serde_json::to_value("模型加载中，请稍候...").unwrap_or_default(),
                        );
                        return;
                    }
                }

                if can_record {
                    let config = ps.config_cache.read_cached();
                    let mode = config.pipeline_mode();

                    // Show floating window near text caret.
                    // RealtimeReview mode shows the review window instead.
                    let show_floating =
                        !matches!(mode, crate::config::PipelineMode::RealtimeReview);
                    info!(
                        "hotkey press: mode={:?}, show_floating={}",
                        mode, show_floating
                    );
                    if show_floating {
                        ps.window_controller.show_floating_near_caret();
                    }
                    ps.emitter.emit("recording-start", serde_json::Value::Null);

                    // Start audio capture with RMS-emitting callback (~30 fps).
                    let last_rms_emit = Arc::new(Mutex::new(Instant::now()));
                    if let Some(mut ac_guard) = crate::util::lock_mutex(&ps.ac, "audio_capture") {
                        let sm_for_audio = Arc::clone(&ps.sm);
                        let emitter_for_rms = ps.emitter.clone();
                        let last_rms_for_cb = Arc::clone(&last_rms_emit);
                        let start_result = ac_guard.start(Box::new(move |data: &[f32]| {
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
                                    emitter_for_rms.emit(
                                        "audio-rms",
                                        serde_json::to_value(rms_val).unwrap_or_default(),
                                    );
                                }
                            }
                        }));

                        if let Err(e) = start_result {
                            warn!("audio capture start failed: {e}");
                            reset_to_idle(&ps);
                            ps.emitter.emit(
                                "speech-error",
                                serde_json::to_value(format!("录音启动失败: {e}"))
                                    .unwrap_or_default(),
                            );
                            return;
                        }

                        // Start real-time transcription for realtime modes.
                        if matches!(
                            mode,
                            crate::config::PipelineMode::RealtimeDirect
                                | crate::config::PipelineMode::RealtimeReview
                        ) {
                            if let Some(sr) = ac_guard.sample_rate() {
                                let audio = Arc::new(
                                    crate::realtime::StateMachineAudioSource::new(ps.sm.clone()),
                                );
                                let emitter = Arc::new(RealtimeEmitterAdapter(ps.emitter.clone()));
                                let rt = crate::realtime::RealtimeTranscriber::start(
                                    audio,
                                    ps.engine.clone(),
                                    emitter,
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

                    // Show review window on press for RealtimeReview mode.
                    if matches!(mode, crate::config::PipelineMode::RealtimeReview) {
                        ps.review.save_foreground();
                        ps.review.set_shown_on_press(true);
                        ps.window_controller.show_review_near_caret();
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
                info!("hotkey release: event received");
                let t_release = Instant::now();

                let mut perf = crate::util::lock_mutex(&perf_slot, "perf")
                    .and_then(|mut s| s.take())
                    .unwrap_or_else(|| PerfMetrics::new(ps.perf_history.next_cycle_id()));

                perf.audio_duration_ms = Some(t_release.elapsed().as_millis() as u64);

                let sample_rate =
                    crate::util::lock_mutex(&ps.ac, "audio_capture").and_then(|a| a.sample_rate());

                // Stop audio capture and realtime transcriber, get accumulated text.
                let realtime_accumulated = ps.stop_recording_resources();

                let config = ps.config_cache.read_cached();
                let mode = config.pipeline_mode();

                // Mode-specific fast paths using exhaustive match.
                match mode {
                    crate::config::PipelineMode::RealtimeReview => {
                        if let Some(accumulated) = realtime_accumulated {
                            info!(
                                "hotkey release: RealtimeReview fast path, {} chars already in textarea",
                                accumulated.len()
                            );
                            if let Some(mut cb) =
                                crate::util::lock_mutex(&ps.clipboard, "clipboard")
                            {
                                let _ = cb.save();
                            }
                            if let Some(mut s) = crate::util::lock_mutex(&ps.sm, "state_machine") {
                                let _ = s.stop_recording();
                                let _ = s.transcribing_to_reviewing(accumulated);
                            }
                            ps.window_controller.hide_floating();
                            return;
                        }
                        info!(
                            "hotkey release: RealtimeReview but no accumulated text, falling through"
                        );
                    }
                    crate::config::PipelineMode::RealtimeDirect => {
                        if let Some(accumulated) = realtime_accumulated {
                            info!(
                                "hotkey release: RealtimeDirect fast path, {} chars",
                                accumulated.len()
                            );
                            if let Some(mut sm_guard) =
                                crate::util::lock_mutex(&ps.sm, "state_machine")
                            {
                                match sm_guard.stop_recording() {
                                    Ok(audio_data) => {
                                        let native_rate = sample_rate.unwrap_or(48000);
                                        perf.audio_samples = audio_data.len();
                                        perf.audio_sample_rate = native_rate;
                                        perf.release_latency_ms =
                                            Some(t_release.elapsed().as_millis() as u64);
                                        let t_press_for_e2e = t_release
                                            - Duration::from_millis(
                                                perf.audio_duration_ms.unwrap_or(0),
                                            );
                                        tauri::async_runtime::spawn(run_realtime_fast_path(
                                            ps.clone(),
                                            accumulated,
                                            audio_data,
                                            native_rate,
                                            perf,
                                            t_press_for_e2e,
                                        ));
                                        return;
                                    }
                                    Err(_) => {
                                        info!(
                                            "hotkey release: RealtimeDirect stop_recording failed"
                                        );
                                        sm_guard.reset();
                                        ps.window_controller.hide_floating();
                                        return;
                                    }
                                }
                            }
                        }
                        info!(
                            "hotkey release: RealtimeDirect but no accumulated text, falling through"
                        );
                    }
                    crate::config::PipelineMode::ClassicDirect
                    | crate::config::PipelineMode::ClassicReview => {
                        // Fall through to full pipeline below.
                    }
                }

                // Full pipeline (Classic modes, or realtime modes with no accumulated text).
                if let Some(mut sm_guard) = crate::util::lock_mutex(&ps.sm, "state_machine") {
                    let record_result = sm_guard.stop_recording();
                    info!(
                        "hotkey release: stop_recording result={}",
                        record_result.is_ok()
                    );
                    if let Ok(audio_data) = record_result {
                        perf.release_latency_ms = Some(t_release.elapsed().as_millis() as u64);
                        perf.audio_samples = audio_data.len();
                        perf.audio_sample_rate = sample_rate.unwrap_or(48000);

                        let native_rate = sample_rate.unwrap_or(48000);
                        let resampled = match preprocess_audio(&audio_data, native_rate) {
                            Some(r) => r,
                            None => {
                                info!(
                                    "hotkey release: preprocess_audio returned None (silent?), samples={}",
                                    audio_data.len()
                                );
                                sm_guard.reset();
                                ps.window_controller.hide_floating();
                                if ps.review.was_shown_on_press() {
                                    ps.window_controller.hide_review();
                                    ps.review.set_shown_on_press(false);
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
                    } else {
                        info!("hotkey release: stop_recording failed (state already reset)");
                        ps.window_controller.hide_floating();
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
