/**
 * Contract: the frontend event registry must match the backend's emit
 * sites.
 *
 * The same class of defect the invoke-registry contract test kills on the
 * command side: a `listen('recording-strat')` typo fails SILENTLY —
 * Tauri does not warn about a listener for an event that is never
 * emitted — so nothing but a two-sided scrape can catch it.
 *
 * The four assertions:
 *
 * 1. every `EVENTS.<key>` reference in ui/ resolves to a key of the
 *    table (catches a typo in the KEY),
 * 2. the table's VALUES are exactly the event names the Rust source
 *    emits (catches a typo in the VALUE, a missing event and a stale
 *    entry),
 * 3. no bare `listen('…')` literal survives in ui/ (catches a bypass of
 *    the registry),
 * 4. raw `window.__TAURI__.event` access appears only in lib/events.js
 *    (its guarded `listen`/`subscribe` are the sole owner — a window
 *    file touching the global directly crashes at module load when the
 *    global is missing).
 *
 * Boundary:
 * - the scrape skips `src-tauri/src/tests/` — the integration tests
 *   emit their own fixture names (`test-event`, `another`), which are
 *   not part of the event namespace;
 * - the `__TAURI__.event` pin matches the direct member-access forms
 *   (dotted and `?.`); a bracket (`window['__TAURI__'].event`) or an
 *   aliased indirection would evade it — the same class of documented
 *   blind spot the ui-import boundary tests carry;
 * - the `RecordingEmitter` decorator forwards a DYNAMIC event name
 *   (`self.inner.emit(event, payload)`) — there is no literal to
 *   scrape, and the forwarded name is one of the registered ones by
 *   construction;
 * - the inline `#[cfg(test)]` modules inside production files are
 *   scraped like production code: they emit real event names.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { extname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { EVENTS } from '../ui/lib/events.js';

const SRC_DIR = fileURLToPath(new URL('../src-tauri/src/', import.meta.url));
const UI_DIR = fileURLToPath(new URL('../ui/', import.meta.url));
const TESTS_DIR = join(SRC_DIR, 'tests');

function listRsFiles(dir) {
    const out = [];
    for (const name of readdirSync(dir)) {
        const full = join(dir, name);
        if (statSync(full).isDirectory()) out.push(...listRsFiles(full));
        else if (extname(name) === '.rs') out.push(full);
    }
    return out;
}

function listJsFiles(dir) {
    const out = [];
    for (const name of readdirSync(dir)) {
        const full = join(dir, name);
        if (statSync(full).isDirectory()) out.push(...listJsFiles(full));
        else if (extname(name) === '.js') out.push(full);
    }
    return out;
}

/**
 * Every event name the Rust source emits.
 *
 * The scrape is multi-line aware: Tauri emit calls usually put the name
 * on the line AFTER `emit(`, so a single-line regex would miss most of
 * them. The call's argument text is extracted by balancing the
 * parentheses, and the first string literal inside it is the event
 * name.
 */
function emittedEventNames() {
    const names = new Set();
    for (const file of listRsFiles(SRC_DIR)) {
        // The integration tests emit their own fixture names.
        if (file.startsWith(TESTS_DIR)) continue;
        const text = readFileSync(file, 'utf8');
        for (const m of text.matchAll(/\.emit\(/g)) {
            const start = m.index + m[0].length;
            let depth = 1;
            let end = text.length;
            for (let i = start; i < text.length; i++) {
                const ch = text[i];
                if (ch === '(') depth++;
                else if (ch === ')') {
                    depth--;
                    if (depth === 0) {
                        end = i;
                        break;
                    }
                }
            }
            const args = text.slice(start, end);
            const literal = args.match(/"([a-z][a-z0-9_-]*)"/);
            if (literal) names.add(literal[1]);
        }
    }
    return names;
}

describe('event registry contract', () => {
    it('every EVENTS key referenced in ui/ exists in the table', () => {
        const keys = new Set(Object.keys(EVENTS));
        const missing = [];
        for (const file of listJsFiles(UI_DIR)) {
            const text = readFileSync(file, 'utf8');
            for (const m of text.matchAll(/\bEVENTS\.(\w+)/g)) {
                if (!keys.has(m[1])) {
                    missing.push(`${relative(UI_DIR, file)}: EVENTS.${m[1]}`);
                }
            }
        }
        expect(missing).toEqual([]);
    });

    it('the table values are exactly the events the backend emits', () => {
        const emitted = emittedEventNames();
        // Check-face: a broken scrape (wrong directory, regex drift) would
        // otherwise compare an empty set against an empty table.
        expect(emitted.size).toBeGreaterThanOrEqual(20);

        const table = new Set(Object.values(EVENTS));
        const notEmitted = [...table].filter((n) => !emitted.has(n)).sort();
        const notListed = [...emitted].filter((n) => !table.has(n)).sort();

        expect({ notEmitted, notListed }).toEqual({
            notEmitted: [],
            notListed: [],
        });
    });

    it("no bare listen('…') literal survives in ui/", () => {
        // A literal is a bypass of the registry — and a typo in it fails
        // silently at runtime.
        const offenders = [];
        for (const file of listJsFiles(UI_DIR)) {
            const text = readFileSync(file, 'utf8');
            if (/listen\(\s*'/.test(text)) {
                offenders.push(relative(UI_DIR, file));
            }
        }
        expect(offenders).toEqual([]);
    });

    it('raw __TAURI__.event access lives only in lib/events.js', () => {
        // The guarded `listen`/`subscribe` in lib/events.js own the raw
        // global access; a window file touching `__TAURI__.event`
        // directly (e.g. `const { listen } = window.__TAURI__.event;`)
        // crashed at module load when the global was missing. The
        // optional chaining in events.js's own guard must not slip the
        // pin, so the regex matches both the dotted and the `?.` form;
        // matchAll (not a single .test) reports every offending line.
        const offenders = [];
        let ownerHits = 0;
        for (const file of listJsFiles(UI_DIR)) {
            const text = readFileSync(file, 'utf8');
            for (const m of text.matchAll(/window\.__TAURI__\??\.event\b/g)) {
                const rel = relative(UI_DIR, file).replace(/\\/g, '/');
                if (rel === 'lib/events.js') {
                    ownerHits++;
                    continue;
                }
                const line = text.slice(0, m.index).split('\n').length;
                offenders.push(`${rel}:${line}`);
            }
        }
        // Check-face: a broken scrape (wrong directory, regex drift) or
        // a moved owner file must not reduce the pin to a vacuous pass.
        expect(ownerHits).toBeGreaterThanOrEqual(1);
        expect(offenders).toEqual([]);
    });
});
