/**
 * Contract: every command name the frontend invokes must be registered in
 * lib.rs `generate_handler![..]`. The `get_default_config` incident
 * (460efda shipped, 9be49f3 hot-fixed) shipped a called-but-unregistered
 * command that only failed at runtime ("Command not found") — no
 * compile-time guard caught it. This test pins both sides so a future
 * add-on must register before invoke lands.
 *
 * Boundary:
 * - `plugin:...` prefixed calls go through tauri-plugin-opener and are
 *   governed by capabilities, not generate_handler — excluded.
 * - Dynamically composed command names are invisible to this scrape;
 *   none exist today. If you must compose one, keep the literal here.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { extname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const LIB_RS = readFileSync(
    new URL('../src-tauri/src/lib.rs', import.meta.url),
    'utf8',
);
const UI_DIR = fileURLToPath(new URL('../ui/', import.meta.url));

function registeredCommands() {
    const block = LIB_RS.match(
        /generate_handler!\[\s*([\s\S]*?)\s*\]\s*\)\s*\n?\.on_window_event/,
    );
    if (!block)
        throw new Error('scrape failed: generate_handler block not found');
    // Strip Rust line comments so a commented-out entry doesn't survive
    // the scrape (and silently stay green).
    const body = block[1]
        .split('\n')
        .map((line) => line.replace(/\/\/.*$/, ''))
        .join('\n');
    const names = [...body.matchAll(/commands::(\w+)::(\w+)/g)].map(
        (m) => m[2],
    );
    if (names.length === 0)
        throw new Error('scrape failed: generate_handler block empty');
    return new Set(names);
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

function invokedCommands() {
    // call/rawInvoke are imported then called as bare names in ui/ (no
    // method-call form like `obj.call(...)`). \b anchors match the bare
    // function-call form; a leading `\.` would zero-match.
    const names = new Set();
    for (const file of listJsFiles(UI_DIR)) {
        const src = readFileSync(file, 'utf8');
        for (const m of src.matchAll(/\b(call|rawInvoke)\(\s*'([^']+)'/g)) {
            const cmd = m[2];
            if (cmd.startsWith('plugin:')) continue;
            names.add(cmd);
        }
    }
    if (names.size === 0)
        throw new Error('scrape failed: no ui/ invoke sites found');
    return names;
}

// Commands registered but never invoked by the frontend. Each entry must
// carry a real reason — listed by Step 1.2 evidence (grep + git log -S
// confirm zero ui/ consumers). The reverse check below catches stale
// allowlist entries that drift on.
const ALLOWED_REGISTERED_BUT_UNINVOKED = new Set([
    // Backend perf history command; perf is exposed via tray tooltip /
    // state, no settings UI surface yet. (perf_cmd.rs:7, registered
    // lib.rs:166 — never called from ui/.)
    'get_perf_history',
    // Backend data-usage command (MB total for saved recordings); no
    // frontend consumer wired today. (data_management_cmd.rs:872,
    // registered lib.rs:171 — never called from ui/.)
    'get_data_usage',
    // Backend open-transcribe-window command; the frontend never invokes
    // it directly — tray menu (tray.rs:151) and the open-window hotkey
    // (open_window_hotkey.rs:35) open the window themselves.
    // (transcribe_cmd.rs:66, registered lib.rs:176.)
    'open_transcribe_window',
]);

describe('invoke registry contract', () => {
    it('every frontend-invoked command is registered in generate_handler', () => {
        const registered = registeredCommands();
        const invoked = invokedCommands();
        const missing = [...invoked].filter((n) => !registered.has(n));
        expect(missing).toEqual([]);
    });

    it('registered-but-never-invoked commands match the explicit allowlist', () => {
        const registered = registeredCommands();
        const invoked = invokedCommands();
        const dead = [...registered].filter(
            (n) => !invoked.has(n) && !ALLOWED_REGISTERED_BUT_UNINVOKED.has(n),
        );
        expect(dead).toEqual([]);
    });

    it('allowlist has no stale entries (commands now invoked from frontend)', () => {
        const invoked = invokedCommands();
        const stale = [...ALLOWED_REGISTERED_BUT_UNINVOKED].filter((n) =>
            invoked.has(n),
        );
        expect(stale).toEqual([]);
    });
});
