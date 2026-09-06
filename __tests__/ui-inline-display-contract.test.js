import { readdirSync, readFileSync, statSync } from 'node:fs';
import { extname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

// Contract: HTML elements whose visibility is controlled by a CSS class
// (`.visible` / `.hidden` rules) must not carry an inline `style="...display..."`.
// An inline display declaration beats any class rule in specificity, silently
// disabling the class-based toggle — the settings error-banner shipped broken
// for this reason (2026-09-04 Nielsen critique P0): showError() added
// `.visible { display: block }` but the inline `display:none` won forever.
//
// Rule is intentionally WIDE: any `style="...display..."` in ui/*.html is a
// violation unless the element id is explicitly exempted below. When this test
// goes red on a legitimate new case, do NOT loosen the rule: either drop the
// inline style (preferred) or add a reasoned exemption entry.

// model-manager.js toggles these three via direct `el.style.display = ...`
// assignments on both sides (show AND hide) — no CSS class visibility rule
// exists for them, so the inline style is not fighting a class toggle.
const EXEMPT_IDS = new Set([
    'model-status-text',
    'btn-download-model',
    'download-progress',
]);

const UI_DIR = fileURLToPath(new URL('../ui/', import.meta.url));

function listHtmlFiles(dir) {
    const out = [];
    for (const name of readdirSync(dir)) {
        const full = join(dir, name);
        if (statSync(full).isDirectory()) out.push(...listHtmlFiles(full));
        else if (extname(name) === '.html') out.push(full);
    }
    return out;
}

// Matches an opening tag carrying a style attribute whose value contains a
// display declaration, and extracts the element's id (if any).
const INLINE_DISPLAY_RE =
    /<([a-z][a-z0-9-]*)\b([^>]*\bstyle\s*=\s*"[^"]*\bdisplay\b[^"]*"[^>]*)>/gi;

function extractId(attrText) {
    const m = attrText.match(/\bid\s*=\s*"([^"]*)"/);
    return m ? m[1] : null;
}

function scanFiles() {
    const violations = [];
    let styleCount = 0;
    for (const file of listHtmlFiles(UI_DIR)) {
        const name = file.slice(UI_DIR.length);
        const src = readFileSync(file, 'utf8');
        for (const m of src.matchAll(INLINE_DISPLAY_RE)) {
            styleCount += 1;
            const id = extractId(m[2]);
            if (!id || !EXEMPT_IDS.has(id)) {
                violations.push(
                    `${name}: <${m[1]}${id ? ` id="${id}"` : ''}> has inline style containing "display"`,
                );
            }
        }
    }
    return { violations, styleCount };
}

describe('ui inline display contract', () => {
    it('self-check: regex catches a violating tag and its id', () => {
        const sample =
            '<div class="error-banner" id="error-banner" style="display:none" role="alert"></div>';
        const matches = [...sample.matchAll(INLINE_DISPLAY_RE)];
        expect(matches).toHaveLength(1);
        expect(extractId(matches[0][2])).toBe('error-banner');
    });

    it('self-check: regex does not match style without display, nor <style> blocks', () => {
        const sample =
            '<div id="a" style="color:red"></div>\n<style>.b { display:none }</style>';
        expect([...sample.matchAll(INLINE_DISPLAY_RE)]).toHaveLength(0);
    });

    it('no inline style containing display outside the exemption list', () => {
        const { violations } = scanFiles();
        expect(violations).toEqual([]);
    });

    it('scan is not vacuously green: finds ≥3 inline display styles (the exemptions)', () => {
        // Baseline: exactly the 3 exempted model-manager elements carry inline
        // display. If this drops to 0 the regex rotted and the test above went
        // green for the wrong reason.
        const { styleCount } = scanFiles();
        expect(styleCount).toBeGreaterThanOrEqual(3);
    });
});
