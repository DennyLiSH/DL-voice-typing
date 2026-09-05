use crate::error::AppError;
use crate::hotkey::{CancelEscCallback, HotkeyCallback, HotkeyEvent, HotkeyManager};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, HHOOK, KBDLLHOOKSTRUCT, KBDLLHOOKSTRUCT_FLAGS, LLKHF_INJECTED,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
};

/// Hotkey specification. Serialises as an object (`{"ctrl":..,"shift":..,"alt":..,"vk":..}`)
/// on save and accepts either object form OR a legacy string (`"RightCtrl"`) on load
/// — the dual-form Deserialize auto-migrates older config files without a separate
/// migration hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub struct HotkeySpec {
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub vk: u32,
}

impl fmt::Display for HotkeySpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

#[derive(Deserialize)]
struct HotkeySpecPlain {
    #[serde(default)]
    ctrl: bool,
    #[serde(default)]
    shift: bool,
    #[serde(default)]
    alt: bool,
    #[serde(default)]
    vk: u32,
}

impl<'de> Deserialize<'de> for HotkeySpec {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        match v {
            serde_json::Value::Object(_) => serde_json::from_value::<HotkeySpecPlain>(v)
                .map(|p| HotkeySpec {
                    ctrl: p.ctrl,
                    shift: p.shift,
                    alt: p.alt,
                    vk: p.vk,
                })
                .map_err(serde::de::Error::custom),
            serde_json::Value::String(s) => crate::hotkey::from_key_name(&s)
                .map(|vk| HotkeySpec {
                    ctrl: false,
                    shift: false,
                    alt: false,
                    vk,
                })
                .ok_or_else(|| serde::de::Error::custom(format!("unknown hotkey: {s}"))),
            _ => Err(serde::de::Error::custom(
                "hotkey must be an object or a legacy key-name string",
            )),
        }
    }
}

impl HotkeySpec {
    /// Render the spec as a human-readable string. Examples:
    /// `RightCtrl` (no modifiers), `Ctrl+Shift+A`, `Ctrl+F9`.
    /// Unknown vk renders as the `"VK0xNN"` form (already produced by
    /// `vk_to_key_name`) so the user sees their actual stored value
    /// rather than a silent rewrite.
    pub fn display(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        parts.push(crate::hotkey::vk_to_key_name(self.vk));
        parts.join("+")
    }

    /// True iff the vk is a Ctrl key (left or right). Used by the modifier
    /// self-family exclusion so the default config (RightCtrl alone) still
    /// matches when the user presses RightCtrl.
    pub fn vk_is_ctrl(vk: u32) -> bool {
        vk == 0xA2 || vk == 0xA3
    }
    pub fn vk_is_shift(vk: u32) -> bool {
        vk == 0xA0 || vk == 0xA1
    }
    pub fn vk_is_alt(vk: u32) -> bool {
        vk == 0xA4 || vk == 0xA5
    }
}

type SlotCallback = Arc<dyn Fn(HotkeyEvent) + Send + Sync>;
type CancelSlot = Arc<dyn Fn() -> bool + Send + Sync>;

/// Global state shared between WindowsHotkeyManager and the hook procedure.
/// Three slots (primary voice-input hotkey + record-only hotkey + cancel-Esc
/// dispatcher) dispatched from a single low-level hook.
#[derive(Default)]
struct HookState {
    primary: Option<(HotkeySpec, SlotCallback)>,
    record_only: Option<(HotkeySpec, SlotCallback)>,
    cancel_esc: Option<CancelSlot>,
}

/// Per-modifier down state. Reset on every key event by the hook proc via
/// `update_modifiers`. Stays process-local — never crosses threads.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
struct ModifiersDown {
    ctrl: bool,
    shift: bool,
    alt: bool,
}

fn is_ctrl_vk(vk: u32) -> bool {
    vk == 0xA2 || vk == 0xA3
}
fn is_shift_vk(vk: u32) -> bool {
    vk == 0xA0 || vk == 0xA1
}
fn is_alt_vk(vk: u32) -> bool {
    vk == 0xA4 || vk == 0xA5
}

