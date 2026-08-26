import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const read = (p) => readFileSync(new URL(p, import.meta.url)).toString('utf8');

// Extract one closed rule block (selector up to the first '}') so assertions
// cannot be satisfied by unrelated rules later in the file.
const blockOf = (css, selector) => {
    const s = css.indexOf(selector);
    return css.slice(s, css.indexOf('}', s) + 1);
};

describe('progress bar scaleX contract', () => {
    it('settings .progress-bar-fill uses width:100% + transform (no width transition)', () => {
        const css = read('../ui/settings.css');
        const block = blockOf(css, '.progress-bar-fill');
        expect(block).toContain('width: 100%');
        expect(block).toContain('transform: scaleX(0)');
        expect(block).toContain('transform-origin: left');
        expect(block).not.toContain('transition: width');
    });
    it('settings.html progress element carries no inline width', () => {
        const html = read('../ui/settings.html');
        expect(html).not.toMatch(/progress-fill[^>]*style="width/);
        expect(html).toContain('id="progress-fill"');
    });
    it('transcribe .progress-fill uses width:100% + transform', () => {
        const css = read('../ui/transcribe.css');
        const block = blockOf(css, '.progress-fill');
        expect(block).toContain('width: 100%');
        expect(block).toContain('transform: scaleX(0)');
        expect(block).toContain('transform-origin: left');
        expect(block).not.toContain('transition: width');
    });
});
