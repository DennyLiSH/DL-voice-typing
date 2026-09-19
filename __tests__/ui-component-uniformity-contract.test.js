import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const read = (p) => readFileSync(new URL(p, import.meta.url)).toString('utf-8');

// Extract one closed rule block (selector up to the first '}') so assertions
// cannot be satisfied by unrelated rules later in the file (same helper
// convention as ui-progress-contract.test.js).
const blockOf = (css, selector) => {
    const s = css.indexOf(selector);
    return css.slice(s, css.indexOf('}', s) + 1);
};

describe('button geometry uniformity', () => {
    it('.btn-danger carries no custom geometry (relies on button base)', () => {
        const css = read('../ui/common.css');
        const block = blockOf(css, '.btn-danger {');
        expect(block).not.toMatch(/padding|border-radius|cursor/);
    });

    it('.btn-danger joins the shared disabled token group', () => {
        const css = read('../ui/common.css');
        const block = blockOf(css, '.btn-primary:disabled,');
        expect(block).toContain('.btn-danger:disabled');
        expect(block).toContain('var(--disabled-bg)');
    });

    it('base button:disabled rule lives in common.css, not settings.css', () => {
        expect(read('../ui/common.css')).toMatch(
            /button:disabled\s*\{\s*cursor: not-allowed;/,
        );
        expect(read('../ui/settings.css')).not.toMatch(
            /^button:disabled\s*\{/m,
        );
    });

    it('download button composes .btn-primary (html + js swap sites)', () => {
        expect(read('../ui/settings.html')).toContain(
            'class="btn-primary btn-download-model"',
        );
        const js = read('../ui/model-manager.js');
        expect(js).toContain(`'btn-primary btn-download-model'`);
        expect(js).toContain(`'btn-primary btn-download-model btn-danger'`);
    });

    it('.btn-download-model is a layout-only modifier (no color/geometry)', () => {
        const block = blockOf(
            read('../ui/settings.css'),
            '.btn-download-model {',
        );
        expect(block).not.toMatch(
            /background|border:|height|padding|font-size|transition|box-shadow/,
        );
        expect(block).toContain('white-space: nowrap');
    });

    it('review window buttons inherit base geometry (no padding, no 0.96 scale)', () => {
        const css = read('../ui/review.css');
        expect(blockOf(css, '.btn-confirm {')).not.toContain('padding');
        expect(blockOf(css, '.btn-cancel {')).not.toContain('padding');
        expect(css).not.toContain('scale(0.96)');
    });
});