/// Update the modifier down state for one key event. Pure function — the
/// hook proc calls it on every event. Key-repeat (consecutive keydowns for
/// the same modifier) is idempotent. Non-modifier keys are a no-op.
fn update_modifiers(mods: &mut ModifiersDown, vk: u32, is_keydown: bool) {
    if is_ctrl_vk(vk) {
        mods.ctrl = is_keydown;
    } else if is_shift_vk(vk) {
        mods.shift = is_keydown;
    } else if is_alt_vk(vk) {
        mods.alt = is_keydown;
    }
}

/// True iff the held modifiers satisfy `spec`'s modifier flags.
///
/// Self-family exclusion: if `spec.vk` IS a Ctrl key, do not compare
/// `mods.ctrl` — because pressing the spec's own vk sets `mods.ctrl`,
/// the default RightCtrl-alone config would never match without this.
/// Symmetric for shift / alt.
fn modifiers_match(spec: &HotkeySpec, mods: ModifiersDown) -> bool {
    spec.ctrl == (mods.ctrl && !is_ctrl_vk(spec.vk))
        && spec.shift == (mods.shift && !is_shift_vk(spec.vk))
        && spec.alt == (mods.alt && !is_alt_vk(spec.vk))
}

/// Three-slot emptiness check used to decide whether the underlying
/// Windows hook can be removed. Pure function — operates on a local
/// `HookState` so tests do not need to touch the process-level
/// `HOOK_STATE` static.
fn all_slots_empty(hs: &HookState) -> bool {
    hs.primary.is_none() && hs.record_only.is_none() && hs.cancel_esc.is_none()
}

