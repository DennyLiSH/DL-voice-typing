/**
 * Contract test: the settings field list has ONE authority —
 * ui/lib/settings-schema.js :: SETTINGS_FIELDS. settings-form.js's
 * DOM_DEFS must wire exactly those keys (no missing wiring, no dead
 * wiring). Follows the text-scrape precedent of
 * undo-window-contract.test.js (settings-form.js touches DOM at module
 * load, so it cannot be imported in a pure test environment).
 */
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { MASKED_MARKER } from '../ui/lib/api-key-mask.js';
import { SETTINGS_FIELDS } from '../ui/lib/settings-schema.js';

const SRC = readFileSync(
    new URL('../ui/settings-form.js', import.meta.url),
    'utf8',
);

function scrapeDomDefsKeys() {
    const start = SRC.indexOf('const DOM_DEFS = {');
    expect(
        start,
        'DOM_DEFS block not found in settings-form.js',
    ).toBeGreaterThanOrEqual(0);
    const end = SRC.indexOf('};', start);
    const block = SRC.slice(start, end);
    return [...block.matchAll(/^ {4}([a-z_0-9]+): \{$/gm)].map((m) => m[1]);
}

describe('settings field list single-source contract', () => {
    it('DOM_DEFS keys exactly match SETTINGS_FIELDS keys', () => {
        const domKeys = scrapeDomDefsKeys();
        const schemaKeys = SETTINGS_FIELDS.map((f) => f.key);
        expect([...new Set(domKeys)].sort()).toEqual(
            [...new Set(schemaKeys)].sort(),
        );
        expect(domKeys.length, 'no duplicate DOM_DEFS keys').toBe(
            new Set(domKeys).size,
        );
    });

    it('custom equal comparators follow the unchanged-predicate contract (reflexive)', async () => {
        // Locks the equal-slot POLARITY: equal(v, v) must be true ("unchanged").
        // A dirty-predicate pasted into the equal slot (the Iteration 1 review's
        // S4-M1 bug class) fails reflexivity for hotkey objects and fails the
        // masked-key identical case — either way this test goes red.
        const { isConfigDirty } = await import('../ui/lib/settings-utils.js');
        for (const { key, equal } of SETTINGS_FIELDS) {
            if (!equal) continue;
            expect(
                equal,
                `${key}.equal must be an unchanged predicate`,
            ).toBeTypeOf('function');
        }
        // Identical configs (including masked key + hotkey objects) are never dirty.
        const cfg = Object.fromEntries(
            SETTINGS_FIELDS.map((f) => [f.key, 'x']),
        );
        cfg.llm_api_key = MASKED_MARKER;
        cfg.hotkey = { ctrl: true, shift: false, alt: false, vk: 65 };
        cfg.record_only_hotkey = {
            ctrl: false,
            shift: false,
            alt: true,
            vk: 66,
        };
        expect(isConfigDirty(cfg, structuredClone(cfg))).toBe(false);
    });

    it('object-valued fields must carry a custom equal (reference-equality trap)', () => {
        // Shallow !== is always true on objects — a field whose value is an
        // object MUST NOT rely on the default comparator (hotkey specs hit
        // this trap before 2026-09-06). Known object fields today: the two
        // hotkey specs. If a NEW object-valued field is added without equal,
        // the field lists here must be updated — and this test reminds you.
        const objectFieldsWithEqual = ['hotkey', 'record_only_hotkey'];
        for (const key of objectFieldsWithEqual) {
            const entry = SETTINGS_FIELDS.find((f) => f.key === key);
            expect(entry, `${key} must exist in SETTINGS_FIELDS`).toBeDefined();
            expect(
                entry.equal,
                `${key} is object-valued and needs a custom equal`,
            ).toBeTypeOf('function');
        }
    });
});
