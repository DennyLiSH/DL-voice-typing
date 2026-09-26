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

/// Credential-like words for query-param redaction. When any word appears
/// as a token of a `?name=value` / `&name=value` / `#name=value`
/// parameter name (name is
/// split on non-alphanumerics, ASCII-case-insensitive), the value is
/// replaced with `***` before the string reaches the plaintext log file —
/// reqwest's Display embeds the full request URL, so a key placed in the
/// `api_url` query (some OpenAI-compatible endpoints accept `?key=…`)
/// would otherwise leak.
///
/// Token-split aligned with the frontend warn list
/// `ui/lib/settings-utils.js::CREDENTIAL_WORDS` (sync guarded by
/// `__tests__/credential-words-contract.test.js`): compound names
/// (`client_secret`) and hyphenated forms (`api-key`) match via their
/// tokens (`api_key` → [`api`, `key`]). `code` is deliberately NOT in the
/// list: token matching would also hit `error_code`/`status_code`
/// diagnostic params; the short-lived OAuth authorization-code residual
/// is accepted (spec 2026-09-17). Parameters outside the list (e.g.
/// `sig`/`hmac`) are a known boundary — layer 2 below only catches values
/// equal to the configured key.
///
/// Layer 2 of `redact_error_detail` (api_key value replace) catches
/// unlisted parameter names whose value equals the configured key.
/// Over-redaction is the safe direction; do NOT remove a marker on a
/// false-positive report, tighten the match instead.
const CREDENTIAL_QUERY_WORDS: [&str; 11] = [
    "key",
    "apikey",
    "token",
    "secret",
    "password",
    "signature",
    "auth",
    "authorization",
    "bearer",
    "credential",
    "sk",
];

/// Redact an LLM error string before it reaches the tracing log:
/// 1. replace `?name=value` / `&name=value` / `#name=value` query/fragment
///    credentials (token of `name` matches [`CREDENTIAL_QUERY_WORDS`],
///    ASCII-case-insensitive) with `***`;
/// 2. replace any occurrence of `api_key` with `[REDACTED]` (skipped when
///    empty — `str::replace("", x)` would insert between every char);
/// 3. truncate to 500 chars (multi-byte safe, applied last so truncation
///    never cuts through un-redacted text).
pub(crate) fn redact_error_detail(s: &str, api_key: &str) -> String {
    let redacted = redact_query_credentials(s);
    let redacted = if api_key.is_empty() {
        redacted
    } else {
        redacted.replace(api_key, "[REDACTED]")
    };
    redacted.chars().take(500).collect()
}

/// Replace query credential values with `***`. The value runs to the next
/// `&`, ASCII whitespace, or end of string — `)` is deliberately NOT a
/// terminator: reqwest wraps URLs as `url (…)`, so swallowing a trailing `)`
/// only hurts log readability (the safe direction). Cutting at `)` instead
/// would leak values that legitimately contain `)`. The parameter name
/// runs to the next `=`/`&`/`?`/`#`/ASCII whitespace — an exclusion set,
/// so names containing other punctuation (e.g. `api.key`) still reach the
/// tokenizer, keeping this layer aligned with the detector and the
/// frontend warn check.
pub(crate) fn redact_query_credentials(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'?' || b == b'&' || b == b'#' {
            let mut j = i + 1;
            while j < bytes.len()
                && !(bytes[j] == b'='
                    || bytes[j] == b'&'
                    || bytes[j] == b'?'
                    || bytes[j] == b'#'
                    || bytes[j].is_ascii_whitespace())
            {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'=' {
                let name = &s[i + 1..j];
                if name_matches_credential_word(name) {
                    out.push(b as char);
                    out.push_str(name);
                    out.push('=');
                    out.push_str("***");
                    let mut k = j + 1;
                    while k < bytes.len() {
                        let c = bytes[k];
                        if c == b'&' || c.is_ascii_whitespace() {
                            break;
                        }
                        k += 1;
                        while k < bytes.len() && !s.is_char_boundary(k) {
                            k += 1;
                        }
                    }
                    i = k;
                    continue;
                }
            }
        }
        let size = s[i..].chars().next().map_or(1, |c| c.len_utf8());
        out.push_str(&s[i..i + size]);
        i += size;
    }
    out
}

