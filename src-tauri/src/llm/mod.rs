pub mod prompt;

use crate::error::AppError;
use crate::llm::prompt::build_correction_prompt;
use serde::{Deserialize, Serialize};

const DEFAULT_TIMEOUT_SECS: u64 = 10;

/// Trait for text correction via LLM, enabling test seams with sync wrappers.
pub trait TextCorrector: Send + Sync {
    fn correct_sync(&self, text: &str) -> Result<String, AppError>;
    fn matches_config(&self, api_url: &str, api_key: &str, model: &str) -> bool;
    fn test_connection_sync(&self) -> Result<(), AppError>;
}

/// Enum-based dispatch for text correctors.
pub enum AnyCorrector {
    Live(LLMClient),
    Mock(MockCorrector),
}

impl TextCorrector for AnyCorrector {
    fn correct_sync(&self, text: &str) -> Result<String, AppError> {
        match self {
            AnyCorrector::Live(c) => c.correct_sync(text),
            AnyCorrector::Mock(m) => m.correct_sync(text),
        }
    }

    fn matches_config(&self, api_url: &str, api_key: &str, model: &str) -> bool {
        match self {
            AnyCorrector::Live(c) => c.matches_config(api_url, api_key, model),
            AnyCorrector::Mock(m) => m.matches_config(api_url, api_key, model),
        }
    }

    fn test_connection_sync(&self) -> Result<(), AppError> {
        match self {
            AnyCorrector::Live(c) => c.test_connection_sync(),
            AnyCorrector::Mock(m) => m.test_connection_sync(),
        }
    }
}

/// LLM API response format (OpenAI-compatible).
#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
}

/// LLM client for post-transcription text correction.
#[derive(Clone)]
pub struct LLMClient {
    client: reqwest::Client,
    api_url: String,
    api_key: String,
    model: String,
}

impl LLMClient {
    pub fn new(api_url: String, api_key: String, model: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .unwrap_or_default();

        Self {
            client,
            api_url,
            api_key,
            model,
        }
    }

    /// Correct text using LLM (async).
    pub async fn correct(&self, text: &str) -> Result<String, AppError> {
        let system_prompt = build_correction_prompt();
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system_prompt.to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: text.to_string(),
                },
            ],
            temperature: 0.1,
        };

        let api_key = &self.api_key;
        let response = self
            .client
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| AppError::Llm(format!("request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::Llm(format!("API error {status}: {body}")));
        }

        let chat_response: ChatResponse = response
            .json()
            .await
            .map_err(|e| AppError::Llm(format!("parse response failed: {e}")))?;

        let corrected = chat_response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_else(|| text.to_string());

        Ok(corrected)
    }

    /// Synchronous wrapper for `correct()`.
    pub fn correct_sync(&self, text: &str) -> Result<String, AppError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.correct(text))
        })
    }

    pub fn matches_config(&self, api_url: &str, api_key: &str, model: &str) -> bool {
        self.api_url == api_url && self.api_key == api_key && self.model == model
    }

    /// Test the connection by sending a simple request (async).
    pub async fn test_connection(&self) -> Result<(), AppError> {
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "Hi".to_string(),
            }],
            temperature: 0.0,
        };

        let api_key = &self.api_key;
        let response = self
            .client
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| AppError::Llm(format!("connection test failed: {e}")))?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            Err(AppError::Llm(format!(
                "connection test failed: HTTP {status}"
            )))
        }
    }

    /// Synchronous wrapper for `test_connection()`.
    pub fn test_connection_sync(&self) -> Result<(), AppError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.test_connection())
        })
    }
}

impl TextCorrector for LLMClient {
    fn correct_sync(&self, text: &str) -> Result<String, AppError> {
        LLMClient::correct_sync(self, text)
    }

    fn matches_config(&self, api_url: &str, api_key: &str, model: &str) -> bool {
        LLMClient::matches_config(self, api_url, api_key, model)
    }

    fn test_connection_sync(&self) -> Result<(), AppError> {
        LLMClient::test_connection_sync(self)
    }
}

/// Mock corrector for testing.
pub struct MockCorrector {
    response: String,
    config: (String, String, String),
}

impl MockCorrector {
    pub fn new(response: &str) -> Self {
        Self {
            response: response.to_string(),
            config: (String::new(), String::new(), String::new()),
        }
    }

    pub fn with_config(mut self, api_url: &str, api_key: &str, model: &str) -> Self {
        self.config = (api_url.to_string(), api_key.to_string(), model.to_string());
        self
    }
}

impl TextCorrector for MockCorrector {
    fn correct_sync(&self, _text: &str) -> Result<String, AppError> {
        Ok(self.response.clone())
    }

    fn matches_config(&self, api_url: &str, api_key: &str, model: &str) -> bool {
        self.config.0 == api_url && self.config.1 == api_key && self.config.2 == model
    }

    fn test_connection_sync(&self) -> Result<(), AppError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_client_new() {
        let client = LLMClient::new(
            "https://api.example.com/v1/chat/completions".to_string(),
            "test-key".to_string(),
            "gpt-4".to_string(),
        );
        assert_eq!(client.model, "gpt-4");
    }

    #[test]
    fn test_mock_corrector() {
        let mock = MockCorrector::new("corrected text");
        assert_eq!(mock.correct_sync("raw text").unwrap(), "corrected text");
        assert!(mock.test_connection_sync().is_ok());
    }

    #[test]
    fn test_mock_corrector_config_matching() {
        let mock = MockCorrector::new("ok").with_config("url", "key", "model");
        assert!(mock.matches_config("url", "key", "model"));
        assert!(!mock.matches_config("other", "key", "model"));
    }

    #[test]
    fn test_any_corrector_mock() {
        let corrector = AnyCorrector::Mock(MockCorrector::new("mock result"));
        assert_eq!(corrector.correct_sync("input").unwrap(), "mock result");
    }
}
