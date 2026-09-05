use crate::error::AppError;
use crate::hotkey::windows::HotkeySpec;

pub mod windows;

/// Single authoritative key-name table (the frontend `ui/lib/hotkeys.js` mirrors
/// this table — both sides have pointer-comments to each other, matching the
/// `MODEL_SIZES` ↔ `BUILT_IN_MODELS` convention).
///
/// `(key name, vk code)` — compared case-insensitively. A–Z (0x41–0x5A) and
/// 0–9 (0x30–0x39) are handled by `from_key_name`'s single-character rule
/// and are NOT in the table. Aliases (rctrl etc., 6 entries) plus
/// escape/esc are listed: the accept domain is a strict superset of the
/// old `parse_key_code` table — users who hand-edited config files using
/// legacy aliases must keep working after upgrade, otherwise deserialization
/// would fail and `lib.rs::load` would silently fall back to all defaults
/// (existing behavior we must preserve).
pub const NAMED_KEYS: &[(&str, u32)] = &[
    ("rightctrl", 0xA3),
    ("rctrl", 0xA3),
    ("leftctrl", 0xA2),
    ("lctrl", 0xA2),
    ("rightalt", 0xA5),
    ("ralt", 0xA5),
    ("leftalt", 0xA4),
    ("lalt", 0xA4),
    ("rightshift", 0xA1),
    ("rshift", 0xA1),
    ("leftshift", 0xA0),
    ("lshift", 0xA0),
    ("escape", 0x1B),
    ("esc", 0x1B),
    ("f1", 0x70),
    ("f2", 0x71),
    ("f3", 0x72),
    ("f4", 0x73),
    ("f5", 0x74),
    ("f6", 0x75),
    ("f7", 0x76),
    ("f8", 0x77),
    ("f9", 0x78),
    ("f10", 0x79),
    ("f11", 0x7A),
    ("f12", 0x7B),
];

/// Resolve a key name to its virtual key code. Accepts NAMED_KEYS entries
/// (case-insensitive) and single ASCII letters/digits.
pub fn from_key_name(name: &str) -> Option<u32> {
    let lower = name.to_lowercase();
    if let Some((_, vk)) = NAMED_KEYS.iter().find(|(n, _)| *n == lower) {
        return Some(*vk);
    }
    // Single-letter / single-digit normalisation: lowercase letter -> 0x41..0x5A,
    // digit -> 0x30..0x39.
    if lower.len() == 1 {
        let c = lower.as_bytes()[0];
        if c.is_ascii_lowercase() {
            return Some(0x41 + (c - b'a') as u32);
        }
        if c.is_ascii_digit() {
            return Some(0x30 + (c - b'0') as u32);
        }
    }
    None
}

/// Reverse lookup. Returns the Title-Case canonical name (`"RightCtrl"` /
/// `"F9"` / `"A"`) for any vk in `NAMED_KEYS` or the ASCII letter/digit
/// ranges — alias vk codes (e.g. 0xA2 leftctrl) return the canonical
/// form (`"LeftCtrl"`). Unknown vk returns `format!("VK{vk:#x}")` so
/// the UI can display it verbatim instead of silently rewriting config.
pub fn vk_to_key_name(vk: u32) -> String {
    // NAMED_KEYS may list multiple names for the same vk (alias + canonical).
    // Walk in order and pick the FIRST entry that matches — the canonical
    // (long form) name is listed before the alias in each pair, so the
    // canonical name wins.
    if let Some((name, _)) = NAMED_KEYS.iter().find(|(_, v)| *v == vk) {
        // Title-case capitalisation for compound names:
        //   "rightctrl"  -> "RightCtrl"
        //   "rightshift" -> "RightShift"
        //   "esc"        -> "Esc" (single segment)
        // We split on ascii-lowercase + ascii-uppercase boundaries, then
        // uppercase the first letter of each segment.
        return title_case(name);
    }
    // Single ASCII letter / digit normalisations.
    if (0x41..=0x5A).contains(&vk) {
        let ch = (vk - 0x41) as u8 + b'A';
        return (ch as char).to_string();
    }
    if (0x30..=0x39).contains(&vk) {
        let ch = (vk - 0x30) as u8 + b'0';
        return (ch as char).to_string();
    }
    format!("VK{vk:#x}")
}

/// Convert a lowercase name like "rightctrl" / "esc" / "f1" into Title Case:
/// "RightCtrl" / "Esc" / "F1". Splits at the boundary between a modifier
/// prefix ("right"/"left") and the key name ("ctrl"/"alt"/"shift"/"escape")
/// using a fixed suffix table; for unknown shapes (e.g. "f1", "esc") we
/// fall back to upper-casing the first character only.
fn title_case(name: &str) -> String {
    // Recognised CamelCase boundaries: name ends with one of these suffixes.
    // Try longest match first.
    const SUFFIXES: &[&str] = &["escape", "shift", "ctrl", "alt"];
    for suf in SUFFIXES {
        if let Some(prefix) = name.strip_suffix(suf) {
            if !prefix.is_empty() {
                return capitalize_first(prefix) + &capitalize_first(suf);
            }
        }
    }
    // No compound split — uppercase the first letter.
    capitalize_first(name)
}

