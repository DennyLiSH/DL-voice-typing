use crate::config::ApiKeyMask;
use crate::error::CommandError;
use crate::llm::LLMClient;

/// Test the LLM connection with the given settings.
/// If api_key is the masked marker, uses the saved key from config.
#[tauri::command]
pub async fn test_llm_connection(
    api_url: String,
    api_key: String,
    model: String,
    config_cache: tauri::State<'_, crate::config::ConfigCache>,
) -> Result<(), CommandError> {
    let api_key = ApiKeyMask::unmask_or_keep(&api_key, &config_cache.read_cached().llm_api_key);
    let client = LLMClient::new(api_url, api_key, model);
    client.test_connection_sync().map_err(CommandError::from)
}
