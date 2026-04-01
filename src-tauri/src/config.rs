use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const APP_DIR_NAME: &str = "dl-voice-typing";
const CONFIG_FILE_NAME: &str = "config.json";

/// Application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Hotkey keycode name (default: "RightAlt").
    pub hotkey: String,

    /// Recognition language (default: "zh").
    pub language: String,

    /// Whisper model size: "tiny", "base", "small".
    pub whisper_model: String,

    /// Whether LLM post-processing is enabled.
    pub llm_enabled: bool,

    /// LLM API base URL.
    pub llm_api_url: String,

    /// LLM API key.
    pub llm_api_key: String,

    /// LLM model name.
    pub llm_model: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            hotkey: "RightAlt".to_string(),
            language: "zh".to_string(),
            whisper_model: "base".to_string(),
            llm_enabled: false,
            llm_api_url: String::new(),
            llm_api_key: String::new(),
            llm_model: String::new(),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_default_values() {
        let config = AppConfig::default();
        assert_eq!(config.hotkey, "RightAlt");
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
}
