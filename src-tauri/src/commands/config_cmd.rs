use super::hotkey_pipeline::make_hotkey_callback;
use crate::config::{ApiKeyMask, AppConfig};
use crate::error::CommandError;
use crate::hotkey::HotkeyManager;
use crate::hotkey::windows::HotkeySpec;
use crate::hotkey::windows::WindowsHotkeyManager;
use crate::hotkey::{SPEC_SLOTS, SlotKind};
use crate::speech::SpeechEngine;
use std::sync::Arc;
use std::sync::{Mutex, mpsc};
use std::time::Duration;
use tauri::{Emitter, Manager};
use tracing::warn;

// ===== Open-slot swap seam =====
//
// Private to config_cmd.rs: this trait exists solely so the open-window
// slot re-registration logic can be unit-tested with a MockRegistrar
// without dragging in a real Win32 keyboard hook. Two adapters (the real
// WindowsHotkeyManager + MockRegistrar in tests) — a real seam, not a
// hypothetical one. The hotkey module's public interface does NOT grow
// per-consumer methods (2026-09-12 user ruling: don't split traits by
// caller).

trait OpenSlotRegistrar {
    fn unregister_open_slot(&mut self, kind: crate::commands::WindowKind);
    fn register_open_slot(
        &mut self,
        kind: crate::commands::WindowKind,
        spec: crate::hotkey::windows::HotkeySpec,
        cb: crate::hotkey::HotkeyCallback,
    ) -> Result<(), String>;
}

impl OpenSlotRegistrar for WindowsHotkeyManager {
    fn unregister_open_slot(&mut self, kind: crate::commands::WindowKind) {
        // Errors ignored by design (mirrors the current `let _ =` sites).
        match kind {
            crate::commands::WindowKind::Settings => {
                let _ = self.unregister_open_settings();
            }
            crate::commands::WindowKind::Transcribe => {
                let _ = self.unregister_open_transcribe();
            }
        }
    }
    fn register_open_slot(
        &mut self,
        kind: crate::commands::WindowKind,
        spec: crate::hotkey::windows::HotkeySpec,
        cb: crate::hotkey::HotkeyCallback,
    ) -> Result<(), String> {
        let result = match kind {
            crate::commands::WindowKind::Settings => self.register_open_settings(spec, cb),
            crate::commands::WindowKind::Transcribe => self.register_open_transcribe(spec, cb),
        };
        result.map_err(|e| e.to_string())
    }
}

/// Which spec slot a swap touches.
#[derive(Clone, Copy)]
enum SpecSlot {
    /// The primary hotkey slot.
    Primary,
    /// An open-window slot (settings / transcribe).
    Open(crate::commands::WindowKind),
}

impl SpecSlot {
    /// The slot's label in error messages — the hook layer's own label
    /// for the slot (see `hotkey::SPEC_SLOTS`).
    fn label(&self) -> &'static str {
        match self {
            SpecSlot::Primary => "primary",
            SpecSlot::Open(crate::commands::WindowKind::Settings) => "open-settings",
            SpecSlot::Open(crate::commands::WindowKind::Transcribe) => "open-transcribe",
        }
    }
}

/// The operations a spec-slot swap needs.
///
/// A trait so the swap transaction can be driven by a test double (the
/// real path runs inside a Tauri app).
trait SpecSlotSwap {
    /// Unregister the slot. The primary slot reports its failure; the
    /// open-window slots clear theirs by design (their errors were
    /// ignored at the call sites).
    fn unregister_slot(&mut self, slot: SpecSlot) -> Result<(), String>;
    /// Register the slot.
    fn register_slot(
        &mut self,
        slot: SpecSlot,
        spec: crate::hotkey::windows::HotkeySpec,
        cb: crate::hotkey::HotkeyCallback,
    ) -> Result<(), String>;
}

impl SpecSlotSwap for WindowsHotkeyManager {
    fn unregister_slot(&mut self, slot: SpecSlot) -> Result<(), String> {
        // Unregister ONLY this slot — a full unregister would wipe the
        // cancel_esc slot the Esc-cancel dispatcher reads (M2 fix).
        match slot {
            SpecSlot::Primary => self
                .unregister_primary()
                .map_err(|e| format!("unregister failed: {e}")),
            SpecSlot::Open(kind) => {
                self.unregister_open_slot(kind);
                Ok(())
            }
        }
    }

