use crate::error::AppError;
use crate::hotkey::{CancelEscCallback, HotkeyCallback, HotkeyEvent, HotkeyManager};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT,
};
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
}

type SlotCallback = Arc<dyn Fn(HotkeyEvent) + Send + Sync>;
type CancelSlot = Arc<dyn Fn() -> bool + Send + Sync>;

/// Global state shared between WindowsHotkeyManager and the hook procedure.
/// Three slots (primary voice-input hotkey + record-only hotkey + cancel-Esc
/// dispatcher) dispatched from a single low-level hook.
struct HookState {
    primary: Option<(HotkeySpec, SlotCallback)>,
    record_only: Option<(HotkeySpec, SlotCallback)>,
    open_settings: Option<(HotkeySpec, SlotCallback)>,
    open_transcribe: Option<(HotkeySpec, SlotCallback)>,
    cancel_esc: Option<CancelSlot>,
    /// keydown 匹配时咨询的 modifier 采样器。默认 OS 真相探针（生产）；
    /// 测试必须经 `with_probe` 注入合成实现——Default 装的是真实键盘状态。
    mods_probe: ModsProbe,
}

impl HookState {
    fn with_probe(mods_probe: ModsProbe) -> Self {
        Self {
            primary: None,
            record_only: None,
            open_settings: None,
            open_transcribe: None,
            cancel_esc: None,
            mods_probe,
        }
    }
}

impl Default for HookState {
    fn default() -> Self {
        Self::with_probe(Arc::new(os_modifiers_held))
    }
}

/// 按键时刻实际按着的 modifier 快照，取自 OS 异步键状态
/// （`GetAsyncKeyState`，family 级：左右 Ctrl/Shift/Alt 合并）。2026-09-28
/// 替换跨事件累积器：LL hook 对注入 keyup（LLKHF_INJECTED 守卫提前过滤）
/// 与安全桌面 keyup（UAC / Ctrl+Alt+Del 绕过 LL hook）结构性不可见，累积
/// 器必然漂移成 stale-true 并静默压制全部 keydown（v26.9.4 press 失效根因）。
/// OS async key state 对注入路径免疫（SendInput/keybd_event 同样更新它）；
/// 安全桌面场景依赖系统对输入状态的复位行为——即便短暂 stale，也只持续到
/// 下一次真实按键，不会像累积器那样永久卡死。Stays process-local。
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
struct ModifiersHeld {
    ctrl: bool,
    shift: bool,
    alt: bool,
}

/// Live modifier sampler. A trait-object seam so tests can drive synthetic
/// held-state without a real keyboard; production reads OS truth.
type ModsProbe = Arc<dyn Fn() -> ModifiersHeld + Send + Sync>;

