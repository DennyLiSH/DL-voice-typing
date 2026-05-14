//! Utility helpers for lock management and state machine transitions.

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing::warn;

/// Lock a Mutex, returning None and logging a warning if poisoned.
pub fn lock_mutex<'a, T: ?Sized>(m: &'a Mutex<T>, label: &str) -> Option<MutexGuard<'a, T>> {
    m.lock()
        .inspect_err(|e| warn!(target: "lock", "{label} poisoned: {e}"))
        .ok()
}

/// Read-lock an RwLock, returning None and logging a warning if poisoned.
#[allow(dead_code)]
pub fn read_rwlock<'a, T>(m: &'a RwLock<T>, label: &str) -> Option<RwLockReadGuard<'a, T>> {
    m.read()
        .inspect_err(|e| warn!(target: "lock", "{label} read poisoned: {e}"))
        .ok()
}

/// Write-lock an RwLock, returning None and logging a warning if poisoned.
#[allow(dead_code)]
pub fn write_rwlock<'a, T>(m: &'a RwLock<T>, label: &str) -> Option<RwLockWriteGuard<'a, T>> {
    m.write()
        .inspect_err(|e| warn!(target: "lock", "{label} write poisoned: {e}"))
        .ok()
}

/// Wrap a state machine transition with tracing on failure.
/// Usage: `transition!(sm, start_recording)` or `transition!(sm, llm_to_injecting, text)`
#[macro_export]
macro_rules! transition {
    ($sm:expr, $method:ident $(, $arg:expr)*) => {{
        let result = $sm.$method($($arg),*);
        if let Err(ref e) = result {
            tracing::warn!("transition {} failed: {e}", stringify!($method));
        }
        result
    }};
}