    fn register_slot(
        &mut self,
        slot: SpecSlot,
        spec: crate::hotkey::windows::HotkeySpec,
        cb: crate::hotkey::HotkeyCallback,
    ) -> Result<(), String> {
        match slot {
            SpecSlot::Primary => self.register(spec, cb).map_err(|e| e.to_string()),
            SpecSlot::Open(kind) => self.register_open_slot(kind, spec, cb),
        }
    }
}

/// Single-slot swap: unregister → register (or clear) → on failure
/// re-register the OLD spec so the slot never goes dead; a failed revert
/// only warns (slot dead until restart).
///
/// The primary slot and the open-window slots used to run this
/// transaction twice — the primary inline in `save_settings`, the open
/// slots in `swap_open_slot` — with slightly different messages and
/// different error handling. One implementation owns it now.
fn swap_spec_slot(
    hm: &mut impl SpecSlotSwap,
    slot: SpecSlot,
    new: Option<crate::hotkey::windows::HotkeySpec>,
    old: Option<crate::hotkey::windows::HotkeySpec>,
    make_cb: impl Fn() -> crate::hotkey::HotkeyCallback,
) -> Result<(), String> {
    let label = slot.label();
    hm.unregister_slot(slot)?;
    let Some(spec) = new else { return Ok(()) };
    if let Err(e) = hm.register_slot(slot, spec, make_cb()) {
        if let Some(old_spec) = old
            && let Err(e2) = hm.register_slot(slot, old_spec, make_cb())
        {
            warn!("{label} revert ALSO failed; slot dead until restart: {e2}");
        }
        return Err(format!("{label}: {e}"));
    }
    Ok(())
}

/// Run a hotkey-mutating task on the main thread and wait for its ack.
/// Owns the entire tail (lock, recv_timeout(5s), ErrorHistory record,
/// hotkey-error emit, CommandError) — formerly duplicated verbatim between
/// the primary and open-window re-registration blocks. BehaviorChange:
/// open-window re-registration path now warns when ErrorHistory is
/// unmanaged (was primary-only); lock-poison message carries the source
/// error (was bare string in the open path).
fn run_hotkey_task_on_main(
    app: &tauri::AppHandle,
    timeout_msg: &'static str,
    task: impl FnOnce(&tauri::AppHandle, &mut WindowsHotkeyManager) -> Result<(), String>
    + Send
    + 'static,
) -> Result<(), CommandError> {
    let (tx, rx) = mpsc::channel();
    let app_clone = app.clone();
    let _ = app.run_on_main_thread(move || {
        let hm_state = app_clone.state::<Mutex<WindowsHotkeyManager>>();
        let mut hm = match hm_state.lock() {
            Ok(hm) => hm,
            Err(e) => {
                let _ = tx.send(Err(format!("lock failed: {e}")));
                return;
            }
        };
        let _ = tx.send(task(&app_clone, &mut hm));
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            // Config is already persisted at this point; try_state (not state)
            // so a missing history cannot turn a saved config into "save failed".
            if let Some(history) =
                app.try_state::<Arc<crate::commands::error_history::ErrorHistory>>()
            {
                history.record("hotkey-error", &serde_json::json!(e));
            } else {
                tracing::warn!(
                    target: "error_history",
                    "hotkey-error not recorded: ErrorHistory not managed"
                );
            }
            let _ = app.emit("hotkey-error", &e);
            Err(CommandError::new("HOTKEY", e))
        }
        Err(_) => Err(CommandError::new("HOTKEY", timeout_msg)),
    }
}

/// The spec slots whose spec changed and that live-swap on save —
/// derived from SPEC_SLOTS (swaps_on_save); record_only is excluded by
/// its descriptor (restart-to-apply by design).
fn live_swap_slots(new: &AppConfig, old: &AppConfig) -> Vec<SlotKind> {
    SPEC_SLOTS
        .iter()
        .filter(|slot| slot.swaps_on_save)
        .filter(|slot| {
            let (new_spec, old_spec) = spec_pair(new, old, slot.kind);
            new_spec != old_spec
        })
        .map(|slot| slot.kind)
        .collect()
}

