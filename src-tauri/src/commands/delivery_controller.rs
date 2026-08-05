use crate::clipboard::ClipboardProvider;
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
    clipboard: Arc<dyn ClipboardProvider>,
    review: Arc<dyn crate::commands::review_provider::ReviewProvider>,
    perf_history: Arc<crate::perf::PerfHistory>,
    context: Mutex<DeliveryContext>,
}

// -------------------------------------------------------------------------
// FinishOutcome: centralized post-delivery cleanup
// -------------------------------------------------------------------------

/// Encodes the post-delivery cleanup recipe. Each variant carries only the
/// data finish() actually needs; site-specific pre-side-effects (entry guards,
/// clipboard save, window show, save_and_inject itself, foreground restore)
/// stay in the caller.
#[allow(dead_code)] // TODO(Task 3): remove once EarlyStateMismatch is wired
enum FinishOutcome {
    /// Successful paste delivery. Caller has ALREADY done save_and_inject and
    /// pre-inject foreground restore. finish handles sm_finish_injecting,
    /// emit injection-complete, hide windows, JSON update, perf record.
    ///
    /// If sm_finish_injecting returns false (TOCTOU — watchdog reset state
    /// machine mid-deliver), finish follows current source semantics: warn!
    /// and continue with injection-complete (paste already happened, clipboard
    /// already self-restored by inject_inner).
    Deliver {
        final_text: String, // for JSON final_text field (raw/llm come from ctx.review_data)
        hide_review: bool,
    },
    /// User-cancel from Reviewing. No paste happened. Caller did NOT do
    /// save_and_inject. finish does take_context + restore_clipboard_if_saved
    /// (clipboard was saved on show_review) + restore_foreground + hide_windows
    /// + set_shown_on_press(false) + JSON with None final_text.
    Cancel,
    /// confirm_review / cancel_review called from wrong state (Recording /
    /// Transcribing / Idle). Caller returns Err(CommandError) AFTER finish
    /// runs. finish does take_context + stop_resources + sm_reset + cleanup_review_ui.
    EarlyStateMismatch,
}

