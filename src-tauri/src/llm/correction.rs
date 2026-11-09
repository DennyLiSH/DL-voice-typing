//! The single LLM-correction channel shared by the classic pipeline and
//! the transcribe window.
//!
//! Both call sites used to implement the same recipe twice — live-read
//! the config, match the cached corrector, correct, fall back to the raw
//! text — and the two copies had drifted: the transcribe window never
//! wrote the rebuilt corrector back into the shared slot, so the two
//! callers filled the same cache with two different policies.
//!
//! This module owns the policy (resolve + correct + fall back). What it
//! deliberately does NOT own is the event surface: the classic pipeline
//! emits the fixed-string `llm-refining` / `llm-error` events, the
//! transcribe window emits `transcription-progress` (stage=llm) and
//! `transcription-error`. Those differences are load-bearing for their
//! respective frontends, so event emission stays with the callers.
//!
//! Lock protocol: callers MUST call [`correct_with_fallback`] from
//! inside a `PipelineState::with_cached_llm` closure. The closure holds
//! the slot lock for the duration of the HTTP call, which is the
//! pre-existing invariant ("HTTP inside the lock") — a corrector must
//! not be rebuilt while another one is mid-request.

use super::{LLMClient, TextCorrector, redact_error_detail};
use std::sync::Arc;
use tracing::warn;

/// User-facing message for the LLM-failure fallback path.
///
/// The classic pipeline sends it as the `llm-error` payload (bare JSON
/// string); the transcribe window sends it as the `transcription-error`
/// message. Both copies used to spell the message out independently —
/// this constant is the single source.
pub(crate) const LLM_FALLBACK_USER_MSG: &str = "LLM 纠错失败，已保留原始转录";

/// Resolve the corrector for the given config, rebuilding it into
/// `cached_slot` when the cached one is missing or does not match.
///
/// Single cache policy for both callers (the classic pipeline and the
/// transcribe window): a cached corrector whose (url, key, model) still
/// matches the config is reused; anything else is rebuilt and written
/// back into the slot, so the next caller benefits from it.
///
/// The rebuild is infallible (`LLMClient::new` cannot fail), so this
/// function returns the corrector directly — the only error the callers
/// have to handle is slot-lock poisoning, reported by
/// `PipelineState::with_cached_llm` as `None`.
pub(crate) fn resolve_corrector(
    cached_slot: &mut Option<Arc<dyn TextCorrector>>,
    cfg_url: &str,
    cfg_key: &str,
    cfg_model: &str,
) -> Arc<dyn TextCorrector> {
    if let Some(cached) = cached_slot.as_ref()
        && cached.matches_config(cfg_url, cfg_key, cfg_model)
    {
        return cached.clone();
    }
    let corrector: Arc<dyn TextCorrector> = Arc::new(LLMClient::new(
        cfg_url.to_string(),
        cfg_key.to_string(),
        cfg_model.to_string(),
    ));
    *cached_slot = Some(corrector.clone());
    corrector
}

/// Result of a correction attempt: the text to use (corrected, or the
/// raw text when the LLM failed) and whether the LLM actually ran.
pub(crate) struct CorrectionOutcome {
    pub text: String,
    pub used_llm: bool,
}

/// Correct `text` with the LLM, falling back to `text` itself when the
/// call fails.
///
/// The failure detail is logged (redacted) and never surfaced to the
/// user: the caller emits the fixed Chinese summary
/// ([`LLM_FALLBACK_USER_MSG`]) instead.
///
/// `api_key` is used for the log-side redaction only. `LLMClient::correct`
/// already redacts at the source, so this is belt-and-suspenders for
/// alternative `TextCorrector` implementations.
pub(crate) fn correct_with_fallback(
    corrector: &Arc<dyn TextCorrector>,
    text: &str,
    api_key: &str,
) -> CorrectionOutcome {
    match corrector.correct_sync(text) {
        Ok(corrected) => CorrectionOutcome {
            text: corrected,
            used_llm: true,
        },
        Err(e) => {
            warn!(
                target: "llm",
                "LLM correction failed, using raw transcription: {}",
                redact_error_detail(&e.to_string(), api_key)
            );
            CorrectionOutcome {
                text: text.to_string(),
                used_llm: false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockCorrector;

    fn mock(config: Option<(&str, &str, &str)>) -> Arc<dyn TextCorrector> {
        let corrector = match config {
            Some((url, key, model)) => MockCorrector::new("ok").with_config(url, key, model),
            None => MockCorrector::new("ok"),
        };
        Arc::new(corrector)
    }

    #[test]
    fn resolve_corrector_rebuilds_and_writes_back_when_slot_is_empty() {
        let mut slot: Option<Arc<dyn TextCorrector>> = None;
        let corrector = resolve_corrector(&mut slot, "u", "k", "m");
        assert!(
            slot.is_some(),
            "the rebuilt corrector must be written back into the slot"
        );
        let cached = slot.as_ref().expect("slot must hold the corrector");
        assert!(
            cached.matches_config("u", "k", "m"),
            "the cached corrector must match the requested config"
        );
        assert!(
            Arc::ptr_eq(&corrector, cached),
            "the returned handle must be the one stored in the slot"
        );
    }

    #[test]
    fn resolve_corrector_reuses_matching_cached_corrector() {
        let seeded = mock(Some(("u", "k", "m")));
        let mut slot = Some(seeded.clone());
        let corrector = resolve_corrector(&mut slot, "u", "k", "m");
        assert!(
            Arc::ptr_eq(&corrector, &seeded),
            "a matching cached corrector must be reused, not rebuilt"
        );
    }

    #[test]
    fn resolve_corrector_rebuilds_and_writes_back_on_mismatch() {
        let seeded = mock(Some(("u1", "k", "m")));
        let mut slot = Some(seeded.clone());
        let corrector = resolve_corrector(&mut slot, "u2", "k", "m");
        assert!(
            !Arc::ptr_eq(&corrector, &seeded),
            "a mismatching cached corrector must not be reused"
        );
        let cached = slot.as_ref().expect("slot must hold the corrector");
        assert!(
            cached.matches_config("u2", "k", "m"),
            "the rebuilt corrector must be written back with the new config"
        );
    }

    #[test]
    fn resolve_corrector_rebuilds_on_key_rotation() {
        // The key is part of the match predicate: a rotated key must
        // force a rebuild even when url and model are unchanged.
        let seeded = mock(Some(("u", "k1", "m")));
        let mut slot = Some(seeded.clone());
        let corrector = resolve_corrector(&mut slot, "u", "k2", "m");
        assert!(!Arc::ptr_eq(&corrector, &seeded));
        let cached = slot.as_ref().expect("slot must hold the corrector");
        assert!(cached.matches_config("u", "k2", "m"));
    }

    #[test]
    fn correct_with_fallback_returns_corrected_text_on_success() {
        let corrector = mock(None);
        let outcome = correct_with_fallback(&corrector, "raw", "k");
        assert!(outcome.used_llm);
        assert_eq!(outcome.text, "ok");
    }

    #[test]
    fn correct_with_fallback_returns_raw_text_on_failure() {
        let corrector: Arc<dyn TextCorrector> = Arc::new(MockCorrector::failing());
        let outcome = correct_with_fallback(&corrector, "raw", "SECRET123");
        assert!(!outcome.used_llm);
        assert_eq!(outcome.text, "raw");
    }
}
