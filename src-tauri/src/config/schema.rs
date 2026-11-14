use super::catalog::{DownloadMirror, Language, WhisperModel};
use crate::error::AppError;
use crate::hotkey::windows::HotkeySpec;
use crate::hotkey::{SPEC_SLOTS, SlotKind};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Operational mode of the voice pipeline, derived from realtime_transcription
/// and review_before_paste config flags. Used for exhaustive dispatch instead of
/// scattered boolean checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineMode {
    /// RT=off, REVIEW=off: Classic full Whisper, direct inject.
    ClassicDirect,
    /// RT=off, REVIEW=on: Classic full Whisper, review window.
    ClassicReview,
    /// RT=on, REVIEW=off: Realtime transcription, direct inject (skip Whisper).
    RealtimeDirect,
    /// RT=on, REVIEW=on: Realtime transcription, live review window.
    RealtimeReview,
}

/// Application configuration.
#[derive(Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Hotkey spec (default: RightCtrl with no modifier flags).
    #[serde(default = "default_hotkey")]
    pub hotkey: HotkeySpec,

    /// Recognition language.
    #[serde(default)]
    pub language: Language,

    /// Whisper model size.
    #[serde(default)]
    pub whisper_model: WhisperModel,

    /// Whether LLM post-processing is enabled.
    pub llm_enabled: bool,

    /// LLM API base URL.
    pub llm_api_url: String,

    /// LLM API key.
    pub llm_api_key: String,

    /// LLM model name.
    pub llm_model: String,

    /// Download mirror.
    #[serde(default)]
    pub download_mirror: DownloadMirror,

    /// Whether to save training data (audio + transcription) locally.
    #[serde(default)]
    pub data_saving_enabled: bool,

    /// Directory path for saving training data (WAV + JSON).
    #[serde(default)]
    pub data_saving_path: String,

    /// Whether to show a review window before pasting transcribed text.
    #[serde(default)]
    pub review_before_paste: bool,

    /// Whether to auto-start on system boot.
    #[serde(default)]
    pub autostart: bool,

    /// Whether real-time transcription is enabled.
    #[serde(default)]
    pub realtime_transcription: bool,

    /// Whether record-only mode (hold to record, release to save, no transcription) is enabled.
    #[serde(default)]
    pub record_only_enabled: bool,

    /// Hotkey spec for record-only mode (default: RightAlt with no modifier flags).
    #[serde(default = "default_record_only_hotkey")]
    pub record_only_hotkey: HotkeySpec,

    /// Optional global hotkey to open the settings window. None = disabled.
    /// Default None to avoid stealing keys the user already uses globally.
    #[serde(default)]
    pub open_settings_hotkey: Option<HotkeySpec>,

    /// Optional global hotkey to open the transcribe window. None = disabled.
    /// Default None to avoid stealing keys the user already uses globally.
    #[serde(default)]
    pub open_transcribe_hotkey: Option<HotkeySpec>,
}

fn default_hotkey() -> HotkeySpec {
    HotkeySpec {
        ctrl: false,
        shift: false,
        alt: false,
        vk: 0xA3, // RightCtrl
    }
}

fn default_record_only_hotkey() -> HotkeySpec {
    HotkeySpec {
        ctrl: false,
        shift: false,
        alt: false,
        vk: 0xA5, // RightAlt
    }
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("hotkey", &self.hotkey)
            .field("language", &self.language)
            .field("whisper_model", &self.whisper_model)
            .field("llm_enabled", &self.llm_enabled)
            .field(
                "llm_api_url",
                &crate::redaction::redact_error_detail(&self.llm_api_url, &self.llm_api_key),
            )
            .field(
                "llm_api_key",
                &if self.llm_api_key.is_empty() {
                    ""
                } else {
                    "******"
                },
            )
            .field("llm_model", &self.llm_model)
            .field("download_mirror", &self.download_mirror)
            .field("data_saving_enabled", &self.data_saving_enabled)
            .field("data_saving_path", &self.data_saving_path)
            .field("review_before_paste", &self.review_before_paste)
            .field("autostart", &self.autostart)
            .field("realtime_transcription", &self.realtime_transcription)
            .field("record_only_enabled", &self.record_only_enabled)
            .field("record_only_hotkey", &self.record_only_hotkey)
            .field("open_settings_hotkey", &self.open_settings_hotkey)
            .field("open_transcribe_hotkey", &self.open_transcribe_hotkey)
            .finish()
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey(),
            language: Language::Zh,
            whisper_model: WhisperModel::Base,
            llm_enabled: false,
            llm_api_url: String::new(),
            llm_api_key: String::new(),
            llm_model: String::new(),
            download_mirror: DownloadMirror::HfMirror,
            data_saving_enabled: false,
            data_saving_path: String::new(),
            review_before_paste: false,
            autostart: false,
            realtime_transcription: false,
            record_only_enabled: false,
            record_only_hotkey: default_record_only_hotkey(),
            open_settings_hotkey: None,
            open_transcribe_hotkey: None,
        }
    }
}

