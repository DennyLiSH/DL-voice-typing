// FFI contract test: ui/lib/hotkeys.js must stay aligned with the Rust
// sources of truth (hotkey/mod.rs NAMED_KEYS + schema.rs defaults).
// Scrapes the Rust sources so a key added on one side without the other
// turns CI red. Precedent: __tests__/css-text-color-contract.test.js.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
    DEFAULT_PRIMARY_SPEC,
    DEFAULT_RECORD_ONLY_SPEC,
    MAIN_KEYS,
    nameToVk,
    SELECT_EXCLUDED,
    specLabel,
} from '../ui/lib/hotkeys.js';

const MOD_RS = readFileSync(
    new URL('../src-tauri/src/hotkey/mod.rs', import.meta.url),
    'utf8',
);
const SCHEMA_RS = readFileSync(
    new URL('../src-tauri/src/config/schema.rs', import.meta.url),
    'utf8',
);

// Mirror of Rust title_case (hotkey/mod.rs): compound suffixes
// escape/shift/ctrl/alt split + capitalize; everything else capitalizes
// the first letter only (f1 -> F1, esc -> Esc).
function rustTitleCase(name) {
    for (const suf of ['escape', 'shift', 'ctrl', 'alt']) {
        if (name.endsWith(suf) && name.length > suf.length) {
            const prefix = name.slice(0, -suf.length);
            return (
                prefix[0].toUpperCase() +
                prefix.slice(1) +
                suf[0].toUpperCase() +
                suf.slice(1)
            );
        }
    }
    return name[0].toUpperCase() + name.slice(1);
}

// First name per vk wins — Rust vk_to_key_name picks the first matching
// entry, and the canonical (long) form is listed before its alias.
function scrapeCanonicalNamedKeys() {
    const block = MOD_RS.match(/pub const NAMED_KEYS[^=]*=\s*&\[\n(.*?)\n\];/s);
    if (!block) throw new Error('NAMED_KEYS block not found in mod.rs');
    const pairs = [
        ...block[1].matchAll(/\("([a-z0-9]+)",\s*(0x[0-9A-Fa-f]+)\)/g),
    ].map((m) => [m[1], Number(m[2])]);
    const canonical = new Map();
    for (const [name, vk] of pairs) {
        if (!canonical.has(vk)) canonical.set(vk, name);
    }
    return [...canonical.entries()].map(([vk, name]) => [
        rustTitleCase(name),
        vk,
    ]);
}

function scrapeDefaultSpec(fnName) {
    const m = SCHEMA_RS.match(
        new RegExp(`fn ${fnName}\\(\\) -> HotkeySpec \\{([\\s\\S]*?)\\}`),
    );
    if (!m) throw new Error(`${fnName} not found in schema.rs`);
    const body = m[1];
    const flag = (f) => body.includes(`${f}: true`);
    const vk = Number(body.match(/vk:\s*(0x[0-9A-Fa-f]+)/)[1]);
    return { ctrl: flag('ctrl'), shift: flag('shift'), alt: flag('alt'), vk };
}

describe('hotkeys FFI contract', () => {
    it('every canonical backend key resolves in the frontend display table', () => {
        for (const [name, vk] of scrapeCanonicalNamedKeys()) {
            expect(nameToVk(name), `nameToVk(${name})`).toBe(vk);
        }
    });

    it('specLabel renders backend canonical names byte-identically (Escape)', () => {
        // Regression: 0x1B used to render as VK0x1b on the frontend while
        // the backend displays "Escape" (hand-edited escape config).
        expect(
            specLabel({ ctrl: false, shift: false, alt: false, vk: 0x1b }),
        ).toBe('Escape');
        expect(
            specLabel({ ctrl: false, shift: false, alt: false, vk: 0xa3 }),
        ).toBe('RightCtrl');
    });

    it('MAIN_KEYS (selectable) excludes only the documented display-only keys (bidirectional)', () => {
        const canonical = scrapeCanonicalNamedKeys().filter(
            ([name]) => !/^[A-Z0-9]$/.test(name) && !/^F\d+$/.test(name),
        );
        const canonicalNames = new Set(canonical.map(([n]) => n));
        const selectable = new Set(MAIN_KEYS);
        // Forward: every backend canonical key is selectable or documented.
        for (const [name] of canonical) {
            if (selectable.has(name)) continue;
            expect(
                SELECT_EXCLUDED,
                `${name} must be in SELECT_EXCLUDED if not selectable`,
            ).toContain(name);
        }
        // Reverse 1: every compound MAIN_KEYS entry exists on the backend —
        // a frontend-only compound key would configure a dead hotkey the
        // backend's from_key_name cannot resolve.
        for (const name of MAIN_KEYS) {
            if (/^[A-Z0-9]$/.test(name) || /^F\d+$/.test(name)) continue;
            expect(
                canonicalNames,
                `MAIN_KEYS entry ${name} has no backend canonical key`,
            ).toContain(name);
        }
        // Reverse 2: SELECT_EXCLUDED must not outlive its backend key.
        for (const name of SELECT_EXCLUDED) {
            expect(
                canonicalNames,
                `SELECT_EXCLUDED entry ${name} no longer exists on the backend`,
            ).toContain(name);
        }
    });

    it('DEFAULT_*_SPEC mirror the backend schema defaults', () => {
        expect(DEFAULT_PRIMARY_SPEC).toEqual(
            scrapeDefaultSpec('default_hotkey'),
        );
        expect(DEFAULT_RECORD_ONLY_SPEC).toEqual(
            scrapeDefaultSpec('default_record_only_hotkey'),
        );
    });
});
