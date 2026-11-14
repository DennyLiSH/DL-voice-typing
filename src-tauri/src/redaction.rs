//! Credential-redaction domain — the single owner of log-side secret
//! scrubbing and the credential-URL detectors.
//!
//! Extracted from `llm/mod.rs` (Round 6 P9 Task 3, 2026-11-14): the
//! detectors are consumed by `config/persistence.rs` (conditional DPAPI
//! encryption at save time) and `config/schema.rs` (Debug redaction),
//! which previously had to reach through the `llm` module. `llm` remains
//! a consumer — `LLMClient` redacts at the source via
//! `crate::redaction::redact_error_detail` — but the reverse
//! config→llm dependency is gone.
//!
//! This file is scraped by the frontend contract tests
//! (`credential-words-contract.test.js` pins the word list below,
//! `userinfo-detection-contract.test.js` pins the fixture URLs in
//! `test_url_contains_userinfo_fixtures`): keep both surfaces here
//! verbatim.

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
/// Acceptance rationale (ADR-0021, local _Project/adr/, not committed):
/// tokenizing `code` would also hit error_code/status_code; short tokens
/// (sig/hmac) have real non-credential uses; the api_key-value-equal
/// replace layer backstops any param name. Re-open on: real leak report,
/// a sig/hmac-query endpoint, or structured redaction in reqwest.
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
/// 1. replace `://userinfo@` spans with `://***@` (the userinfo scan
///    terminates at `/`, `?`, `#`, or ASCII whitespace; `)` is NOT a
///    terminator — see [`redact_userinfo`]);
/// 2. replace `?name=value` / `&name=value` / `#name=value` query/fragment
///    credentials (token of `name` matches [`CREDENTIAL_QUERY_WORDS`],
///    ASCII-case-insensitive) with `***`;
/// 3. replace any occurrence of `api_key` with `[REDACTED]` (skipped when
///    empty — `str::replace("", x)` would insert between every char);
/// 4. truncate to 500 chars (multi-byte safe, applied last so truncation
///    never cuts through un-redacted text).
pub(crate) fn redact_error_detail(s: &str, api_key: &str) -> String {
    let redacted = redact_userinfo(s);
    let redacted = redact_query_credentials(&redacted);
    let redacted = if api_key.is_empty() {
        redacted
    } else {
        redacted.replace(api_key, "[REDACTED]")
    };
    redacted.chars().take(500).collect()
}