impl AppConfig {
    /// Validate config fields.
    /// Model, language, and mirror are enforced by the type system (enums).
    pub fn validate(&self) -> Result<(), AppError> {
        // vk must be non-zero AND the name roundtrip must succeed — this
        // rejects arbitrary u32 values that the object-form Deserialize
        // would otherwise accept silently (then surprise the user with a
        // "dead" hotkey on first save).
        if self.hotkey.vk == 0 || !crate::hotkey::is_resolvable_vk(self.hotkey.vk) {
            let hotkey = &self.hotkey;
            return Err(AppError::Config(format!("invalid hotkey: {hotkey}")));
        }
        if self.record_only_hotkey.vk == 0
            || !crate::hotkey::is_resolvable_vk(self.record_only_hotkey.vk)
        {
            let key = &self.record_only_hotkey;
            return Err(AppError::Config(format!(
                "invalid record_only_hotkey: {key}"
            )));
        }
        // The spec slots must be pairwise distinct where set (M6-a matrix
        // at the config boundary; the hook layer re-checks at register
        // time). The slot list comes from `SPEC_SLOTS` — the single
        // source shared with the hook boundary; only the spec lookup is
        // local (the two layers check different things).
        let slots: Vec<(&str, Option<HotkeySpec>)> = SPEC_SLOTS
            .iter()
            .map(|slot| {
                let spec = match slot.kind {
                    SlotKind::Primary => Some(self.hotkey),
                    SlotKind::RecordOnly => Some(self.record_only_hotkey),
                    SlotKind::OpenSettings => self.open_settings_hotkey,
                    SlotKind::OpenTranscribe => self.open_transcribe_hotkey,
                };
                (slot.config_field, spec)
            })
            .collect();
        for i in 0..slots.len() {
            for j in (i + 1)..slots.len() {
                if let (Some(a), Some(b)) = (slots[i].1, slots[j].1)
                    && a == b
                {
                    return Err(AppError::Config(format!(
                        "{} and {} must be different",
                        slots[i].0, slots[j].0
                    )));
                }
            }
        }
        if self.llm_enabled
            && (self.llm_api_url.is_empty()
                || self.llm_api_key.is_empty()
                || self.llm_model.is_empty())
        {
            return Err(AppError::Config(
                "LLM API URL, Key, and Model are required when LLM is enabled".to_string(),
            ));
        }
        if self.data_saving_enabled && self.data_saving_path.trim().is_empty() {
            return Err(AppError::Config(
                "Data saving path is required when data saving is enabled".to_string(),
            ));
        }
        if self.record_only_enabled && self.data_saving_path.trim().is_empty() {
            return Err(AppError::Config(
                "Data saving path is required when record-only mode is enabled".to_string(),
            ));
        }
        Ok(())
    }

