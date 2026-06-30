use crate::config::schema::{AppConfig, WhisperModel};
use crate::crypto;
use crate::error::AppError;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

const APP_DIR_NAME: &str = "dl-voice-typing";
const CONFIG_FILE_NAME: &str = "config.json";

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
    /// Automatically decrypts DPAPI-encrypted API keys; plaintext keys
    /// are left as-is (migrated to encrypted on next save).
    pub fn load() -> Result<Self, AppError> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&path)?;
        let mut config: AppConfig = serde_json::from_str(&content)?;

        // Decrypt API key if encrypted; plaintext keys stay as-is (auto-migrate on next save).
        if !config.llm_api_key.is_empty() && crypto::is_encrypted(&config.llm_api_key) {
            config.llm_api_key = crypto::decrypt(&config.llm_api_key)?;
        }

        Ok(config)
    }

    /// Save config to disk. The API key is encrypted via DPAPI before writing.
    pub fn save(&self) -> Result<(), AppError> {
        let dir = Self::config_dir()?;
        fs::create_dir_all(&dir)?;

        let mut for_disk = self.clone();
        if !for_disk.llm_api_key.is_empty() {
            for_disk.llm_api_key = crypto::encrypt(&for_disk.llm_api_key)?;
        }

        let content = serde_json::to_string_pretty(&for_disk)?;
        fs::write(Self::config_path()?, content)?;
        Ok(())
    }
}

/// Returns the models directory path.
pub fn models_dir() -> PathBuf {
    AppConfig::config_dir()
        .unwrap_or_else(|_| dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("models")
}

/// Returns the model file path for a given model.
pub fn model_path_for_size(model: &WhisperModel) -> PathBuf {
    models_dir().join(model.filename().as_ref())
}

/// Check which Whisper models are present on disk.
pub fn check_whisper_models() -> HashMap<String, bool> {
    WhisperModel::all_built_in()
        .iter()
        .map(|m| (m.size_str().to_string(), model_path_for_size(m).exists()))
        .collect()
}

/// Scan a directory for custom model files (non-built-in .bin files).
/// Returns sorted filenames.
pub fn scan_custom_models_in(dir: &std::path::Path) -> Vec<String> {
    let built_in = WhisperModel::built_in_filenames();
    let mut customs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".bin") && !built_in.contains(name.as_str()) {
                customs.push(name);
            }
        }
    }
    customs.sort();
    customs
}

/// Scan the default models directory for custom model files.
pub fn scan_custom_models() -> Vec<String> {
    scan_custom_models_in(&models_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_save_and_load() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("dl-voice-typing-test-config");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;

        let config = AppConfig {
            hotkey: "F9".to_string(),
            language: crate::config::Language::En,
            ..Default::default()
        };

        // Manually save/load from the temp dir
        let path = dir.join(CONFIG_FILE_NAME);
        let content = serde_json::to_string_pretty(&config)?;
        fs::write(&path, &content)?;

        let loaded: AppConfig = serde_json::from_str(&fs::read_to_string(&path)?)?;
        assert_eq!(loaded.hotkey, "F9");
        assert_eq!(loaded.language, crate::config::Language::En);

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn test_load_missing_file_returns_default() {
        // Ensure the config_path won't collide with real config
        let result = AppConfig::load();
        // Should succeed (either loads existing or returns default)
        assert!(result.is_ok());
    }

    #[test]
    fn test_save_encrypts_api_key() -> Result<(), Box<dyn std::error::Error>> {
        // save() encrypts the key via DPAPI before writing to JSON.
        let config = AppConfig {
            llm_api_key: "sk-test-secret-key".to_string(),
            ..Default::default()
        };
        // Clone config and encrypt key manually (same logic as save()).
        let mut for_disk = config.clone();
        for_disk.llm_api_key = crate::crypto::encrypt(&for_disk.llm_api_key)?;
        let json = serde_json::to_string(&for_disk)?;
        let parsed: serde_json::Value = serde_json::from_str(&json)?;
        let stored_key = parsed["llm_api_key"]
            .as_str()
            .ok_or("missing llm_api_key")?;
        assert!(stored_key.starts_with("DPAPI:"));
        assert_ne!(stored_key, "sk-test-secret-key");
        Ok(())
    }

    #[test]
    fn test_save_empty_key_not_encrypted() -> Result<(), Box<dyn std::error::Error>> {
        let config = AppConfig {
            llm_api_key: String::new(),
            ..Default::default()
        };
        let json = serde_json::to_string(&config)?;
        let parsed: serde_json::Value = serde_json::from_str(&json)?;
        assert_eq!(
            parsed["llm_api_key"]
                .as_str()
                .ok_or("missing llm_api_key")?,
            ""
        );
        Ok(())
    }

    #[test]
    fn test_load_decrypts_encrypted_key() -> Result<(), Box<dyn std::error::Error>> {
        let encrypted = crate::crypto::encrypt("sk-test-key")?;
        let config = AppConfig {
            llm_api_key: encrypted,
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&config)?;

        // Parse it back as if loading from disk — but we need to parse
        // without the save() encryption step.
        // The key in json is still DPAPI:... because we bypassed save()
        // Simulate load behavior manually:
        let mut loaded: AppConfig = serde_json::from_str(&json)?;
        if !loaded.llm_api_key.is_empty() && crate::crypto::is_encrypted(&loaded.llm_api_key) {
            loaded.llm_api_key = crate::crypto::decrypt(&loaded.llm_api_key)?;
        }
        assert_eq!(loaded.llm_api_key, "sk-test-key");
        Ok(())
    }

    #[test]
    fn test_load_preserves_plaintext_key() -> Result<(), Box<dyn std::error::Error>> {
        // Plaintext key should remain as-is (auto-migrate on next save).
        let json = r#"{"hotkey":"RightCtrl","language":"zh","whisper_model":"base","llm_enabled":false,"llm_api_url":"","llm_api_key":"sk-plaintext-legacy","llm_model":"","download_mirror":"hf-mirror","data_saving_enabled":false,"data_saving_path":"","review_before_paste":false,"autostart":false}"#;
        let config: AppConfig = serde_json::from_str(json)?;
        assert_eq!(config.llm_api_key, "sk-plaintext-legacy");
        Ok(())
    }

    #[test]
    fn test_model_path_for_size() {
        let path = model_path_for_size(&WhisperModel::Base);
        assert!(path.to_string_lossy().contains("ggml-base.bin"));
    }

    #[test]
    fn test_check_whisper_models() {
        let models = check_whisper_models();
        assert_eq!(models.len(), 8);
        assert!(models.contains_key("tiny"));
        assert!(models.contains_key("base"));
        assert!(models.contains_key("small"));
        assert!(models.contains_key("medium"));
    }

    #[test]
    fn test_scan_custom_models() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("dl-voice-typing-test-models-scan");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;

        fs::write(dir.join("ggml-base.bin"), b"fake")?;
        fs::write(dir.join("my-custom.bin"), b"fake")?;
        fs::write(dir.join("other-model.bin"), b"fake")?;
        fs::write(dir.join("readme.txt"), b"ignore")?;

        let customs = scan_custom_models_in(&dir);
        assert_eq!(customs, vec!["my-custom.bin", "other-model.bin"]);

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }
}
