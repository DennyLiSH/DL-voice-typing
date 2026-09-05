// @vitest-environment jsdom
//
// Unit tests for ui/lib/hotkeys.js — frontend mirror of the backend
// NAMED_KEYS table (src-tauri/src/hotkey/mod.rs). The two sides carry
// pointer-comments to each other (MODEL_SIZES ↔ BUILT_IN_MODELS convention).
//
// Coverage:
//   - MAIN_KEYS contains all canonical key names (canonical form only;
//     aliases stay on the backend since the frontend only emits the
//     canonical Title-Case form via vk_to_key_name).
//   - populateMainKeySelects() fills the two hotkey <select>s with options.
//   - specFromUI / writeSpecToUI round-trip a HotkeySpec object via DOM.
//   - writeSpecToUI with unknown vk leaves select empty + label shows
//     VK0xNN — the spec itself is NOT mutated, preventing the
//     "open settings = silently rewrite config" anti-pattern.
//   - specLabel formats for known / unknown vk.
//   - sameSpec is order-independent deep equality.

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
    MAIN_KEYS,
    populateMainKeySelects,
    sameSpec,
    specFromUI,
    specLabel,
    writeSpecToUI,
} from '../ui/lib/hotkeys.js';

const SELECT_IDS = ['hotkey', 'record-only-hotkey'];

function buildComboDom() {
    document.body.innerHTML = '';
    for (const prefix of SELECT_IDS) {
        const wrap = document.createElement('div');
        wrap.id = `${prefix}-combo`;
        for (const mod of ['ctrl', 'shift', 'alt']) {
            const cb = document.createElement('input');
            cb.type = 'checkbox';
            cb.id = `${prefix}-${mod}`;
            wrap.appendChild(cb);
        }
        const sel = document.createElement('select');
        sel.id = prefix;
        wrap.appendChild(sel);
        document.body.appendChild(wrap);
    }
}

describe('MAIN_KEYS', () => {
    it('contains all required canonical modifier keys', () => {
        for (const k of [
            'RightCtrl',
            'LeftCtrl',
            'RightAlt',
            'LeftAlt',
            'RightShift',
            'LeftShift',
        ]) {
            expect(MAIN_KEYS).toContain(k);
        }
    });

    it('contains all F1-F12 keys', () => {
        for (let i = 1; i <= 12; i++) {
            expect(MAIN_KEYS).toContain(`F${i}`);
        }
    });

    it('contains A-Z', () => {
        for (let i = 0; i < 26; i++) {
            expect(MAIN_KEYS).toContain(String.fromCharCode(65 + i));
        }
    });

    it('contains 0-9', () => {
        for (let i = 0; i < 10; i++) {
            expect(MAIN_KEYS).toContain(String(i));
        }
    });
});

describe('populateMainKeySelects', () => {
    beforeEach(buildComboDom);
    afterEach(() => {
        document.body.innerHTML = '';
    });

    it('populates both <select> elements with MAIN_KEYS options', () => {
        populateMainKeySelects();
        for (const id of SELECT_IDS) {
            const sel = document.getElementById(id);
            expect(sel.options.length).toBe(MAIN_KEYS.length);
            for (let i = 0; i < MAIN_KEYS.length; i++) {
                expect(sel.options[i].value).toBe(MAIN_KEYS[i]);
                expect(sel.options[i].textContent).toBe(MAIN_KEYS[i]);
            }
        }
    });

    it('is idempotent (second call does not duplicate)', () => {
        populateMainKeySelects();
        populateMainKeySelects();
        for (const id of SELECT_IDS) {
            expect(document.getElementById(id).options.length).toBe(
                MAIN_KEYS.length,
            );
        }
    });
});

