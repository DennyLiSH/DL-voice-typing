//! Model / mirror / language catalog: [`WhisperModel`] with the
//! `BUILT_IN_MODELS` metadata table, [`Language`], [`DownloadMirror`], and
//! the Q8_0 models base URL. Split out of `schema.rs` (Round 6 P9 Task 2)
//! so the schema file keeps only the AppConfig envelope; every consumer
//! keeps resolving through the `crate::config::*` re-exports in `mod.rs`.

use serde::{Deserialize, Serialize};

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
}

const BUILT_IN_MODELS: &[ModelMeta] = &[
    ModelMeta {
        variant: WhisperModel::Tiny,
        serde_key: "tiny",
        filename: "ggml-tiny.bin",
    },
    ModelMeta {
        variant: WhisperModel::TinyQ8,
        serde_key: "tiny-q8_0",
        filename: "ggml-tiny-q8_0.bin",
    },
    ModelMeta {
        variant: WhisperModel::Base,
        serde_key: "base",
        filename: "ggml-base.bin",
    },
    ModelMeta {
        variant: WhisperModel::BaseQ8,
        serde_key: "base-q8_0",
        filename: "ggml-base-q8_0.bin",
    },
    ModelMeta {
        variant: WhisperModel::Small,
        serde_key: "small",
        filename: "ggml-small.bin",
    },
    ModelMeta {
        variant: WhisperModel::SmallQ8,
        serde_key: "small-q8_0",
        filename: "ggml-small-q8_0.bin",
    },
    ModelMeta {
        variant: WhisperModel::Medium,
        serde_key: "medium",
        filename: "ggml-medium.bin",
    },
    ModelMeta {
        variant: WhisperModel::MediumQ8,
        serde_key: "medium-q8_0",
        filename: "ggml-medium-q8_0.bin",
    },
];

impl Serialize for WhisperModel {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_id())
    }
}

impl<'de> Deserialize<'de> for WhisperModel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse_id(&s).map_err(serde::de::Error::custom)
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

    /// Parse a model id string ("tiny", "base-q8_0", "custom:<filename>")
    /// into a [`WhisperModel`]. Single authority for id resolution — the
    /// serde `Deserialize` string arm delegates here. Custom names must be
    /// plain filenames: the id can arrive from the frontend and ends up in
    /// `models_dir().join(name)`, so path separators, drive letters and the
    /// `..` parent component (whole-name match only — mid-name `..` in
    /// `a..b.bin` is a legal NTFS filename with no traversal power) are
    /// rejected.
    pub fn parse_id(s: &str) -> Result<Self, crate::error::AppError> {
        if let Some(name) = s.strip_prefix("custom:") {
            if name.is_empty() {
                return Err(crate::error::AppError::Config(
                    "custom model name cannot be empty".to_string(),
                ));
            }
            if name.contains(['\\', '/', ':']) || name == ".." {
                return Err(crate::error::AppError::Config(format!(
                    "custom model name must be a plain filename: {name:?}"
                )));
            }
            return Ok(Self::Custom(name.to_string()));
        }
        BUILT_IN_MODELS
            .iter()
            .find(|m| m.serde_key == s)
            .map(|m| m.variant.clone())
            .ok_or_else(|| crate::error::AppError::Config(format!("unknown model: {s}")))
    }

    /// The id string for this model — inverse of [`parse_id`]. The serde
    /// `Serialize` impl delegates here.
    pub fn to_id(&self) -> String {
        if let Some(meta) = BUILT_IN_MODELS.iter().find(|m| m.variant == *self) {
            meta.serde_key.to_string()
        } else if let Self::Custom(name) = self {
            format!("custom:{name}")
        } else {
            unreachable!()
        }
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
    /// Short language code (e.g. "zh", "en").
    pub fn code(self) -> &'static str {
        match self {
            Self::Zh => "zh",
            Self::En => "en",
            Self::Ja => "ja",
            Self::Ko => "ko",
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
    /// Base URL for downloading Whisper models.
    pub fn base_url(self) -> &'static str {
        match self {
            Self::HfMirror => "https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main",
            Self::HuggingFace => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        // Ratchet (2026-11-16 audit D2): the hand-list must mirror the
        // model table row-for-row, order included — a new model row
        // without its variant here silently drops from the frontend
        // catalog (and vice versa).
        let table: Vec<WhisperModel> = BUILT_IN_MODELS.iter().map(|m| m.variant.clone()).collect();
        assert_eq!(WhisperModel::all_built_in(), table.as_slice());

        assert_eq!(WhisperModel::Tiny.size_str(), "tiny");
        assert_eq!(WhisperModel::Base.size_str(), "base");
        assert_eq!(WhisperModel::Small.size_str(), "small");
        assert_eq!(WhisperModel::Medium.size_str(), "medium");
    }

    #[test]
    fn test_language_helpers() {
        assert_eq!(Language::Zh.code(), "zh");
        assert_eq!(Language::En.code(), "en");
        assert_eq!(Language::Ja.code(), "ja");
        assert_eq!(Language::Ko.code(), "ko");
    }

    #[test]
    fn test_download_mirror_helpers() {
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
    fn test_whisper_model_custom_serde_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let model = WhisperModel::Custom("my-model.bin".to_string());
        let json = serde_json::to_string(&model)?;
        assert_eq!(json, r#""custom:my-model.bin""#);
        let parsed: WhisperModel = serde_json::from_str(&json)?;
        assert_eq!(parsed, model);
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
        assert_eq!(WhisperModel::BaseQ8.size_str(), "base-q8_0");
        assert!(WhisperModel::BaseQ8.is_q8());
        assert!(!WhisperModel::Base.is_q8());
    }

    #[test]
    fn test_parse_id_built_in_and_custom() {
        assert_eq!(WhisperModel::parse_id("tiny").unwrap(), WhisperModel::Tiny);
        assert_eq!(
            WhisperModel::parse_id("base-q8_0").unwrap(),
            WhisperModel::BaseQ8
        );
        assert_eq!(
            WhisperModel::parse_id("custom:my-model.bin").unwrap(),
            WhisperModel::Custom("my-model.bin".to_string())
        );
        // Mid-name ".." is a legal NTFS filename with no traversal power.
        assert_eq!(
            WhisperModel::parse_id("custom:a..b.bin").unwrap(),
            WhisperModel::Custom("a..b.bin".to_string())
        );
    }

    #[test]
    fn test_parse_id_rejects_unknown_and_path_like_custom() {
        assert!(WhisperModel::parse_id("giant").is_err());
        assert!(WhisperModel::parse_id("custom:").is_err());
        assert!(WhisperModel::parse_id("custom:a/b.bin").is_err());
        assert!(WhisperModel::parse_id("custom:a\\b.bin").is_err());
        assert!(WhisperModel::parse_id("custom:..\\x.bin").is_err());
        assert!(WhisperModel::parse_id("custom:C:\\x.bin").is_err());
        // Whole-name ".." is the parent-directory traversal vector.
        assert!(WhisperModel::parse_id("custom:..").is_err());
    }

    #[test]
    fn test_to_id_roundtrip() {
        for m in WhisperModel::all_built_in() {
            assert_eq!(WhisperModel::parse_id(&m.to_id()).unwrap(), m.clone());
        }
        let c = WhisperModel::Custom("m.bin".to_string());
        assert_eq!(WhisperModel::parse_id(&c.to_id()).unwrap(), c);
    }
}
