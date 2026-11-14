// @vitest-environment jsdom
//
// floating-mode.js behavior tests + floating.js wiring static guard
// (Round 6 P6 Task 3).
//
// The mode slot is module state: transition and class-sync tests load a
// fresh module instance per test (and per matrix pair) via loadFresh so
// tests cannot leak into each other. The static guard follows the
// transcribe-controller wiring-surface precedent (transcribe-controller
// .test.js): a typo'd MODES reference or mode-machine call in
// ui/floating.js would only surface as a runtime TypeError or a silent
// no-op in the WebView, so it is pinned here instead.
import { describe, expect, it } from 'vitest';

import {
    currentMode,
    MODES,
    resetIndicatorInline,
    setMode,
} from '../ui/lib/floating-mode.js';
import { loadFresh, readSource } from './helpers.js';

const M = MODES;
const ALL_MODES = Object.values(M);
// Export-surface map for the wiring static guard (named imports — the
// guard's dynamic lookup must not be a dynamic namespace access).
const EXPORT_SURFACE = { currentMode, resetIndicatorInline, setMode };

// Legal paths from IDLE used to drive a fresh module's slot to each mode.
// Every step is a listed pair, so the drive itself never exercises a
// rejection.
const DRIVE_PATHS = {
    [M.IDLE]: [],
    [M.RECORDING]: [M.RECORDING],
    [M.PROCESSING]: [M.RECORDING, M.PROCESSING],
    [M.ERROR]: [M.ERROR],
    [M.RECORD_ONLY]: [M.RECORD_ONLY],
    [M.RECORD_ONLY_SAVED]: [M.RECORD_ONLY, M.RECORD_ONLY_SAVED],
    [M.RECORD_ONLY_DISCARDED]: [M.RECORD_ONLY, M.RECORD_ONLY_DISCARDED],
};

// The transition matrix. Duplicated here ON PURPOSE: this table is the
// pin — editing MODE_TRANSITIONS without updating it fails the tests
// below.
const LISTED_PAIRS = [
    // from IDLE
    [M.IDLE, M.RECORDING],
    [M.IDLE, M.RECORD_ONLY],
    [M.IDLE, M.ERROR],
    // from RECORDING
    [M.RECORDING, M.IDLE],
    [M.RECORDING, M.PROCESSING],
    [M.RECORDING, M.ERROR],
    [M.RECORDING, M.RECORD_ONLY],
    // from PROCESSING
    [M.PROCESSING, M.IDLE],
    [M.PROCESSING, M.RECORDING],
    [M.PROCESSING, M.RECORD_ONLY],
    [M.PROCESSING, M.ERROR],
    // from ERROR
    [M.ERROR, M.IDLE],
    [M.ERROR, M.RECORDING],
    [M.ERROR, M.RECORD_ONLY],
    // from RECORD_ONLY
    [M.RECORD_ONLY, M.IDLE],
    [M.RECORD_ONLY, M.RECORDING],
    [M.RECORD_ONLY, M.RECORD_ONLY_SAVED],
    [M.RECORD_ONLY, M.RECORD_ONLY_DISCARDED],
    [M.RECORD_ONLY, M.ERROR],
    // from RECORD_ONLY_SAVED
    [M.RECORD_ONLY_SAVED, M.IDLE],
    [M.RECORD_ONLY_SAVED, M.RECORDING],
    [M.RECORD_ONLY_SAVED, M.RECORD_ONLY],
    [M.RECORD_ONLY_SAVED, M.ERROR],
    // from RECORD_ONLY_DISCARDED
    [M.RECORD_ONLY_DISCARDED, M.IDLE],
    [M.RECORD_ONLY_DISCARDED, M.RECORDING],
    [M.RECORD_ONLY_DISCARDED, M.RECORD_ONLY],
    [M.RECORD_ONLY_DISCARDED, M.ERROR],
];

/** Fresh module + a detached div, slot driven to `mode` via legal steps. */
async function machineAt(mode) {
    const mod = await loadFresh('../ui/lib/floating-mode.js');
    const el = document.createElement('div');
    for (const step of DRIVE_PATHS[mode]) {
        expect(mod.setMode(el, step), `drive ${mode}`).toBe(true);
    }
    expect(mod.currentMode()).toBe(mode);
    return { mod, el };
}

