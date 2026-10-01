/**
 * Contract: backend `RECORDED_EVENTS` (error_history.rs) and frontend
 * `CHANNEL_LABELS` (app-shell.js help page) must list the same set of
 * error event names. Drift either way:
 * - backend adds an event but frontend forgets → help page falls back to
 *   the raw event name (`CHANNEL_LABELS[record.event] || record.event`).
 * - frontend adds a label but backend never emits → dead UI label.
 */
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const RS = readFileSync(
    new URL('../src-tauri/src/commands/error_history.rs', import.meta.url),
    'utf8',
);
const APP_SHELL = readFileSync(
    new URL('../ui/app-shell.js', import.meta.url),
    'utf8',
);

function backendEvents() {
    const m = RS.match(
        /const RECORDED_EVENTS:\s*\[&str;\s*\d+\]\s*=\s*\[([\s\S]*?)\];/,
    );
    if (!m) throw new Error('RECORDED_EVENTS not found in error_history.rs');
    const words = [...m[1].matchAll(/"([^"]+)"/g)].map((x) => x[1]);
    if (words.length === 0)
        throw new Error('scrape failed: RECORDED_EVENTS empty');
    return words.slice().sort();
}

function frontendKeys() {
    const m = APP_SHELL.match(/const CHANNEL_LABELS\s*=\s*\{([\s\S]*?)\};/);
    if (!m) throw new Error('CHANNEL_LABELS not found in app-shell.js');
    const keys = [...m[1].matchAll(/'([^']+)'\s*:/g)].map((x) => x[1]);
    if (keys.length === 0)
        throw new Error('scrape failed: CHANNEL_LABELS empty');
    return keys.slice().sort();
}

describe('error channel twin-table contract', () => {
    it('RECORDED_EVENTS === CHANNEL_LABELS keys (both directions)', () => {
        expect(backendEvents()).toEqual(frontendKeys());
    });
});
