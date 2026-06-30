/// Presentation-layer adapter for the API-key masking protocol.
///
/// This module only handles the display-side contract (masking/unmasking the
/// `llm_api_key` value at the command boundary). It does not persist keys or
/// perform cryptography.
pub const MASKED_MARKER: &str = "__MASKED__";

/// Adapter for masking API keys at the presentation boundary.
pub struct ApiKeyMask;

impl ApiKeyMask {
    /// Mask a non-empty API key. Empty strings remain empty so the frontend can
    /// distinguish "no key" from "key exists".
    pub fn mask(key: &str) -> String {
        if key.is_empty() {
            String::new()
        } else {
            MASKED_MARKER.to_string()
        }
    }

    /// Resolve the API key that should be persisted.
    ///
    /// - If the frontend sent the masked marker, keep the existing key.
    /// - If the frontend sent an empty string but a key already exists, keep
    ///   the existing key (empty input does not delete the saved key).
    /// - Otherwise, use the incoming value (new key or intentional clear).
    pub fn unmask_or_keep(incoming: &str, current: &str) -> String {
        if incoming == MASKED_MARKER || (incoming.is_empty() && !current.is_empty()) {
            current.to_string()
        } else {
            incoming.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_empty_returns_empty() {
        assert_eq!(ApiKeyMask::mask(""), "");
    }

    #[test]
    fn test_mask_non_empty_returns_marker() {
        assert_eq!(ApiKeyMask::mask("sk-secret"), MASKED_MARKER);
    }

    #[test]
    fn test_unmask_marker_keeps_current() {
        assert_eq!(
            ApiKeyMask::unmask_or_keep(MASKED_MARKER, "sk-old"),
            "sk-old"
        );
    }

    #[test]
    fn test_unmask_empty_with_current_keeps_current() {
        assert_eq!(ApiKeyMask::unmask_or_keep("", "sk-old"), "sk-old");
    }

    #[test]
    fn test_unmask_empty_without_current_stays_empty() {
        assert_eq!(ApiKeyMask::unmask_or_keep("", ""), "");
    }

    #[test]
    fn test_unmask_new_key_replaces_current() {
        assert_eq!(ApiKeyMask::unmask_or_keep("sk-new", "sk-old"), "sk-new");
    }

    #[test]
    fn test_unmask_marker_with_empty_current_stays_empty() {
        assert_eq!(ApiKeyMask::unmask_or_keep(MASKED_MARKER, ""), "");
    }

    #[test]
    fn test_whitespace_and_special_chars_passthrough() {
        assert_eq!(
            ApiKeyMask::unmask_or_keep("  sk-key-with-spaces  ", "sk-old"),
            "  sk-key-with-spaces  "
        );
        assert_eq!(
            ApiKeyMask::unmask_or_keep("sk-key\nwith-newline", "sk-old"),
            "sk-key\nwith-newline"
        );
    }

    #[test]
    fn test_masked_marker_value() {
        assert_eq!(MASKED_MARKER, "__MASKED__");
    }
}