/// The (new, old) spec pair a slot reads from the config. Compiler-
/// exhaustive on SlotKind: a new slot without a field mapping here fails
/// to compile. The required slots wrap in `Some` so every slot compares
/// the same way.
fn spec_pair(
    new: &AppConfig,
    old: &AppConfig,
    kind: SlotKind,
) -> (Option<HotkeySpec>, Option<HotkeySpec>) {
    match kind {
        SlotKind::Primary => (Some(new.hotkey), Some(old.hotkey)),
        SlotKind::RecordOnly => (Some(new.record_only_hotkey), Some(old.record_only_hotkey)),
        SlotKind::OpenSettings => (new.open_settings_hotkey, old.open_settings_hotkey),
        SlotKind::OpenTranscribe => (new.open_transcribe_hotkey, old.open_transcribe_hotkey),
    }
}

/// Whether autostart is available in the current build.
/// - Release: always true.
/// - Debug: only when DL_AUTOSTART=1 env var is set.
#[tauri::command]
pub fn is_autostart_available() -> bool {
    crate::autostart_gate()
}

/// Return the current compute mode: "gpu", "cpu", or "unloaded".
#[tauri::command]
pub fn get_compute_mode(
    engine: tauri::State<'_, Arc<dyn SpeechEngine>>,
) -> Result<String, CommandError> {
    Ok(engine.compute_mode().to_string())
}

/// Return the current application config to the frontend.
/// The API key is replaced with a masked marker if set.
#[tauri::command]
pub fn get_config(
    config_cache: tauri::State<'_, crate::config::ConfigCache>,
) -> Result<AppConfig, CommandError> {
    let mut config: AppConfig = (*config_cache.read_cached()).clone();
    config.llm_api_key = ApiKeyMask::mask(&config.llm_api_key);
    Ok(config)
}

/// Backend-authoritative defaults for the settings "restore defaults"
/// action (M3-a). Masked like get_config so the wire shape feeds
/// populateFields unchanged; the API key default is empty so the mask is
/// a no-op — kept for shape symmetry and future non-empty defaults.
#[tauri::command]
pub fn get_default_config() -> AppConfig {
    let mut config = AppConfig::default();
    // Route through the same mask as get_config so a future non-empty
    // key default can never ship plaintext to the frontend (runtime
    // guarantee, not a debug_assert that release builds strip).
    config.llm_api_key = ApiKeyMask::mask(&config.llm_api_key);
    config
}

