//! Utility helpers for lock management.

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing::warn;

/// A slot whose value is claimed, taken or released exactly once.
///
/// Two lifecycles need it, and both used to hand-roll the same mutex
/// choreography:
///
/// - the transcribe window's target HWND: captured when the window
///   opens, READ once by the inject command (a second inject without
///   reopening finds nothing), cleared when the window closes;
/// - the in-flight cancel token: claimed before the first await,
///   flipped from another thread, released on EVERY exit path
///   (including the task future being dropped mid-await).
///
/// The contract:
/// - `claim` stores a value only into an empty slot (the "one live
///   owner" rule),
/// - `take` removes and returns the value — the take-once read,
/// - `snapshot` reads without consuming,
/// - `replace` overwrites (the HWND slot re-captures its target on
///   every window open),
/// - `clear` removes without returning (the release path).
///
/// A poisoned lock is reported as "no value" — the same conservative
/// reading the rest of the codebase uses for poisoned slots.
pub struct TakeOnceSlot<T> {
    inner: Mutex<Option<T>>,
    label: &'static str,
}

impl<T> TakeOnceSlot<T> {
    /// Create an empty slot. `label` names the lock in the poison warning.
    pub fn new(label: &'static str) -> Self {
        Self {
            inner: Mutex::new(None),
            label,
        }
    }

    /// Store `value` only when the slot is empty.
    ///
    /// @returns false when the slot was already occupied
    pub fn claim(&self, value: T) -> bool {
        let Some(mut guard) = lock_mutex(&self.inner, self.label) else {
            return false;
        };
        if guard.is_some() {
            return false;
        }
        *guard = Some(value);
        true
    }

    /// Store `value`, replacing any previous one.
    ///
    /// @returns the previous value
    pub fn replace(&self, value: T) -> Option<T> {
        let mut guard = lock_mutex(&self.inner, self.label)?;
        guard.replace(value)
    }

    /// Remove and return the value — the take-once read.
    pub fn take(&self) -> Option<T> {
        let mut guard = lock_mutex(&self.inner, self.label)?;
        guard.take()
    }

    /// Read the value without consuming it.
    pub fn snapshot(&self) -> Option<T>
    where
        T: Clone,
    {
        let guard = lock_mutex(&self.inner, self.label)?;
        guard.as_ref().cloned()
    }

    /// Remove the value without returning it (the release path).
    pub fn clear(&self) {
        if let Some(mut guard) = lock_mutex(&self.inner, self.label) {
            guard.take();
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::TakeOnceSlot;

    #[test]
    fn take_once_slot_claims_only_into_an_empty_slot() {
        let slot = TakeOnceSlot::new("t");
        assert!(slot.claim(1));
        // A second claim is refused — the slot has one live owner.
        assert!(!slot.claim(2));
        assert_eq!(slot.snapshot(), Some(1));
    }

    #[test]
    fn take_once_slot_takes_the_value_out_once() {
        let slot = TakeOnceSlot::new("t");
        slot.replace(7);
        assert_eq!(slot.take(), Some(7));
        // The take-once read: a second take finds nothing.
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn take_once_slot_replace_overwrites_and_clear_empties() {
        let slot = TakeOnceSlot::new("t");
        slot.replace(1);
        assert_eq!(slot.replace(2), Some(1));
        assert_eq!(slot.snapshot(), Some(2));
        slot.clear();
        assert_eq!(slot.snapshot(), None);
    }
}