impl DeliveryController {
    pub(crate) fn new(
        emitter: Arc<dyn EventEmitter>,
        window_controller: Arc<dyn WindowController>,
        clipboard: Arc<dyn ClipboardProvider>,
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
        let transitioned = if llm_transition {
            ps.sm_llm_to_injecting()
        } else {
            ps.sm_transcribing_to_injecting()
        };
        if !transitioned {
            warn!(
                "inject_direct: sm entry transition failed (llm_transition={llm_transition}); aborting inject"
            );
            ps.sm_reset();
            return;
        }

        let t_inject = Instant::now();
        let inject_result = self.save_and_inject(&text).await;

        perf.injection_ms = Some(t_inject.elapsed().as_millis() as u64);
        perf.end_to_end_ms = Some(t_press_for_e2e.elapsed().as_millis() as u64);
        perf.text_length = text.len();

        if let Err(ref e) = inject_result {
            warn!("inject_direct: injection failed: {e}");
            self.emitter.emit(
                "injection-error",
                serde_json::to_value(e).unwrap_or_default(),
            );
            let _ = self.restore_clipboard();
            self.window_controller.hide_floating();
            ps.sm_reset();
            return;
        }

        let review_data = save_result.map(|sr| ReviewData {
            json_path: sr.json_path,
            raw_transcription: transcription,
            llm_text: if policy.llm_enabled {
                Some(text.clone())
            } else {
                None
            },
        });
        self.store_context(review_data, perf.clone(), t_press_for_e2e);
        let ctx = self.take_context();
        let perf_for_record = ctx.2.clone();
        let t_press_for_record = ctx.3.unwrap_or_else(Instant::now);
        self.finish(
            ps,
            ctx,
            FinishOutcome::Deliver {
                final_text: text,
                hide_review: false,
            },
            "inject_direct",
            Some((&perf_for_record, t_press_for_record)),
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
        if let Err(e) = self.clipboard.save() {
            warn!("show_review: clipboard save failed: {e}");
        }

        // Transition to Reviewing.
        let transitioned = if llm_transition {
            ps.sm_llm_to_reviewing()
        } else {
            ps.sm_transcribing_to_reviewing()
        };
        if !transitioned {
            warn!(
                "show_review: sm entry transition failed (llm_transition={llm_transition}); restoring clipboard and aborting"
            );
            if let Err(e) = self.restore_clipboard() {
                warn!("show_review: clipboard restore failed on transition-failure path: {e}");
            }
            ps.sm_reset();
            return;
        }

        // Store text for the review window to fetch on load.
        self.review.store_text(final_text.clone());
        debug!(
            "show_review: stored pending text ({} chars)",
            final_text.len()
        );

        let was_shown_on_press = self.review.was_shown_on_press();

        if was_shown_on_press {
            info!(
                "show_review: was_shown_on_press=true, final_text={} chars",
                final_text.len()
            );

            // Migrate foreground ownership from PendingReview to DeliveryController.
            if let Some(hwnd) = self.review.take_foreground() {
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
            let mut fallback_perf = perf;
            let t_inject = Instant::now();
            let inject_result = self.save_and_inject(&final_text).await;

            fallback_perf.injection_ms = Some(t_inject.elapsed().as_millis() as u64);
            fallback_perf.end_to_end_ms = Some(t_press_for_e2e.elapsed().as_millis() as u64);
            fallback_perf.text_length = final_text.len();

            if let Err(ref e) = inject_result {
                warn!("show_review: fallback injection failed: {e}");
                self.emitter.emit(
                    "injection-error",
                    serde_json::to_value(e).unwrap_or_default(),
                );
                let _ = self.restore_clipboard();
                self.window_controller.hide_review();
                self.window_controller.hide_floating();
                ps.sm_reset();
                return;
            }

            let review_data = save_result.map(|sr| ReviewData {
                json_path: sr.json_path,
                raw_transcription: transcription,
                llm_text: if policy.llm_enabled {
                    Some(final_text.clone())
                } else {
                    None
                },
            });
            self.store_context(review_data, fallback_perf.clone(), t_press_for_e2e);
            let ctx = self.take_context();
            let perf_for_record = ctx.2.clone();
            let t_press_for_record = ctx.3.unwrap_or_else(Instant::now);
            self.finish(
                ps,
                ctx,
                FinishOutcome::Deliver {
                    final_text,
                    hide_review: true,
                },
                "show_review_fallback",
                Some((&perf_for_record, t_press_for_record)),
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

        if let Err(e) = self.clipboard.save() {
            warn!("realtime_review_handoff: clipboard save failed: {e}");
        }

        ps.sm_stop_recording();
        ps.sm_transcribing_to_reviewing();
        self.window_controller.hide_floating();

        if let Some(text) = accumulated {
            self.review.store_text(text);
        }

        if let Some(hwnd) = self.review.take_foreground() {
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
                self.cleanup_review_ui().await;
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
        let ctx = self.take_context();
        self.finish(ps, ctx, FinishOutcome::Cancel, "cancel_review", None)
            .await;

        info!("cancel_review: done");
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Private helpers
    // -------------------------------------------------------------------------

    async fn confirm_from_reviewing(&self, ps: &PipelineState, text: String) {
        // 0. Take context at method TOP (decision#8 exception): pre-inject
        // foreground restore needs ctx.0, and the sm_reviewing_to_injecting
        // early-return must not leak context.
        let mut ctx = self.take_context();

        // 1. Restore focus to target app BEFORE paste.
        info!("confirm_from_reviewing: saved_hwnd={:?}", ctx.0);
        if let Some(hwnd_val) = ctx.0 {
            self.window_controller.restore_foreground_hwnd(hwnd_val);
            // Wait for OS to fully process the focus change before simulating
            // keyboard input. Without this delay, SendInput (Ctrl+V) may still
            // be dispatched to the review window.
            std::thread::sleep(Duration::from_millis(100));
        }

        // 2. Inject text in a blocking thread to avoid starving the runtime.
        let t_inject = Instant::now();
        let inject_result = self.save_and_inject(&text).await;

        ctx.2.injection_ms = Some(t_inject.elapsed().as_millis() as u64);
        ctx.2.text_length = text.len();
        if let Some(t_press_for_e2e) = ctx.3 {
            ctx.2.end_to_end_ms = Some(t_press_for_e2e.elapsed().as_millis() as u64);
        }

        if let Err(ref e) = inject_result {
            warn!("confirm_from_reviewing: inject failed: {e}");
            self.emitter.emit(
                "injection-error",
                serde_json::to_value(e).unwrap_or_default(),
            );
            // Best-effort cleanup: restore clipboard, restore focus, hide windows, reset state.
            let _ = self.restore_clipboard();
            if let Some(hwnd_val) = ctx.0 {
                debug!(
                    target: "delivery",
                    "confirm_from_reviewing: restoring foreground on inject failure, hwnd={hwnd_val}"
                );
                self.window_controller.restore_foreground_hwnd(hwnd_val);
            }
            self.window_controller.hide_review();
            self.window_controller.hide_floating();
            ps.sm_reset();
            return;
        }

        // 3. State transition Reviewing -> Injecting.
        if !ps.sm_reviewing_to_injecting() {
            warn!("confirm_from_reviewing: reviewing_to_injecting failed");
            return;
        }

        // 4. Centralized post-delivery cleanup.
        let perf_for_record = ctx.2.clone();
        let t_press_for_record = ctx.3.unwrap_or_else(Instant::now);
        self.finish(
            ps,
            ctx,
            FinishOutcome::Deliver {
                final_text: text,
                hide_review: true,
            },
            "confirm_from_reviewing",
            Some((&perf_for_record, t_press_for_record)),
        )
        .await;
    }

    /// Single authority for post-delivery cleanup ordering. Caller passes the
    /// already-taken context tuple (callers call take_context themselves —
    /// default timing is immediately before finish() to minimize the panic
    /// window; the ONE exception is confirm_from_reviewing which takes at
    /// method top because its pre-inject foreground restore needs ctx.0).
    ///
    /// `site_label` is emitted via `debug!(target: "delivery", ...)` so production
    /// traces stay distinguishable across the 4 historical M2 sites + future ones.
    async fn finish(
        &self,
        ps: &PipelineState,
        ctx: (
            Option<isize>,
            Option<ReviewData>,
            PerfMetrics,
            Option<Instant>,
        ),
        outcome: FinishOutcome,
        site_label: &'static str,
        perf_record_input: Option<(&PerfMetrics, Instant)>,
    ) {
        let (foreground_hwnd, review_data, _ctx_perf, _ctx_t_press) = ctx;
        debug!(target: "delivery", "finish({site_label}): entered");

        match outcome {
            FinishOutcome::Deliver {
                final_text,
                hide_review,
            } => {
                // Caller did save_and_inject + pre-inject foreground restore.
                if !ps.sm_finish_injecting() {
                    // TOCTOU: watchdog reset state machine mid-deliver.
                    // Current source semantics (inject_and_finish): warn
                    // and continue — paste already happened, clipboard already
                    // self-restored by inject_inner.
                    warn!(target: "delivery",
                        "finish({site_label}): sm_finish_injecting failed (TOCTOU) — paste already happened, continuing injection-complete"
                    );
                }
                self.emitter
                    .emit("injection-complete", serde_json::Value::Null);
                if hide_review {
                    self.window_controller.hide_review();
                }
                self.window_controller.hide_floating();
                match self.update_json_deliver(review_data.as_ref(), &final_text) {
                    Ok(()) => {}
                    Err(e) => {
                        warn!(target: "delivery", "finish({site_label}): json update failed: {e}")
                    }
                }
                if let Some((p, t)) = perf_record_input {
                    self.record_perf(p, t);
                }
            }
            FinishOutcome::Cancel => {
                // Cancel from Reviewing: no paste happened.
                self.restore_clipboard_if_saved();
                if let Some(hwnd) = foreground_hwnd {
                    self.window_controller.restore_foreground_hwnd(hwnd);
                }
                self.window_controller.hide_floating();
                self.window_controller.hide_review();
                ps.review.set_shown_on_press(false);
                match self.update_json_cancel(review_data.as_ref()) {
                    Ok(()) => {}
                    Err(e) => {
                        warn!(target: "delivery", "finish({site_label}): cancel json update failed: {e}")
                    }
                }
            }
            FinishOutcome::EarlyStateMismatch => {
                ps.stop_recording_resources_graceful();
                ps.sm_reset();
                self.cleanup_review_ui().await;
            }
        }
        debug!(target: "delivery", "finish({site_label}): done");
    }

    /// Restore clipboard only if save_and_inject (or save alone) was called this
    /// cycle. Uses ClipboardProvider::was_saved() trait method (mirrors
    /// recover() usage in recording_session). No ctx parameter.
    fn restore_clipboard_if_saved(&self) {
        if self.clipboard.was_saved() {
            if let Err(e) = self.clipboard.restore() {
                warn!(target: "delivery", "restore_clipboard_if_saved: restore failed: {e}");
            }
        }
    }

    /// Build + write success JSON. Mirrors the current inject_and_finish pattern.
    /// ReviewData carries raw_transcription + llm_text; only final_text comes from caller.
    fn update_json_deliver(
        &self,
        review_data: Option<&ReviewData>,
        final_text: &str,
    ) -> Result<(), crate::error::AppError> {
        let Some(rd) = review_data else {
            return Ok(());
        };
        crate::data_saving::update_json_with_text(
            &rd.json_path,
            &rd.raw_transcription,
            rd.llm_text.as_deref(),
            Some(final_text),
        )
    }

    /// Build + write cancel JSON (final_text=None per existing cancel_review behavior).
    fn update_json_cancel(
        &self,
        review_data: Option<&ReviewData>,
    ) -> Result<(), crate::error::AppError> {
        let Some(rd) = review_data else {
            return Ok(());
        };
        crate::data_saving::update_json_with_text(
            &rd.json_path,
            &rd.raw_transcription,
            rd.llm_text.as_deref(),
            None,
        )
    }

    /// Record perf + emit perf-metrics event.
    fn record_perf(&self, perf: &PerfMetrics, _t_press: Instant) {
        self.perf_history.record(perf.clone());
        self.emitter.emit(
            "perf-metrics",
            serde_json::to_value(perf).unwrap_or_default(),
        );
        info!("{}", perf.summary());
    }

    /// Save the current clipboard and inject text. Returns `Ok(())` on success
    /// or an error string on failure.
    async fn save_and_inject(&self, text: &str) -> Result<(), String> {
        let cb = self.clipboard.clone();
        let text_for_inject = text.to_string();
        match tokio::task::spawn_blocking(move || {
            cb.save_and_inject(&text_for_inject)
                .map_err(|e| e.to_string())
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
        self.clipboard.restore().map_err(|e| e.to_string())
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

    async fn cleanup_review_ui(&self) {
        self.window_controller.hide_floating();
        self.window_controller.hide_review();
        self.review.set_shown_on_press(false);
    }
}
