use super::hotkey_pipeline::make_hotkey_callback;
use crate::config::{ApiKeyMask, AppConfig};
use crate::error::CommandError;
use crate::hotkey::HotkeyManager;
use crate::hotkey::windows::WindowsHotkeyManager;
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

fn window_slot_label(kind: crate::commands::WindowKind) -> &'static str {
    match kind {
        crate::commands::WindowKind::Settings => "settings",
        crate::commands::WindowKind::Transcribe => "transcribe",
    }
}

/// Single-slot swap: unregister → register (or clear) → on failure
/// re-register the OLD spec so the slot never goes dead; a failed revert
/// only warns (slot dead until restart). Message texts byte-identical to
/// the former inline blocks (they surface via hotkey-error to the
/// settings UI).
fn swap_open_slot(
    hm: &mut dyn OpenSlotRegistrar,
    kind: crate::commands::WindowKind,
    new: Option<crate::hotkey::windows::HotkeySpec>,
    old: Option<crate::hotkey::windows::HotkeySpec>,
    make_cb: impl Fn() -> crate::hotkey::HotkeyCallback,
) -> Result<(), String> {
    let label = window_slot_label(kind);
    hm.unregister_open_slot(kind);
    let Some(spec) = new else { return Ok(()) };
    if let Err(e) = hm.register_open_slot(kind, spec, make_cb()) {
        if let Some(old_spec) = old
            && let Err(e2) = hm.register_open_slot(kind, old_spec, make_cb())
        {
            warn!("open-{label} revert ALSO failed; slot dead until restart: {e2}");
        }
        return Err(format!("open-{label}: {e}"));
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

/// Whether autostart is available in the current build.
/// - Release: always true.
/// - Debug: only when DL_AUTOSTART=1 env var is set.
#[tauri::command]
pub fn is_autostart_available() -> bool {
    if cfg!(debug_assertions) {
        std::env::var("DL_AUTOSTART").as_deref() == Ok("1")
    } else {
        true
    }
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
    let hotkey_changed = config.hotkey != old_config.hotkey;
    let open_settings_changed = config.open_settings_hotkey != old_config.open_settings_hotkey;
    let open_transcribe_changed =
        config.open_transcribe_hotkey != old_config.open_transcribe_hotkey;

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
                // Unregister ONLY the primary slot (M2 fix — keeps record_only
                // and cancel_esc live across save_settings re-registration).
                // Full unregister() here would wipe the cancel_esc slot, which
                // the Esc-cancel dispatcher reads from to abort an in-flight
                // transcription. The hook itself is removed by unregister_primary
                // only when all three slots are clear.
                if let Err(e) = hm.unregister_primary() {
                    return Err(format!("unregister failed: {e}"));
                }

                // M3 fix — pull the managed single PipelineState instance instead
                // of constructing a new one with PipelineState::from_app. The new
                // instance would have an independent DeliveryController context,
                // and the cancel_esc callback would target the wrong PS.
                let ps_managed = app_clone
                    .state::<super::pipeline_state::PipelineState>()
                    .inner()
                    .clone();
                let callback = make_hotkey_callback(ps_managed.clone());

                // Try registering the new key.
                match hm.register(new_key, callback) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        // Fallback: re-register the old key.
                        let fallback_callback = make_hotkey_callback(ps_managed);
                        let _ = hm.register(old_key, fallback_callback);
                        Err(format!(
                            "新热键注册失败({e})，已回退到旧热键: {}",
                            old_key.display()
                        ))
                    }
                }
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
                    if let Err(msg) = swap_open_slot(
                        hm,
                        crate::commands::WindowKind::Settings,
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
                    if let Err(msg) = swap_open_slot(
                        hm,
                        crate::commands::WindowKind::Transcribe,
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
    use crate::hotkey::{HotkeyCallback, HotkeyEvent};

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

    impl super::OpenSlotRegistrar for MockRegistrar {
        fn unregister_open_slot(&mut self, _kind: crate::commands::WindowKind) {
            self.calls.push(("unregister", 0));
        }
        fn register_open_slot(
            &mut self,
            _kind: crate::commands::WindowKind,
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
    fn swap_open_slot_registers_new_spec_after_unregister() {
        let mut m = MockRegistrar::default();
        let r = super::swap_open_slot(
            &mut m,
            crate::commands::WindowKind::Settings,
            Some(spec_with_vk(1)),
            Some(spec_with_vk(2)),
            dummy_callback,
        );
        assert!(r.is_ok());
        assert_eq!(m.calls, vec![("unregister", 0), ("register", 1)]);
    }

    #[test]
    fn swap_open_slot_reverts_to_old_spec_on_register_failure() {
        let mut m = MockRegistrar {
            fail_vks: vec![1],
            ..Default::default()
        };
        let r = super::swap_open_slot(
            &mut m,
            crate::commands::WindowKind::Settings,
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
        let r = super::swap_open_slot(
            &mut m,
            crate::commands::WindowKind::Transcribe,
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
        let r = super::swap_open_slot(
            &mut m,
            crate::commands::WindowKind::Settings,
            None,
            Some(spec_with_vk(9)),
            dummy_callback,
        );
        assert!(r.is_ok());
        assert_eq!(m.calls, vec![("unregister", 0)]);
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
}
