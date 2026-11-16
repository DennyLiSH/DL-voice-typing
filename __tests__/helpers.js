/**
 * Shared frontend test infrastructure (Round 6 P4 Task 1).
 *
 * One module for the cross-test plumbing that used to be inlined per
 * contract test (readSource/blockOf) or per orchestration test
 * (stubTauri/loadFresh). Round 6 P4 Tasks 2-4 landed the consolidation:
 * the 4 contract tests' inline `read` and 2 inline `blockOf` variants
 * now import from here, the settings fixtures load the real HTML via
 * loadSettingsDom(), and stubTauri is the suite's default __TAURI__
 * stub path (helpers.js holds the only vi.stubGlobal('__TAURI__') call
 * site in __tests__/).
 *
 * Conventions pinned here:
 * - readSource paths are REPO-ROOT-relative ('ui/settings.html' reads
 *   <repo>/ui/settings.html), not __tests__-relative — helpers.js sits
 *   in __tests__/, so '../' is the repo root.
 * - blockOf throws on a missing start OR end marker (a silent empty
 *   slice would let a contract test pass against a renamed or deleted
 *   selector) and includes the end marker in the returned slice; the
 *   endMarker defaults to '}' so the existing selector-only call sites
 *   consolidate mechanically.
 * - stubTauri is a thin wrapper, not a framework: defaults cover the
 *   dominant shape observed in __tests__/ (core.invoke + event.listen —
 *   the default listen resolves to an unlisten function, the
 *   Promise<UnlistenFn> shape ui/lib/events.js::subscribe awaits). Raw
 *   window.__TAURI__ access is contract-pinned to ui/lib/events.js,
 *   whose guarded listen survives a missing event key instead of
 *   throwing at import time; settings-specific keys (app/autostart/
 *   dialog) are overrides, not defaults.
 * - loadSettingsDom reads the REAL ui/settings.html and strips
 *   <script>/<link> tags — jsdom must not re-run the entry scripts or
 *   fetch the stylesheets. Its strip regexes match ACROSS HTML comments,
 *   so a literal script/link tag inside a comment in ui/settings.html
 *   deletes the whole body — see its own docblock below.
 */

import { readdirSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { vi } from 'vitest';

/**
 * Directory of this module — the `../` anchor that makes readSource
 * repo-root-relative.
 *
 * Deliberately NOT the ui-copy-contract `new URL(`../${rel}`,
 * import.meta.url)` shape: under vitest's jsdom environment (vite 8.3.2)
 * vite's asset-URL plugin rewrites new URL(..., import.meta.url) call
 * sites, and a dynamic template's first argument becomes undefined for
 * non-asset paths (observed 2026-11-14, vitest 5.0.2 — node-env contract
 * tests are unaffected). helpers.js is the single owner of this
 * resolve-based shape: it was jsdom-proven here first, and the read+strip
 * logic settings.reset-defaults.test.js used to inline was consolidated
 * into loadSettingsDom() in Round 6 P4 (Tasks 1/3) — that file now
 * delegates to the helper.
 */
const HELPERS_DIR = dirname(fileURLToPath(import.meta.url));

/**
 * Read a repo file as UTF-8 text. `rel` is repo-root-relative:
 * readSource('ui/settings.html') reads <repo>/ui/settings.html.
 * @param {string} rel
 * @returns {string} file contents
 */
export function readSource(rel) {
    return readFileSync(resolve(HELPERS_DIR, '..', rel), 'utf8');
}

/**
 * List the window HTML files in ui/ (sorted, repo-root-relative paths).
 *
 * The cache-stamp contract test needs to know EVERY window HTML file, so
 * a newly added window cannot slip in without the shared `?v=` stamp.
 * helpers.js is the suite's single source for filesystem reads — the test
 * must not open its own readdirSync.
 *
 * @returns {string[]} e.g. ['ui/floating.html', 'ui/review.html', ...]
 */
export function listUiHtml() {
    return readdirSync(resolve(HELPERS_DIR, '..', 'ui'))
        .filter((f) => f.endsWith('.html'))
        .sort()
        .map((f) => `ui/${f}`);
}

/**
 * Extract one closed block from `src`: from `startMarker` up to and
 * including `endMarker` (default '}': the first closing brace after the
 * start — the selector-only shape used by the CSS contract tests).
 * Throws when either marker is missing.
 * @param {string} src
 * @param {string} startMarker
 * @param {string} [endMarker='}']
 * @returns {string} the block, including the end marker
 */
export function blockOf(src, startMarker, endMarker = '}') {
    const start = src.indexOf(startMarker);
    if (start === -1) {
        throw new Error(`blockOf: start marker not found: ${startMarker}`);
    }
    const end = src.indexOf(endMarker, start);
    if (end === -1) {
        throw new Error(`blockOf: end marker not found: ${endMarker}`);
    }
    return src.slice(start, end + endMarker.length);
}

/**
 * Stub the global `__TAURI__` object and return the stubbed handle.
 *
 * The suite's default __TAURI__ stub path (Round 6 P4 Task 4 sweep):
 * this helper holds the ONLY vi.stubGlobal('__TAURI__') call site in
 * __tests__/ — plain literal sites convert mechanically (overrides
 * shallow-merge, so a full top-level key like `event` replaces the
 * default); raw stubGlobal survives only for non-__TAURI__ globals
 * (requestAnimationFrame & co, out of scope).
 *
 * Defaults cover the dominant stub shape in __tests__/ (surveyed
 * 2026-11-14, 13 files): `core.invoke` and `event.listen`. The default
 * listen resolves to an unlisten function — the Promise<UnlistenFn>
 * shape events.js::subscribe awaits; the default invoke is a bare
 * vi.fn() the test reconfigures per command. `overrides` shallow-merge
 * over the defaults: a key in overrides replaces the whole top-level
 * value. Calling repeatedly is safe — each call builds a fresh object
 * and re-stubs; the latest handle wins.
 * @param {Record<string, unknown>} [overrides]
 * @returns {Record<string, unknown>} the stubbed handle
 */
export function stubTauri(overrides = {}) {
    const handle = {
        core: { invoke: vi.fn() },
        event: { listen: vi.fn(() => Promise.resolve(() => {})) },
        ...overrides,
    };
    vi.stubGlobal('__TAURI__', handle);
    return handle;
}

/**
 * Reset the vitest module registry, then dynamically import
 * `modulePath` and return its (fresh) module namespace. Path
 * convention: relative specifiers as used by the tests
 * ('../ui/lib/events.js') — helpers.js sits in __tests__/ next to its
 * callers, so both resolve identically.
 * @param {string} modulePath
 * @returns {Promise<object>} the freshly evaluated module namespace
 */
export async function loadFresh(modulePath) {
    vi.resetModules();
    return import(modulePath);
}

/**
 * Load the REAL ui/settings.html into the jsdom document body.
 *
 * <script> and <link> tags are stripped with the same pattern as
 * settings.reset-defaults.test.js — jsdom must not re-run the entry
 * scripts or fetch the stylesheets. The calling test file must run
 * under the jsdom environment (`// @vitest-environment jsdom`).
 *
 * HAZARD — the strip regexes match ACROSS HTML comments:
 *   `.replace(/<script[\s\S]*?<\/script>/g, '')`
 *   `.replace(/<link[^>]*>/g, '')`
 * A literal script tag (or link tag) written INSIDE A COMMENT in
 * ui/settings.html starts a match that runs to the next real closing
 * script tag, so the whole `<body>` is deleted and the jsdom suite dies
 * with null-element errors (observed 2026-11-16: ~107 failing tests,
 * first symptom `whisperModelSelect.addEventListener` on null in
 * ui/model-manager.js). Comment prose must therefore avoid those tag
 * literals — say "script tag" / "link tag" instead. The shipped
 * ui/*.html comments follow this rule (the cache-bust stamp comments in
 * particular). Fixing the stripping itself (parse real tags instead of
 * regex-matching) would change this helper's behavior and is out of
 * scope here — the hazard is documented instead.
 */
export function loadSettingsDom() {
    const html = readSource('ui/settings.html')
        .replace(/<script[\s\S]*?<\/script>/g, '')
        .replace(/<link[^>]*>/g, '');
    document.body.innerHTML = html;
}
