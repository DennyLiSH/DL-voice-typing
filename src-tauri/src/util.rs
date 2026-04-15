//! Utility helpers for lock management.

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing::warn;

/// Lock a Mutex, returning None and logging a warning if poisoned.
pub fn lock_mutex<'a, T>(m: &'a Mutex<T>, label: &str) -> Option<MutexGuard<'a, T>> {
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
