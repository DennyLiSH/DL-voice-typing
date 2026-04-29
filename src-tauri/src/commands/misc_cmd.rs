use crate::config::AppConfig;
use crate::error::CommandError;
use crate::llm::LLMClient;
use crate::perf::{PerfHistory, PerfMetrics};
use crate::speech::AnyEngine;
use crate::speech::SpeechEngine;
use std::sync::{Arc, Mutex};

use super::MASKED_MARKER;

/// Test the LLM connection with the given settings.
/// If api_key is the masked marker, uses the saved key from config.
#[tauri::command]
pub async fn test_llm_connection(
    api_url: String,
    api_key: String,
    model: String,
    config_cache: tauri::State<'_, crate::config::ConfigCache>,
) -> Result<(), CommandError> {
    let api_key = if api_key == MASKED_MARKER {
        let config = AppConfig::read_cached(&config_cache).map_err(CommandError::from)?;
        config.llm_api_key
    } else {
        api_key
    };
    let client = LLMClient::new(api_url, api_key, model);
    client.test_connection_sync().map_err(CommandError::from)
}

/// Return recent performance metrics history.
#[tauri::command]
pub fn get_perf_history(
    perf: tauri::State<'_, Arc<PerfHistory>>,
    n: Option<usize>,
) -> Result<Vec<PerfMetrics>, CommandError> {
    Ok(perf.recent(n.unwrap_or(10)))
}

/// Return the current compute mode: "gpu", "cpu", or "unloaded".
#[tauri::command]
pub fn get_compute_mode(
    engine: tauri::State<'_, Arc<Mutex<AnyEngine>>>,
) -> Result<String, CommandError> {
    let e = crate::util::lock_mutex(&engine, "engine").ok_or_else(|| CommandError {
        code: "LOCK".to_string(),
        message: "engine lock poisoned".to_string(),
    })?;
    if e.is_ready() {
        Ok(if e.is_gpu_mode() {
            "gpu".to_string()
        } else {
            "cpu".to_string()
        })
    } else {
        Ok("unloaded".to_string())
    }
}