describe('floating mode machine — transition matrix', () => {
    it('accepts every listed pair and updates currentMode()', async () => {
        for (const [from, to] of LISTED_PAIRS) {
            const { mod, el } = await machineAt(from);
            expect(mod.canTransition(from, to), `${from}->${to}`).toBe(true);
            expect(mod.setMode(el, to), `${from}->${to}`).toBe(true);
            expect(mod.currentMode()).toBe(to);
        }
    });

    it('rejects every unlisted non-self pair and leaves the slot untouched', async () => {
        const listed = new Set(LISTED_PAIRS.map(([a, b]) => `${a}->${b}`));
        const rejected = [];
        for (const from of ALL_MODES) {
            for (const to of ALL_MODES) {
                if (from === to) continue;
                if (!listed.has(`${from}->${to}`)) rejected.push([from, to]);
            }
        }
        // 49 ordered pairs − 27 listed − 7 self = 15 rejections. Pins that
        // the complement enumeration is not silently empty.
        expect(rejected.length).toBe(15);
        for (const [from, to] of rejected) {
            const { mod, el } = await machineAt(from);
            expect(mod.canTransition(from, to), `${from}->${to}`).toBe(false);
            expect(mod.setMode(el, to), `${from}->${to}`).toBe(false);
            expect(mod.currentMode()).toBe(from);
        }
    });

    it('rejects the named wiring-bug-surface pairs', async () => {
        // Each pair pins one documented rejection fact from the module doc
        // block (the brief names these five explicitly).
        const NAMED = [
            [M.IDLE, M.PROCESSING], // processing only follows a classic recording
            [M.RECORD_ONLY, M.PROCESSING], // pipelines are mutually exclusive backend-side
            [M.RECORD_ONLY_SAVED, M.RECORD_ONLY_DISCARDED], // one release classifies once, never both
            [M.IDLE, M.RECORD_ONLY_SAVED], // the substate only follows a record-only session
            [M.ERROR, M.PROCESSING], // the showProcessing settle-race guard
        ];
        for (const [from, to] of NAMED) {
            const { mod, el } = await machineAt(from);
            expect(mod.setMode(el, to), `${from}->${to}`).toBe(false);
            expect(mod.currentMode()).toBe(from);
        }
    });

    it('accepts self-transitions for every mode (no-op re-entry)', async () => {
        for (const mode of ALL_MODES) {
            const { mod, el } = await machineAt(mode);
            expect(mod.canTransition(mode, mode)).toBe(true);
            expect(mod.setMode(el, mode)).toBe(true);
            expect(mod.currentMode()).toBe(mode);
        }
    });

    it('unknown mode strings are rejected (either argument)', async () => {
        const mod = await loadFresh('../ui/lib/floating-mode.js');
        const el = document.createElement('div');
        expect(mod.canTransition('bogus', M.IDLE)).toBe(false);
        expect(mod.canTransition(M.IDLE, 'bogus')).toBe(false);
        expect(mod.canTransition('bogus', 'bogus')).toBe(false);
        // Prototype keys are not modes (same hazard lib/banner.js guards
        // with Map.get).
        expect(mod.canTransition('toString', M.IDLE)).toBe(false);
        expect(mod.canTransition(M.IDLE, 'constructor')).toBe(false);
        expect(mod.setMode(el, 'toString')).toBe(false);
        expect(mod.currentMode()).toBe(M.IDLE);
    });
});

