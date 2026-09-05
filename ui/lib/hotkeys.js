// Mirror of the backend NAMED_KEYS table + ASCII letter/digit rules at
// src-tauri/src/hotkey/mod.rs. Both sides carry pointer-comments to
// each other (same convention as MODEL_SIZES ↔ BUILT_IN_MODELS).
//
// Frontend responsibilities:
//   - Drive the two <select> widgets on the hotkey page.
//   - Translate between DOM state and the HotkeySpec object shape that
//     the backend persists via serde.
//   - Display a label for any spec, including unknown vk codes (so users
//     can see what is in their config without it being silently rewritten).
//
// Only canonical (Title-Case) names live here; the hint "切换需重启生效"
// means the user only ever round-trips one canonical name. Backend aliases
// (rctrl/lctrl/...) are accepted on parse for backward compatibility but
// the UI never emits them.

const MODIFIER_KEYS = [
    'RightCtrl',
    'LeftCtrl',
    'RightAlt',
    'LeftAlt',
    'RightShift',
    'LeftShift',
];
const FUNCTION_KEYS = Array.from({ length: 12 }, (_, i) => `F${i + 1}`);
const LETTER_KEYS = Array.from({ length: 26 }, (_, i) =>
    String.fromCharCode(65 + i),
);
const DIGIT_KEYS = Array.from({ length: 10 }, (_, i) => String(i));

export const MAIN_KEYS = [
    ...MODIFIER_KEYS,
    ...FUNCTION_KEYS,
    ...LETTER_KEYS,
    ...DIGIT_KEYS,
];

/**
 * Canonical-name -> vk table. Mirrors NAMED_KEYS on the backend but only
 * stores canonical (Title-Case) entries — the frontend never emits
 * aliases. Letter/digit keys (0x41-0x5A, 0x30-0x39) are derived from the
 * MAIN_KEYS string via nameToVk below, not stored here.
 */
const NAME_TO_VK = new Map();
for (const name of MAIN_KEYS) {
    let vk;
    if (name.length === 1) {
        const ch = name.charCodeAt(0);
        if (ch >= 65 && ch <= 90) vk = ch;
        else if (ch >= 48 && ch <= 57) vk = ch;
        else throw new Error(`Unexpected single-char MAIN_KEYS entry: ${name}`);
    } else if (/^F([1-9]|1[0-2])$/.test(name)) {
        vk = 0x70 + (Number.parseInt(name.slice(1), 10) - 1);
    } else {
        switch (name) {
            case 'RightCtrl':
                vk = 0xa3;
                break;
            case 'LeftCtrl':
                vk = 0xa2;
                break;
            case 'RightAlt':
                vk = 0xa5;
                break;
            case 'LeftAlt':
                vk = 0xa4;
                break;
            case 'RightShift':
                vk = 0xa1;
                break;
            case 'LeftShift':
                vk = 0xa0;
                break;
            default:
                throw new Error(`Unmapped MAIN_KEYS entry: ${name}`);
        }
    }
    NAME_TO_VK.set(name, vk);
}

/** Canonical name -> vk. Returns undefined for unknown names. */
function nameToVk(name) {
    return NAME_TO_VK.get(name);
}

/**
 * vk -> canonical name. Mirrors vk_to_key_name in
 * src-tauri/src/hotkey/mod.rs (Title-Case output, `VK0xNN` fallback for
 * unknown vk). Used by writeSpecToUI to find the matching <option>.
 */
function vkToName(vk) {
    for (const [name, code] of NAME_TO_VK) {
        if (code === vk) return name;
    }
    // Backend uses `format!("VK{vk:#x}")` which yields `VK0xdead`
    // (lowercase hex with explicit `0x` prefix). Mirror that exactly so
    // the label string is byte-identical to the backend display().
    return `VK0x${vk.toString(16)}`;
}

