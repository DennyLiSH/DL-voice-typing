use super::MASKED_MARKER;
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
    let api_key = if api_key == MASKED_MARKER {
        let config = config_cache.read_cached();
        config.llm_api_key.clone()
    } else {
        api_key
    };
    let client = LLMClient::new(api_url, api_key, model);
    client.test_connection_sync().map_err(CommandError::from)
}
