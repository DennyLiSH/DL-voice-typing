use crate::clipboard::{AnyClipboard, ClipboardProvider};
use crate::commands::EventEmitter;
use crate::commands::pipeline_state::PipelineState;
use crate::commands::review::ReviewData;
use crate::commands::window_controller::WindowController;
use crate::data_saving::SaveResult;
use crate::error::CommandError;
use crate::perf::PerfMetrics;
use crate::state::StateTag;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use super::recording_session::SessionPolicy;

/// Mutable delivery context held for the duration of a review cycle.
///
/// `foreground_hwnd`, `data_saving`, and `perf` are accumulated while the
/// review window is visible and consumed by `confirm_review` / `cancel_review`.
struct DeliveryContext {
    foreground_hwnd: Option<isize>,
    data_saving: Option<ReviewData>,
    perf: Option<PerfMetrics>,
    t_press_for_e2e: Option<Instant>,
}

impl DeliveryContext {
    fn new() -> Self {
        Self {
            foreground_hwnd: None,
            data_saving: None,
            perf: None,
            t_press_for_e2e: None,
        }
    }

    fn take(
        &mut self,
    ) -> (
        Option<isize>,
        Option<ReviewData>,
        Option<PerfMetrics>,
        Option<Instant>,
    ) {
        (
            self.foreground_hwnd.take(),
            self.data_saving.take(),
            self.perf.take(),
            self.t_press_for_e2e.take(),
        )
    }
}

/// Single authority for the post-transcription delivery lifecycle.
///
/// Owns clipboard ordering, focus restoration, window visibility, state
/// transitions, data-saving JSON updates, and perf recording for both the
/// direct-inject and review-before-paste paths.
pub(crate) struct DeliveryController {
    emitter: Arc<dyn EventEmitter>,
    window_controller: Arc<dyn WindowController>,
    clipboard: Arc<Mutex<AnyClipboard>>,
    review: Arc<dyn crate::commands::review_provider::ReviewProvider>,
    perf_history: Arc<crate::perf::PerfHistory>,
    context: Mutex<DeliveryContext>,
}

impl DeliveryController {
    pub(crate) fn new(
        emitter: Arc<dyn EventEmitter>,
        window_controller: Arc<dyn WindowController>,
        clipboard: Arc<Mutex<AnyClipboard>>,
        review: Arc<dyn crate::commands::review_provider::ReviewProvider>,
        perf_history: Arc<crate::perf::PerfHistory>,
    ) -> Self {
        Self {
            emitter,
            window_controller,
            clipboard,
            review,
            perf_history,
            context: Mutex::new(DeliveryContext::new()),
        }
    }

    // -------------------------------------------------------------------------
    // Direct injection path
    // -------------------------------------------------------------------------

