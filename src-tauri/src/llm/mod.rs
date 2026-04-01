pub mod prompt;

use crate::error::AppError;
use crate::llm::prompt::build_correction_prompt;
use serde::{Deserialize, Serialize};

const DEFAULT_TIMEOUT_SECS: u64 = 10;

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

    /// Correct text using LLM. Returns the corrected text, or the original on failure.
    pub async fn correct(&self, text: &str) -> Result<String, AppError> {
        let system_prompt = build_correction_prompt();
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system_prompt,
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: text.to_string(),
                },
            ],
            temperature: 0.1,
        };

        let response = self
            .client
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| AppError::Llm(format!("request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::Llm(format!("API error {}: {}", status, body)));
        }

        let chat_response: ChatResponse = response
            .json()
            .await
            .map_err(|e| AppError::Llm(format!("parse response failed: {}", e)))?;

        let corrected = chat_response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_else(|| text.to_string());

        Ok(corrected)
    }

    /// Test the connection by sending a simple request.
    pub async fn test_connection(&self) -> Result<(), AppError> {
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "Hi".to_string(),
            }],
            temperature: 0.0,
        };

        let response = self
            .client
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| AppError::Llm(format!("connection test failed: {}", e)))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(AppError::Llm(format!(
                "connection test failed: HTTP {}",
                response.status()
            )))
        }
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
}
