use crate::error::AppError;
use crate::hotkey::windows::HotkeySpec;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Available languages for speech recognition: (code, display name).
pub const LANGUAGES: &[(&str, &str)] = &[
    ("zh", "中文"),
    ("en", "English"),
    ("ja", "日本語"),
    ("ko", "한국어"),
];

/// Available Whisper models: (size, filename, display size).
pub const WHISPER_MODELS: &[(&str, &str, &str)] = &[
    ("tiny", "ggml-tiny.bin", "75MB"),
    ("tiny-q8_0", "ggml-tiny-q8_0.bin", "~40MB"),
    ("base", "ggml-base.bin", "142MB"),
    ("base-q8_0", "ggml-base-q8_0.bin", "~75MB"),
    ("small", "ggml-small.bin", "466MB"),
    ("small-q8_0", "ggml-small-q8_0.bin", "~250MB"),
    ("medium", "ggml-medium.bin", "1.5GB"),
    ("medium-q8_0", "ggml-medium-q8_0.bin", "~800MB"),
];

/// Download mirror options: (id, display name, base URL).
pub const DOWNLOAD_MIRRORS: &[(&str, &str, &str)] = &[
    (
        "hf-mirror",
        "HF-Mirror (国内加速)",
        "https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main",
    ),
    (
        "huggingface",
        "HuggingFace (国际)",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main",
    ),
];

/// Base URL for downloading Q8_0 quantized Whisper models.
pub const Q8_MODELS_BASE_URL: &str =
    "https://huggingface.co/denny-lg/whisper-quantized/resolve/main";

// ---------------------------------------------------------------------------
// Typed enums for config fields (serde serializes as lowercase strings)
// ---------------------------------------------------------------------------

/// Whisper model variant used for transcription, including built-in sizes and user-supplied custom models.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum WhisperModel {
    Tiny,
    TinyQ8,
    #[default]
    Base,
    BaseQ8,
    Small,
    SmallQ8,
    Medium,
    MediumQ8,
    Custom(String),
}

/// Metadata for each built-in Whisper model variant.
/// Centralizes the variant-to-data mapping in one place.
struct ModelMeta {
    variant: WhisperModel,
    serde_key: &'static str,
    filename: &'static str,
    display_size: &'static str,
}

const BUILT_IN_MODELS: &[ModelMeta] = &[
    ModelMeta {
        variant: WhisperModel::Tiny,
        serde_key: "tiny",
        filename: "ggml-tiny.bin",
        display_size: "75MB",
    },
    ModelMeta {
        variant: WhisperModel::TinyQ8,
        serde_key: "tiny-q8_0",
        filename: "ggml-tiny-q8_0.bin",
        display_size: "~40MB",
    },
    ModelMeta {
        variant: WhisperModel::Base,
        serde_key: "base",
        filename: "ggml-base.bin",
        display_size: "142MB",
    },
    ModelMeta {
        variant: WhisperModel::BaseQ8,
        serde_key: "base-q8_0",
        filename: "ggml-base-q8_0.bin",
        display_size: "~75MB",
    },
    ModelMeta {
        variant: WhisperModel::Small,
        serde_key: "small",
        filename: "ggml-small.bin",
        display_size: "466MB",
    },
    ModelMeta {
        variant: WhisperModel::SmallQ8,
        serde_key: "small-q8_0",
        filename: "ggml-small-q8_0.bin",
        display_size: "~250MB",
    },
    ModelMeta {
        variant: WhisperModel::Medium,
        serde_key: "medium",
        filename: "ggml-medium.bin",
        display_size: "1.5GB",
    },
    ModelMeta {
        variant: WhisperModel::MediumQ8,
        serde_key: "medium-q8_0",
        filename: "ggml-medium-q8_0.bin",
        display_size: "~800MB",
    },
];

impl Serialize for WhisperModel {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(meta) = BUILT_IN_MODELS.iter().find(|m| m.variant == *self) {
            serializer.serialize_str(meta.serde_key)
        } else if let Self::Custom(name) = self {
            serializer.serialize_str(&format!("custom:{name}"))
        } else {
            unreachable!()
        }
    }
}

impl<'de> Deserialize<'de> for WhisperModel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if let Some(meta) = BUILT_IN_MODELS.iter().find(|m| m.serde_key == s) {
            Ok(meta.variant.clone())
        } else if let Some(name) = s.strip_prefix("custom:") {
            if name.is_empty() {
                Err(serde::de::Error::custom(
                    "custom model name cannot be empty",
                ))
            } else {
                Ok(Self::Custom(name.to_string()))
            }
        } else {
            Err(serde::de::Error::custom(format!("unknown model: {s}")))
        }
    }
}