describe('floating mode machine — inline cleanup matrix', () => {
    // Per-mode cleanup sets. Duplicated here ON PURPOSE: this table is the
    // pin — editing INLINE_CLEANUP without updating it fails the tests.
    const CLEANUP_SETS = {
        [M.IDLE]: ['filter'],
        [M.RECORDING]: ['filter'],
        [M.RECORD_ONLY]: ['background', 'boxShadow', 'transform', 'filter'],
        [M.PROCESSING]: ['background', 'boxShadow'],
        [M.ERROR]: ['background', 'boxShadow', 'filter'],
        [M.RECORD_ONLY_SAVED]: ['filter'],
        [M.RECORD_ONLY_DISCARDED]: ['filter'],
    };
    const ALL_PROPS = ['background', 'boxShadow', 'transform', 'filter'];

    function elWithAllInline() {
        const el = document.createElement('div');
        el.style.background = 'rgba(1, 2, 3, 0.5)';
        el.style.boxShadow = '0 4px 20px red';
        el.style.transform = 'scale(1.5)';
        el.style.filter = 'brightness(2)';
        return el;
    }

    it('clears exactly the mode set, leaving the other props untouched', async () => {
        const { resetIndicatorInline } = await loadFresh(
            '../ui/lib/floating-mode.js',
        );
        for (const mode of ALL_MODES) {
            const el = elWithAllInline();
            resetIndicatorInline(el, mode);
            const cleared = CLEANUP_SETS[mode];
            for (const prop of ALL_PROPS) {
                if (cleared.includes(prop)) {
                    expect(el.style[prop], `${mode}: ${prop}`).toBe('');
                } else {
                    // jsdom normalizes some values, so assert "still set"
                    // rather than byte-identical.
                    expect(el.style[prop], `${mode}: ${prop}`).not.toBe('');
                }
            }
        }
    });

    it('unknown mode throws before any mutation', async () => {
        const { resetIndicatorInline } = await loadFresh(
            '../ui/lib/floating-mode.js',
        );
        // 'toString' guards the prototype-key hazard: a naive truthiness
        // lookup would treat Object.prototype.toString as a cleanup set.
        for (const bogus of ['bogus', 'toString']) {
            const el = elWithAllInline();
            expect(() => resetIndicatorInline(el, bogus)).toThrow(
                /unknown mode/,
            );
            for (const prop of ALL_PROPS) {
                expect(el.style[prop]).not.toBe('');
            }
        }
    });
});

describe('floating mode machine — setMode class sync', () => {
    it('entering RECORD_ONLY adds the class; leaving removes it', async () => {
        const { mod, el } = await machineAt(M.IDLE);
        expect(el.classList.contains('record-only')).toBe(false);
        expect(mod.setMode(el, M.RECORD_ONLY)).toBe(true);
        expect(el.classList.contains('record-only')).toBe(true);
        expect(mod.setMode(el, M.RECORDING)).toBe(true);
        expect(el.classList.contains('record-only')).toBe(false);
    });

    it('a rejected setMode leaves the class untouched', async () => {
        const { mod, el } = await machineAt(M.IDLE);
        // Class present while the slot is IDLE — the rejection must not
        // remove it (no mutation on rejection).
        el.classList.add('record-only');
        expect(mod.setMode(el, M.PROCESSING)).toBe(false);
        expect(el.classList.contains('record-only')).toBe(true);
        // And the rejection must not add it either.
        const el2 = document.createElement('div');
        expect(mod.setMode(el2, M.RECORD_ONLY_SAVED)).toBe(false);
        expect(el2.classList.contains('record-only')).toBe(false);
    });
});

describe('floating.js wiring static guard', () => {
    const src = readSource('ui/floating.js');

    it('every MODES.X reference in floating.js is a real MODES member', () => {
        const refs = [...src.matchAll(/MODES\.([A-Za-z_]+)/g)].map((m) => m[1]);
        expect(refs.length).toBeGreaterThan(0);
        // Member NAMES (keys), not string values: a typo'd or value-form
        // reference (MODES.record_only) is undefined at runtime and would
        // degrade into a silent no-op transition.
        const members = new Set(Object.keys(MODES));
        const unknown = [...new Set(refs)].filter((name) => !members.has(name));
        expect(unknown).toEqual([]);
    });

    it('every mode-machine function floating.js calls is on the export surface', () => {
        for (const fn of ['setMode', 'resetIndicatorInline', 'currentMode']) {
            const used = new RegExp(`\\b${fn}\\(`).test(src);
            expect(used, `${fn} should be called in ui/floating.js`).toBe(true);
            expect(
                typeof EXPORT_SURFACE[fn],
                `${fn} should be exported by floating-mode.js`,
            ).toBe('function');
        }
    });

    it('floating.js imports the mode module', () => {
        expect(src).toContain("from './lib/floating-mode.js'");
    });

    it('floating.js contains zero record-only class sniffs', () => {
        expect(src).not.toMatch(/classList\.contains\('record-only'\)/);
    });
});