/// Symmetric cross-slot conflict check. Called from both `register()`
/// and `register_record_only()` so a new spec is rejected when it equals
/// the OTHER slot's spec. `self_slot` tells the helper which slot the
/// caller is filling in (so it only inspects the opposite slot).
/// Pure function on `&HookState` — enables direct unit tests without
/// driving the Win32 hook install path.
fn validate_no_conflict(
    spec: HotkeySpec,
    hs: &HookState,
    self_slot: SlotKind,
) -> Result<(), AppError> {
    match self_slot {
        SlotKind::Primary => {
            if let Some((other, _)) = &hs.record_only {
                if *other == spec {
                    return Err(AppError::Hotkey(
                        "primary hotkey must differ from the record-only hotkey".to_string(),
                    ));
                }
            }
        }
        SlotKind::RecordOnly => {
            if let Some((other, _)) = &hs.primary {
                if *other == spec {
                    return Err(AppError::Hotkey(
                        "record-only hotkey must differ from the primary hotkey".to_string(),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Identifies which of the two user-facing slots a `register*` call is
/// filling. Used by `validate_no_conflict` to look only at the OTHER
/// slot's spec, keeping the helper symmetric.
#[derive(Debug, Clone, Copy)]
enum SlotKind {
    Primary,
    RecordOnly,
}

static HOOK_STATE: Mutex<Option<HookState>> = Mutex::new(None);

/// P1 Esc-cancel swallow decision: true only when a cancel callback is
/// registered, the event is a plain key-down, and the callback actually
/// handled the cancellation. Idle-time Esc must always pass through to
/// the focused application.
fn esc_should_swallow(has_slot: bool, is_keydown: bool, cancel_handled: bool) -> bool {
    has_slot && is_keydown && cancel_handled
}

/// Windows global keyboard hook implementation.
pub struct WindowsHotkeyManager {
    hook: Option<HHOOK>,
}

impl WindowsHotkeyManager {
    pub fn new() -> Self {
        Self { hook: None }
    }
}

impl HotkeyManager for WindowsHotkeyManager {
    fn register(&mut self, spec: HotkeySpec, callback: HotkeyCallback) -> Result<(), AppError> {
        // Stage 6 fix #2 — preserve load-path observability when an
        // arbitrary object-form vk arrives that the key-name table cannot
        // represent. We still register (the spec is syntactically valid),
        // but a warn line tells anyone reading the log that the spec is
        // effectively a "dead key" (keyup will fire, keydown will not match).
        if crate::hotkey::from_key_name(&crate::hotkey::vk_to_key_name(spec.vk)).is_none() {
            tracing::warn!(
                target: "hotkey",
                "register: spec vk={:#x} has no name representation; keydown will not match any displayable name",
                spec.vk
            );
        }

        self.ensure_hook()?;
        let mut state = HOOK_STATE
            .lock()
            .map_err(|e| AppError::Hotkey(format!("global state lock poisoned: {e}")))?;
        let hook_state = state.get_or_insert_with(HookState::default);
        validate_no_conflict(spec, hook_state, SlotKind::Primary)?;
        hook_state.primary = Some((spec, Arc::from(callback)));
        Ok(())
    }

    fn register_record_only(
        &mut self,
        spec: HotkeySpec,
        callback: HotkeyCallback,
    ) -> Result<(), AppError> {
        if crate::hotkey::from_key_name(&crate::hotkey::vk_to_key_name(spec.vk)).is_none() {
            tracing::warn!(
                target: "hotkey",
                "register_record_only: spec vk={:#x} has no name representation; keydown will not match",
                spec.vk
            );
        }

        self.ensure_hook()?;
        let mut state = HOOK_STATE
            .lock()
            .map_err(|e| AppError::Hotkey(format!("global state lock poisoned: {e}")))?;
        let hook_state = state.get_or_insert_with(HookState::default);
        validate_no_conflict(spec, hook_state, SlotKind::RecordOnly)?;
        hook_state.record_only = Some((spec, Arc::from(callback)));
        Ok(())
    }

    fn register_cancel_esc(&mut self, callback: CancelEscCallback) -> Result<(), AppError> {
        // The hook must already exist (set up by register()). If it does not,
        // we still record the slot in HOOK_STATE so the keyboard_hook_proc can
        // dispatch once the user later registers a primary hotkey. However,
        // because primary registration is required to come first (lib.rs
        // ordering), the slot is functionally live as soon as the hook is up.
        // We do NOT ensure_hook() here — that would create a phantom hook
        // before any user-facing hotkey exists.
        match HOOK_STATE.lock() {
            Ok(mut state) => {
                state.get_or_insert_with(HookState::default).cancel_esc = Some(Arc::from(callback));
            }
            Err(e) => {
                tracing::warn!(target: "cancel", "cancel-esc slot not registered: HOOK_STATE poisoned ({e})");
            }
        }
        Ok(())
    }

    fn unregister(&mut self) -> Result<(), AppError> {
        self.remove_hook()?;
        // Clear global state (all three slots). Used at shutdown / Drop —
        // the comment on `unregister_primary` explains why we keep these
        // paths separate.
        if let Ok(mut state) = HOOK_STATE.lock() {
            *state = None;
        }
        Ok(())
    }

    fn unregister_primary(&mut self) -> Result<(), AppError> {
        // Take ONLY the primary slot; record_only + cancel_esc remain live.
        // Hook removal is decided by the THREE-slot emptiness test:
        // cancel_esc and record_only are still active after this call,
        // so the hook must stay installed.
        let needs_unhook = {
            let mut state = HOOK_STATE
                .lock()
                .map_err(|e| AppError::Hotkey(format!("global state lock poisoned: {e}")))?;
            match state.as_mut() {
                Some(hs) => {
                    hs.primary = None;
                    all_slots_empty(hs)
                }
                None => false,
            }
        };
        if needs_unhook {
            self.remove_hook()?;
        }
        Ok(())
    }

    fn unregister_record_only(&mut self) -> Result<(), AppError> {
        // Note: `*state = None` is INTENTIONALLY NOT used here.
        // cancel_esc is now a live slot, so the prior "clear the entire
        // state when both primary + record_only are empty" short-circuit
        // would silently nuke cancel_esc. The hook-removal decision must
        // be driven by `all_slots_empty(hs)` — i.e. cancel_esc counts.
        let needs_unhook = {
            let mut state = HOOK_STATE
                .lock()
                .map_err(|e| AppError::Hotkey(format!("global state lock poisoned: {e}")))?;
            match state.as_mut() {
                Some(hs) => {
                    hs.record_only = None;
                    all_slots_empty(hs)
                }
                None => false,
            }
        };
        if needs_unhook {
            self.remove_hook()?;
        }
        Ok(())
    }

    fn is_registered(&self) -> bool {
        self.hook.is_some()
    }
}

impl WindowsHotkeyManager {
    /// Install the low-level keyboard hook if not already installed.
    fn ensure_hook(&mut self) -> Result<(), AppError> {
        if self.hook.is_some() {
            return Ok(());
        }
        unsafe {
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), None, 0)
                .map_err(|e| AppError::Hotkey(format!("failed to set hook: {e}")))?;
            self.hook = Some(hook);
        }
        Ok(())
    }

    /// Remove the low-level keyboard hook if installed.
    fn remove_hook(&mut self) -> Result<(), AppError> {
        if let Some(hook) = self.hook.take() {
            // SAFETY: UnhookWindowsHookEx removes a hook installed by SetWindowsHookExW.
            // `hook` is a valid HHOOK from a successful SetWindowsHookExW call.
            unsafe {
                UnhookWindowsHookEx(hook)
                    .map_err(|e| AppError::Hotkey(format!("failed to unhook: {e}")))?;
            }
        }
        Ok(())
    }
}

impl Default for WindowsHotkeyManager {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: HHOOK is a Windows hook handle (pointer) not Send/Sync by default.
// WindowsHotkeyManager is only accessed through `Mutex<WindowsHotkeyManager>`
// (see lib.rs). The Mutex guarantees exclusive access. The hook handle
// is created in register(), used in the static hook proc (via separate HOOK_STATE
// Mutex), and destroyed in unregister()/Drop — all under Mutex protection.
unsafe impl Send for WindowsHotkeyManager {}
unsafe impl Sync for WindowsHotkeyManager {}

impl Drop for WindowsHotkeyManager {
    fn drop(&mut self) {
        if let Some(hook) = self.hook.take() {
            // SAFETY: UnhookWindowsHookEx removes a hook installed by SetWindowsHookExW.
            // `hook` is a valid HHOOK. Best-effort cleanup in Drop — error is discarded.
            unsafe {
                let _ = UnhookWindowsHookEx(hook);
            }
        }
    }
}

/// Low-level keyboard hook procedure.
///
/// Reads the registered key_code and callback from global state,
/// detects press/release, and invokes the callback.
///
/// # Safety
/// This function is called by Windows from the hook chain. `l_param` must point to a
/// valid `KBDLLHOOKSTRUCT`. The function reads global state through a Mutex and clones
/// any data before releasing the lock to avoid re-entrant deadlocks.
unsafe extern "system" fn keyboard_hook_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code >= 0 {
        // SAFETY: Windows guarantees l_param points to a valid KBDLLHOOKSTRUCT when
        // called from a WH_KEYBOARD_LL hook with n_code >= 0.
        let kb_struct = unsafe { *(l_param.0 as *const KBDLLHOOKSTRUCT) };
        let vk = kb_struct.vkCode;

        // Ignore synthetic (injected) key events from SendInput/keybd_event.
        // This prevents tools like Ditto from accidentally triggering the hotkey.
        if kb_struct.flags & LLKHF_INJECTED != KBDLLHOOKSTRUCT_FLAGS(0) {
            // SAFETY: CallNextHookEx passes the event to the next hook in the chain.
            // All parameters are forwarded unchanged from our hook proc.
            return unsafe { CallNextHookEx(None, n_code, w_param, l_param) };
        }

        // P1 Esc-cancel: intercept plain Esc key-down only (NOT WM_SYSKEYDOWN —
        // Alt+Esc is the system window-cycle shortcut). Dispatched before the
        // regular slot table so the cancel path is independent of which hotkey
        // the user registered as primary/record-only.
        const VK_ESCAPE: u32 = 0x1B;
        if vk == VK_ESCAPE && w_param.0 as u32 == WM_KEYDOWN {
            let (has_slot, cb) = {
                let state = HOOK_STATE.lock();
                match state {
                    Ok(guard) => {
                        let hs = guard.as_ref();
                        let slot = hs.and_then(|s| s.cancel_esc.clone());
                        (slot.is_some(), slot)
                    }
                    Err(_) => (false, None),
                }
            };
            if let Some(cb) = cb {
                // Lock released — call outside (same convention as slot dispatch).
                let handled = cb();
                if esc_should_swallow(has_slot, true, handled) {
                    // SAFETY: swallow this Esc — the focused app must not receive
                    // the same key-press that just cancelled its transcription.
                    return LRESULT(1);
                }
            }
        }

        // Determine event type from w_param.
        let event = match w_param.0 as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => Some((HotkeyEvent::Pressed, true)),
            WM_KEYUP | WM_SYSKEYUP => Some((HotkeyEvent::Released, false)),
            _ => None,
        };

        if let Some((event, is_keydown)) = event {
            // Compute the matching slot's callback. Update the per-process
            // modifier state table inside the lock, then snapshot the
            // matched callback out so we can release the lock before the
            // user callback runs (deadlock-safe; matches prior convention).
            let callback = {
                let mut state = HOOK_STATE.lock();
                let mut mods = ModifiersDown::default();
                match state.as_mut() {
                    Ok(guard) => {
                        if let Some(hs) = guard.as_mut() {
                            update_modifiers(&mut mods, vk, is_keydown);
                            find_callback(hs, vk, mods, is_keydown)
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                }
            };
            if let Some(cb) = callback {
                cb(event);
            }
        }
    }

    // SAFETY: CallNextHookEx passes the event to the next hook in the chain.
    // All parameters are forwarded unchanged from our hook proc.
    unsafe { CallNextHookEx(None, n_code, w_param, l_param) }
}

/// Dispatch a virtual key code to the matching slot's callback. Pure
/// function on `&HookState` so tests can call it on local instances
/// without touching the global `HOOK_STATE`.
///
/// Primary slot wins if both slots somehow hold the same vk (config
/// validation and register_record_only both reject that case).
///
/// Keydown requires full spec match (vk + modifier flags). Keyup fires
/// on the main vk alone — modifier drift must not stall recording-stop
/// (the user lifting a modifier before the spec key would otherwise
/// deadlock the press/release pairing).
fn find_callback(
    hs: &HookState,
    vk: u32,
    mods: ModifiersDown,
    is_keydown: bool,
) -> Option<SlotCallback> {
    if let Some((spec, cb)) = &hs.primary {
        if spec.vk == vk && (!is_keydown || modifiers_match(spec, mods)) {
            return Some(cb.clone());
        }
    }
    if let Some((spec, cb)) = &hs.record_only {
        if spec.vk == vk && (!is_keydown || modifiers_match(spec, mods)) {
            return Some(cb.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_manager() {
        let manager = WindowsHotkeyManager::new();
        assert!(!manager.is_registered());
    }

    #[test]
    fn test_hotkey_manager_is_send_sync_via_mutex() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<std::sync::Mutex<WindowsHotkeyManager>>();
    }

    fn dummy_callback() -> SlotCallback {
        Arc::new(|_event: HotkeyEvent| {})
    }

    fn default_primary() -> HotkeySpec {
        HotkeySpec {
            ctrl: false,
            shift: false,
            alt: false,
            vk: 0xA3,
        }
    }

    fn default_record_only() -> HotkeySpec {
        HotkeySpec {
            ctrl: false,
            shift: false,
            alt: false,
            vk: 0xA5,
        }
    }

    // ---- HookState slot semantics ----

    #[test]
    fn test_find_callback_dispatches_by_vk() {
        let primary_cb = dummy_callback();
        let record_cb = dummy_callback();
        let hs = HookState {
            primary: Some((default_primary(), primary_cb.clone())),
            record_only: Some((default_record_only(), record_cb.clone())),
            cancel_esc: None,
        };
        let found_primary = find_callback(&hs, 0xA3, ModifiersDown::default(), true);
        assert!(found_primary.is_some());
        let found_record = find_callback(&hs, 0xA5, ModifiersDown::default(), true);
        assert!(found_record.is_some());
        // Distinct slots: the two callbacks are different allocations.
        if let (Some(p), Some(r)) = (found_primary, found_record) {
            assert!(!Arc::ptr_eq(&p, &r));
            assert!(Arc::ptr_eq(&p, &primary_cb));
            assert!(Arc::ptr_eq(&r, &record_cb));
        }
    }

    #[test]
    fn test_find_callback_returns_none_for_unregistered_vk() {
        let hs = HookState {
            primary: Some((default_primary(), dummy_callback())),
            record_only: Some((default_record_only(), dummy_callback())),
            cancel_esc: None,
        };
        assert!(find_callback(&hs, 0x70, ModifiersDown::default(), true).is_none());
    }

    #[test]
    fn test_find_callback_handles_empty_slots() {
        let hs = HookState::default();
        assert!(find_callback(&hs, 0xA3, ModifiersDown::default(), true).is_none());

        let only_record = HookState {
            primary: None,
            record_only: Some((default_record_only(), dummy_callback())),
            cancel_esc: None,
        };
        assert!(only_record.primary.is_none());
        assert!(find_callback(&only_record, 0xA3, ModifiersDown::default(), true).is_none());
        assert!(find_callback(&only_record, 0xA5, ModifiersDown::default(), true).is_some());
    }

    // ---- P1 Esc-cancel: swallow key down only when a cancel callback handled it ----

    #[test]
    fn esc_swallow_requires_slot_keydown_and_dispatched_cancel() {
        // (has_slot, is_keydown, cancel_handled) -> should_swallow
        assert!(
            esc_should_swallow(true, true, true),
            "slot+keydown+handled -> swallow"
        );
        assert!(
            !esc_should_swallow(true, true, false),
            "callback declined (idle) -> pass through"
        );
        assert!(
            !esc_should_swallow(true, false, true),
            "keyup always passes"
        );
        assert!(
            !esc_should_swallow(false, true, true),
            "no slot -> pass through"
        );
    }

    #[test]
    fn esc_is_not_syskeydown_alias() {
        // WM_SYSKEYDOWN must NOT trigger swallow: Alt+Esc is a system window
        // cycle shortcut, swallowing it would block that system operation.
        // The hook proc guards this by checking w_param == WM_KEYDOWN before
        // dispatching to cancel_cb; here we only verify the swallow decision
        // logic itself is "plain keydown only".
        assert!(!esc_should_swallow(true, false, true));
    }

    // ---- P2 modifier state table matching (default RightCtrl = regression target) ----

    #[test]
    fn plain_key_matches_only_without_modifiers() {
        // Default config spec: RightCtrl alone, no modifier flags.
        let spec = default_primary();
        // No modifiers held + RightCtrl pressed -> match.
        let mut mods = ModifiersDown::default();
        assert!(modifiers_match(&spec, mods));
        update_modifiers(&mut mods, 0xA3, true);
        // RightCtrl itself just set mods.ctrl — self-family exclusion: still match.
        // (Without the exclusion, mods.ctrl would equal true vs spec.ctrl=false
        // and the default config would never trigger.)
        assert!(modifiers_match(&spec, mods));
        // User now ALSO presses Shift — spec.shift=false vs mods.shift=true ->
        // Shift is NOT in the self-family (spec.vk is ctrl, not shift), so we
        // do compare; spec.shift=false vs true -> no match.
        update_modifiers(&mut mods, 0xA0, true);
        assert!(
            !modifiers_match(&spec, mods),
            "extra shift held -> no match (strict)"
        );
        // Both-direction Ctrl self-exclusion: a LeftCtrl spec must match
        // when RightCtrl is held (and vice versa). Both vks live in the
        // ctrl family, so the self-exclusion treats them symmetrically —
        // a config using LeftCtrl (0xA2) would be dead without this.
        let left_ctrl_spec = HotkeySpec {
            ctrl: false,
            shift: false,
            alt: false,
            vk: 0xA2,
        };
        let mut mods = ModifiersDown::default();
        // RightCtrl held while spec is LeftCtrl -> match (self-family).
        update_modifiers(&mut mods, 0xA3, true);
        assert!(
            modifiers_match(&left_ctrl_spec, mods),
            "LeftCtrl spec matches RightCtrl press (self-family both directions)"
        );
        // Inverse: LeftCtrl spec still matches when LeftCtrl is pressed.
        let mut mods = ModifiersDown::default();
        update_modifiers(&mut mods, 0xA2, true);
        assert!(
            modifiers_match(&left_ctrl_spec, mods),
            "LeftCtrl spec matches LeftCtrl press (own vk)"
        );
        // And RightCtrl spec matches when LeftCtrl is pressed (the reverse
        // direction of the existing first assertion).
        let mut mods = ModifiersDown::default();
        update_modifiers(&mut mods, 0xA2, true);
        assert!(
            modifiers_match(&spec, mods),
            "RightCtrl spec matches LeftCtrl press (self-family both directions)"
        );
    }

    #[test]
    fn combo_matches_only_with_exact_modifiers() {
        // Spec: Ctrl+Shift+A.
        let spec = HotkeySpec {
            ctrl: true,
            shift: true,
            alt: false,
            vk: 0x41,
        };
        // Both modifiers held -> match.
        let mods = ModifiersDown {
            ctrl: true,
            shift: true,
            alt: false,
        };
        assert!(modifiers_match(&spec, mods));
        // Only ctrl held -> no match.
        let mods = ModifiersDown {
            ctrl: true,
            shift: false,
            alt: false,
        };
        assert!(!modifiers_match(&spec, mods));
        // All three held -> no match (over-modifier rejected).
        let mods = ModifiersDown {
            ctrl: true,
            shift: true,
            alt: true,
        };
        assert!(!modifiers_match(&spec, mods));
        // Ctrl+Shift+A spec against Ctrl+Shift held: not self-family (A is not
        // a modifier), so the regular comparison applies.
        let mods = ModifiersDown {
            ctrl: true,
            shift: true,
            alt: false,
        };
        assert!(modifiers_match(&spec, mods));
    }

    #[test]
    fn released_dispatches_on_main_vk_only() {
        // Keyup must fire on main vk regardless of which modifiers are still
        // held. This prevents the user lifting a modifier first from stalling
        // the press/release pairing.
        let spec = default_primary();
        let hs = HookState {
            primary: Some((spec, dummy_callback())),
            record_only: None,
            cancel_esc: None,
        };
        // RightCtrl pressed + modifier drift: shift still held.
        let mods = ModifiersDown {
            ctrl: false,
            shift: true,
            alt: false,
        };
        // Keyup matches even though mods drift (modifier state ignored on release).
        let found = find_callback(&hs, 0xA3, mods, false);
        assert!(
            found.is_some(),
            "keyup on main vk matches regardless of mods"
        );
        // Same mods on keydown -> NO match (strict).
        let not_found = find_callback(&hs, 0xA3, mods, true);
        assert!(
            not_found.is_none(),
            "keydown with strict-mismatch mods rejected"
        );
    }

    #[test]
    fn record_only_conflict_requires_full_spec_equality() {
        // {ctrl, A} vs {shift, A}: not the same spec -> no conflict.
        let a_ctrl = HotkeySpec {
            ctrl: true,
            shift: false,
            alt: false,
            vk: 0x41,
        };
        let a_shift = HotkeySpec {
            ctrl: false,
            shift: true,
            alt: false,
            vk: 0x41,
        };
        // Different specs, neither slot populated -> ok.
        let hs = HookState::default();
        assert!(validate_no_conflict(a_ctrl, &hs, SlotKind::RecordOnly).is_ok());
        assert!(validate_no_conflict(a_shift, &hs, SlotKind::RecordOnly).is_ok());
        // Full equality: registering record_only that equals existing primary
        // must error.
        let hs_with_primary = HookState {
            primary: Some((a_ctrl, dummy_callback())),
            record_only: None,
            cancel_esc: None,
        };
        let err = validate_no_conflict(a_ctrl, &hs_with_primary, SlotKind::RecordOnly)
            .expect_err("record_only == primary must error");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("record-only hotkey must differ from the primary hotkey"),
            "expected record-only vs primary message, got: {msg}"
        );
        // Spec mismatch against existing primary -> still ok (no false-positive).
        assert!(
            validate_no_conflict(a_shift, &hs_with_primary, SlotKind::RecordOnly).is_ok(),
            "different vk/modifier combos do not collide"
        );
    }

    #[test]
    fn primary_conflict_requires_full_spec_equality() {
        // Symmetric guard: registering a primary that equals existing
        // record_only must error (silent-degradation regression — see
        // commit d0f1288 / validate_no_conflict helper).
        let spec_a = HotkeySpec {
            ctrl: false,
            shift: false,
            alt: false,
            vk: 0xA3,
        };
        let spec_b = HotkeySpec {
            ctrl: false,
            shift: false,
            alt: false,
            vk: 0xA5,
        };
        // Empty state: anything goes.
        let hs = HookState::default();
        assert!(validate_no_conflict(spec_a, &hs, SlotKind::Primary).is_ok());
        // record_only holds spec_b; registering primary == spec_b must error.
        let hs_with_record = HookState {
            primary: None,
            record_only: Some((spec_b, dummy_callback())),
            cancel_esc: None,
        };
        let err = validate_no_conflict(spec_b, &hs_with_record, SlotKind::Primary)
            .expect_err("primary == record_only must error");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("primary hotkey must differ from the record-only hotkey"),
            "expected primary vs record-only message, got: {msg}"
        );
        // Different spec -> ok (false-positive guard).
        assert!(
            validate_no_conflict(spec_a, &hs_with_record, SlotKind::Primary).is_ok(),
            "different vk does not collide"
        );
    }

    // ---- P2 write-side modifier table: pure function, table-driven ----

    #[test]
    fn update_modifiers_table_driven() {
        // Keydown sets the bit; keyup clears it. Left/right symmetric.
        for vk in [0xA2u32, 0xA3] {
            let mut m = ModifiersDown::default();
            update_modifiers(&mut m, vk, true);
            assert!(m.ctrl, "keydown 0x{vk:x} -> ctrl set");
            update_modifiers(&mut m, vk, false);
            assert!(!m.ctrl, "keyup 0x{vk:x} -> ctrl cleared");
        }
        for vk in [0xA0u32, 0xA1] {
            let mut m = ModifiersDown::default();
            update_modifiers(&mut m, vk, true);
            assert!(m.shift);
            update_modifiers(&mut m, vk, false);
            assert!(!m.shift);
        }
        for vk in [0xA4u32, 0xA5] {
            let mut m = ModifiersDown::default();
            update_modifiers(&mut m, vk, true);
            assert!(m.alt);
            update_modifiers(&mut m, vk, false);
            assert!(!m.alt);
        }
        // Key repeat (consecutive keydowns) is idempotent.
        let mut m = ModifiersDown::default();
        update_modifiers(&mut m, 0xA3, true);
        update_modifiers(&mut m, 0xA3, true);
        update_modifiers(&mut m, 0xA3, true);
        assert!(m.ctrl);
        // Non-modifier keys no-op.
        update_modifiers(&mut m, 0x41, true);
        assert!(!m.shift && !m.alt);
        update_modifiers(&mut m, 0x20, false);
        assert!(!m.shift && !m.alt);
    }

    // ---- P2 unregister_primary three-slot semantics (default config scenario) ----

    fn hook_state_with(
        primary: Option<HotkeySpec>,
        record_only: Option<HotkeySpec>,
        cancel_esc: bool,
    ) -> HookState {
        HookState {
            primary: primary.map(|s| (s, dummy_callback())),
            record_only: record_only.map(|s| (s, dummy_callback())),
            cancel_esc: if cancel_esc {
                Some(Arc::new(|| true))
            } else {
                None
            },
        }
    }

    #[test]
    fn unregister_primary_keeps_other_slots_record_only_and_cancel() {
        // Scenario A: all three slots present; clear primary -> two slots remain.
        let mut hs = hook_state_with(Some(default_primary()), Some(default_record_only()), true);
        hs.primary = None;
        assert!(!all_slots_empty(&hs), "two slots still present");
        assert!(hs.record_only.is_some());
        assert!(hs.cancel_esc.is_some());
    }

    #[test]
    fn unregister_primary_keeps_cancel_esc_when_record_only_absent() {
        // Scenario B (default config when record_only_enabled=false):
        // only primary + cancel_esc; clear primary -> cancel_esc must remain
        // live so the Esc cancel path keeps working.
        let mut hs = hook_state_with(Some(default_primary()), None, true);
        hs.primary = None;
        assert!(
            !all_slots_empty(&hs),
            "cancel_esc alone is enough to keep hook"
        );
        assert!(hs.cancel_esc.is_some());
    }

    #[test]
    fn unregister_primary_with_only_record_only_present() {
        // Scenario C: only record_only + cancel_esc; clear primary (already None).
        let hs = hook_state_with(None, Some(default_record_only()), true);
        assert!(!all_slots_empty(&hs));
        // No-op primary clear.
        assert!(hs.primary.is_none());
    }

    #[test]
    fn all_slots_empty_returns_true_only_when_all_three_clear() {
        // Only primary -> not empty (cancel_esc/record_only missing but the
        // emptiness check counts them too).
        let hs = hook_state_with(Some(default_primary()), None, false);
        assert!(!all_slots_empty(&hs), "primary alone is not empty");
        // Cancel_esc only -> not empty.
        let hs = hook_state_with(None, None, true);
        assert!(!all_slots_empty(&hs), "cancel_esc alone is not empty");
        // All three None -> empty.
        let hs = HookState::default();
        assert!(all_slots_empty(&hs));
    }
}