fn capitalize_first(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    if let Some(first) = chars.next() {
        for u in first.to_uppercase() {
            out.push(u);
        }
        out.push_str(chars.as_str());
    }
    out
}

/// Callback type for hotkey events.
pub type HotkeyCallback = Box<dyn Fn(HotkeyEvent) + Send + Sync>;

/// Callback type for the Esc-cancel dispatcher. Returns `true` when the
/// cancellation actually happened (hook swallows the Esc); `false` when
/// no active pipeline exists and the key should pass through.
pub type CancelEscCallback = Box<dyn Fn() -> bool + Send + Sync>;

/// Hotkey event types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// Hotkey was pressed (start recording).
    Pressed,
    /// Hotkey was released (stop recording).
    Released,
}

/// Trait for hotkey management (platform-agnostic).
pub trait HotkeyManager: Send {
    /// Register a global hotkey from the given spec.
    /// Calls the callback on press/release events.
    fn register(&mut self, spec: HotkeySpec, callback: HotkeyCallback) -> Result<(), AppError>;

    /// Register the record-only hotkey (second, independent slot).
    /// Shares the same low-level keyboard hook as the primary hotkey;
    /// events are dispatched by virtual key code.
    fn register_record_only(
        &mut self,
        spec: HotkeySpec,
        callback: HotkeyCallback,
    ) -> Result<(), AppError>;

    /// Register the Esc-to-cancel dispatcher for the classic pipeline.
    /// The callback returns whether a cancellation actually happened;
    /// the hook swallows Esc only when the callback returns `true`.
    /// Must be called after `register()` (so the hook is installed).
    fn register_cancel_esc(&mut self, callback: CancelEscCallback) -> Result<(), AppError>;

    /// Unregister all hotkeys (primary + record-only + cancel_esc) and
    /// remove the hook. Used at shutdown / Drop.
    fn unregister(&mut self) -> Result<(), AppError>;

    /// Unregister ONLY the primary hotkey slot, leaving record-only and
    /// cancel_esc intact. Used by save_settings when only the primary key
    /// changes — full `unregister()` would wipe cancel_esc (Esc cancel
    /// silently stops working until restart) and record_only (a known
    /// pre-existing bug where changing the primary key also nukes the
    /// record-only slot until restart).
    fn unregister_primary(&mut self) -> Result<(), AppError>;

    /// Unregister only the record-only hotkey slot, leaving the primary
    /// hotkey active. Used when settings change just the record-only key.
    fn unregister_record_only(&mut self) -> Result<(), AppError>;

    /// Check if hotkey is currently registered.
    fn is_registered(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_name_vk_roundtrip() {
        // Every NAMED_KEYS entry must roundtrip — but the alias vk must
        // return the Title-Case canonical name (aliases are "in only").
        let expected_canonical: &[(&str, u32, &str)] = &[
            ("RightCtrl", 0xA3, "RightCtrl"),
            ("LeftCtrl", 0xA2, "LeftCtrl"),
            ("Rctrl", 0xA3, "RightCtrl"), // alias -> canonical
            ("RightAlt", 0xA5, "RightAlt"),
            ("LeftAlt", 0xA4, "LeftAlt"),
            ("Ralt", 0xA5, "RightAlt"),
            ("RightShift", 0xA1, "RightShift"),
            ("LeftShift", 0xA0, "LeftShift"),
            ("Escape", 0x1B, "Escape"),
            ("Esc", 0x1B, "Escape"),
            ("F1", 0x70, "F1"),
            ("F9", 0x78, "F9"),
            ("F12", 0x7B, "F12"),
        ];
        for (input_name, input_vk, expected_out) in expected_canonical {
            let resolved = from_key_name(input_name).expect("known name");
            assert_eq!(resolved, *input_vk, "from_key_name({input_name})");
            let back = vk_to_key_name(*input_vk);
            assert_eq!(back, *expected_out, "vk_to_key_name({input_vk:#x})");
        }

        // ASCII letters: a-z -> 0x41-0x5A; roundtrip preserves case.
        for c in b'a'..=b'z' {
            let name = (c as char).to_string();
            let vk = from_key_name(&name).expect("letter");
            assert_eq!(vk, 0x41 + (c - b'a') as u32);
            let back = vk_to_key_name(vk);
            assert_eq!(back, (c as char).to_uppercase().to_string());
        }
        // ASCII digits: 0-9 -> 0x30-0x39.
        for d in b'0'..=b'9' {
            let name = (d as char).to_string();
            let vk = from_key_name(&name).expect("digit");
            assert_eq!(vk, 0x30 + (d - b'0') as u32);
            let back = vk_to_key_name(vk);
            assert_eq!(back, (d as char).to_string());
        }

        // Unknown input -> None.
        assert_eq!(from_key_name("no-such-key"), None);
        assert_eq!(from_key_name(""), None);
        // Unknown vk -> "VK{vk:#x}" form.
        assert_eq!(vk_to_key_name(0xDEAD), "VK0xdead");
    }
}