/// Production probe: OS async key state is updated by the system for ALL
/// input — physical and injected — so a lost keyup cannot stale it.
fn os_modifiers_held() -> ModifiersHeld {
    // SAFETY: GetAsyncKeyState 是线程无关的异步键状态纯查询，无句柄、不
    // 合成输入，可安全地在 hook 线程调用。
    fn down(vk: i32) -> bool {
        (unsafe { GetAsyncKeyState(vk) } as u16) & 0x8000 != 0
    }
    ModifiersHeld {
        ctrl: down(VK_CONTROL.0 as i32),
        shift: down(VK_SHIFT.0 as i32),
        alt: down(VK_MENU.0 as i32),
    }
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

/// True iff the held modifiers satisfy `spec`'s modifier flags.
///
/// Self-family exclusion: if `spec.vk` IS a Ctrl key, do not compare
/// `mods.ctrl` — because holding the spec's own vk registers as Ctrl held
/// at the OS level (GetAsyncKeyState(VK_CONTROL) 反映左右任一 Ctrl), the
/// default RightCtrl-alone config would never match without this.
/// Symmetric for shift / alt.
fn modifiers_match(spec: &HotkeySpec, mods: ModifiersHeld) -> bool {
    spec.ctrl == (mods.ctrl && !is_ctrl_vk(spec.vk))
        && spec.shift == (mods.shift && !is_shift_vk(spec.vk))
        && spec.alt == (mods.alt && !is_alt_vk(spec.vk))
}

/// Process one key event: sample live modifier state from the probe, then
/// match against the slot specs. Keydown requires full spec match (vk +
/// modifier flags); keyup fires on the main vk alone — modifier drift must
/// not stall recording-stop (the user lifting a modifier before the spec
/// key would otherwise deadlock the press/release pairing). Pure function
/// on `&HookState` so tests can drive multi-event sequences without
/// touching the process-level `HOOK_STATE` static.
fn dispatch_key_event(hs: &HookState, vk: u32, is_keydown: bool) -> Option<SlotCallback> {
    let mods = (hs.mods_probe)();
    find_callback(hs, vk, mods, is_keydown)
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
    // M6-a conflict matrix: pairwise comparison across all spec slots. Any
    // matching pair (ignoring self_slot) errors with the same backend msg
    // shape that get_pair_conflict_msg produces for the frontend.
    let slots: [SlotKind; 4] = [
        SlotKind::Primary,
        SlotKind::RecordOnly,
        SlotKind::OpenSettings,
        SlotKind::OpenTranscribe,
    ];
    let spec_of = |kind: SlotKind| -> Option<HotkeySpec> {
        match kind {
            SlotKind::Primary => hs.primary.as_ref().map(|(s, _)| *s),
            SlotKind::RecordOnly => hs.record_only.as_ref().map(|(s, _)| *s),
            SlotKind::OpenSettings => hs.open_settings.as_ref().map(|(s, _)| *s),
            SlotKind::OpenTranscribe => hs.open_transcribe.as_ref().map(|(s, _)| *s),
        }
    };
    for other in slots {
        if other == self_slot {
            continue;
        }
        if let Some(other_spec) = spec_of(other)
            && other_spec == spec
        {
            return Err(AppError::Hotkey(format!(
                "hotkey already used by {} slot",
                slot_label(other)
            )));
        }
    }
    Ok(())
}

fn slot_label(kind: SlotKind) -> &'static str {
    match kind {
        SlotKind::Primary => "primary",
        SlotKind::RecordOnly => "record-only",
        SlotKind::OpenSettings => "open-settings",
        SlotKind::OpenTranscribe => "open-transcribe",
    }
}

/// Identifies which of the four user-facing slots a `register*` call is
/// filling. Used by `validate_no_conflict` to look only at the OTHER
/// slots' specs, keeping the helper symmetric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotKind {
    Primary,
    RecordOnly,
    OpenSettings,
    OpenTranscribe,
}

static HOOK_STATE: Mutex<Option<HookState>> = Mutex::new(None);

/// P1 Esc-cancel swallow decision: true only when a cancel callback is
/// registered, the event is a plain key-down, and the callback actually
/// handled the cancellation. Idle-time Esc must always pass through to
/// the focused application.
fn esc_should_swallow(has_slot: bool, is_keydown: bool, cancel_handled: bool) -> bool {
    has_slot && is_keydown && cancel_handled
}

/// Decoded keyboard event — the hook proc's only job is translating
/// KBDLLHOOKSTRUCT/w_param into this, so routing is testable without Win32.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeyEvent {
    vk: u32,
    is_keydown: bool,
    /// WM_SYSKEYDOWN/UP (Alt-combined system events). Esc-cancel only
    /// intercepts plain keydown — Alt+Esc is the window-cycle shortcut.
    is_sys: bool,
}

/// What the hook proc should do after `route_event` decides. BOTH callback
/// arms execute OUTSIDE the HOOK_STATE lock (the pre-existing
/// "lock released — call outside" convention): the hook thread must never
/// run user callbacks — which may dispatch window calls (hide_floating)
/// or emit events — while holding the lock, or a settings-page hotkey
/// change waiting on HOOK_STATE plus the LowLevelHooksTimeout budget
/// turns into an ABBA deadlock / silent hook removal (审查 S5-1)。
enum HookAction {
    /// Esc-cancel candidate: call the cancel callback outside the lock;
    /// swallow iff it returned true, otherwise fall through to slot
    /// dispatch (the proc performs a second locked pass).
    EscCancel(CancelSlot),
    /// Fire a slot callback with the event (outside the lock).
    Fire(SlotCallback, HotkeyEvent),
    /// No slot matched — pass through to the next hook / focused app.
    Pass,
}

