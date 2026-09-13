// Text-scrape contract for the 盲录电平 CSS: the .indicator.record-only
// transition list must carry opacity + transform + filter — a lone
// `filter` entry would REPLACE the base show/hide transitions for this
// mode, and jsdom cannot catch that (same pattern as undo-window-contract).
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const css = readFileSync(
    fileURLToPath(new URL('../ui/floating.css', import.meta.url)),
    'utf8',
);

describe('record-only indicator CSS contract (盲录电平)', () => {
    it('.indicator.record-only transition keeps opacity+transform and adds filter', () => {
        const start = css.indexOf('.indicator.record-only');
        expect(start).toBeGreaterThanOrEqual(0);
        const block = css.slice(start, css.indexOf('}', start));
        expect(block).toContain('opacity');
        expect(block).toContain('transform');
        expect(block).toContain('filter 150ms ease');
    });

    it('red ripple variant exists', () => {
        expect(css).toContain('.ripple.red');
    });
});