impl WhisperModel {
    /// Returns the model filename (e.g. `ggml-base.bin`), or the custom name for user-supplied models.
    pub fn filename(&self) -> std::borrow::Cow<'static, str> {
        if let Some(meta) = BUILT_IN_MODELS.iter().find(|m| m.variant == *self) {
            meta.filename.into()
        } else if let Self::Custom(name) = self {
            name.clone().into()
        } else {
            unreachable!()
        }
    }

    /// Returns the human-readable download size (e.g. `"142MB"`, `"~75MB"`), or `""` for custom models.
    pub fn display_size(&self) -> &'static str {
        BUILT_IN_MODELS
            .iter()
            .find(|m| m.variant == *self)
            .map(|m| m.display_size)
            .unwrap_or("")
    }

    /// All built-in variants in order (excludes Custom).
    pub fn all_built_in() -> &'static [WhisperModel] {
        &[
            Self::Tiny,
            Self::TinyQ8,
            Self::Base,
            Self::BaseQ8,
            Self::Small,
            Self::SmallQ8,
            Self::Medium,
            Self::MediumQ8,
        ]
    }

    /// The size identifier string used by the frontend and download API (e.g. "tiny", "base").
    pub fn size_str(&self) -> &'static str {
        BUILT_IN_MODELS
            .iter()
            .find(|m| m.variant == *self)
            .map(|m| m.serde_key)
            .unwrap_or("")
    }

    /// Returns true if this is a custom user-added model.
    pub fn is_custom(&self) -> bool {
        matches!(self, Self::Custom(_))
    }

    /// Returns true if this is a Q8_0 quantized model.
    pub fn is_q8(&self) -> bool {
        matches!(
            self,
            Self::TinyQ8 | Self::BaseQ8 | Self::SmallQ8 | Self::MediumQ8
        )
    }

    /// Returns the set of built-in model filenames (for scanner exclusion).
    pub fn built_in_filenames() -> std::collections::HashSet<&'static str> {
        BUILT_IN_MODELS.iter().map(|m| m.filename).collect()
    }
}

/// Supported recognition languages for Whisper transcription (Chinese, English, Japanese, Korean).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Zh,
    En,
    Ja,
    Ko,
}

impl Language {
    /// All variants in order.
    pub fn all() -> &'static [Language] {
        &[Self::Zh, Self::En, Self::Ja, Self::Ko]
    }

    /// Short language code (e.g. "zh", "en").
    pub fn code(self) -> &'static str {
        match self {
            Self::Zh => "zh",
            Self::En => "en",
            Self::Ja => "ja",
            Self::Ko => "ko",
        }
    }

    /// Human-readable display name.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Zh => "中文",
            Self::En => "English",
            Self::Ja => "日本語",
            Self::Ko => "한국어",
        }
    }
}

/// Download mirror source for Whisper model files (domestic HF-Mirror or international HuggingFace).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadMirror {
    #[default]
    #[serde(rename = "hf-mirror")]
    HfMirror,
    HuggingFace,
}

