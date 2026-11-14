pub mod prompt;

pub(crate) mod correction;

use crate::error::AppError;
use crate::llm::prompt::build_correction_prompt;
use crate::redaction::redact_error_detail;
use serde::{Deserialize, Serialize};

const DEFAULT_TIMEOUT_SECS: u64 = 10;

/// Trait for text correction via LLM, enabling test seams with sync wrappers.
pub trait TextCorrector: Send + Sync {
    fn correct_sync(&self, text: &str) -> Result<String, AppError>;
    fn matches_config(&self, api_url: &str, api_key: &str, model: &str) -> bool;
    fn test_connection_sync(&self) -> Result<(), AppError>;
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
    /// Create a new LLM client targeting the given OpenAI-compatible endpoint.
    pub fn new(api_url: String, api_key: String, model: String) -> Self {
        Self {
            client: Self::build_client(),
            api_url,
            api_key,
            model,
        }
    }

    /// Build the reqwest client used by every LLM call.
    fn build_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .unwrap_or_default()
    }

    /// Test-only constructor that bypasses the system proxy.
    ///
    /// `reqwest` 0.13 enables the `system-proxy` feature by default, so on
    /// a machine with a system proxy configured the transport error for a
    /// refused connection is never observed — the proxy answers `502 Bad
    /// Gateway` instead, and the request URL (which carries the credential)
    /// never reaches the error string. Disabling the proxy makes the
    /// connect-failure path observable, which is exactly what the
    /// redaction guard for that path needs.
    #[cfg(test)]
    pub(crate) fn new_without_proxy(api_url: String, api_key: String, model: String) -> Self {
        let client = reqwest::Client::builder()
            .no_proxy()
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
            // Redact at the source (same shape as `test_connection`): every
            // error constructed here may embed the request URL — reqwest's
            // Display carries it verbatim — and the URL may itself carry a
            // credential (`?key=…`) or the userinfo form. Callers keep their
            // own redaction (belt-and-suspenders), but a future caller must
            // not have to remember it.
            .map_err(|e| {
                AppError::Llm(format!(
                    "request failed: {}",
                    redact_error_detail(&e.to_string(), &self.api_key)
                ))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            // The error body may echo the key back (some gateways echo the
            // request). Redact before it becomes an error string.
            return Err(AppError::Llm(format!(
                "API error {status}: {}",
                redact_error_detail(&body, &self.api_key)
            )));
        }

        let chat_response: ChatResponse = response.json().await.map_err(|e| {
            AppError::Llm(format!(
                "parse response failed: {}",
                redact_error_detail(&e.to_string(), &self.api_key)
            ))
        })?;

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

    /// Check whether this client's endpoint, key, and model match the given config values.
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
            // Redact at the source (not in log_cmd's generic sink) so this
            // covers both the UI toast and the frontend-error log forwarder.
            .map_err(|e| {
                AppError::Llm(format!(
                    "连接失败：请检查网络与 API 配置（详情: {}）",
                    redact_error_detail(&e.to_string(), &self.api_key)
                ))
            })?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            Err(AppError::Llm(format!("连接失败：服务端返回 HTTP {status}")))
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
    fail: bool,
}

impl MockCorrector {
    pub fn new(response: &str) -> Self {
        Self {
            response: response.to_string(),
            config: (String::new(), String::new(), String::new()),
            fail: false,
        }
    }

    /// Corrector whose `correct_sync` always fails — for exercising the
    /// LLM-fallback path (raw transcription survives, `llm-error` emitted).
    pub fn failing() -> Self {
        Self {
            response: String::new(),
            config: (String::new(), String::new(), String::new()),
            fail: true,
        }
    }

    pub fn with_config(mut self, api_url: &str, api_key: &str, model: &str) -> Self {
        self.config = (api_url.to_string(), api_key.to_string(), model.to_string());
        self
    }
}

impl TextCorrector for MockCorrector {
    fn correct_sync(&self, _text: &str) -> Result<String, AppError> {
        if self.fail {
            // Shape mirrors a real reqwest Display error (URL embedded, query
            // credential included) so redaction is exercised realistically.
            return Err(AppError::Llm(
                "request failed: error sending request for url \
                 (http://localhost:9/v1/chat/completions?key=SECRET123)"
                    .to_string(),
            ));
        }
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

    /// Start a one-shot loopback HTTP server that answers the single
    /// accepted connection with `response`. Returns the base URL
    /// (`http://127.0.0.1:<port>`).
    ///
    /// Loopback bind failure panics rather than skipping: a silently
    /// skipped guard is a hole, not a pass (the 5xx regression in
    /// `transcribe_cmd` uses the lenient variant because it is a
    /// whole-pipeline assertion).
    fn spawn_one_shot_server(status: &str, body: &str) -> String {
        use std::io::{Read, Write};
        let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(l) => l,
            Err(e) => panic!("loopback listener must bind for the redaction guards: {e}"),
        };
        let addr = match listener.local_addr() {
            Ok(a) => a,
            Err(e) => panic!("local_addr must be readable for the redaction guards: {e}"),
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 8192];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    /// The API key must never reach an error string produced by
    /// `correct()`. Every fixture below embeds the key in the URL (query
    /// form) — reqwest's `Display` carries the request URL verbatim, so a
    /// fixture WITHOUT the key in the URL would make the assertion
    /// vacuously true for the connect-failure path (the Authorization
    /// header never appears in the error string).
    const REDACT_KEY: &str = "SECRET123";

    #[tokio::test]
    async fn correct_connect_failure_redacts_api_key() {
        // Connect-refused path: the error embeds the request URL, which
        // carries the key in the query string. The proxy-bypassing client
        // is required — with the system proxy enabled the transport error
        // never surfaces (the proxy answers 502), which would make this
        // assertion vacuously true.
        let client = LLMClient::new_without_proxy(
            format!("http://127.0.0.1:1/v1/chat/completions?key={REDACT_KEY}"),
            REDACT_KEY.to_string(),
            "gpt-4".to_string(),
        );
        let err = client
            .correct("你好")
            .await
            .expect_err("connection to an unreachable endpoint must fail")
            .to_string();
        assert!(
            !err.contains(REDACT_KEY),
            "connect-failure error must not leak the API key: got {err}"
        );
        // The un-redacted form of this error DOES carry the URL — assert
        // that the guard is not vacuous: the request URL must be part of
        // the error before redaction, otherwise the assertion above would
        // hold for any implementation.
        assert!(
            err.contains("127.0.0.1:1"),
            "the transport error must still identify the endpoint: got {err}"
        );
    }

    #[tokio::test]
    async fn correct_http_error_body_redacts_api_key() {
        // HTTP non-2xx path: the gateway echoes the key back in the body.
        let server = spawn_one_shot_server("500 Internal Server Error", "SECRET123-body");
        let client = LLMClient::new_without_proxy(
            format!("{server}/v1/chat/completions?key={REDACT_KEY}"),
            REDACT_KEY.to_string(),
            "gpt-4".to_string(),
        );
        let err = client
            .correct("你好")
            .await
            .expect_err("a 500 response must fail the correction")
            .to_string();
        assert!(
            !err.contains(REDACT_KEY),
            "HTTP error body must not leak the API key: got {err}"
        );
    }

    #[tokio::test]
    async fn correct_parse_failure_redacts_api_key() {
        // Parse-failure path: 200 OK with a body that is not valid JSON.
        // reqwest's decode error Display does not embed the response body
        // today, so this assertion guards against a future change that
        // starts including it (and against a URL leak) — the marker
        // `PARSE-BODY-MARKER` makes the check non-vacuous for the body.
        let server = spawn_one_shot_server("200 OK", "PARSE-BODY-MARKER-not-json");
        let client = LLMClient::new_without_proxy(
            format!("{server}/v1/chat/completions?key={REDACT_KEY}"),
            REDACT_KEY.to_string(),
            "gpt-4".to_string(),
        );
        let err = client
            .correct("你好")
            .await
            .expect_err("a non-JSON body must fail the correction")
            .to_string();
        assert!(
            !err.contains(REDACT_KEY),
            "parse-failure error must not leak the API key: got {err}"
        );
        assert!(
            !err.contains("PARSE-BODY-MARKER"),
            "parse-failure error must not embed the raw response body: got {err}"
        );
    }

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

    #[tokio::test]
    async fn test_llm_connection_error_is_chinese_summary() {
        // M4-a: connect failure error must start with 「连接失败」 (Chinese summary)
        // and the redacted detail must not leak SECRET.
        let client = LLMClient::new(
            "http://127.0.0.1:1/v1/chat/completions".to_string(),
            "SECRET".to_string(),
            "gpt-4".to_string(),
        );
        let result = client.test_connection().await;
        assert!(
            result.is_err(),
            "connection to unreachable endpoint should fail"
        );
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("连接失败"),
            "error must contain Chinese summary: got {err}"
        );
        assert!(
            !err.contains("SECRET"),
            "redacted detail must not leak the API key: got {err}"
        );
    }

    #[test]
    fn test_mock_corrector_config_matching() {
        let mock = MockCorrector::new("ok").with_config("url", "key", "model");
        assert!(mock.matches_config("url", "key", "model"));
        assert!(!mock.matches_config("other", "key", "model"));
    }

    #[test]
    fn test_mock_corrector_failing() {
        let mock = MockCorrector::failing();
        assert!(mock.correct_sync("raw").is_err());
    }
}
