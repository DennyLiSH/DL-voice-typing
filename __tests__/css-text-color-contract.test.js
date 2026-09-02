import { readdirSync, readFileSync, statSync } from 'node:fs';
import { extname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

// Contract: text colors must go through the accessible -text token family
// (or neutral tokens), never raw semantic base colors or hardcoded chroma —
// small text on raw --success/--warning/--error/--accent fails WCAG AA.
// See the convention comment in common.css (:root, "-text family").
//
// When this test goes red on a legitimate new case, do NOT loosen the rules
// or whitelist to make it green silently: either fix the CSS (preferred) or
// add a reasoned entry to the exemption list below.

// Fixed dark-translucent windows that opt out of the theme system entirely —
// theme tokens would resolve to light-theme values under a light OS theme and
// get WORSE there.
const EXEMPT_FILES = new Set(['floating.css', 'review.css']);

// Neutral (non-chromatic) values allowed as text color.
const NEUTRAL_KEYWORDS = new Set(['transparent', 'inherit', 'currentColor']);

// Semantic base colors — for backgrounds/borders/decoration only, never text.
const RAW_SEMANTIC_VARS = new Set([
    'var(--success)',
    'var(--warning)',
    'var(--error)',
    'var(--accent)',
]);

const UI_DIR = fileURLToPath(new URL('../ui/', import.meta.url));

function listCssFiles(dir) {
    const out = [];
    for (const name of readdirSync(dir)) {
        const full = join(dir, name);
        if (statSync(full).isDirectory()) out.push(...listCssFiles(full));
        else if (extname(name) === '.css' && !EXEMPT_FILES.has(name))
            out.push(full);
    }
    return out;
}

// Anchored to the property name so background-color / border-top-color /
// etc. never match. A declaration starts at a rule boundary ({), a previous
// declaration (;), or line start; the property is `color` itself.
const COLOR_DECL_RE = /(?:^|[;{])[ \t]*color[ \t]*:[ \t]*([^;}]*)[ \t]*;/gm;

function isGrayscaleHex(hex) {
    let h = hex.slice(1);
    if (h.length === 3)
        h = h
            .split('')
            .map((c) => c + c)
            .join('');
    if (h.length !== 6 || /[^0-9a-fA-F]/.test(h)) return false;
    const r = h.slice(0, 2);
    const g = h.slice(2, 4);
    const b = h.slice(4, 6);
    return r === g && g === b;
}

/**
 * Classify one color value. Returns 'ok' or a violation reason.
 * Module-private; the self-check fixtures below guard the classifier itself
 * against rotting (which would make the scan vacuously green).
 */
function classifyTextColor(value) {
    const v = value.trim().toLowerCase();
    if (RAW_SEMANTIC_VARS.has(v)) return 'raw semantic base color as text';
    if (/^#[0-9a-f]{3,8}$/.test(v) && !isGrayscaleHex(v))
        return 'hardcoded chromatic hex as text';
    if (v.startsWith('rgba(') || v.startsWith('rgb(')) {
        const nums = v.match(/[\d.]+/g)?.map(Number) ?? [];
        const [r, g, b] = nums;
        // Grayscale rgba (r==g==b) is allowed (dimming helpers).
        if (r !== g || g !== b) return 'hardcoded chromatic rgba() as text';
        return 'ok';
    }
    if (v.startsWith('var(')) return 'ok'; // token reference (-text family etc.)
    if (NEUTRAL_KEYWORDS.has(v)) return 'ok';
    if (v.startsWith('#')) return 'ok'; // grayscale hex (checked above)
    return `named color "${value.trim()}" as text`;
}

function scanFiles() {
    const violations = [];
    let declCount = 0;
    for (const file of listCssFiles(UI_DIR)) {
        const name = file.slice(UI_DIR.length);
        const src = readFileSync(file, 'utf8');
        for (const m of src.matchAll(COLOR_DECL_RE)) {
            declCount += 1;
            const verdict = classifyTextColor(m[1]);
            if (verdict !== 'ok')
                violations.push(`${name}: color: ${m[1].trim()} — ${verdict}`);
        }
    }
    return { violations, declCount };
}

describe('css text-color contract', () => {
    it('self-check: classifier catches the violation classes it exists for', () => {
        // Rule 1 — raw semantic base colors.
        expect(classifyTextColor('var(--error)')).not.toBe('ok');
        expect(classifyTextColor('var(--accent)')).not.toBe('ok');
        // Rule 2 — chromatic literals (6-digit, 3-digit, named, rgba).
        expect(classifyTextColor('#ca8a04')).not.toBe('ok');
        expect(classifyTextColor('#F00')).not.toBe('ok');
        expect(classifyTextColor('white')).not.toBe('ok');
        expect(classifyTextColor('rgba(248, 113, 113, 0.95)')).not.toBe('ok');
        // Allowed: -text tokens, neutrals, grayscale hex (incl. 3-digit).
        expect(classifyTextColor('var(--error-text)')).toBe('ok');
        expect(classifyTextColor('transparent')).toBe('ok');
        expect(classifyTextColor('inherit')).toBe('ok');
        expect(classifyTextColor('#fff')).toBe('ok');
        expect(classifyTextColor('#808080')).toBe('ok');
        expect(classifyTextColor('#888')).toBe('ok');
        expect(classifyTextColor('rgba(255, 255, 255, 0.06)')).toBe('ok');
    });

    it('self-check: the anchor regex excludes -color suffixed properties', () => {
        const sample =
            '.x { border-top-color: var(--accent); background-color: var(--error); color: var(--error-text); }';
        const matches = [...sample.matchAll(COLOR_DECL_RE)].map((m) =>
            m[1].trim(),
        );
        expect(matches).toEqual(['var(--error-text)']);
    });

    it('no raw semantic color or chromatic literal used as text color', () => {
        const { violations } = scanFiles();
        expect(violations).toEqual([]);
    });

    it('scan is not vacuously green: finds ≥70 standalone color declarations', () => {
        // Baseline measured 77 (settings 49 + common 10 + transcribe 18).
        // If this drops hard, the regex rotted and the test above went green
        // for the wrong reason.
        const { declCount } = scanFiles();
        expect(declCount).toBeGreaterThanOrEqual(70);
    });
});