/// Save all settings. Handles hotkey re-registration on the main thread.
#[tauri::command]
pub fn save_settings(
    config: AppConfig,
    config_cache: tauri::State<'_, crate::config::ConfigCache>,
    _hotkey_manager: tauri::State<'_, Mutex<WindowsHotkeyManager>>,
    app: tauri::AppHandle,
) -> Result<(), CommandError> {
    // Validate first.
    config.validate().map_err(CommandError::from)?;

    // Load old config to detect hotkey change and preserve API key if masked.
    let old_config = config_cache.read_cached();
    // The live-swap set derives from SPEC_SLOTS (the slot manifest): a
    // slot swaps at save time iff its spec changed AND its descriptor
    // marks it swaps_on_save. record_only never appears here —
    // restart-to-apply by design.
    let changed = live_swap_slots(&config, &old_config);
    let hotkey_changed = changed.contains(&SlotKind::Primary);
    let open_settings_changed = changed.contains(&SlotKind::OpenSettings);
    let open_transcribe_changed = changed.contains(&SlotKind::OpenTranscribe);

    // If the frontend sent the masked marker, preserve the existing decrypted key.
    let mut config = config;
    config.llm_api_key = ApiKeyMask::unmask_or_keep(&config.llm_api_key, &old_config.llm_api_key);

    // Save new config to disk and update cache (save_cached encrypts the API key).
    config_cache
        .save_cached(&config)
        .map_err(CommandError::from)?;

    // Re-register hotkey if changed.
    if hotkey_changed {
        let old_key = old_config.hotkey;
        let new_key = config.hotkey;
        run_hotkey_task_on_main(
            &app,
            "hotkey re-registration timed out",
            move |app_clone, hm| {
                // M3 fix — pull the managed single PipelineState instance instead
                // of constructing a new one with PipelineState::from_app. The new
                // instance would have an independent DeliveryController context,
                // and the cancel_esc callback would target the wrong PS.
                let ps_managed = app_clone
                    .state::<super::pipeline_state::PipelineState>()
                    .inner()
                    .clone();
                swap_spec_slot(hm, SpecSlot::Primary, Some(new_key), Some(old_key), || {
                    make_hotkey_callback(ps_managed.clone())
                })
            },
        )?;
    }
    if open_settings_changed || open_transcribe_changed {
        run_hotkey_task_on_main(
            &app,
            "open-windows hotkey re-registration timed out",
            move |app_clone, hm| {
                let mut failures: Vec<String> = Vec::new();
                if open_settings_changed {
                    let new_s = config.open_settings_hotkey;
                    let old_s = old_config.open_settings_hotkey;
                    let make_cb = || {
                        super::make_open_window_callback(
                            app_clone.clone(),
                            crate::commands::WindowKind::Settings,
                        )
                    };
                    if let Err(msg) = swap_spec_slot(
                        hm,
                        SpecSlot::Open(crate::commands::WindowKind::Settings),
                        new_s,
                        old_s,
                        make_cb,
                    ) {
                        failures.push(msg);
                    }
                }
                if open_transcribe_changed {
                    let new_t = config.open_transcribe_hotkey;
                    let old_t = old_config.open_transcribe_hotkey;
                    let make_cb = || {
                        super::make_open_window_callback(
                            app_clone.clone(),
                            crate::commands::WindowKind::Transcribe,
                        )
                    };
                    if let Err(msg) = swap_spec_slot(
                        hm,
                        SpecSlot::Open(crate::commands::WindowKind::Transcribe),
                        new_t,
                        old_t,
                        make_cb,
                    ) {
                        failures.push(msg);
                    }
                }
                if failures.is_empty() {
                    Ok(())
                } else {
                    Err(failures.join("; "))
                }
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::config::AppConfig;
    use crate::config::WhisperModel;
    use crate::hotkey::windows::HotkeySpec;
    use crate::hotkey::{HotkeyCallback, HotkeyEvent, SlotKind};

    // ---- swap_open_slot revert semantics (was: only reachable in a real Tauri app) ----

    /// Test seam for the open-window slot swap sequence. Mock uses a
    /// `fail_vks: Vec<u32>` to drive the three revert paths:
    ///
    /// - vk not in fail_vks → register succeeds
    /// - vk in fail_vks → register returns Err
    ///
    /// The MockRegistrar does not distinguish first-register from
    /// revert-register — that's why the test cases below reason about
    /// call ordering, not just return values.
    #[derive(Default)]
    struct MockRegistrar {
        calls: Vec<(&'static str, u32)>, // (action, vk): action ∈ unregister/register
        fail_vks: Vec<u32>,
    }

    impl super::SpecSlotSwap for MockRegistrar {
        fn unregister_slot(&mut self, _slot: super::SpecSlot) -> Result<(), String> {
            self.calls.push(("unregister", 0));
            Ok(())
        }
        fn register_slot(
            &mut self,
            _slot: super::SpecSlot,
            spec: HotkeySpec,
            _cb: HotkeyCallback,
        ) -> Result<(), String> {
            self.calls.push(("register", spec.vk));
            if self.fail_vks.contains(&spec.vk) {
                Err("boom".to_string())
            } else {
                Ok(())
            }
        }
    }

    /// The primary slot runs the same transaction as the open-window
    /// slots (one implementation) — the symmetric revert case.
    #[test]
    fn swap_spec_slot_primary_reverts_to_old_spec_on_register_failure() {
        let mut m = MockRegistrar {
            fail_vks: vec![1],
            ..Default::default()
        };
        let r = super::swap_spec_slot(
            &mut m,
            super::SpecSlot::Primary,
            Some(spec_with_vk(1)),
            Some(spec_with_vk(2)),
            dummy_callback,
        );
        let err = r.expect_err("first register must fail");
        assert!(err.starts_with("primary: "), "got: {err}");
        assert_eq!(
            m.calls,
            vec![("unregister", 0), ("register", 1), ("register", 2)],
            "the old spec must be re-registered after the failure"
        );
    }

    fn spec_with_vk(vk: u32) -> HotkeySpec {
        HotkeySpec {
            ctrl: false,
            shift: false,
            alt: false,
            vk,
        }
    }

    fn dummy_callback() -> HotkeyCallback {
        Box::new(|_event: HotkeyEvent| {})
    }

    #[test]
    fn swap_spec_slot_registers_new_spec_after_unregister() {
        let mut m = MockRegistrar::default();
        let r = super::swap_spec_slot(
            &mut m,
            super::SpecSlot::Open(crate::commands::WindowKind::Settings),
            Some(spec_with_vk(1)),
            Some(spec_with_vk(2)),
            dummy_callback,
        );
        assert!(r.is_ok());
        assert_eq!(m.calls, vec![("unregister", 0), ("register", 1)]);
    }

    #[test]
    fn swap_spec_slot_reverts_to_old_spec_on_register_failure() {
        let mut m = MockRegistrar {
            fail_vks: vec![1],
            ..Default::default()
        };
        let r = super::swap_spec_slot(
            &mut m,
            super::SpecSlot::Open(crate::commands::WindowKind::Settings),
            Some(spec_with_vk(1)),
            Some(spec_with_vk(2)),
            dummy_callback,
        );
        let err = r.expect_err("first register must fail");
        assert!(err.starts_with("open-settings: "), "got: {err}");
        assert_eq!(
            m.calls,
            vec![
                ("unregister", 0),
                ("register", 1),
                ("register", 2), // revert
            ]
        );
    }

    #[test]
    fn swap_open_slot_revert_failure_returns_first_error_only() {
        let mut m = MockRegistrar {
            fail_vks: vec![1, 2],
            ..Default::default()
        };
        let r = super::swap_spec_slot(
            &mut m,
            super::SpecSlot::Open(crate::commands::WindowKind::Transcribe),
            Some(spec_with_vk(1)),
            Some(spec_with_vk(2)),
            dummy_callback,
        );
        let err = r.expect_err("both registers fail");
        assert!(err.starts_with("open-transcribe: "), "got: {err}");
        // Revert failure only emits a warn (slot dead until restart) — the
        // returned error stays focused on the user-facing failure, NOT
        // concatenated with the revert string.
        assert!(
            !err.contains("revert"),
            "revert failure must only warn, not join: {err}"
        );
        assert_eq!(
            m.calls,
            vec![
                ("unregister", 0),
                ("register", 1),
                ("register", 2), // revert also failed — only warned
            ]
        );
    }

    #[test]
    fn swap_open_slot_none_only_unregisters() {
        let mut m = MockRegistrar::default();
        let r = super::swap_spec_slot(
            &mut m,
            super::SpecSlot::Open(crate::commands::WindowKind::Settings),
            None,
            Some(spec_with_vk(9)),
            dummy_callback,
        );
        assert!(r.is_ok());
        assert_eq!(m.calls, vec![("unregister", 0)]);
    }

    // ---- live_swap_slots: the save-time swap set derives from SPEC_SLOTS ----

    /// THE behavior pin: changing the record-only hotkey never live-swaps —
    /// it applies on restart by design (the settings page's directive save
    /// message「录音快捷键重启应用后生效」). The exclusion lives in the
    /// descriptor (`swaps_on_save == false`, pinned by the hotkey module's
    /// ratchet test), not in this function.
    #[test]
    fn live_swap_slots_ignores_record_only_change() {
        let old = AppConfig::default();
        let new = AppConfig {
            record_only_hotkey: spec_with_vk(0x2C),
            ..Default::default()
        };
        assert!(super::live_swap_slots(&new, &old).is_empty());
    }

    #[test]
    fn live_swap_slots_primary_change() {
        let old = AppConfig::default();
        let new = AppConfig {
            hotkey: spec_with_vk(0x21),
            ..Default::default()
        };
        assert_eq!(super::live_swap_slots(&new, &old), vec![SlotKind::Primary]);
    }

    #[test]
    fn live_swap_slots_open_settings_change() {
        let old = AppConfig::default();
        let new = AppConfig {
            open_settings_hotkey: Some(spec_with_vk(0x7B)),
            ..Default::default()
        };
        assert_eq!(
            super::live_swap_slots(&new, &old),
            vec![SlotKind::OpenSettings]
        );
    }

    #[test]
    fn live_swap_slots_multiple_changes_follow_spec_slots_order() {
        let old = AppConfig::default();
        // Mutate out of SPEC_SLOTS order (transcribe → settings → primary);
        // the result must follow the manifest's canonical order, not the
        // mutation order.
        let new = AppConfig {
            open_transcribe_hotkey: Some(spec_with_vk(0x7C)),
            open_settings_hotkey: Some(spec_with_vk(0x7B)),
            hotkey: spec_with_vk(0x22),
            ..Default::default()
        };
        assert_eq!(
            super::live_swap_slots(&new, &old),
            vec![
                SlotKind::Primary,
                SlotKind::OpenSettings,
                SlotKind::OpenTranscribe
            ]
        );
    }

    #[test]
    fn test_read_cached_returns_default() {
        let cache = crate::config::ConfigCache::new(AppConfig::default());
        let result = cache.read_cached();
        assert_eq!(
            result.hotkey,
            HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0xA3,
            }
        );
    }

    #[test]
    fn test_default_config_validates() {
        let config = AppConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_enum_serialization_roundtrip() {
        let config = AppConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        // WhisperModel::Base should serialize as "base".
        assert!(json.contains("\"whisper_model\":\"base\""));
        // Language::Zh should serialize as "zh".
        assert!(json.contains("\"language\":\"zh\""));
        // DownloadMirror::HfMirror should serialize as "hf-mirror".
        assert!(json.contains("\"download_mirror\":\"hf-mirror\""));
        let parsed: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.whisper_model, WhisperModel::Base);
    }

    #[test]
    fn test_validate_rejects_invalid_hotkey() {
        // vk=0 is the simplest invalid object form — no name roundtrip.
        let config = AppConfig {
            hotkey: HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0,
            },
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_rejects_empty_llm_when_enabled() {
        let config = AppConfig {
            llm_enabled: true,
            llm_api_url: String::new(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_accepts_valid_config() {
        let config = AppConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_accepts_llm_disabled_with_empty_fields() {
        let config = AppConfig {
            llm_enabled: false,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    // ---- M3-a two-layer penetration proof ----

    #[test]
    fn test_clear_sentinel_then_save_leaves_no_dpapi_residue() {
        // Two-layer penetration proof (M3-a hard requirement): a config that
        // HAD an encrypted key, resolved through unmask_or_keep(CLEAR), ends
        // up with an empty key — which save() writes as "" (never DPAPI:).
        let old = "DPAPI:AAAAZmFrZQ=="; // stand-in for a previously stored blob
        let resolved = crate::config::ApiKeyMask::unmask_or_keep(crate::config::CLEAR_MARKER, old);
        assert_eq!(resolved, "");
        assert!(!resolved.contains("DPAPI:"));
        // Defaults themselves carry no credentials.
        let d = crate::config::AppConfig::default();
        assert_eq!(d.llm_api_key, "");
        assert_eq!(d.llm_api_url, "");
        assert_eq!(d.data_saving_path, "");
    }

    // ---- autostart gate single source (Round 6 P10 Task 1) ----

    /// Cut the file at its `#[cfg(test)]` module boundary so the ratchet
    /// below inspects the PRODUCTION region only. The test module is
    /// excluded, so this test's own source can neither satisfy nor break
    /// the assertions (no self-matching).
    fn production_region(src: &str) -> &str {
        // Marker assembled at compile time so this test's own source
        // never spells it contiguously.
        const MARKER: &str = concat!("#[cfg(", "test)]");
        src.split_once(MARKER)
            .map(|(production, _test_module)| production)
            .expect("config_cmd.rs must contain a `#[cfg(test)]` module boundary")
    }

    /// Drop comment-only lines so the ratchet below can assert on this
    /// file's CODE. The `is_autostart_available` doc comment names the
    /// environment variable (documentation, not an implementation) — that
    /// mention must not be mistaken for an inlined gate.
    ///
    /// Boundary: only WHOLE-LINE comments are dropped — a line whose
    /// trimmed text starts with `//` (which also covers `///` doc lines).
    /// A trailing comment at the end of a code line is NOT stripped and
    /// stays visible to the assertions.
    fn code_without_comments(src: &str) -> String {
        src.lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn autostart_gate_has_single_source_in_lib() {
        // Self-reference guards: the tokens below are assembled at compile
        // time (concat!) so this test's own source never spells either of
        // them contiguously.
        const ENV_TOKEN: &str = concat!("DL_", "AUTOSTART");
        const DELEGATION: &str = concat!("crate::autostart", "_gate()");

        let src = include_str!("config_cmd.rs");
        // The PRODUCTION region only — the test module is excluded, so the
        // delegation assertion below cannot be satisfied by the test's own
        // source.
        let production = production_region(src);
        let code = code_without_comments(production);
        assert!(
            !code.contains(ENV_TOKEN),
            "config_cmd.rs must not inline the autostart gate — the env var \
             name may only live in lib.rs::autostart_gate"
        );
        assert!(
            production.contains(DELEGATION),
            "is_autostart_available must delegate to the lib.rs gate"
        );
        // Type-level check: the command and the gate agree by construction
        // (same predicate, same env, same moment).
        assert_eq!(super::is_autostart_available(), crate::autostart_gate());
    }
}