    /// Inject text directly and finish the cycle.
    ///
    /// Caller must have already stopped recording. This method transitions
    /// `Transcribing -> Injecting` (or `LLMRefining -> Injecting` when
    /// `llm_transition` is true), performs the clipboard paste, and returns to
    /// `Idle`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn inject_direct(
        &self,
        ps: &PipelineState,
        text: String,
        transcription: String,
        save_result: Option<SaveResult>,
        policy: &SessionPolicy,
        perf: &mut PerfMetrics,
        t_press_for_e2e: Instant,
        llm_transition: bool,
    ) {
        if llm_transition {
            ps.sm_llm_to_injecting();
        } else {
            ps.sm_transcribing_to_injecting();
        }

        let llm_text = if policy.llm_enabled {
            Some(text.clone())
        } else {
            None
        };

        self.inject_and_finish(
            ps,
            text,
            save_result,
            policy,
            perf,
            t_press_for_e2e,
            Some(transcription.as_str()),
            llm_text,
            false,
        )
        .await;
    }

    // -------------------------------------------------------------------------
    // Review path
    // -------------------------------------------------------------------------

    /// Show the review window and store the delivery context for the
    /// subsequent confirm/cancel call.
    ///
    /// If the review window cannot be shown, falls back to direct injection.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn show_review(
        &self,
        ps: &PipelineState,
        final_text: String,
        transcription: String,
        save_result: Option<SaveResult>,
        policy: &SessionPolicy,
        perf: PerfMetrics,
        t_press_for_e2e: Instant,
        llm_transition: bool,
    ) {
        info!(
            "show_review: ENTER ({} chars, llm_transition={})",
            final_text.len(),
            llm_transition
        );

        // Save clipboard before entering review state.
        if let Some(mut cb) = crate::util::lock_mutex(&ps.clipboard, "clipboard") {
            if let Err(e) = cb.save() {
                warn!("show_review: clipboard save failed: {e}");
            }
        } else {
            warn!("show_review: clipboard lock poisoned");
        }

        // Transition to Reviewing.
        if llm_transition {
            ps.sm_llm_to_reviewing();
        } else {
            ps.sm_transcribing_to_reviewing();
        }

        // Store text for the review window to fetch on load.
        ps.review.store_text(final_text.clone());
        debug!(
            "show_review: stored pending text ({} chars)",
            final_text.len()
        );

        let was_shown_on_press = ps.review.was_shown_on_press();

        if was_shown_on_press {
            info!(
                "show_review: was_shown_on_press=true, final_text={} chars",
                final_text.len()
            );

            // Migrate foreground ownership from PendingReview to DeliveryController.
            if let Some(hwnd) = ps.review.take_foreground() {
                self.store_foreground(hwnd);
            }

            // Push the final transcription into the already-visible textarea.
            let json_text = match serde_json::to_string(&final_text) {
                Ok(s) => s,
                Err(e) => {
                    warn!("show_review: failed to serialize final text: {e}");
                    String::new()
                }
            };
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
            if self.window_controller.eval_review_js(&js) {
                info!(
                    "show_review: set final text via eval OK ({} chars)",
                    final_text.len()
                );
            } else {
                warn!("show_review: get_webview_window('review') returned None");
            }

            self.window_controller.emit_review_final_text(&final_text);
            info!(
                "show_review: emitted review-final-text OK ({} chars)",
                final_text.len()
            );

            let review_data = save_result.map(|sr| ReviewData {
                json_path: sr.json_path,
                raw_transcription: transcription.clone(),
                llm_text: if policy.llm_enabled {
                    Some(final_text.clone())
                } else {
                    None
                },
            });
            self.store_context(review_data, perf, t_press_for_e2e);
            return;
        }

        // Classic review path: capture foreground, then show the window.
        self.review.save_foreground();
        if let Some(hwnd) = self.review.take_foreground() {
            self.store_foreground(hwnd);
        }

        if self.window_controller.show_review_near_caret() {
            debug!("show_review: window shown, review-show emitted");
            let review_data = save_result.map(|sr| ReviewData {
                json_path: sr.json_path,
                raw_transcription: transcription.clone(),
                llm_text: if policy.llm_enabled {
                    Some(final_text.clone())
                } else {
                    None
                },
            });
            self.store_context(review_data, perf, t_press_for_e2e);
        } else {
            warn!("show_review: review window not found. Falling back to direct injection.");
            // Clear residual context: foreground never left the original app in fallback
            // path, so no restoration needed. Without this, the stored hwnd would leak
            // into the next confirm/cancel cycle and restore focus to a stale window.
            // take_context() returns a tuple (no Result); Option::take is infallible.
            // On mutex poison, lock_mutex logs and unwrap_or returns defaults.
            let _ = self.take_context();
            // Reviewing -> Injecting, then inject.
            ps.sm_reviewing_to_injecting();
            let llm_text = if policy.llm_enabled {
                Some(final_text.clone())
            } else {
                None
            };
            let mut fallback_perf = perf;
            self.inject_and_finish(
                ps,
                final_text,
                save_result,
                policy,
                &mut fallback_perf,
                t_press_for_e2e,
                Some(transcription.as_str()),
                llm_text,
                true,
            )
            .await;
        }
    }

    /// RealtimeReview handoff on hotkey release.
    ///
    /// The review window was already shown on press, so this only saves the
    /// clipboard, advances the state machine, hides the floating window, and
    /// migrates the foreground handle into our context.
    pub(crate) fn realtime_review_handoff(&self, ps: &PipelineState, accumulated: Option<String>) {
        info!(
            "realtime_review_handoff: accumulated={} chars",
            accumulated.as_ref().map(|s| s.len()).unwrap_or(0)
        );

        if let Some(mut cb) = crate::util::lock_mutex(&ps.clipboard, "clipboard") {
            if let Err(e) = cb.save() {
                warn!("realtime_review_handoff: clipboard save failed: {e}");
            }
        } else {
            warn!("realtime_review_handoff: clipboard lock poisoned");
        }

        ps.sm_stop_recording();
        ps.sm_transcribing_to_reviewing();
        self.window_controller.hide_floating();

        if let Some(text) = accumulated {
            ps.review.store_text(text);
        }

        if let Some(hwnd) = ps.review.take_foreground() {
            self.store_foreground(hwnd);
        }
    }

    /// Confirm the reviewed text and inject it.
    pub(crate) async fn confirm_review(
        &self,
        ps: &PipelineState,
        text: String,
    ) -> Result<(), CommandError> {
        info!("confirm_review: start ({} chars)", text.len());

        match ps.sm_state() {
            Some(StateTag::Reviewing) => {
                self.confirm_from_reviewing(ps, text).await;
                info!("confirm_review: done");
                Ok(())
            }
            Some(StateTag::Recording) | Some(StateTag::Transcribing) => {
                info!("confirm_review: early confirm during recording/transcribing");
                let _ = self.take_context();
                debug!(
                    target: "delivery",
                    "take_context cleared on confirm_review recording/transcribing branch"
                );
                ps.stop_recording_resources_graceful();
                ps.sm_reset();
                self.cleanup_review_ui(ps).await;
                Err(CommandError {
                    code: "STATE".to_string(),
                    message: "cannot confirm from current state".to_string(),
                })
            }
            _ => {
                let _ = self.take_context();
                debug!(
                    target: "delivery",
                    "take_context cleared on confirm_review catch-all branch"
                );
                Err(CommandError {
                    code: "STATE".to_string(),
                    message: "cannot confirm from current state".to_string(),
                })
            }
        }
    }

    /// Cancel the review and return to idle.
    pub(crate) async fn cancel_review(&self, ps: &PipelineState) -> Result<(), CommandError> {
        info!("cancel_review: start");

        match ps.sm_state() {
            Some(StateTag::Reviewing) => {
                if !ps.sm_cancel_reviewing() {
                    let _ = self.take_context();
                    debug!(
                        target: "delivery",
                        "take_context cleared on cancel_review sm_cancel_reviewing failure branch"
                    );
                    return Err(CommandError {
                        code: "STATE".to_string(),
                        message: "cancel_reviewing failed".to_string(),
                    });
                }
            }
            Some(StateTag::Recording) | Some(StateTag::Transcribing) => {
                info!("cancel_review: early cancel during recording/transcribing");
                ps.stop_recording_resources_graceful();
                ps.sm_reset();
            }
            _ => {
                let _ = self.take_context();
                debug!(
                    target: "delivery",
                    "take_context cleared on cancel_review catch-all branch"
                );
                return Err(CommandError {
                    code: "STATE".to_string(),
                    message: "cannot cancel from current state".to_string(),
                });
            }
        }

        // Take the stored delivery context before any await point.
        let (foreground_hwnd, data_saving, _perf, _t_press) = self.take_context();

        // Restore clipboard.
        if let Some(mut cb) = crate::util::lock_mutex(&ps.clipboard, "clipboard") {
            if let Err(e) = cb.restore() {
                warn!("cancel_review: clipboard restore failed: {e}");
            }
        }

        // Restore focus and hide windows.
        if let Some(hwnd_val) = foreground_hwnd {
            crate::win32::restore_foreground_hwnd(hwnd_val);
        }
        self.window_controller.hide_floating();
        self.window_controller.hide_review();

        // Reset shown_on_press flag.
        ps.review.set_shown_on_press(false);

        // Update data-saving JSON: preserve raw transcription, mark no final text.
        if let Some(review_data) = data_saving {
            if let Err(e) = crate::data_saving::update_json_with_text(
                &review_data.json_path,
                &review_data.raw_transcription,
                review_data.llm_text.as_deref(),
                None,
            ) {
                warn!(
                    "cancel_review: failed to update JSON {}: {e}",
                    review_data.json_path.display()
                );
            }
        }

        info!("cancel_review: done");
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Private helpers
    // -------------------------------------------------------------------------

    async fn confirm_from_reviewing(&self, ps: &PipelineState, text: String) {
        let (foreground_hwnd, data_saving, mut perf, t_press) = self.take_context();

        // 1. Save data before text is consumed.
        if let Some(review_data) = data_saving {
            if let Err(e) = crate::data_saving::update_json_with_text(
                &review_data.json_path,
                &review_data.raw_transcription,
                review_data.llm_text.as_deref(),
                Some(&text),
            ) {
                warn!(
                    "confirm_from_reviewing: failed to update JSON {}: {e}",
                    review_data.json_path.display()
                );
            }
        }

        // 2. Restore focus to target app BEFORE paste.
        info!("confirm_from_reviewing: saved_hwnd={:?}", foreground_hwnd);
        if let Some(hwnd_val) = foreground_hwnd {
            crate::win32::restore_foreground_hwnd(hwnd_val);
            // Wait for OS to fully process the focus change before simulating
            // keyboard input. Without this delay, SendInput (Ctrl+V) may still
            // be dispatched to the review window.
            std::thread::sleep(Duration::from_millis(100));
        }

        // 3. Inject text in a blocking thread to avoid starving the runtime.
        let t_inject = Instant::now();
        let inject_result = self.save_and_inject(&text).await;

        perf.injection_ms = Some(t_inject.elapsed().as_millis() as u64);
        perf.text_length = text.len();
        if let Some(t_press_for_e2e) = t_press {
            perf.end_to_end_ms = Some(t_press_for_e2e.elapsed().as_millis() as u64);
        }

        if let Err(ref e) = inject_result {
            warn!("confirm_from_reviewing: inject failed: {e}");
            self.emitter.emit(
                "injection-error",
                serde_json::to_value(e).unwrap_or_default(),
            );
            // Best-effort cleanup: restore clipboard, hide windows, reset state.
            let _ = self.restore_clipboard();
            self.window_controller.hide_review();
            self.window_controller.hide_floating();
            ps.sm_reset();
            return;
        }

        // 4. State transition Reviewing -> Injecting.
        if !ps.sm_reviewing_to_injecting() {
            warn!("confirm_from_reviewing: reviewing_to_injecting failed");
            return;
        }

        // 5. Hide review window AFTER paste, then finish.
        self.window_controller.hide_review();
        ps.sm_finish_injecting();
        self.emitter
            .emit("injection-complete", serde_json::Value::Null);
        self.window_controller.hide_floating();

        // 6. Reset shown_on_press flag.
        ps.review.set_shown_on_press(false);

        // 7. Record perf.
        self.perf_history.record(perf.clone());
        self.emitter.emit(
            "perf-metrics",
            serde_json::to_value(&perf).unwrap_or_default(),
        );
        info!("{}", perf.summary());
    }

    /// Shared injection tail used by direct inject and fallback paths.
    ///
    /// Caller is responsible for transitioning into `Injecting` before calling.
    #[allow(clippy::too_many_arguments)]
    async fn inject_and_finish(
        &self,
        ps: &PipelineState,
        text: String,
        save_result: Option<SaveResult>,
        _policy: &SessionPolicy,
        perf: &mut PerfMetrics,
        t_press_for_e2e: Instant,
        raw_transcription: Option<&str>,
        llm_text: Option<String>,
        hide_review: bool,
    ) {
        let t_inject = Instant::now();
        let inject_result = self.save_and_inject(&text).await;

        perf.injection_ms = Some(t_inject.elapsed().as_millis() as u64);
        perf.end_to_end_ms = Some(t_press_for_e2e.elapsed().as_millis() as u64);
        perf.text_length = text.len();

        match inject_result {
            Ok(()) => {
                info!("inject_and_finish: injection succeeded");
            }
            Err(e) => {
                warn!("inject_and_finish: injection failed: {e}");
                self.emitter.emit(
                    "injection-error",
                    serde_json::to_value(&e).unwrap_or_default(),
                );
                let _ = self.restore_clipboard();
                if hide_review {
                    self.window_controller.hide_review();
                }
                self.window_controller.hide_floating();
                ps.sm_reset();
                return;
            }
        }

        ps.sm_finish_injecting();
        self.emitter
            .emit("injection-complete", serde_json::Value::Null);

        if hide_review {
            self.window_controller.hide_review();
        }
        self.window_controller.hide_floating();

        if let Some(sr) = save_result {
            if let Some(raw) = raw_transcription {
                if let Err(e) = crate::data_saving::update_json_with_text(
                    &sr.json_path,
                    raw,
                    llm_text.as_deref(),
                    Some(&text),
                ) {
                    warn!(
                        "inject_and_finish: failed to update JSON {}: {e}",
                        sr.json_path.display()
                    );
                }
            }
        }

        self.perf_history.record(perf.clone());
        self.emitter.emit(
            "perf-metrics",
            serde_json::to_value(&*perf).unwrap_or_default(),
        );
        info!("{}", perf.summary());
    }

    /// Save the current clipboard and inject text. Returns `Ok(())` on success
    /// or an error string on failure.
    async fn save_and_inject(&self, text: &str) -> Result<(), String> {
        let cb = self.clipboard.clone();
        let text_for_inject = text.to_string();
        match tokio::task::spawn_blocking(move || {
            let mut cb = crate::util::lock_mutex(&cb, "delivery::clipboard")
                .ok_or_else(|| "clipboard lock poisoned".to_string())?;
            cb.save().map_err(|e| e.to_string())?;
            cb.inject_text(&text_for_inject).map_err(|e| e.to_string())
        })
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(e) => {
                let msg = format!("inject task panicked: {e}");
                error!("{msg}");
                Err(msg)
            }
        }
    }

    fn restore_clipboard(&self) -> Result<(), String> {
        if let Some(mut cb) = crate::util::lock_mutex(&self.clipboard, "clipboard") {
            cb.restore().map_err(|e| e.to_string())
        } else {
            Err("clipboard lock poisoned".to_string())
        }
    }

    fn store_foreground(&self, hwnd: isize) {
        if let Some(mut ctx) = crate::util::lock_mutex(&self.context, "delivery_context") {
            ctx.foreground_hwnd = Some(hwnd);
        }
    }

    fn store_context(
        &self,
        review_data: Option<ReviewData>,
        perf: PerfMetrics,
        t_press_for_e2e: Instant,
    ) {
        if let Some(mut ctx) = crate::util::lock_mutex(&self.context, "delivery_context") {
            ctx.data_saving = review_data;
            ctx.perf = Some(perf);
            ctx.t_press_for_e2e = Some(t_press_for_e2e);
        }
    }

    pub(crate) fn take_context(
        &self,
    ) -> (
        Option<isize>,
        Option<ReviewData>,
        PerfMetrics,
        Option<Instant>,
    ) {
        let mut perf = PerfMetrics::new(self.perf_history.next_cycle_id());
        let mut t_press = None;
        let (hwnd, data) = crate::util::lock_mutex(&self.context, "delivery_context")
            .map(|mut ctx| {
                let (h, d, p, tp) = ctx.take();
                if let Some(p) = p {
                    perf = p;
                }
                t_press = tp;
                (h, d)
            })
            .unwrap_or((None, None));
        (hwnd, data, perf, t_press)
    }

    async fn cleanup_review_ui(&self, ps: &PipelineState) {
        self.window_controller.hide_floating();
        self.window_controller.hide_review();
        ps.review.set_shown_on_press(false);
    }
}
