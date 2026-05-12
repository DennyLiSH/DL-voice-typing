use crate::state::StateMachine;
use crate::util;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tracing::{error, info, warn};

/// Abstract recovery actions performed when the watchdog forces a reset.
/// Decouples the watchdog from Tauri-specific window names and tooltip text.
pub trait RecoveryActions: Send + Sync {
    /// Hide the floating recording indicator window.
    fn hide_floating_window(&self);
    /// Hide the review-before-paste window.
    fn hide_review_window(&self);
    /// Emit a reset event so the frontend can update its state.
    fn emit_watchdog_reset(&self);
    /// Update the tray tooltip to indicate automatic recovery.
    fn set_tray_recovered(&self);
}

/// Tauri-based implementation of recovery actions.
pub struct TauriRecoveryActions {
    app: AppHandle,
}

impl TauriRecoveryActions {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl RecoveryActions for TauriRecoveryActions {
    fn hide_floating_window(&self) {
        if let Some(win) = self.app.get_webview_window("floating") {
            let _ = win.hide();
        }
    }

    fn hide_review_window(&self) {
        if let Some(win) = self.app.get_webview_window("review") {
            let _ = win.hide();
        }
    }

    fn emit_watchdog_reset(&self) {
        let _ = self.app.emit("watchdog-reset", ());
    }

    fn set_tray_recovered(&self) {
        if let Some(tray) = self.app.tray_by_id("default") {
            let _ = tray.set_tooltip(Some("语文兔 - 已自动恢复"));
        }
    }
}

/// State machine watchdog: periodically checks if the state machine is stuck
/// in a non-Idle state and forcibly resets it after a threshold.
pub struct Watchdog {
    sm: Arc<Mutex<StateMachine>>,
    recovery: Arc<dyn RecoveryActions + Send + Sync>,
    check_interval: Duration,
    stuck_threshold: Duration,
    last_non_idle_at: Option<Instant>,
}

impl Watchdog {
    pub fn new(
        sm: Arc<Mutex<StateMachine>>,
        recovery: Arc<dyn RecoveryActions + Send + Sync>,
        check_interval: Duration,
        stuck_threshold: Duration,
    ) -> Self {
        Self {
            sm,
            recovery,
            check_interval,
            stuck_threshold,
            last_non_idle_at: None,
        }
    }

    /// Run the watchdog loop. Should be spawned on a dedicated thread.
    pub fn run(mut self) {
        info!(
            "Watchdog started: check_interval={:?}, stuck_threshold={:?}",
            self.check_interval, self.stuck_threshold
        );
        loop {
            std::thread::sleep(self.check_interval);
            self.check();
        }
    }

    fn check(&mut self) {
        let guard = util::lock_mutex(&self.sm, "state_machine_watchdog");
        let Some(ref sm) = guard else {
            error!("Watchdog: state_machine lock poisoned, cannot check state");
            return;
        };

        let state_name = sm.state_name();
        let is_idle = matches!(sm.state(), crate::state::AppState::Idle);

        if is_idle {
            if self.last_non_idle_at.take().is_some() {
                info!("Watchdog: state recovered to Idle");
            }
            return;
        }

        // Non-Idle state
        let elapsed = match self.last_non_idle_at {
            Some(t) => t.elapsed(),
            None => {
                self.last_non_idle_at = Some(Instant::now());
                info!("Watchdog: detected non-Idle state: {state_name}");
                return;
            }
        };

        if elapsed >= self.stuck_threshold {
            error!(
                "Watchdog: state machine stuck in {state_name} for {:?}, forcing reset",
                elapsed
            );
            // Force reset: drop the guard to release the lock before calling reset helpers
            drop(guard);
            self.force_reset();
        } else {
            warn!(
                "Watchdog: state machine in {state_name} for {:?}, waiting...",
                elapsed
            );
        }
    }

    fn force_reset(&self) {
        // Use try_lock instead of blocking lock_mutex to avoid the watchdog
        // itself hanging when the state machine lock is deadlocked by another thread.
        match self.sm.try_lock() {
            Ok(mut sm) => {
                sm.reset();
                info!("Watchdog: state machine forcibly reset to Idle");
            }
            Err(e) => {
                error!("Watchdog: failed to acquire state_machine lock for reset: {e}");
                error!("Watchdog: state machine lock may be deadlocked!");
            }
        }
        // Always perform UI recovery even if lock acquisition failed.
        self.recovery.hide_floating_window();
        self.recovery.hide_review_window();
        self.recovery.emit_watchdog_reset();
        self.recovery.set_tray_recovered();
    }
}