/// Pure routing decision for one key event, in the hook's canonical order:
///   1. Esc cancel — plain keydown only, BEFORE the slot table so cancel
///      is independent of which hotkeys are registered (test pins this:
///      EscCancel wins even when a slot spec also matches Esc);
///   2. slot dispatch — modifiers update + spec match (keyup matches on
///      vk alone, see find_callback).
///
/// `SlotCallback` / `CancelSlot` are the existing type aliases at
/// windows.rs:96-97 (`Arc<dyn Fn(HotkeyEvent)…>` / `Arc<dyn Fn() -> bool…>`)
/// — the trait's boxed `HotkeyCallback` params are wrapped into them at
/// register time (`Arc::from(callback)`), same as today.
fn route_event(hs: &HookState, ev: KeyEvent) -> HookAction {
    const VK_ESCAPE: u32 = 0x1B;
    if ev.vk == VK_ESCAPE
        && ev.is_keydown
        && !ev.is_sys
        && let Some(cb) = hs.cancel_esc.clone()
    {
        return HookAction::EscCancel(cb);
    }
    if let Some(cb) = dispatch_key_event(hs, ev.vk, ev.is_keydown) {
        return HookAction::Fire(
            cb,
            if ev.is_keydown {
                HotkeyEvent::Pressed
            } else {
                HotkeyEvent::Released
            },
        );
    }
    HookAction::Pass
}