/// Replace `://userinfo@` spans with `://***@`. The userinfo scan
/// terminates at `/`, `?`, `#`, or ASCII whitespace; `)` is NOT a
/// terminator — RFC 3986 allows `)` unencoded in userinfo, so a valid
/// URL may contain one, and swallowing it is the conservative
/// over-redaction direction (same convention as
/// `redact_query_credentials`). Applied before query redaction so a
/// URL carrying both forms is scrubbed in one pass.
///
/// Malformed-input boundary: the detector (`url_contains_userinfo`)
/// splits the authority on `/ ? #` only, so input whose authority
/// contains whitespace before an `@` is detected (and conditionally
/// encrypted) but NOT redacted here (the scan aborts at whitespace) —
/// accepted residual; reqwest error Display never emits such a
/// malformed URL.
fn redact_userinfo(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < bytes.len() {
        // Scan for the next "://" separator. ASCII-only, so byte indices
        // are char boundaries.
        if i + 2 < bytes.len() && bytes[i] == b':' && bytes[i + 1] == b'/' && bytes[i + 2] == b'/' {
            // Copy "://" verbatim and advance.
            out.push_str(&s[i..i + 3]);
            let scan_start = i + 3;
            let mut k = scan_start;
            let mut at_idx: Option<usize> = None;
            while k < bytes.len() {
                let c = bytes[k];
                if c == b'@' {
                    at_idx = Some(k);
                    break;
                }
                if c == b'/' || c == b'?' || c == b'#' || c.is_ascii_whitespace() {
                    break;
                }
                k += 1;
            }
            if let Some(at) = at_idx {
                // Replace [scan_start..at] with "***" — both ends are
                // ASCII byte indices on a UTF-8 string (the bytes before
                // `@` are scanned as ASCII bytes only), so the slice is
                // guaranteed to land on char boundaries.
                out.push_str("***");
                out.push('@');
                i = at + 1;
            } else {
                // No `@` before a terminator — copy the remainder of the
                // authority (or up to the terminator) verbatim and continue.
                let copy_end = if k < bytes.len() { k } else { bytes.len() };
                out.push_str(&s[scan_start..copy_end]);
                i = copy_end;
            }
            continue;
        }
        // Copy one char to the output (multi-byte safe).
        let size = s[i..].chars().next().map_or(1, |c| c.len_utf8());
        out.push_str(&s[i..i + size]);
        i += size;
    }
    out
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

/// True when the URL embeds userinfo (`scheme://user[:pass]@host/…`).
/// Detection: substring after `://` up to the first `/`, `?`, or `#`
/// (the authority) contains `@`. Mirrors frontend
/// `ui/lib/settings-utils.js::hasUserinfoInUrl` — fixture parity guarded
/// by `__tests__/userinfo-detection-contract.test.js`. Parity is only
/// guaranteed for well-formed absolute URLs: on malformed input (missing
/// or non-alpha-leading scheme, leading whitespace) the frontend regex
/// is anchored and under-warns while this substring search over-detects —
/// both are the safe direction for their layer.
pub(crate) fn url_contains_userinfo(url: &str) -> bool {
    let Some(scheme_end) = url.find("://") else {
        return false;
    };
    let authority = &url[scheme_end + 3..];
    let authority = authority.split(['/', '?', '#']).next().unwrap_or_default();
    authority.contains('@')
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Shared fixture table (also embedded in
    /// `__tests__/userinfo-detection-contract.test.js`): backend and
    /// frontend detectors must agree on every URL. Each literal must
    /// appear unchanged in this file — the contract test scans for it.
    #[test]
    fn test_url_contains_userinfo_fixtures() {
        // Positive cases (URL embeds userinfo → true)
        assert!(url_contains_userinfo("http://user:pass@host/v1"));
        assert!(url_contains_userinfo("https://user@h.com/v1"));
        assert!(url_contains_userinfo("http://u:p@[2001:db8::1]:8080/v1"));
        assert!(url_contains_userinfo("http://user:pass@host"));
        assert!(url_contains_userinfo("https://u:p@h.com/v1?key=abc#frag"));

        // Negative cases (no userinfo → false)
        assert!(!url_contains_userinfo("http://host/v1"));
        assert!(!url_contains_userinfo("http://host/v1?next=@x"));
        assert!(!url_contains_userinfo("https://h.com/v1#frag@ment"));
        assert!(!url_contains_userinfo("mailto:user@host"));
        assert!(!url_contains_userinfo("http://[::1]:8080/v1"));
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

    #[test]
    fn test_redact_userinfo_basic() {
        // Trailing `)` is preserved (the userinfo scan terminates on `@`,
        // not on `)`, so only the userinfo span is replaced).
        assert_eq!(
            redact_error_detail(
                "error sending request for url (http://user:pass@h.com/v1)",
                ""
            ),
            "error sending request for url (http://***@h.com/v1)"
        );
    }

    #[test]
    fn test_redact_userinfo_combined_with_query_and_key() {
        // userinfo + query credential + api_key value — all three layers
        // hit. The trailing `)` after `SECRET` is swallowed by the query
        // redaction layer (the existing
        // `redact_query_credentials` value scan breaks on `&` / whitespace
        // only — `)` is intentionally NOT a terminator to avoid leaking
        // values that legitimately contain `)`). Userinfo alone preserves
        // a bare `)` (see `test_redact_userinfo_basic`).
        assert_eq!(
            redact_error_detail("url (https://u:p@h.com/v1?key=SECRET) sk-123", "sk-123"),
            "url (https://***@h.com/v1?key=*** [REDACTED]"
        );
    }

    #[test]
    fn test_redact_userinfo_no_userinfo_unchanged() {
        // Query-only URL: userinfo layer is a no-op (no `://...@`),
        // query layer handles `key=x`.
        assert_eq!(
            redact_error_detail("http://host/v1?key=x", ""),
            "http://host/v1?key=***"
        );
        // IPv6 literal without userinfo: untouched by either layer.
        assert_eq!(
            redact_error_detail("http://[::1]:8080/v1", ""),
            "http://[::1]:8080/v1"
        );
    }

    #[test]
    fn test_redact_userinfo_multiple_urls() {
        assert_eq!(
            redact_error_detail("first http://a:b@x.com/v1 then https://c@d.com/v2 end", ""),
            "first http://***@x.com/v1 then https://***@d.com/v2 end"
        );
    }

    #[test]
    fn test_redact_userinfo_nested_scheme_separator() {
        // The replace must resume AFTER `@` so a subsequent `://...@`
        // span in the same string is also scrubbed. Without the
        // post-replace resume, "b://c@d" would leak.
        assert_eq!(
            redact_userinfo("x http://a@b://c@d y"),
            "x http://***@b://***@d y"
        );
    }

    #[test]
    fn test_redact_userinfo_whitespace_terminates_scan() {
        // Whitespace in the authority aborts the scan: no `@` before
        // the terminator means no redaction. The trailing `user@x` is
        // not URL userinfo (no preceding `://`), so it stays put.
        assert_eq!(
            redact_userinfo("http://host see user@x"),
            "http://host see user@x"
        );
        // `@` hits before whitespace: replace and keep the trailing text.
        assert_eq!(
            redact_userinfo("see http://u:p@h.com/v1 next"),
            "see http://***@h.com/v1 next"
        );
    }
}
