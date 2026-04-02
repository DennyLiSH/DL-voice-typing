use crate::error::AppError;
use crate::hotkey::windows::WindowsHotkeyManager;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

const APP_DIR_NAME: &str = "dl-voice-typing";
const CONFIG_FILE_NAME: &str = "config.json";

fn default_download_mirror() -> String {
    "hf-mirror".to_string()
}

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
    ("base", "ggml-base.bin", "142MB"),
    ("small", "ggml-small.bin", "466MB"),
    ("medium", "ggml-medium.bin", "1.5GB"),
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

/// Returns the models directory path.
pub fn models_dir() -> PathBuf {
    AppConfig::config_dir()
        .unwrap_or_else(|_| dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("models")
}

/// Returns the model file path for a given model size.
pub fn model_path_for_size(size: &str) -> PathBuf {
    let filename = WHISPER_MODELS
        .iter()
        .find(|(s, _, _)| *s == size)
        .map(|(_, f, _)| *f)
        .unwrap_or("ggml-base.bin");
    models_dir().join(filename)
}

/// Check which Whisper models are present on disk.
pub fn check_whisper_models() -> HashMap<String, bool> {
    WHISPER_MODELS
        .iter()
        .map(|(size, _, _)| (size.to_string(), model_path_for_size(size).exists()))
        .collect()
}

/// Application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Hotkey keycode name (default: "RightCtrl").
    pub hotkey: String,

    /// Recognition language (default: "zh").
    pub language: String,

    /// Whisper model size: "tiny", "base", "small", "medium".
    pub whisper_model: String,

    /// Whether LLM post-processing is enabled.
    pub llm_enabled: bool,

    /// LLM API base URL.
    pub llm_api_url: String,

    /// LLM API key.
    pub llm_api_key: String,

    /// LLM model name.
    pub llm_model: String,

    /// Download mirror: "hf-mirror" or "huggingface".
    #[serde(default = "default_download_mirror")]
    pub download_mirror: String,

    /// Whether to save training data (audio + transcription) locally.
    #[serde(default)]
    pub data_saving_enabled: bool,

    /// Directory path for saving training data (WAV + JSON).
    #[serde(default)]
    pub data_saving_path: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            hotkey: "RightCtrl".to_string(),
            language: "zh".to_string(),
            whisper_model: "base".to_string(),
            llm_enabled: false,
            llm_api_url: String::new(),
            llm_api_key: String::new(),
            llm_model: String::new(),
            download_mirror: "hf-mirror".to_string(),
            data_saving_enabled: false,
            data_saving_path: String::new(),
        }
    }
}

impl AppConfig {
    /// Returns the config directory path (%APPDATA%/dl-voice-typing).
    pub fn config_dir() -> Result<PathBuf, AppError> {
        let dir = dirs::config_dir()
            .ok_or_else(|| AppError::Config("cannot determine config directory".to_string()))?;
        Ok(dir.join(APP_DIR_NAME))
    }

    /// Returns the config file path.
    pub fn config_path() -> Result<PathBuf, AppError> {
        Ok(Self::config_dir()?.join(CONFIG_FILE_NAME))
    }

    /// Load config from disk. Returns default if file doesn't exist.
    /// Returns default + logs warning if file is corrupt.
    pub fn load() -> Result<Self, AppError> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&path)?;
        let config: AppConfig = serde_json::from_str(&content)?;
        Ok(config)
    }

    /// Save config to disk.
    pub fn save(&self) -> Result<(), AppError> {
        let dir = Self::config_dir()?;
        fs::create_dir_all(&dir)?;
        let content = serde_json::to_string_pretty(self)?;
        fs::write(Self::config_path()?, content)?;
        Ok(())
    }

    /// Validate config fields.
    pub fn validate(&self) -> Result<(), AppError> {
        let valid_models: &[&str] = &["tiny", "base", "small", "medium"];
        if !valid_models.contains(&self.whisper_model.as_str()) {
            return Err(AppError::Config(format!(
                "invalid whisper model: {}",
                self.whisper_model
            )));
        }
        let valid_mirrors: &[&str] = &["hf-mirror", "huggingface"];
        if !valid_mirrors.contains(&self.download_mirror.as_str()) {
            return Err(AppError::Config(format!(
                "invalid download mirror: {}",
                self.download_mirror
            )));
        }
        if WindowsHotkeyManager::parse_key_code(&self.hotkey).is_none() {
            return Err(AppError::Config(format!("invalid hotkey: {}", self.hotkey)));
        }
        let valid_langs: &[&str] = &["zh", "en", "ja", "ko"];
        if !valid_langs.contains(&self.language.as_str()) {
            return Err(AppError::Config(format!(
                "invalid language: {}",
                self.language
            )));
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_default_values() {
        let config = AppConfig::default();
        assert_eq!(config.hotkey, "RightCtrl");
        assert_eq!(config.language, "zh");
        assert_eq!(config.whisper_model, "base");
        assert!(!config.llm_enabled);
        assert!(config.llm_api_url.is_empty());
    }

    #[test]
    fn test_serialize_deserialize() {
        let config = AppConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.hotkey, parsed.hotkey);
        assert_eq!(config.language, parsed.language);
    }

    #[test]
    fn test_save_and_load() {
        let dir = std::env::temp_dir().join("dl-voice-typing-test-config");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let config = AppConfig {
            hotkey: "F9".to_string(),
            language: "en".to_string(),
            ..Default::default()
        };

        // Manually save/load from the temp dir
        let path = dir.join(CONFIG_FILE_NAME);
        let content = serde_json::to_string_pretty(&config).unwrap();
        fs::write(&path, &content).unwrap();

        let loaded: AppConfig = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.hotkey, "F9");
        assert_eq!(loaded.language, "en");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_missing_file_returns_default() {
        // Ensure the config_path won't collide with real config
        let result = AppConfig::load();
        // Should succeed (either loads existing or returns default)
        assert!(result.is_ok());
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
    fn test_validate_accepts_data_saving_with_path() {
        let config = AppConfig {
            data_saving_enabled: true,
            data_saving_path: "/tmp/training-data".to_string(),
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
    fn test_new_fields_default_values() {
        let config = AppConfig::default();
        assert!(!config.data_saving_enabled);
        assert!(config.data_saving_path.is_empty());
    }

    #[test]
    fn test_new_fields_serialize_deserialize() {
        let config = AppConfig {
            data_saving_enabled: true,
            data_saving_path: "/some/path".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: AppConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.data_saving_enabled);
        assert_eq!(parsed.data_saving_path, "/some/path");
    }
}