describe('specFromUI / writeSpecToUI', () => {
    beforeEach(buildComboDom);
    afterEach(() => {
        document.body.innerHTML = '';
    });

    it('reads checkboxes + select value into a HotkeySpec object', () => {
        populateMainKeySelects();
        document.getElementById('hotkey-ctrl').checked = true;
        document.getElementById('hotkey-alt').checked = true;
        const sel = document.getElementById('hotkey');
        sel.value = 'A';
        const spec = specFromUI('hotkey');
        expect(spec).toEqual({ ctrl: true, shift: false, alt: true, vk: 65 });
    });

    it('round-trips a complex spec through the DOM', () => {
        populateMainKeySelects();
        const input = {
            ctrl: true,
            shift: true,
            alt: false,
            vk: 0x41, // 'A'
        };
        writeSpecToUI('hotkey', input);
        expect(document.getElementById('hotkey-ctrl').checked).toBe(true);
        expect(document.getElementById('hotkey-shift').checked).toBe(true);
        expect(document.getElementById('hotkey-alt').checked).toBe(false);
        expect(document.getElementById('hotkey').value).toBe('A');
        const back = specFromUI('hotkey');
        expect(sameSpec(back, input)).toBe(true);
    });

    it('writes RightCtrl back to its canonical select option', () => {
        populateMainKeySelects();
        writeSpecToUI('hotkey', {
            ctrl: false,
            shift: false,
            alt: false,
            vk: 0xa3,
        });
        expect(document.getElementById('hotkey').value).toBe('RightCtrl');
    });

    it('with unknown vk, leaves select empty AND does not mutate spec', () => {
        populateMainKeySelects();
        // Pre-set a known value to confirm writeSpecToUI clears it.
        const sel = document.getElementById('hotkey');
        sel.value = 'RightCtrl';
        const spec = { ctrl: true, shift: false, alt: false, vk: 0xdead };
        writeSpecToUI('hotkey', spec);
        // Select was emptied (no option matches 0xDEAD).
        expect(sel.value).toBe('');
        // The spec object was NOT rewritten to a known vk.
        expect(spec.vk).toBe(0xdead);
        expect(spec.ctrl).toBe(true);
    });

    it('does not mutate the input spec on a known vk either (defense in depth)', () => {
        populateMainKeySelects();
        const spec = { ctrl: true, shift: false, alt: false, vk: 0xa3 };
        const frozen = JSON.parse(JSON.stringify(spec));
        writeSpecToUI('hotkey', spec);
        // Identity check — same object reference, no field changes.
        expect(spec).toEqual(frozen);
    });
});

describe('specLabel', () => {
    it('formats modifier+letter combo', () => {
        expect(
            specLabel({ ctrl: true, shift: false, alt: false, vk: 65 }),
        ).toBe('Ctrl+A');
    });
    it('formats a single canonical modifier (RightCtrl, no mods)', () => {
        expect(
            specLabel({ ctrl: false, shift: false, alt: false, vk: 0xa3 }),
        ).toBe('RightCtrl');
    });
    it('formats Ctrl+RightCtrl (the spec.vk is a modifier — still appended)', () => {
        // The form lets the user add mods even when main key is a modifier.
        // specLabel just joins them; we don't enforce "main is not a modifier".
        expect(
            specLabel({ ctrl: true, shift: false, alt: false, vk: 0xa3 }),
        ).toBe('Ctrl+RightCtrl');
    });
    it('formats Ctrl+Shift+A (multi-modifier)', () => {
        expect(specLabel({ ctrl: true, shift: true, alt: false, vk: 65 })).toBe(
            'Ctrl+Shift+A',
        );
    });
    it('formats unknown vk as VK0xNN (hex, lowercase x per backend)', () => {
        // Backend format!("VK{vk:#x}") uses lowercase hex (0xdead), not 0xDEAD.
        expect(
            specLabel({ ctrl: true, shift: false, alt: false, vk: 0xdead }),
        ).toBe('Ctrl+VK0xdead');
    });
    it('formats F-key without modifier prefix', () => {
        expect(
            specLabel({ ctrl: false, shift: false, alt: false, vk: 0x70 }),
        ).toBe('F1');
    });
});

describe('sameSpec', () => {
    it('returns true for identical objects', () => {
        expect(
            sameSpec(
                { ctrl: true, shift: false, alt: false, vk: 65 },
                {
                    ctrl: true,
                    shift: false,
                    alt: false,
                    vk: 65,
                },
            ),
        ).toBe(true);
    });
    it('returns true regardless of property order', () => {
        expect(
            sameSpec(
                { ctrl: true, shift: false, alt: false, vk: 65 },
                { vk: 65, ctrl: true, alt: false, shift: false },
            ),
        ).toBe(true);
    });
    it('returns false when ctrl differs', () => {
        expect(
            sameSpec(
                { ctrl: false, shift: false, alt: false, vk: 65 },
                {
                    ctrl: true,
                    shift: false,
                    alt: false,
                    vk: 65,
                },
            ),
        ).toBe(false);
    });
    it('returns false when vk differs', () => {
        expect(
            sameSpec(
                { ctrl: false, shift: false, alt: false, vk: 65 },
                {
                    ctrl: false,
                    shift: false,
                    alt: false,
                    vk: 66,
                },
            ),
        ).toBe(false);
    });
    it('returns false when shift differs', () => {
        expect(
            sameSpec(
                { ctrl: true, shift: false, alt: false, vk: 65 },
                {
                    ctrl: true,
                    shift: true,
                    alt: false,
                    vk: 65,
                },
            ),
        ).toBe(false);
    });
    it('returns false when alt differs', () => {
        expect(
            sameSpec(
                { ctrl: false, shift: false, alt: false, vk: 65 },
                {
                    ctrl: false,
                    shift: false,
                    alt: true,
                    vk: 65,
                },
            ),
        ).toBe(false);
    });
});