/// Map a Win32 w_param message id to a KeyEvent shape. Non-key messages
/// (e.g. WM_CHAR already filtered earlier) yield None.
fn decode_key_event(w_param: u32, vk: u32) -> Option<KeyEvent> {
    let (is_keydown, is_sys) = match w_param {
        WM_KEYDOWN => (true, false),
        WM_SYSKEYDOWN => (true, true),
        WM_KEYUP => (false, false),
        WM_SYSKEYUP => (false, true),
        _ => return None,
    };
    Some(KeyEvent {
        vk,
        is_keydown,
        is_sys,
    })
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
        self.register_spec_slot(spec, callback, SlotKind::Primary)
    }

    fn register_record_only(
        &mut self,
        spec: HotkeySpec,
        callback: HotkeyCallback,
    ) -> Result<(), AppError> {
        self.register_spec_slot(spec, callback, SlotKind::RecordOnly)
    }

    fn register_open_settings(
        &mut self,
        spec: HotkeySpec,
        callback: HotkeyCallback,
    ) -> Result<(), AppError> {
        self.register_spec_slot(spec, callback, SlotKind::OpenSettings)
    }

    fn register_open_transcribe(
        &mut self,
        spec: HotkeySpec,
        callback: HotkeyCallback,
    ) -> Result<(), AppError> {
        self.register_spec_slot(spec, callback, SlotKind::OpenTranscribe)
    }

    fn register_cancel_esc(&mut self, callback: CancelEscCallback) -> Result<(), AppError> {
        // The hook must already exist (set up by register()). If it does not,
        // we still record the slot in HOOK_STATE so the keyboard_hook_proc can
        // dispatch once the user later registers a primary hotkey. However,
        // because primary registration is required to come first (lib.rs
        // ordering), the slot is functionally live as soon as the hook is up.
        // We do NOT ensure_hook() here — that would create a phantom hook
        // before any user-facing hotkey exists.
        let mut state = HOOK_STATE
            .lock()
            .map_err(|e| AppError::Hotkey(format!("global state lock poisoned: {e}")))?;
        state.get_or_insert_with(HookState::default).cancel_esc = Some(Arc::from(callback));
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
        self.clear_spec_slot(SlotKind::Primary)
    }

    fn unregister_record_only(&mut self) -> Result<(), AppError> {
        self.clear_spec_slot(SlotKind::RecordOnly)
    }

    fn unregister_open_settings(&mut self) -> Result<(), AppError> {
        self.clear_spec_slot(SlotKind::OpenSettings)
    }

    fn unregister_open_transcribe(&mut self) -> Result<(), AppError> {
        self.clear_spec_slot(SlotKind::OpenTranscribe)
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

    /// Shared register body for the two spec slots (primary / record-only):
    /// dead-key warn -> ensure hook -> cross-slot conflict check -> install.
    /// `register_cancel_esc` stays separate (no spec, no hook install).
    fn register_spec_slot(
        &mut self,
        spec: HotkeySpec,
        callback: HotkeyCallback,
        slot: SlotKind,
    ) -> Result<(), AppError> {
        if !crate::hotkey::is_resolvable_vk(spec.vk) {
            tracing::warn!(
                target: "hotkey",
                "register {slot:?}: spec vk={:#x} has no name representation; keydown will not match any displayable name",
                spec.vk
            );
        }
        self.ensure_hook()?;
        let mut state = HOOK_STATE
            .lock()
            .map_err(|e| AppError::Hotkey(format!("global state lock poisoned: {e}")))?;
        let hook_state = state.get_or_insert_with(HookState::default);
        validate_no_conflict(spec, hook_state, slot)?;
        match slot {
            SlotKind::Primary => hook_state.primary = Some((spec, Arc::from(callback))),
            SlotKind::RecordOnly => hook_state.record_only = Some((spec, Arc::from(callback))),
            SlotKind::OpenSettings => hook_state.open_settings = Some((spec, Arc::from(callback))),
            SlotKind::OpenTranscribe => {
                hook_state.open_transcribe = Some((spec, Arc::from(callback)))
            }
        }
        Ok(())
    }

    /// Shared unregister body: clear one spec slot, unhook only when all
    /// three slots are empty (cancel_esc counts).
    fn clear_spec_slot(&mut self, slot: SlotKind) -> Result<(), AppError> {
        let needs_unhook = {
            let mut state = HOOK_STATE
                .lock()
                .map_err(|e| AppError::Hotkey(format!("global state lock poisoned: {e}")))?;
            match state.as_mut() {
                Some(hs) => {
                    match slot {
                        SlotKind::Primary => hs.primary = None,
                        SlotKind::RecordOnly => hs.record_only = None,
                        SlotKind::OpenSettings => hs.open_settings = None,
                        SlotKind::OpenTranscribe => hs.open_transcribe = None,
                    }
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

        if let Some(ev) = decode_key_event(w_param.0 as u32, vk) {
            // Routing decision under the lock; EVERY callback invocation
            // happens after release (deadlock-safe; the prior convention —
            // the cancel path used to do this inline, now both arms do).
            let action = {
                let state = HOOK_STATE.lock();
                match state.as_ref() {
                    Ok(guard) => guard.as_ref().map(|hs| route_event(hs, ev)),
                    Err(_) => None,
                }
            };
            match action {
                Some(HookAction::EscCancel(cb)) => {
                    // Lock released — call outside (pre-existing convention:
                    // user callbacks may dispatch window calls / emit events).
                    let handled = cb();
                    if esc_should_swallow(true, true, handled) {
                        // SAFETY: swallow this Esc — the focused app must not
                        // receive the same key-press that just cancelled its
                        // transcription.
                        return LRESULT(1);
                    }
                    // Cancel didn't happen (no active pipeline): fall through
                    // to slot dispatch via a second locked pass — a user
                    // hotkey registered on Esc must still fire. Esc is not a
                    // modifier, so the first pass updated no modifier state.
                    let action = {
                        let state = HOOK_STATE.lock();
                        match state.as_ref() {
                            Ok(guard) => guard
                                .as_ref()
                                .and_then(|hs| dispatch_key_event(hs, ev.vk, ev.is_keydown)),
                            Err(_) => None,
                        }
                    };
                    if let Some(cb) = action {
                        cb(HotkeyEvent::Pressed);
                    }
                }
                Some(HookAction::Fire(cb, event)) => cb(event),
                _ => {}
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
///
/// A keydown whose vk matches but modifiers don't is LOUD: the 2026-09-28
/// incident shipped precisely because this rejection was silent. 已接受的
/// 权衡：主键若配置为常用打字键，日常 modifier 组合会高频触发本 warn
/// （含击键时间元数据，本地日志文件）——可观测性优先，节流留待后续。
fn find_callback(
    hs: &HookState,
    vk: u32,
    mods: ModifiersHeld,
    is_keydown: bool,
) -> Option<SlotCallback> {
    if let Some((spec, cb)) = &hs.primary
        && spec.vk == vk
    {
        if !is_keydown || modifiers_match(spec, mods) {
            return Some(cb.clone());
        }
        tracing::warn!(
            target: "hotkey",
            "primary keydown rejected: key={} held={mods:?} spec={spec}",
            crate::hotkey::vk_to_key_name(vk)
        );
    }
    if let Some((spec, cb)) = &hs.record_only
        && spec.vk == vk
    {
        if !is_keydown || modifiers_match(spec, mods) {
            return Some(cb.clone());
        }
        tracing::warn!(
            target: "hotkey",
            "record-only keydown rejected: key={} held={mods:?} spec={spec}",
            crate::hotkey::vk_to_key_name(vk)
        );
    }
    if let Some((spec, cb)) = &hs.open_settings
        && spec.vk == vk
        && (!is_keydown || modifiers_match(spec, mods))
    {
        return Some(cb.clone());
    }
    if let Some((spec, cb)) = &hs.open_transcribe
        && spec.vk == vk
        && (!is_keydown || modifiers_match(spec, mods))
    {
        return Some(cb.clone());
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

    fn probe_returning(mods: ModifiersHeld) -> ModsProbe {
        Arc::new(move || mods)
    }

    /// Mutable probe cell: tests flip the reported held-state between
    /// dispatch calls without a real keyboard.
    fn probe_cell(initial: ModifiersHeld) -> (Arc<Mutex<ModifiersHeld>>, ModsProbe) {
        let cell = Arc::new(Mutex::new(initial));
        let cell_for_probe = Arc::clone(&cell);
        let probe: ModsProbe =
            Arc::new(move || *cell_for_probe.lock().expect("probe cell poisoned"));
        (cell, probe)
    }

    // ---- HookState slot semantics ----

    #[test]
    fn test_find_callback_dispatches_by_vk() {
        let primary_cb = dummy_callback();
        let record_cb = dummy_callback();
        let hs = HookState {
            primary: Some((default_primary(), primary_cb.clone())),
            record_only: Some((default_record_only(), record_cb.clone())),
            open_settings: None,
            open_transcribe: None,
            cancel_esc: None,
            mods_probe: probe_returning(ModifiersHeld::default()),
        };
        let found_primary = find_callback(&hs, 0xA3, ModifiersHeld::default(), true);
        assert!(found_primary.is_some());
        let found_record = find_callback(&hs, 0xA5, ModifiersHeld::default(), true);
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
            open_settings: None,
            open_transcribe: None,
            cancel_esc: None,
            mods_probe: probe_returning(ModifiersHeld::default()),
        };
        assert!(find_callback(&hs, 0x70, ModifiersHeld::default(), true).is_none());
    }

    #[test]
    fn test_find_callback_handles_empty_slots() {
        let hs = HookState::default();
        assert!(find_callback(&hs, 0xA3, ModifiersHeld::default(), true).is_none());

        let only_record = HookState {
            primary: None,
            record_only: Some((default_record_only(), dummy_callback())),
            open_settings: None,
            open_transcribe: None,
            cancel_esc: None,
            mods_probe: probe_returning(ModifiersHeld::default()),
        };
        assert!(only_record.primary.is_none());
        assert!(find_callback(&only_record, 0xA3, ModifiersHeld::default(), true).is_none());
        assert!(find_callback(&only_record, 0xA5, ModifiersHeld::default(), true).is_some());
    }

    // ---- Cross-event modifier accumulation (P0 regression guard) ----
    //
    // The hook proc sees a combo as SEPARATE events: Ctrl down, then A down.
    // The "A down" event must still observe ctrl=true from the earlier event,
    // so the modifier state has to live in HookState and persist across
    // dispatch_key_event calls. These tests fail against the old per-event
    // `ModifiersHeld::default()` local, where no combo ever fired.

    #[test]
    fn combo_fires_from_live_probe_modifiers() {
        let spec = HotkeySpec {
            ctrl: true,
            shift: false,
            alt: false,
            vk: 0x41, // A
        };
        let mut hs = HookState::with_probe(probe_returning(ModifiersHeld {
            ctrl: true,
            shift: false,
            alt: false,
        }));
        hs.primary = Some((spec, dummy_callback()));
        // Ctrl 实际按着（探针=OS 真相）：A down 组合命中。
        assert!(dispatch_key_event(&hs, 0x41, true).is_some());
        // Keyup fires on the main vk alone (modifier drift must not stall stop).
        assert!(dispatch_key_event(&hs, 0x41, false).is_some());
    }

    #[test]
    fn combo_requires_full_modifier_set_from_probe() {
        let spec = HotkeySpec {
            ctrl: true,
            shift: true,
            alt: false,
            vk: 0x41, // A
        };
        let (cell, probe) = probe_cell(ModifiersHeld {
            ctrl: true,
            shift: false,
            alt: false,
        });
        let mut hs = HookState::with_probe(probe);
        hs.primary = Some((spec, dummy_callback()));
        // 只按着 Ctrl：Ctrl+Shift+A 不命中。
        assert!(dispatch_key_event(&hs, 0x41, true).is_none());
        // Shift 加入：完整组合命中。
        *cell.lock().expect("cell poisoned") = ModifiersHeld {
            ctrl: true,
            shift: true,
            alt: false,
        };
        assert!(dispatch_key_event(&hs, 0x41, true).is_some());
    }

    #[test]
    fn combo_stops_matching_once_modifier_released() {
        let spec = HotkeySpec {
            ctrl: true,
            shift: false,
            alt: false,
            vk: 0x41, // A
        };
        let (cell, probe) = probe_cell(ModifiersHeld {
            ctrl: true,
            shift: false,
            alt: false,
        });
        let mut hs = HookState::with_probe(probe);
        hs.primary = Some((spec, dummy_callback()));
        assert!(dispatch_key_event(&hs, 0x41, true).is_some());
        // Ctrl 松开（OS 真相变化）：A down 不再命中。
        *cell.lock().expect("cell poisoned") = ModifiersHeld::default();
        assert!(dispatch_key_event(&hs, 0x41, true).is_none());
    }

    #[test]
    fn dispatch_bare_key_still_fires_without_modifiers() {
        // 默认配置（RightCtrl 裸键）必须持续可用：按下 RightCtrl 本身会让
        // OS 报 ctrl held，self-family 排除让匹配仍然成立。
        let mut hs = HookState::with_probe(probe_returning(ModifiersHeld::default()));
        hs.primary = Some((default_primary(), dummy_callback()));
        assert!(dispatch_key_event(&hs, 0xA3, true).is_some());
        assert!(dispatch_key_event(&hs, 0xA3, false).is_some());
        // OS 报 ctrl=true（RightCtrl 正被按着的真实形态）：仍必须命中。
        let hs_ctrl_held = HookState {
            primary: Some((default_primary(), dummy_callback())),
            record_only: None,
            open_settings: None,
            open_transcribe: None,
            cancel_esc: None,
            mods_probe: probe_returning(ModifiersHeld {
                ctrl: true,
                shift: false,
                alt: false,
            }),
        };
        assert!(dispatch_key_event(&hs_ctrl_held, 0xA3, true).is_some());
    }

    #[test]
    fn dispatch_stray_modifier_suppresses_bare_key_exact_match_semantics() {
        // 裸 spec（无 modifier）是精确匹配：按着无关 modifier（Ctrl）时敲
        // 裸键（RightAlt）是另一个组合，不得触发裸槽。
        let mut hs = HookState::with_probe(probe_returning(ModifiersHeld {
            ctrl: true,
            shift: false,
            alt: false,
        }));
        hs.record_only = Some((default_record_only(), dummy_callback()));
        assert!(dispatch_key_event(&hs, 0xA5, true).is_none());
        // 无杂 modifier 时裸键正常触发。
        let mut hs_clean = HookState::with_probe(probe_returning(ModifiersHeld::default()));
        hs_clean.record_only = Some((default_record_only(), dummy_callback()));
        assert!(dispatch_key_event(&hs_clean, 0xA5, true).is_some());
    }

    #[test]
    fn bare_key_fires_when_modifier_keyup_never_arrived() {
        // 2026-09-28 v26.9.4 回归：一次从未送达 hook 的 Shift keyup（注入
        // keyup 被 LLKHF_INJECTED 过滤、或安全桌面 keyup 绕过 LL hook）曾让
        // 跨事件累积器永久卡在 shift=true，此后裸键 keydown 全部被压制。
        // 现在匹配状态直接采样 OS 真相：事件本身无法污染它。
        let mut hs = HookState::with_probe(probe_returning(ModifiersHeld::default()));
        hs.primary = Some((default_primary(), dummy_callback()));
        // Shift 按下事件到达，其 keyup 永远没有送达……
        assert!(dispatch_key_event(&hs, 0xA0, true).is_none());
        // ……OS 真相说没有 modifier 按着：裸键 keydown 必须照常触发。
        assert!(
            dispatch_key_event(&hs, 0xA3, true).is_some(),
            "lost modifier keyup must not suppress the bare hotkey keydown"
        );
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
        // "not keydown" covers both keyup AND WM_SYSKEYDOWN routing: the
        // hook proc only dispatches plain WM_KEYDOWN to the cancel slot —
        // Alt+Esc is a system window-cycle shortcut and must pass through.
        assert!(
            !esc_should_swallow(true, false, true),
            "keyup / syskeydown always passes"
        );
        assert!(
            !esc_should_swallow(false, true, true),
            "no slot -> pass through"
        );
    }

    // ---- P2 modifier state table matching (default RightCtrl = regression target) ----

    #[test]
    fn plain_key_matches_only_without_modifiers() {
        // Default config spec: RightCtrl alone, no modifier flags.
        let spec = default_primary();
        // No modifiers held + RightCtrl pressed -> match.
        assert!(modifiers_match(&spec, ModifiersHeld::default()));
        // RightCtrl 本身让 OS 报 ctrl held —— self-family：仍匹配。
        // （无排除则 mods.ctrl=true vs spec.ctrl=false，默认配置永不触发。）
        assert!(modifiers_match(
            &spec,
            ModifiersHeld {
                ctrl: true,
                shift: false,
                alt: false
            }
        ));
        // 同时还按着 Shift：非 self-family，参与比较，false vs true -> 不匹配。
        assert!(
            !modifiers_match(
                &spec,
                ModifiersHeld {
                    ctrl: true,
                    shift: true,
                    alt: false
                }
            ),
            "extra shift held -> no match (strict)"
        );
        // Self-family 双向：LeftCtrl spec 在 RightCtrl 按着时也匹配（同族
        // vk 一律豁免族比较，双向对称）。
        let left_ctrl_spec = HotkeySpec {
            ctrl: false,
            shift: false,
            alt: false,
            vk: 0xA2,
        };
        assert!(modifiers_match(
            &left_ctrl_spec,
            ModifiersHeld {
                ctrl: true,
                shift: false,
                alt: false
            }
        ));
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
        let mods = ModifiersHeld {
            ctrl: true,
            shift: true,
            alt: false,
        };
        assert!(modifiers_match(&spec, mods));
        // Only ctrl held -> no match.
        let mods = ModifiersHeld {
            ctrl: true,
            shift: false,
            alt: false,
        };
        assert!(!modifiers_match(&spec, mods));
        // All three held -> no match (over-modifier rejected).
        let mods = ModifiersHeld {
            ctrl: true,
            shift: true,
            alt: true,
        };
        assert!(!modifiers_match(&spec, mods));
        // Ctrl+Shift+A spec against Ctrl+Shift held: not self-family (A is not
        // a modifier), so the regular comparison applies.
        let mods = ModifiersHeld {
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
            open_settings: None,
            open_transcribe: None,
            cancel_esc: None,
            mods_probe: probe_returning(ModifiersHeld::default()),
        };
        // RightCtrl pressed + modifier drift: shift still held.
        let mods = ModifiersHeld {
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
            open_settings: None,
            open_transcribe: None,
            cancel_esc: None,
            mods_probe: probe_returning(ModifiersHeld::default()),
        };
        let err = validate_no_conflict(a_ctrl, &hs_with_primary, SlotKind::RecordOnly)
            .expect_err("record_only == primary must error");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("hotkey already used by primary slot"),
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
            open_settings: None,
            open_transcribe: None,
            cancel_esc: None,
            mods_probe: probe_returning(ModifiersHeld::default()),
        };
        let err = validate_no_conflict(spec_b, &hs_with_record, SlotKind::Primary)
            .expect_err("primary == record_only must error");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("hotkey already used by record-only slot"),
            "expected primary vs record-only message, got: {msg}"
        );
        // Different spec -> ok (false-positive guard).
        assert!(
            validate_no_conflict(spec_a, &hs_with_record, SlotKind::Primary).is_ok(),
            "different vk does not collide"
        );
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
            open_settings: None,
            open_transcribe: None,
            cancel_esc: if cancel_esc {
                Some(Arc::new(|| true))
            } else {
                None
            },
            mods_probe: probe_returning(ModifiersHeld::default()),
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

    // ---- Step 3.1: route_event pure router contract ----
    //
    // Esc-cancel ordering and WM_SYSKEYDOWN exclusion become testable
    // interface instead of inline glue in the hook proc. The proc now
    // shrinks to decode+dispatch; these tests pin the routing decision
    // shape that the proc depends on.

    #[test]
    fn route_event_esc_cancels_before_slot_table() {
        let mut hs = HookState::with_probe(probe_returning(ModifiersHeld::default()));
        hs.cancel_esc = Some(Arc::new(|| true));
        // Primary slot ALSO registered on Esc — cancel routing must win:
        // route_event returns EscCancel (the cancel branch runs BEFORE the
        // slot table), never Fire(primary).
        hs.primary = Some((
            HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0x1B,
            },
            Arc::new(|_| {}),
        ));
        let action = route_event(
            &hs,
            KeyEvent {
                vk: 0x1B,
                is_keydown: true,
                is_sys: false,
            },
        );
        assert!(matches!(action, HookAction::EscCancel(_)));
    }

    #[test]
    fn esc_not_handled_falls_back_to_slot_dispatch() {
        // The hook proc calls the cancel callback OUTSIDE the lock; when it
        // returns false (no active pipeline) the proc re-enters slot
        // dispatch. This test pins the fallback: after an EscCancel route,
        // dispatch_key_event still matches a primary slot registered on Esc.
        let mut hs = HookState::with_probe(probe_returning(ModifiersHeld::default()));
        hs.cancel_esc = Some(Arc::new(|| false));
        hs.primary = Some((
            HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0x1B,
            },
            Arc::new(|_| {}),
        ));
        let action = route_event(
            &hs,
            KeyEvent {
                vk: 0x1B,
                is_keydown: true,
                is_sys: false,
            },
        );
        assert!(matches!(action, HookAction::EscCancel(_)));
        let cb = dispatch_key_event(&hs, 0x1B, true); // proc's 2nd pass
        assert!(
            cb.is_some(),
            "Esc-down must still match the primary spec after a non-handled cancel"
        );
    }

    #[test]
    fn route_event_excludes_syskeydown_from_esc_cancel() {
        let hs = HookState {
            primary: None,
            record_only: None,
            open_settings: None,
            open_transcribe: None,
            cancel_esc: Some(Arc::new(|| true)),
            mods_probe: probe_returning(ModifiersHeld::default()),
        };
        // Alt+Esc (WM_SYSKEYDOWN) is the window-cycle shortcut — never a
        // cancel candidate; with no slot matching it, routing is Pass.
        let action = route_event(
            &hs,
            KeyEvent {
                vk: 0x1B,
                is_keydown: true,
                is_sys: true,
            },
        );
        assert!(matches!(action, HookAction::Pass));
    }

    #[test]
    fn decode_key_event_maps_windows_messages() {
        assert_eq!(
            decode_key_event(WM_KEYDOWN, 0x41),
            Some(KeyEvent {
                vk: 0x41,
                is_keydown: true,
                is_sys: false
            })
        );
        assert_eq!(
            decode_key_event(WM_SYSKEYUP, 0x41),
            Some(KeyEvent {
                vk: 0x41,
                is_keydown: false,
                is_sys: true
            })
        );
        assert_eq!(decode_key_event(0xdead_u32, 0x41), None);
    }
}