    /// Derive the pipeline mode from realtime_transcription and review_before_paste.
    pub fn pipeline_mode(&self) -> PipelineMode {
        match (self.realtime_transcription, self.review_before_paste) {
            (false, false) => PipelineMode::ClassicDirect,
            (false, true) => PipelineMode::ClassicReview,
            (true, false) => PipelineMode::RealtimeDirect,
            (true, true) => PipelineMode::RealtimeReview,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audit test: every pub-serialized field must appear by NAME in the
    /// Debug output (values are intentionally redacted). Driven by
    /// serde_json::to_value so a future `#[serde(skip_serializing_if)]` on
    /// a field that is None/empty in BOTH default + populated forms
    /// would still surface here (the known residual blind spot —
    /// m6a-fix plan defect #5). Today there are no skip attributes on
    /// any field (verified at planning).
    #[test]
    fn debug_impl_lists_every_serialized_field() {
        let spec = HotkeySpec {
            ctrl: false,
            shift: false,
            alt: false,
            vk: 0xA3,
        };
        let populated = AppConfig {
            open_settings_hotkey: Some(spec),
            open_transcribe_hotkey: Some(spec),
            ..AppConfig::default()
        };
        for config in [AppConfig::default(), populated] {
            let dbg = format!("{config:?}");
            let value = serde_json::to_value(&config).unwrap();
            for key in value.as_object().unwrap().keys() {
                assert!(
                    dbg.contains(key),
                    "pub field `{key}` missing from AppConfig Debug output"
                );
            }
        }
    }

    #[test]
    fn test_default_values() {
        let config = AppConfig::default();
        assert_eq!(
            config.hotkey,
            HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0xA3,
            }
        );
        assert_eq!(config.language, Language::Zh);
        assert_eq!(config.whisper_model, WhisperModel::Base);
        assert_eq!(config.download_mirror, DownloadMirror::HfMirror);
        assert!(!config.llm_enabled);
        assert!(config.llm_api_url.is_empty());
    }

    #[test]
    fn test_serialize_deserialize() -> Result<(), Box<dyn std::error::Error>> {
        let config = AppConfig::default();
        let json = serde_json::to_string(&config)?;
        let parsed: AppConfig = serde_json::from_str(&json)?;
        assert_eq!(config.hotkey, parsed.hotkey);
        assert_eq!(config.language, parsed.language);
        Ok(())
    }

    #[test]
    fn test_validate_rejects_invalid_hotkey() {
        // vk=0 is invalid (the roundtrip would still fail because vk_to_key_name
        // returns "VK0x0" and from_key_name cannot resolve it back).
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

    #[test]
    fn test_validate_accepts_data_saving_disabled() {
        let config = AppConfig {
            data_saving_enabled: false,
            data_saving_path: String::new(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_rejects_data_saving_without_path() {
        let config = AppConfig {
            data_saving_enabled: true,
            data_saving_path: String::new(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_debug_masks_api_key() {
        let config = AppConfig {
            llm_api_key: "sk-super-secret".to_string(),
            ..Default::default()
        };
        let debug_str = format!("{config:?}");
        assert!(debug_str.contains("******"));
        assert!(!debug_str.contains("sk-super-secret"));
    }

    #[test]
    fn test_debug_shows_empty_key() {
        let config = AppConfig::default();
        let debug_str = format!("{config:?}");
        // Empty key should show as "" not "******"
        assert!(!debug_str.contains("******"));
        assert!(debug_str.contains("llm_api_key"));
    }

    #[test]
    fn test_debug_masks_llm_api_url_query_credentials() {
        let config = AppConfig {
            llm_api_url: "https://api.example.com/v1?key=SECRET&model=gpt".to_string(),
            ..Default::default()
        };
        let dbg = format!("{config:?}");
        assert!(dbg.contains("v1?key=***"));
        assert!(!dbg.contains("SECRET"));
        assert!(dbg.contains("model=gpt"));
    }

    #[test]
    fn test_debug_redacts_unlisted_param_carrying_api_key() {
        // Layer 2: a query param NAME outside the 11-word list whose VALUE
        // equals the configured key must still be masked in {:?} output.
        let config = AppConfig {
            llm_api_url: "https://api.example.com/v1?sig=sk-live-abc123".to_string(),
            llm_api_key: "sk-live-abc123".to_string(),
            ..Default::default()
        };
        let dbg = format!("{config:?}");
        assert!(
            !dbg.contains("sk-live-abc123"),
            "unlisted param name must not leak the key value: {dbg}"
        );
    }

    #[test]
    fn test_debug_keeps_plain_llm_api_url() {
        let config = AppConfig {
            llm_api_url: "https://api.example.com/v1".to_string(),
            ..Default::default()
        };
        assert!(format!("{config:?}").contains("https://api.example.com/v1"));
    }

    #[test]
    fn test_whisper_model_custom_in_config() -> Result<(), Box<dyn std::error::Error>> {
        let config = AppConfig {
            whisper_model: WhisperModel::Custom("my-model.bin".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&config)?;
        assert!(json.contains(r#""whisper_model":"custom:my-model.bin""#));
        let parsed: AppConfig = serde_json::from_str(&json)?;
        assert_eq!(
            parsed.whisper_model,
            WhisperModel::Custom("my-model.bin".to_string())
        );
        Ok(())
    }

    #[test]
    fn test_record_only_defaults() {
        let config = AppConfig::default();
        assert!(!config.record_only_enabled);
        assert_eq!(
            config.record_only_hotkey,
            HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0xA5,
            }
        );
    }

    #[test]
    fn test_old_config_without_record_only_fields_deserializes()
    -> Result<(), Box<dyn std::error::Error>> {
        // Config JSON written by a version before record-only mode existed.
        let old_json = r#"{
            "hotkey": "RightCtrl",
            "language": "zh",
            "whisper_model": "base",
            "llm_enabled": false,
            "llm_api_url": "",
            "llm_api_key": "",
            "llm_model": "",
            "download_mirror": "hf-mirror",
            "data_saving_enabled": true,
            "data_saving_path": "D:\\recordings",
            "review_before_paste": true,
            "autostart": true,
            "realtime_transcription": false
        }"#;
        let parsed: AppConfig = serde_json::from_str(old_json)?;
        assert_eq!(
            parsed.hotkey,
            HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0xA3,
            }
        );
        assert!(parsed.data_saving_enabled);
        assert_eq!(parsed.data_saving_path, "D:\\recordings");
        assert!(parsed.review_before_paste);
        assert!(parsed.autostart);
        assert!(!parsed.record_only_enabled);
        assert_eq!(
            parsed.record_only_hotkey,
            HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0xA5,
            }
        );
        Ok(())
    }

    #[test]
    fn test_validate_rejects_same_hotkeys_when_record_only_enabled() {
        let config = AppConfig {
            record_only_enabled: true,
            record_only_hotkey: HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0xA3, // same as default hotkey
            },
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_open_settings_equal_to_primary() {
        let config = AppConfig {
            open_settings_hotkey: Some(HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0xA3, // same as default hotkey
            }),
            ..Default::default()
        };
        let err = config
            .validate()
            .expect_err("open_settings == primary must error");
        assert!(
            err.to_string().contains("hotkey") && err.to_string().contains("open_settings_hotkey"),
            "message must name both slots, got: {err}"
        );
    }

    #[test]
    fn validate_allows_both_open_slots_none() {
        // Default config (both open slots None) -> ok (regression lock).
        assert!(AppConfig::default().validate().is_ok());
    }

    #[test]
    fn validate_rejects_two_identical_open_slots() {
        let spec = HotkeySpec {
            ctrl: true,
            shift: false,
            alt: false,
            vk: 0x53, // S
        };
        let config = AppConfig {
            open_settings_hotkey: Some(spec),
            open_transcribe_hotkey: Some(spec),
            ..Default::default()
        };
        let err = config
            .validate()
            .expect_err("two identical open slots must error");
        assert!(
            err.to_string().contains("open_settings_hotkey")
                && err.to_string().contains("open_transcribe_hotkey"),
            "message must name both slots, got: {err}"
        );
    }

    #[test]
    fn test_validate_rejects_invalid_record_only_hotkey() {
        let config = AppConfig {
            record_only_hotkey: HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0, // invalid — roundtrip fails
            },
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_accepts_record_only_with_distinct_hotkey() {
        let config = AppConfig {
            record_only_enabled: true,
            record_only_hotkey: HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0xA5,
            },
            data_saving_path: "D:\\recordings".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_rejects_record_only_without_path() {
        let config = AppConfig {
            record_only_enabled: true,
            record_only_hotkey: HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0xA5,
            },
            data_saving_path: String::new(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    // ---- P2 HotkeySpec dual-form deserialize ----

    #[test]
    fn hotkey_spec_parses_object_form() {
        let json = r#"{
            "hotkey": {"ctrl":true,"shift":false,"alt":false,"vk":65},
            "language": "zh",
            "whisper_model": "base",
            "llm_enabled": false,
            "llm_api_url": "",
            "llm_api_key": "",
            "llm_model": "",
            "data_saving_path": "",
            "review_before_paste": false,
            "autostart": false
        }"#;
        let parsed: AppConfig = serde_json::from_str(json).expect("object form");
        assert_eq!(
            parsed.hotkey,
            HotkeySpec {
                ctrl: true,
                shift: false,
                alt: false,
                vk: 65,
            }
        );
    }

    #[test]
    fn hotkey_spec_parses_legacy_string() {
        let json = r#"{
            "hotkey": "RightCtrl",
            "language": "zh",
            "whisper_model": "base",
            "llm_enabled": false,
            "llm_api_url": "",
            "llm_api_key": "",
            "llm_model": "",
            "data_saving_path": "",
            "review_before_paste": false,
            "autostart": false
        }"#;
        let parsed: AppConfig = serde_json::from_str(json).expect("legacy string");
        assert_eq!(parsed.hotkey.vk, 0xA3);
        assert!(!parsed.hotkey.ctrl);

        // Aliases resolve to the same vk as canonical names.
        let rctrl: AppConfig = serde_json::from_str(
            r#"{"hotkey":"rctrl","language":"zh","whisper_model":"base","llm_enabled":false,"llm_api_url":"","llm_api_key":"","llm_model":"","data_saving_path":"","review_before_paste":false,"autostart":false}"#,
        )
        .expect("alias");
        assert_eq!(rctrl.hotkey.vk, 0xA3);

        // "escape" -> 0x1B.
        let esc: AppConfig = serde_json::from_str(
            r#"{"hotkey":"escape","language":"zh","whisper_model":"base","llm_enabled":false,"llm_api_url":"","llm_api_key":"","llm_model":"","data_saving_path":"","review_before_paste":false,"autostart":false}"#,
        )
        .expect("escape");
        assert_eq!(esc.hotkey.vk, 0x1B);
    }

    #[test]
    fn hotkey_spec_legacy_string_unknown_errors() {
        // Unknown key name in legacy form must fail — silently accepting
        // would let a typo downgrade to vk=0 and create a dead hotkey.
        let json = r#"{
            "hotkey": "NoSuchKey",
            "language": "zh",
            "whisper_model": "base",
            "llm_enabled": false,
            "llm_api_url": "",
            "llm_api_key": "",
            "llm_model": "",
            "data_saving_path": "",
            "review_before_paste": false,
            "autostart": false
        }"#;
        let result: Result<AppConfig, _> = serde_json::from_str(json);
        assert!(result.is_err(), "unknown legacy name must error");
    }

    #[test]
    fn hotkey_spec_serializes_as_object() {
        let config = AppConfig::default();
        let json = serde_json::to_string(&config).expect("serialize");
        // Save format: object, not string.
        assert!(
            json.contains(r#""hotkey":{"ctrl":false,"shift":false,"alt":false,"vk":163"#),
            "hotkey must serialize as object: {json}"
        );
        assert!(
            !json.contains(r#""hotkey":"RightCtrl""#),
            "no legacy string form"
        );
    }

    #[test]
    fn validate_rejects_zero_vk_and_duplicate_spec() {
        // Zero vk -> invalid.
        let zero = AppConfig {
            hotkey: HotkeySpec {
                vk: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(zero.validate().is_err(), "vk=0 rejected");

        // Unknown vk (arbitrary u32 with no name representation) -> invalid.
        let unknown = AppConfig {
            hotkey: HotkeySpec {
                vk: 32, // space — not in NAMED_KEYS, not a letter/digit
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            unknown.validate().is_err(),
            "arbitrary u32 vk rejected by roundtrip check"
        );

        // Duplicate spec when record_only_enabled -> invalid.
        let same = AppConfig {
            record_only_enabled: true,
            record_only_hotkey: HotkeySpec {
                vk: 0xA3, // same as default hotkey
                ..Default::default()
            },
            data_saving_path: "D:\\recordings".to_string(),
            ..Default::default()
        };
        assert!(same.validate().is_err(), "duplicate hotkey spec rejected");
    }

    #[test]
    fn default_specs_unchanged() {
        let config = AppConfig::default();
        assert_eq!(
            config.hotkey,
            HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0xA3,
            }
        );
        assert_eq!(
            config.record_only_hotkey,
            HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0xA5,
            }
        );
    }

    #[test]
    fn hotkey_spec_display_string() {
        assert_eq!(
            HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0xA3,
            }
            .display(),
            "RightCtrl"
        );
        assert_eq!(
            HotkeySpec {
                ctrl: true,
                shift: true,
                alt: false,
                vk: 0x41,
            }
            .display(),
            "Ctrl+Shift+A"
        );
        assert_eq!(
            HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0xA5,
            }
            .display(),
            "RightAlt"
        );
    }
}
