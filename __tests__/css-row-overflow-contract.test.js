import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

// Contract: the recording-row preview must never declare a min-width floor
// greater than 0. A floor only engages exactly when the fixed siblings
// (checkbox / timestamp / source badge / duration / play / delete) have
// already filled the row budget — at that point the floor manufactures
// horizontal overflow (#data-list grows a scrollbar; E2E 2026-09-12: classic
// rows overflowed by ~27px because the 语音输入 badge is wider than the
// layout estimate baked into the old 72px floor). Fixed siblings alone
// always fit, so no floor is needed: the preview collapses to whatever
// width is left and ellipsizes.
//
// Regression shape this locks out: re-adding a floor (or a variant-specific
// floor) after a future fixed-sibling widening.

const css = readFileSync(
    fileURLToPath(new URL('../ui/settings.css', import.meta.url)),
    'utf8',
);

// Anchored at line start so the audio-missing variant selector
// (`.data-row.audio-missing .data-row-preview`) cannot satisfy the match.
const BASE_RULE_RE = /^\.data-row-preview\s*\{[^}]*\}/m;

describe('data row preview overflow contract', () => {
    it('base .data-row-preview rule declares min-width: 0 (no floor)', () => {
        const m = css.match(BASE_RULE_RE);
        expect(m).not.toBeNull();
        expect(m[0]).toMatch(/min-width:\s*0\s*;/);
    });

    it('no min-width floor value remains anywhere in settings.css', () => {
        expect(css).not.toMatch(/min-width:\s*72px/);
    });
});