/// True when any non-alphanumeric-separated token of `name` matches a
/// [`CREDENTIAL_QUERY_WORDS`] entry (ASCII-case-insensitive):
/// `client_secret` → [`client`, `secret`] hits; `keyboard`/`monkey`/
/// `author` must not.
fn name_matches_credential_word(name: &str) -> bool {
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| {
            CREDENTIAL_QUERY_WORDS
                .iter()
                .any(|k| word.eq_ignore_ascii_case(k))
        })
}

/// True when the LLM API URL embeds credential-like query/fragment params
/// (e.g. Gemini-style `?key=…`). Rust counterpart of the frontend warn
/// check `ui/lib/settings-utils.js::hasCredentialInUrl` (keep both in
/// sync): segments after `?`/`#`/`&`, each param name (before
/// `=`) is token-matched against [`CREDENTIAL_QUERY_WORDS`]. Used to
/// conditionally DPAPI-encrypt `llm_api_url` at save time.
pub(crate) fn url_contains_credential_query(url: &str) -> bool {
    url.split(['?', '#', '&'])
        .skip(1)
        .any(|pair| name_matches_credential_word(pair.split('=').next().unwrap_or_default()))
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

    /// The const-driven tests below cannot detect a marker being silently
    /// removed (function and tests narrow together and stay green) — pin the
    /// list itself so deleting a marker becomes a visible test change.
    #[test]
    fn test_credential_query_words_content_pinned() {
        assert_eq!(
            CREDENTIAL_QUERY_WORDS,
            [
                "key",
                "apikey",
                "token",
                "secret",
                "password",
                "signature",
                "auth",
                "authorization",
                "bearer",
                "credential",
                "sk"
            ]
        );
    }

    #[test]
    fn test_redact_all_markers_both_prefixes() {
        for name in CREDENTIAL_QUERY_WORDS {
            for prefix in ['?', '&'] {
                let input = format!("http://x/v1{prefix}{name}=SECRET&next=1");
                let out = redact_error_detail(&input, "");
                assert!(
                    out.contains(&format!("{prefix}{name}=***")),
                    "input: {input}"
                );
                assert!(!out.contains("SECRET"), "input: {input}");
                assert!(out.contains("&next=1"), "input: {input}");
            }
        }
    }

    #[test]
    fn test_redact_case_insensitive_variants() {
        assert_eq!(
            redact_error_detail("http://x?key=SECRET", ""),
            "http://x?key=***"
        );
        assert_eq!(
            redact_error_detail("http://x&API_KEY=SECRET", ""),
            "http://x&API_KEY=***"
        );
    }

    #[test]
    fn test_redact_value_runs_to_amp_boundary() {
        assert_eq!(
            redact_error_detail("http://x?key=A1B2&other=2", ""),
            "http://x?key=***&other=2"
        );
    }

    #[test]
    fn test_redact_token_split_matches_compound_and_hyphenated_names() {
        assert_eq!(
            redact_error_detail("http://x?client_secret=SECRET&next=1", ""),
            "http://x?client_secret=***&next=1"
        );
        assert_eq!(
            redact_error_detail("http://x?api-key=SECRET", ""),
            "http://x?api-key=***"
        );
        assert_eq!(
            redact_error_detail("http://x&X-API-Key=SECRET", ""),
            "http://x&X-API-Key=***"
        );
        // Name characters outside [_-] (e.g. `.`): the detector and the
        // frontend tokenize on ANY non-alphanumeric, so the scanner's name
        // collection must not stop early — layer alignment.
        assert_eq!(
            redact_error_detail("http://x?api.key=SECRET&next=1", ""),
            "http://x?api.key=***&next=1"
        );
    }

    #[test]
    fn test_redact_token_split_does_not_match_non_credential_names() {
        // `code` was removed from the list: diagnostic params stay readable.
        assert_eq!(
            redact_error_detail("http://x?error_code=E1&status_code=200", ""),
            "http://x?error_code=E1&status_code=200"
        );
        assert_eq!(
            redact_error_detail("http://x?keyboard=dell&monkey=none&author=me", ""),
            "http://x?keyboard=dell&monkey=none&author=me"
        );
    }

    #[test]
    fn test_redact_fragment_credential_params() {
        // `#` is part of the credential surface: the detector and the
        // frontend treat fragment params as credential-bearing, so the
        // redaction scanner must cover the same prefix.
        assert_eq!(
            redact_error_detail("http://x#access-token=SECRET&next=1", ""),
            "http://x#access-token=***&next=1"
        );
        assert_eq!(
            redact_error_detail("http://x#key=SECRET", ""),
            "http://x#key=***"
        );
    }

    #[test]
    fn test_redact_multibyte_value_runs_to_amp_boundary() {
        // The value-scan loop skips to char boundaries — guard it with a
        // multi-byte value (a non-boundary slice would panic).
        assert_eq!(
            redact_error_detail("http://x?key=密钥值&next=1", ""),
            "http://x?key=***&next=1"
        );
    }

    #[test]
    fn test_url_contains_credential_query_detected() {
        assert!(url_contains_credential_query("https://x.com/v1?key=abc"));
        assert!(url_contains_credential_query(
            "https://x.com/v1/chat?client_secret=abc&x=1"
        ));
        assert!(url_contains_credential_query(
            "https://x.com/v1#access-token=abc"
        ));
        assert!(url_contains_credential_query(
            "https://x.com/v1?API-KEY=abc"
        ));
        // Pathological URL with a leading `&`-separated param (no `?`):
        // the frontend treats `&` as a top-level separator — so must the
        // detector (splitting on `&` too keeps the sync promise true).
        assert!(url_contains_credential_query("http://x.com/v1&key=SECRET"));
    }

    #[test]
    fn test_url_contains_credential_query_clean_urls() {
        assert!(!url_contains_credential_query("https://api.example.com/v1"));
        assert!(!url_contains_credential_query(
            "https://x.com/v1?error_code=E1&status_code=200"
        ));
        assert!(!url_contains_credential_query(
            "https://x.com/v1?keyboard=dell&monkey=none&author=me"
        ));
        // `&` in a path segment (no `=`-bearing credential param after it).
        assert!(!url_contains_credential_query("https://x.com/a&b/c"));
        assert!(!url_contains_credential_query(""));
    }

    /// `)` is deliberately not a value terminator: reqwest wraps URLs as
    /// `url (…)`, so the trailing `)` is swallowed along with the value —
    /// readability loss only, the safe direction.
    #[test]
    fn test_redact_swallows_trailing_paren_conservatively() {
        assert_eq!(
            redact_error_detail("error for url (http://x?key=AB)", ""),
            "error for url (http://x?key=***"
        );
    }

    #[test]
    fn test_redact_replaces_api_key_value() {
        let out = redact_error_detail("LLM error: something sk-12345 went wrong", "sk-12345");
        assert_eq!(out, "LLM error: something [REDACTED] went wrong");
    }

    /// Guards the empty-string branch: `str::replace("", x)` would insert
    /// `x` between every character.
    #[test]
    fn test_redact_empty_api_key_returns_input_unchanged_apart_from_query() {
        assert_eq!(redact_error_detail("plain error", ""), "plain error");
        assert_eq!(redact_error_detail("值 abcdef 值", ""), "值 abcdef 值");
    }

    #[test]
    fn test_redact_truncates_over_500_chars() {
        let long = "x".repeat(600);
        assert_eq!(redact_error_detail(&long, "").chars().count(), 500);
    }

    #[test]
    fn test_redact_exact_500_not_truncated() {
        let exact = "x".repeat(500);
        assert_eq!(redact_error_detail(&exact, "").chars().count(), 500);
    }

    #[test]
    fn test_redact_multibyte_no_panic() {
        let out = redact_error_detail("错误信息：连接失败 请重试", "");
        assert_eq!(out, "错误信息：连接失败 请重试");
    }
}
