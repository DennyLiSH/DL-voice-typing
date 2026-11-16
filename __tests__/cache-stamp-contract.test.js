// @vitest-environment node
//
// Cache-bust stamp contract (Round 6 P10 Task 5c).
//
// WHAT THIS PINS: every static resource referenced by a window HTML file
// (the <link rel="stylesheet"> tags and the entry <script>) carries a
// `?v=` query, and ALL FOUR window files carry the SAME value.
//
// WHY: the installed app serves ui/ through Tauri's asset protocol, which
// sends no Cache-Control header — the WebView2 HTTP cache then happily
// serves the PREVIOUS version of a stylesheet or script across an app
// update. The `?v=` stamp is the cache-bust; the shared value is what
// makes "bump the stamp" a single, reviewable step. This started as a
// production incident fix (review.js?v=8 shipped alone so EXISTING users
// would pull the corrected HTML) and is now uniform across the four
// windows.
//
// KNOWN GAP (not covered here): the entry <script>'s ES-module static
// imports (settings.js -> app-shell.js / settings-form.js / lib/*.js) carry
// no stamp, so the module graph below the entry point is not cache-busted.
// This is a pre-existing hole, deliberately left open by the upper-level
// decision; the contract below only covers the HTML-level references.
//
// The "every window file is covered" assertion uses listUiHtml() from
// helpers.js — the suite's single source for filesystem reads — so a
// newly added window cannot slip in without a stamp.

import { describe, expect, it } from 'vitest';

import { listUiHtml, readSource } from './helpers.js';

/**
 * The four window HTML files, spelled out (the stamp contract's subject).
 * Sorted like listUiHtml()'s output so the coverage assertion below is a
 * direct equality check.
 */
const WINDOW_HTML = [
    'ui/floating.html',
    'ui/review.html',
    'ui/settings.html',
    'ui/transcribe.html',
];

/**
 * Static resource references in an HTML file: the stylesheet <link href>
 * and the entry <script src>. The value may or may not carry a `?v=`
 * stamp — the contract decides which it must be.
 */
function resourceRefs(html) {
    const refs = [];
    const linkRe = /<link\b[^>]*\bhref="([^"]+)"/g;
    const scriptRe = /<script\b[^>]*\bsrc="([^"]+)"/g;
    for (const re of [linkRe, scriptRe]) {
        for (const m of html.matchAll(re)) {
            refs.push(m[1]);
        }
    }
    return refs;
}

/** The `?v=` value of a reference, or null when it carries no stamp. */
function stampOf(ref) {
    const m = /\?v=([^&]+)$/.exec(ref);
    return m ? m[1] : null;
}

/**
 * The HTML comment block that documents the cache-bust convention in one
 * window file.
 *
 * Extraction is by CONTENT, never by position: every HTML comment in the
 * file is scanned and the ones mentioning the stamp parameter itself
 * (`?v=`) are kept. That keeps this independent of the comment's exact
 * wording and of where the comment sits — settings.html carries 19
 * comments and the cache-bust one is only the first by accident.
 *
 * @param {string} html the window file's source
 * @param {string} rel repo-root-relative path, for error messages
 * @returns {string} the comment block, byte for byte
 */
function cacheBustComment(html, rel) {
    const comments = [...html.matchAll(/<!--[\s\S]*?-->/g)].map((m) => m[0]);
    const hits = comments.filter((c) => c.includes('?v='));
    expect(
        hits.length,
        `${rel}: expected exactly ONE HTML comment documenting the cache-bust stamp (?v=), found ${hits.length} — the four window files' comment blocks are compared byte for byte, so the block must stay unique and identifiable`,
    ).toBe(1);
    return hits[0];
}

describe('cache-bust stamp contract', () => {
    it('every window HTML file is covered by the contract', () => {
        // A NEW window file must not slip in unstamped: the contract's
        // subject set is derived from the ui/ directory listing.
        expect(
            listUiHtml(),
            'a new ui/*.html window was added: add it to WINDOW_HTML above AND give every one of its static resource references the shared ?v= stamp (otherwise the installed WebView keeps serving the stale file)',
        ).toEqual(WINDOW_HTML);
    });

    it.each(WINDOW_HTML)(
        '%s: every static resource reference is stamped',
        (rel) => {
            const html = readSource(rel);
            const refs = resourceRefs(html);
            // Guard against an empty scrape letting the assertions below pass
            // against a gutted file.
            expect(refs.length).toBeGreaterThanOrEqual(2);
            for (const ref of refs) {
                expect(
                    stampOf(ref),
                    `${rel}: ${ref} has no ?v= stamp`,
                ).not.toBeNull();
            }
        },
    );

    it.each(WINDOW_HTML)('%s: references a stylesheet and a script', (rel) => {
        const html = readSource(rel);
        const refs = resourceRefs(html);
        expect(refs.some((r) => r.includes('.css'))).toBe(true);
        expect(refs.some((r) => r.includes('.js'))).toBe(true);
    });

    it('all four windows share ONE stamp value', () => {
        const stamps = new Set();
        for (const rel of WINDOW_HTML) {
            for (const ref of resourceRefs(readSource(rel))) {
                const stamp = stampOf(ref);
                expect(stamp, `${rel}: ${ref} has no ?v= stamp`).not.toBeNull();
                stamps.add(stamp);
            }
        }
        // One value across all four files — bumping it is a single,
        // reviewable step (the point of the shared stamp).
        expect([...stamps]).toHaveLength(1);
    });

    it('all four windows carry the SAME cache-bust comment block', () => {
        // The stamp VALUE is pinned above, but nothing pinned the four
        // copies of the comment that explains the convention — so the
        // explanations could drift apart silently (e.g. the "known gap"
        // note describing lib/*.js only in one of the four files). The
        // blocks are documentation of one shared convention, so they must
        // stay byte-identical.
        const blocks = WINDOW_HTML.map((rel) => ({
            rel,
            block: cacheBustComment(readSource(rel), rel),
        }));
        expect(
            new Set(blocks.map((b) => b.block)).size,
            `the cache-bust comment block drifted apart between the window files — keep the four copies byte-identical:\n${blocks
                .map((b) => `--- ${b.rel}\n${b.block}`)
                .join('\n')}`,
        ).toBe(1);
    });
});
