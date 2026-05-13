use crate::state::StateMachine;
use std::sync::atomic::{AtomicBool, Ordering};
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
    stopped: Arc<AtomicBool>,
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
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Run the watchdog loop. Should be spawned on a dedicated thread.
    /// Exits when `stop()` is called.
    pub fn run(mut self) {
        info!(
            "Watchdog started: check_interval={:?}, stuck_threshold={:?}",
            self.check_interval, self.stuck_threshold
        );
        loop {
            if self.stopped.load(Ordering::Relaxed) {
                info!("Watchdog: stopped");
                return;
            }
            std::thread::sleep(self.check_interval);
            self.tick(Instant::now());
        }
    }

    /// Single check cycle. Public for testing.
    /// `now`: injectable clock for deterministic testing.
    pub fn tick(&mut self, now: Instant) {
        let guard = self.sm.try_lock();
        let Ok(sm) = guard else {
            warn!("Watchdog: state_machine lock busy/unavailable, skipping check");
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
            Some(t) => now.duration_since(t),
            None => {
                self.last_non_idle_at = Some(now);
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
            drop(sm);
            self.force_reset();
        } else {
            warn!(
                "Watchdog: state machine in {state_name} for {:?}, waiting...",
                elapsed
            );
        }
    }

    /// Signal the watchdog to stop.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
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

#[cfg(test)]
mod tests {
    use super::*;

    struct MockRecovery {
        actions: Arc<Mutex<Vec<String>>>,
    }

    impl MockRecovery {
        fn new(actions: Arc<Mutex<Vec<String>>>) -> Self {
            Self { actions }
        }
    }

    impl RecoveryActions for MockRecovery {
        fn hide_floating_window(&self) {
            self.actions.lock().unwrap().push("hide_floating".into());
        }
        fn hide_review_window(&self) {
            self.actions.lock().unwrap().push("hide_review".into());
        }
        fn emit_watchdog_reset(&self) {
            self.actions.lock().unwrap().push("emit_reset".into());
        }
        fn set_tray_recovered(&self) {
            self.actions.lock().unwrap().push("set_tray".into());
        }
    }

    fn make_watchdog() -> (Watchdog, Arc<Mutex<Vec<String>>>) {
        let sm = Arc::new(Mutex::new(StateMachine::new()));
        let actions = Arc::new(Mutex::new(Vec::new()));
        let recovery = Arc::new(MockRecovery::new(actions.clone()));
        let wd = Watchdog::new(
            sm,
            recovery,
            Duration::from_secs(10),
            Duration::from_secs(30),
        );
        (wd, actions)
    }

    #[test]
    fn test_tick_idle_does_nothing() {
        let (mut wd, actions) = make_watchdog();
        let t0 = Instant::now();
        wd.tick(t0);
        assert!(actions.lock().unwrap().is_empty());
    }

    #[test]
    fn test_tick_non_idle_first_detect() {
        let (mut wd, actions) = make_watchdog();
        wd.sm.lock().unwrap().start_recording().unwrap();
        let t0 = Instant::now();
        wd.tick(t0);
        // First non-idle detection: sets timestamp but does NOT reset.
        assert!(wd.last_non_idle_at.is_some());
        assert!(actions.lock().unwrap().is_empty());
    }

    #[test]
    fn test_tick_stuck_forces_reset() {
        let (mut wd, actions) = make_watchdog();
        wd.sm.lock().unwrap().start_recording().unwrap();
        let t0 = Instant::now();
        wd.tick(t0); // sets last_non_idle_at
        // Advance past stuck threshold (30s).
        let t1 = t0 + Duration::from_secs(31);
        wd.tick(t1);
        // Should have forced reset and performed recovery actions.
        let actions = actions.lock().unwrap();
        assert!(actions.contains(&"hide_floating".to_string()));
        assert!(actions.contains(&"emit_reset".to_string()));
        assert!(actions.contains(&"set_tray".to_string()));
        assert!(matches!(
            wd.sm.lock().unwrap().state(),
            crate::state::AppState::Idle
        ));
    }

    #[test]
    fn test_tick_before_threshold_waits() {
        let (mut wd, actions) = make_watchdog();
        wd.sm.lock().unwrap().start_recording().unwrap();
        let t0 = Instant::now();
        wd.tick(t0);
        // Not yet stuck (only 10s).
        let t1 = t0 + Duration::from_secs(10);
        wd.tick(t1);
        assert!(actions.lock().unwrap().is_empty());
    }

    #[test]
    fn test_recovery_to_idle_clears_timestamp() {
        let (mut wd, _) = make_watchdog();
        wd.sm.lock().unwrap().start_recording().unwrap();
        let t0 = Instant::now();
        wd.tick(t0);
        assert!(wd.last_non_idle_at.is_some());
        // State recovers to idle.
        wd.sm.lock().unwrap().reset();
        wd.tick(t0 + Duration::from_secs(1));
        assert!(wd.last_non_idle_at.is_none());
    }

    #[test]
    fn test_stop_flag() {
        let wd = Watchdog::new(
            Arc::new(Mutex::new(StateMachine::new())),
            Arc::new(MockRecovery::new(Arc::new(Mutex::new(Vec::new())))),
            Duration::from_secs(1),
            Duration::from_secs(30),
        );
        assert!(!wd.stopped.load(Ordering::Relaxed));
        wd.stop();
        assert!(wd.stopped.load(Ordering::Relaxed));
    }
}
