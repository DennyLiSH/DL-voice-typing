use crate::commands::pipeline_state::PipelineState;
use crate::state::StateTag;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

/// State machine watchdog: periodically checks if the state machine is stuck
/// in a non-Idle state and forcibly resets it after a threshold.
///
/// Holds `Arc<PipelineState>` — no AppHandle. The stuck-reset routes through
/// the full-reset recipe verb `PipelineState::try_recover_to_idle()` (resource
/// drains → non-blocking sm reset → overlay hides), while the caller-facing
/// signals stay HERE at the call site (the recipe deliberately owns no events
/// or tooltip): `watchdog-reset` via the PipelineState emitter, tray tooltip
/// via the PipelineState window controller. `tick`'s state read is
/// non-blocking (`try_sm_state`) — a contended lock skips the check
/// (2026-05-13 decision), so the periodic check never blocks. The
/// forced-reset path retains an accepted blocking exposure: the recipe's
/// record-only recover reads the sm with blocking locks (see
/// `force_reset`).
pub struct Watchdog {
    ps: Arc<PipelineState>,
    check_interval: Duration,
    stuck_threshold: Duration,
    last_non_idle_at: Option<Instant>,
    stopped: Arc<AtomicBool>,
}

impl Watchdog {
    /// Create a new watchdog for the given pipeline state, with timing parameters.
    pub fn new(
        ps: Arc<PipelineState>,
        check_interval: Duration,
        stuck_threshold: Duration,
    ) -> Self {
        Self {
            ps,
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
        // Non-blocking state read: a contended or poisoned lock skips this
        // check entirely (the warn lives in try_sm_state). The lock is
        // released by the time force_reset runs, so force_reset's own
        // internal sm access cannot self-deadlock.
        let Some(tag) = self.ps.try_sm_state() else {
            return;
        };

        if matches!(tag, StateTag::Idle) {
            if self.last_non_idle_at.take().is_some() {
                info!("Watchdog: state recovered to Idle");
            }
            return;
        }

        // RecordOnly is a legitimate long-lived state (meetings can run for
        // tens of minutes): exempt it from the stuck threshold. Clear any
        // stale timer so a following non-Idle state gets a fresh window.
        if matches!(tag, StateTag::RecordOnly) {
            self.last_non_idle_at = None;
            return;
        }

        // Non-Idle state
        let elapsed = match self.last_non_idle_at {
            Some(t) => now.duration_since(t),
            None => {
                self.last_non_idle_at = Some(now);
                info!("Watchdog: detected non-Idle state: {tag:?}");
                return;
            }
        };

        if elapsed >= self.stuck_threshold {
            error!(
                "Watchdog: state machine stuck in {tag:?} for {:?}, forcing reset",
                elapsed
            );
            self.force_reset();
        } else {
            warn!(
                "Watchdog: state machine in {tag:?} for {:?}, waiting...",
                elapsed
            );
        }
    }

    /// Signal the watchdog to stop.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }

    fn force_reset(&mut self) {
        // Full-reset recipe verb: resource drains (realtime-leftover stop,
        // audio capture stop, record-only session recover, cancel-token
        // drain, gated clipboard restore) → sm try_lock reset (warn and
        // continue on contention) → hide floating/review overlays. The
        // tick READ above is try_lock — never blocking. The reset path's
        // sm reset is likewise try_lock (lock failure degrades to UI-only
        // recovery), but RecordOnlySession::recover's internal sm reads
        // remain a blocking exposure — accepted, same as before this
        // refactor: a deadlocked sm can still hang this reset.
        let reset_ok = self.ps.try_recover_to_idle();
        if reset_ok {
            info!("Watchdog: state machine forcibly reset to Idle");
        } else {
            error!("Watchdog: state machine lock may be deadlocked!");
        }
        // Caller-facing signals stay at the call site (the recipe owns only
        // resource cleanup): reset event for the frontend + tray tooltip.
        self.ps
            .emitter()
            .emit("watchdog-reset", serde_json::Value::Null);
        self.ps
            .window_controller()
            .set_tray_tooltip("语文兔 - 已自动恢复");
        // Clear the stuck timer so a fresh recording doesn't immediately
        // trigger another reset.
        self.last_non_idle_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::MockEmitter;
    use crate::commands::pipeline_state::TestComponents;
    use crate::commands::window_controller::WindowController;
    use std::sync::Mutex;

    /// Test-only WindowController recording method calls by name. Tooltip
    /// calls record their payload so the recovered-tooltip text stays pinned
    /// to the watchdog call site.
    struct RecordingWindowController {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl WindowController for RecordingWindowController {
        fn show_floating_near_caret(&self) -> bool {
            false
        }
        fn hide_floating(&self) {
            self.calls.lock().unwrap().push("hide_floating".into());
        }
        fn show_review_near_caret(&self) -> bool {
            false
        }
        fn hide_review(&self) {
            self.calls.lock().unwrap().push("hide_review".into());
        }
        fn set_review_text(&self, _text: &str) -> bool {
            false
        }
        fn emit_review_final_text(&self, _text: &str) {}
        fn restore_foreground_hwnd(&self, _hwnd: isize) {}
        fn show_floating_corner(&self) -> bool {
            false
        }
        fn set_tray_tooltip(&self, tooltip: &str) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("set_tray_tooltip:{tooltip}"));
        }
    }

    /// Fixture bundle: watchdog + the PipelineState it monitors + the
    /// handles the assertions read (emitter event log, window-controller
    /// call log).
    struct Fixture {
        wd: Watchdog,
        ps: Arc<PipelineState>,
        emitter: Arc<MockEmitter>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    fn make_watchdog() -> Fixture {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let wc: Arc<dyn WindowController> = Arc::new(RecordingWindowController {
            calls: calls.clone(),
        });
        let emitter = Arc::new(MockEmitter::new());
        let ps = Arc::new(
            TestComponents::default()
                .with_window_controller(wc)
                .with_emitter(emitter.clone())
                .build(),
        );
        let wd = Watchdog::new(ps.clone(), Duration::from_secs(10), Duration::from_secs(30));
        Fixture {
            wd,
            ps,
            emitter,
            calls,
        }
    }

    #[test]
    fn test_tick_idle_does_nothing() {
        let Fixture {
            mut wd,
            ps: _ps,
            emitter,
            calls,
        } = make_watchdog();
        let t0 = Instant::now();
        wd.tick(t0);
        assert!(emitter.take_events().is_empty());
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn test_tick_non_idle_first_detect() {
        let Fixture {
            mut wd,
            ps,
            emitter,
            calls,
        } = make_watchdog();
        assert!(ps.sm_start_recording());
        let t0 = Instant::now();
        wd.tick(t0);
        // First non-idle detection: sets timestamp but does NOT reset.
        assert!(wd.last_non_idle_at.is_some());
        assert!(calls.lock().unwrap().is_empty());
        assert!(emitter.take_events().is_empty());
    }

    #[test]
    fn test_tick_stuck_forces_reset() {
        let Fixture {
            mut wd,
            ps,
            emitter,
            calls,
        } = make_watchdog();
        assert!(ps.sm_start_recording());
        let t0 = Instant::now();
        wd.tick(t0); // sets last_non_idle_at
        // Advance past stuck threshold (30s).
        let t1 = t0 + Duration::from_secs(31);
        wd.tick(t1);
        // Recovery routed through PipelineState: overlay hides recorded by
        // the mock WindowController (floating + the unconditional review
        // hide), tooltip text stays at the watchdog call site, and the
        // reset event goes out through the PipelineState emitter.
        let calls = calls.lock().unwrap();
        assert!(calls.contains(&"hide_floating".to_string()));
        assert!(calls.contains(&"hide_review".to_string()));
        assert!(calls.contains(&"set_tray_tooltip:语文兔 - 已自动恢复".to_string()));
        drop(calls);
        let events = emitter.take_events();
        assert!(
            events.iter().any(|(name, _)| name == "watchdog-reset"),
            "expected watchdog-reset event, got {events:?}"
        );
        // State machine back to Idle; stuck timer cleared after force reset.
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));
        assert!(wd.last_non_idle_at.is_none());
    }

    #[test]
    fn test_tick_before_threshold_waits() {
        let Fixture {
            mut wd,
            ps,
            emitter,
            calls,
        } = make_watchdog();
        assert!(ps.sm_start_recording());
        let t0 = Instant::now();
        wd.tick(t0);
        // Not yet stuck (only 10s).
        let t1 = t0 + Duration::from_secs(10);
        wd.tick(t1);
        assert!(calls.lock().unwrap().is_empty());
        assert!(emitter.take_events().is_empty());
    }

    #[test]
    fn test_recovery_to_idle_clears_timestamp() {
        let Fixture {
            mut wd,
            ps,
            emitter: _emitter,
            calls: _calls,
        } = make_watchdog();
        assert!(ps.sm_start_recording());
        let t0 = Instant::now();
        wd.tick(t0);
        assert!(wd.last_non_idle_at.is_some());
        // State recovers to idle.
        ps.sm_reset();
        wd.tick(t0 + Duration::from_secs(1));
        assert!(wd.last_non_idle_at.is_none());
    }

    #[test]
    fn test_stop_flag() {
        let Fixture {
            wd,
            ps: _ps,
            emitter: _emitter,
            calls: _calls,
        } = make_watchdog();
        assert!(!wd.stopped.load(Ordering::Relaxed));
        wd.stop();
        assert!(wd.stopped.load(Ordering::Relaxed));
    }

    #[test]
    fn test_record_only_is_exempt_from_stuck_reset() {
        let Fixture {
            mut wd,
            ps,
            emitter,
            calls,
        } = make_watchdog();
        assert!(ps.sm_start_record_only());
        let t0 = Instant::now();
        wd.tick(t0);
        // Well past the stuck threshold: RecordOnly must NOT trigger reset.
        wd.tick(t0 + Duration::from_secs(300));
        assert!(calls.lock().unwrap().is_empty());
        assert!(emitter.take_events().is_empty());
        assert!(wd.last_non_idle_at.is_none());
    }

    #[test]
    fn test_record_only_clears_stale_timer_from_prior_state() {
        let Fixture {
            mut wd,
            ps,
            emitter,
            calls,
        } = make_watchdog();
        // Simulate a prior non-Idle state that started the stuck timer.
        assert!(ps.sm_start_recording());
        let t0 = Instant::now();
        wd.tick(t0);
        assert!(wd.last_non_idle_at.is_some());
        // Transition into RecordOnly: timer must be cleared, no reset.
        ps.sm_reset();
        assert!(ps.sm_start_record_only());
        wd.tick(t0 + Duration::from_secs(300));
        assert!(calls.lock().unwrap().is_empty());
        assert!(emitter.take_events().is_empty());
        assert!(wd.last_non_idle_at.is_none());
    }

    /// Pins the 2026-05-13 non-blocking decision: a contended state-machine
    /// lock makes tick() skip the check (no force reset even past the
    /// threshold, timer preserved) instead of blocking on the lock.
    #[test]
    fn test_tick_skips_check_when_sm_lock_busy() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let wc: Arc<dyn WindowController> = Arc::new(RecordingWindowController {
            calls: calls.clone(),
        });
        let emitter = Arc::new(MockEmitter::new());
        let components = TestComponents::default()
            .with_window_controller(wc)
            .with_emitter(emitter.clone());
        // build() consumes the components; keep a shared handle on the sm
        // so this test can hold its lock to simulate contention.
        let sm = components.sm.clone();
        let ps = Arc::new(components.build());
        assert!(ps.sm_start_recording());
        let mut wd = Watchdog::new(ps.clone(), Duration::from_secs(10), Duration::from_secs(30));
        let t0 = Instant::now();
        wd.tick(t0);
        assert!(wd.last_non_idle_at.is_some());
        // Hold the state-machine lock; tick past the threshold must SKIP
        // (try_sm_state yields None) rather than block or force a reset.
        let sm_guard = sm.lock().unwrap();
        wd.tick(t0 + Duration::from_secs(31));
        drop(sm_guard);
        assert!(calls.lock().unwrap().is_empty());
        assert!(emitter.take_events().is_empty());
        // The timer survives the skipped check: the next uncontended tick
        // (32s >= 30s threshold) forces the reset.
        wd.tick(t0 + Duration::from_secs(32));
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));
        assert!(wd.last_non_idle_at.is_none());
    }
}