/**
 * Deep equality for HotkeySpec objects. Property order does not matter.
 *
 * The spec object has exactly 4 fixed keys (ctrl/shift/alt/vk) produced
 * by specFromUI; sameSpec therefore does a structural compare rather
 * than a generic "same keys + same values" walk. This matches how the
 * backend treats HotkeySpec (PartialEq, structural).
 */
export function sameSpec(a, b) {
    if (a === b) return true;
    if (!a || !b) return false;
    return (
        a.ctrl === b.ctrl &&
        a.shift === b.shift &&
        a.alt === b.alt &&
        a.vk === b.vk
    );
}

/**
 * Human-readable label for a spec. Mirrors `HotkeySpec::display()` on the
 * backend: "Ctrl+Shift+A" / "RightCtrl" / "Ctrl+VK0xdead". Unknown vk
 * fall through as `VK0xNN` (lowercase x) so the user sees the raw value
 * from config instead of it being silently rewritten.
 */
export function specLabel(spec) {
    const parts = [];
    if (spec.ctrl) parts.push('Ctrl');
    if (spec.shift) parts.push('Shift');
    if (spec.alt) parts.push('Alt');
    parts.push(vkToName(spec.vk));
    return parts.join('+');
}

/**
 * Read the three modifier checkboxes + the main key <select> for the
 * given id-prefix ("hotkey" or "record-only-hotkey") into a HotkeySpec
 * object. The <select> value is the canonical key name; if it is empty
 * (unknown vk from prior config) we return vk=0 so the dirty/validation
 * path can flag the spec as invalid without crashing.
 */
export function specFromUI(prefix) {
    const ctrl = document.getElementById(`${prefix}-ctrl`)?.checked ?? false;
    const shift = document.getElementById(`${prefix}-shift`)?.checked ?? false;
    const alt = document.getElementById(`${prefix}-alt`)?.checked ?? false;
    const sel = document.getElementById(prefix);
    const name = sel?.value ?? '';
    const vk = name ? (nameToVk(name) ?? 0) : 0;
    return { ctrl, shift, alt, vk };
}

/**
 * Reflect a HotkeySpec back into the DOM:
 *   - Set each modifier checkbox.
 *   - If vk maps to a canonical name, select that option.
 *   - If vk is unknown, leave the <select> value empty (so specLabel
 *     falls through to VK0xNN display) and DO NOT mutate `spec` — a
 *     loaded config with an unsupported vk must round-trip back out
 *     unchanged, otherwise "open settings page" silently rewrites the
 *     persisted config.
 */
export function writeSpecToUI(prefix, spec) {
    document.getElementById(`${prefix}-ctrl`).checked = !!spec.ctrl;
    document.getElementById(`${prefix}-shift`).checked = !!spec.shift;
    document.getElementById(`${prefix}-alt`).checked = !!spec.alt;
    const sel = document.getElementById(prefix);
    // Direct table lookup: if vk has no canonical name, we must NOT guess
    // — leave the select empty so the user sees `VK0xNN` in specLabel
    // and the spec round-trips out unchanged.
    let knownName = null;
    for (const [name, code] of NAME_TO_VK) {
        if (code === spec.vk) {
            knownName = name;
            break;
        }
    }
    sel.value = knownName ?? '';
}

/**
 * Fill both hotkey <select> elements (`#hotkey`, `#record-only-hotkey`)
 * with one <option> per MAIN_KEYS entry. Called once from settings.js
 * init before any populateFields, so that writeSpecToUI's `sel.value =
 * name` has the option to land on. Idempotent: a second call replaces
 * the existing options rather than appending.
 */
export function populateMainKeySelects() {
    const sels = ['hotkey', 'record-only-hotkey'].map((id) =>
        document.getElementById(id),
    );
    for (const sel of sels) {
        if (!sel) continue;
        sel.innerHTML = '';
        for (const name of MAIN_KEYS) {
            const opt = document.createElement('option');
            opt.value = name;
            opt.textContent = name;
            sel.appendChild(opt);
        }
    }
}
