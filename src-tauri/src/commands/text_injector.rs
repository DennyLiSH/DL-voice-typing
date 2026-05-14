use crate::clipboard::ClipboardProvider;
use crate::config::AppConfig;
use crate::data_saving::SaveResult;
use crate::perf::PerfMetrics;
use std::time::Instant;
use tracing::{error, info, warn};

use super::pipeline_state::PipelineState;

/// Context for a text injection operation.
pub(crate) struct InjectionContext<'a> {
    pub text: String,
    pub transcription: String,
    pub save_result: Option<SaveResult>,
    pub config: &'a AppConfig,
    pub perf: &'a mut PerfMetrics,
    pub t_press_for_e2e: Instant,
}

/// Execute the full injection sequence:
/// state→Injecting → clipboard save+paste → state→Idle → events → data saving → perf.
///
/// The caller is responsible for the state transition *into* Injecting before calling this.
/// This function handles everything from the clipboard operation through returning to Idle.
pub(crate) async fn inject_text(ps: &PipelineState, ctx: &mut InjectionContext<'_>) {
    let t_inject = Instant::now();
    {
        let cb_for_inject = ps.clipboard.clone();
        let text_for_inject = ctx.text.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            let mut cb = cb_for_inject.lock().map_err(|e| e.to_string())?;
            cb.save().map_err(|e| e.to_string())?;
            cb.inject_text(&text_for_inject).map_err(|e| e.to_string())
        })
        .await
        {
            Ok(Ok(())) => {
                info!("inject_text: injection succeeded");
            }
            Ok(Err(e)) => {
                warn!("inject_text: injection failed: {e}");
                ps.emitter.emit(
                    "injection-error",
                    serde_json::to_value(e).unwrap_or_default(),
                );
            }
            Err(e) => {
                error!("inject_text: injection task panicked: {e}");
                ps.emitter.emit(
                    "injection-error",
                    serde_json::to_value(e.to_string()).unwrap_or_default(),
                );
            }
        }
    }
    ctx.perf.injection_ms = Some(t_inject.elapsed().as_millis() as u64);
    ctx.perf.end_to_end_ms = Some(ctx.t_press_for_e2e.elapsed().as_millis() as u64);
    ctx.perf.text_length = ctx.text.len();

    if let Some(mut s) = crate::util::lock_mutex(&ps.sm, "state_machine") {
        let _ = s.finish_injecting();
    }
    ps.emitter
        .emit("injection-complete", serde_json::Value::Null);

    ps.window_controller.hide_floating();

    // Update data_saving JSON with transcription.
    if let Some(sr) = ctx.save_result.take() {
        let llm_text = if ctx.config.llm_enabled {
            Some(ctx.text.as_str())
        } else {
            None
        };
        let _ = crate::data_saving::update_json_with_text(
            &sr.json_path,
            &ctx.transcription,
            llm_text,
            Some(&ctx.text),
        );
    }

    ps.perf_history.record(ctx.perf.clone());
    ps.emitter.emit(
        "perf-metrics",
        serde_json::to_value(&*ctx.perf).unwrap_or_default(),
    );
    info!("{}", ctx.perf.summary());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::MockAudioCapture;
    use crate::clipboard::{AnyClipboard, MockClipboard};
    use crate::commands::review_provider::MockReviewProvider;
    use crate::commands::window_controller::NoopWindowController;
    use crate::commands::EventEmitter;
    use crate::config::AppConfig;
    use crate::llm::{AnyCorrector, MockCorrector};
    use crate::perf::PerfHistory;
    use crate::speech::{AnyEngine, mock::MockEngine};
    use crate::state::StateMachine;
    use std::sync::{Arc, Mutex, RwLock};

    fn build_ps() -> PipelineState {
        let sm = Arc::new(Mutex::new(StateMachine::new()));
        let ac = Arc::new(Mutex::new(MockAudioCapture::new()));
        let engine = Arc::new(Mutex::new(AnyEngine::Mock(MockEngine::new("test"))));
        let clipboard = Arc::new(Mutex::new(AnyClipboard::Mock(MockClipboard::new())));

        let emitter: Arc<dyn EventEmitter> = Arc::new(crate::commands::MockEmitter::new());
        PipelineState::new(
            sm,
            ac,
            engine,
            clipboard,
            Arc::new(PerfHistory::new()),
            Arc::new(RwLock::new(AppConfig::default())),
            Arc::new(Mutex::new(Some(AnyCorrector::Mock(MockCorrector::new("corrected"))))),
            Arc::new(Mutex::new(None)),
            Arc::new(NoopWindowController),
            emitter,
            Arc::new(MockReviewProvider::new()),
        )
    }

    #[tokio::test]
    async fn test_inject_success() {
        let ps = build_ps();
        // Set state to Injecting
        {
            let mut sm = ps.sm.lock().unwrap();
            sm.start_recording().unwrap();
            sm.stop_recording().unwrap();
            sm.add_partial_result("test".to_string()).unwrap();
            sm.transcribing_to_injecting("test".to_string()).unwrap();
        }

        let mut perf = PerfMetrics::new(0);
        let mut ctx = InjectionContext {
            text: "hello world".to_string(),
            transcription: "hello world".to_string(),
            save_result: None,
            config: &AppConfig::default(),
            perf: &mut perf,
            t_press_for_e2e: Instant::now(),
        };

        inject_text(&ps, &mut ctx).await;

        // State should be Idle
        let sm = ps.sm.lock().unwrap();
        assert!(matches!(sm.state(), crate::state::AppState::Idle));
        // Perf metrics should be set
        assert!(ctx.perf.injection_ms.is_some());
        assert!(ctx.perf.end_to_end_ms.is_some());
        assert_eq!(ctx.perf.text_length, 11);
    }
}
