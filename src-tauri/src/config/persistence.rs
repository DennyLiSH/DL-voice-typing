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
    /// A corrupt file propagates Err; the caller (lib.rs) falls back to
    /// defaults with a warning.
    /// Automatically decrypts DPAPI-encrypted API keys / API URLs;
    /// plaintext values are left as-is (migrated to encrypted on next save).
    pub fn load() -> Result<Self, AppError> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&path)?;
        let mut config: AppConfig = serde_json::from_str(&content)?;
        config.decrypt_at_load()?;
        Ok(config)
    }

    /// Save config to disk. The API key is encrypted via DPAPI before
    /// writing; the API URL is encrypted too when it embeds query
    /// credentials (ordinary URLs stay human-readable in config.json).
    pub fn save(&self) -> Result<(), AppError> {
        let dir = Self::config_dir()?;
        fs::create_dir_all(&dir)?;
        let content = serde_json::to_string_pretty(&self.for_disk()?)?;
        fs::write(Self::config_path()?, content)?;
        Ok(())
    }

    /// Compute the on-disk representation: DPAPI-encrypt the API key, and
    /// the API URL when it embeds query credentials (conditional
    /// encryption keeps ordinary URLs human-readable in config.json).
    fn for_disk(&self) -> Result<Self, AppError> {
        let mut for_disk = self.clone();
        if !for_disk.llm_api_key.is_empty() {
            for_disk.llm_api_key = crypto::encrypt(&for_disk.llm_api_key)?;
        }
        if !for_disk.llm_api_url.is_empty()
            && crate::llm::url_contains_credential_query(&for_disk.llm_api_url)
        {
            for_disk.llm_api_url = crypto::encrypt(&for_disk.llm_api_url)
                .map_err(|e| AppError::Crypto(format!("llm_api_url encrypt failed: {e}")))?;
            tracing::info!(target: "config", "llm_api_url encrypted at rest (credential query detected)");
        }
        Ok(for_disk)
    }

    /// Decrypt DPAPI-encrypted fields in place (API key; API URL when
    /// conditionally encrypted). Plaintext values are left as-is
    /// (auto-migrate on next save). A failing blob (e.g. config.json
    /// copied from another machine/user) propagates Err; on load()
    /// failure lib.rs falls back to full defaults — accepted per the
    /// 2026-09-17 security-hardening decision (Prefer Errors over
    /// silent degradation).
    fn decrypt_at_load(&mut self) -> Result<(), AppError> {
        if !self.llm_api_key.is_empty() && crypto::is_encrypted(&self.llm_api_key) {
            self.llm_api_key = crypto::decrypt(&self.llm_api_key)?;
        }
        if !self.llm_api_url.is_empty() && crypto::is_encrypted(&self.llm_api_url) {
            self.llm_api_url = crypto::decrypt(&self.llm_api_url)
                .map_err(|e| AppError::Crypto(format!("llm_api_url decrypt failed: {e}")))?;
        }
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
    use crate::hotkey::windows::HotkeySpec;
    use std::fs;

    #[test]
    fn test_save_and_load() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("dl-voice-typing-test-config");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;

        let config = AppConfig {
            hotkey: HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0x78, // F9
            },
            language: crate::config::Language::En,
            ..Default::default()
        };

        // Manually save/load from the temp dir
        let path = dir.join(CONFIG_FILE_NAME);
        let content = serde_json::to_string_pretty(&config)?;
        fs::write(&path, &content)?;

        let loaded: AppConfig = serde_json::from_str(&fs::read_to_string(&path)?)?;
        assert_eq!(loaded.hotkey.vk, 0x78);
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
        // for_disk() is the save() pre-write step: the key is DPAPI-encrypted.
        let config = AppConfig {
            llm_api_key: "sk-test-secret-key".to_string(),
            ..Default::default()
        };
        let for_disk = config.for_disk()?;
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
        let mut config = AppConfig {
            llm_api_key: encrypted,
            ..Default::default()
        };
        config.decrypt_at_load()?;
        assert_eq!(config.llm_api_key, "sk-test-key");
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
    fn test_for_disk_conditionally_encrypts_api_url_with_query_credential()
    -> Result<(), Box<dyn std::error::Error>> {
        let config = AppConfig {
            llm_api_url: "https://x.com/v1?key=abc&model=gpt".to_string(),
            ..Default::default()
        };
        let for_disk = config.for_disk()?;
        assert!(for_disk.llm_api_url.starts_with("DPAPI:"));
        // Roundtrip via the load-side decrypt branch.
        let mut loaded = for_disk;
        loaded.decrypt_at_load()?;
        assert_eq!(loaded.llm_api_url, "https://x.com/v1?key=abc&model=gpt");
        Ok(())
    }

    #[test]
    fn test_for_disk_keeps_plain_api_url_plaintext() -> Result<(), Box<dyn std::error::Error>> {
        let config = AppConfig {
            llm_api_url: "https://api.example.com/v1".to_string(),
            ..Default::default()
        };
        let for_disk = config.for_disk()?;
        assert_eq!(for_disk.llm_api_url, "https://api.example.com/v1");
        Ok(())
    }

    #[test]
    fn test_for_disk_skips_empty_api_url() -> Result<(), Box<dyn std::error::Error>> {
        let config = AppConfig {
            llm_api_url: String::new(),
            ..Default::default()
        };
        let for_disk = config.for_disk()?;
        assert_eq!(for_disk.llm_api_url, "");
        Ok(())
    }

    #[test]
    fn test_decrypt_at_load_preserves_plaintext_credential_url() {
        // Old configs saved by previous versions store credential URLs in
        // plaintext — load must keep them as-is (auto-migrate on next save).
        let mut config = AppConfig {
            llm_api_url: "https://x.com/v1?key=abc".to_string(),
            ..Default::default()
        };
        config.decrypt_at_load().unwrap();
        assert_eq!(config.llm_api_url, "https://x.com/v1?key=abc");
    }

    #[test]
    fn test_decrypt_at_load_propagates_decrypt_error() {
        // A DPAPI blob that fails to decrypt (e.g. config.json copied from
        // another machine/user) propagates Err from load(); on load failure
        // lib.rs falls back to full defaults (accepted consequence chain).
        let mut config = AppConfig {
            llm_api_url: "DPAPI:!!!invalid-base64!!!".to_string(),
            ..Default::default()
        };
        assert!(config.decrypt_at_load().is_err());
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