impl DownloadMirror {
    /// All variants in order.
    pub fn all() -> &'static [DownloadMirror] {
        &[Self::HfMirror, Self::HuggingFace]
    }

    /// Human-readable display name.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::HfMirror => "HF-Mirror (国内加速)",
            Self::HuggingFace => "HuggingFace (国际)",
        }
    }

    /// Base URL for downloading Whisper models.
    pub fn base_url(self) -> &'static str {
        match self {
            Self::HfMirror => "https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main",
            Self::HuggingFace => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main",
        }
    }
}

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
            .field("llm_api_url", &self.llm_api_url)
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
        if self.hotkey.vk == 0
            || crate::hotkey::from_key_name(&crate::hotkey::vk_to_key_name(self.hotkey.vk))
                .is_none()
        {
            let hotkey = &self.hotkey;
            return Err(AppError::Config(format!("invalid hotkey: {hotkey}")));
        }
        if self.record_only_hotkey.vk == 0
            || crate::hotkey::from_key_name(&crate::hotkey::vk_to_key_name(
                self.record_only_hotkey.vk,
            ))
            .is_none()
        {
            let key = &self.record_only_hotkey;
            return Err(AppError::Config(format!(
                "invalid record_only_hotkey: {key}"
            )));
        }
        if self.record_only_enabled && self.hotkey == self.record_only_hotkey {
            return Err(AppError::Config(
                "hotkey and record_only_hotkey must be different".to_string(),
            ));
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
    fn test_enum_serialization_format() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(serde_json::to_string(&Language::Zh)?, r#""zh""#);
        assert_eq!(serde_json::to_string(&Language::En)?, r#""en""#);
        assert_eq!(serde_json::to_string(&WhisperModel::Base)?, r#""base""#);
        assert_eq!(
            serde_json::to_string(&DownloadMirror::HfMirror)?,
            r#""hf-mirror""#
        );
        assert_eq!(
            serde_json::to_string(&DownloadMirror::HuggingFace)?,
            r#""huggingface""#
        );
        Ok(())
    }

    #[test]
    fn test_enum_deserialization_from_string() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            serde_json::from_str::<WhisperModel>(r#""tiny""#)?,
            WhisperModel::Tiny
        );
        assert_eq!(serde_json::from_str::<Language>(r#""ja""#)?, Language::Ja);
        assert_eq!(
            serde_json::from_str::<DownloadMirror>(r#""huggingface""#)?,
            DownloadMirror::HuggingFace
        );
        Ok(())
    }

    #[test]
    fn test_whisper_model_helpers() {
        assert_eq!(WhisperModel::Tiny.filename(), "ggml-tiny.bin");
        assert_eq!(WhisperModel::Base.filename(), "ggml-base.bin");
        assert_eq!(WhisperModel::Small.filename(), "ggml-small.bin");
        assert_eq!(WhisperModel::Medium.filename(), "ggml-medium.bin");

        assert_eq!(WhisperModel::Tiny.display_size(), "75MB");
        assert_eq!(WhisperModel::Base.display_size(), "142MB");
        assert_eq!(WhisperModel::Small.display_size(), "466MB");
        assert_eq!(WhisperModel::Medium.display_size(), "1.5GB");

        assert_eq!(WhisperModel::all_built_in().len(), 8);

        assert_eq!(WhisperModel::Tiny.size_str(), "tiny");
        assert_eq!(WhisperModel::Base.size_str(), "base");
        assert_eq!(WhisperModel::Small.size_str(), "small");
        assert_eq!(WhisperModel::Medium.size_str(), "medium");
    }

    #[test]
    fn test_language_helpers() {
        assert_eq!(Language::all().len(), 4);
        assert_eq!(Language::Zh.code(), "zh");
        assert_eq!(Language::En.code(), "en");
        assert_eq!(Language::Ja.code(), "ja");
        assert_eq!(Language::Ko.code(), "ko");
        assert_eq!(Language::Zh.display_name(), "中文");
        assert_eq!(Language::En.display_name(), "English");
        assert_eq!(Language::Ja.display_name(), "日本語");
        assert_eq!(Language::Ko.display_name(), "한국어");
    }

    #[test]
    fn test_download_mirror_helpers() {
        assert_eq!(DownloadMirror::all().len(), 2);
        assert_eq!(
            DownloadMirror::HfMirror.display_name(),
            "HF-Mirror (国内加速)"
        );
        assert_eq!(
            DownloadMirror::HuggingFace.display_name(),
            "HuggingFace (国际)"
        );
        assert!(
            DownloadMirror::HfMirror
                .base_url()
                .starts_with("https://hf-mirror.com")
        );
        assert!(
            DownloadMirror::HuggingFace
                .base_url()
                .starts_with("https://huggingface.co")
        );
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
    fn test_whisper_model_custom_serde_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let model = WhisperModel::Custom("my-model.bin".to_string());
        let json = serde_json::to_string(&model)?;
        assert_eq!(json, r#""custom:my-model.bin""#);
        let parsed: WhisperModel = serde_json::from_str(&json)?;
        assert_eq!(parsed, model);
        Ok(())
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
    fn test_whisper_model_custom_deserialize_invalid() {
        let result = serde_json::from_str::<WhisperModel>(r#""something-random""#);
        assert!(result.is_err());
    }

    #[test]
    fn test_whisper_model_custom_deserialize_empty_custom() {
        let result = serde_json::from_str::<WhisperModel>(r#""custom:""#);
        assert!(result.is_err());
    }

    #[test]
    fn test_whisper_model_is_custom() {
        assert!(!WhisperModel::Base.is_custom());
        assert!(WhisperModel::Custom("x.bin".to_string()).is_custom());
    }

    #[test]
    fn test_whisper_model_q8_serde_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        for model in [
            WhisperModel::TinyQ8,
            WhisperModel::BaseQ8,
            WhisperModel::SmallQ8,
            WhisperModel::MediumQ8,
        ] {
            let json = serde_json::to_string(&model)?;
            let parsed: WhisperModel = serde_json::from_str(&json)?;
            assert_eq!(parsed, model);
        }
        Ok(())
    }

    #[test]
    fn test_whisper_model_q8_helpers() {
        assert_eq!(WhisperModel::BaseQ8.filename(), "ggml-base-q8_0.bin");
        assert_eq!(WhisperModel::BaseQ8.display_size(), "~75MB");
        assert_eq!(WhisperModel::BaseQ8.size_str(), "base-q8_0");
        assert!(WhisperModel::BaseQ8.is_q8());
        assert!(!WhisperModel::Base.is_q8());
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
