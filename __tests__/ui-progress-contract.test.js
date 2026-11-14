import { describe, expect, it } from 'vitest';

// readSource is repo-root-relative; blockOf throws on a missing selector
// (both shared from helpers.js — this file's former inline `read` +
// silent blockOf variants consolidated in Round 6 P4 Task 4; every call
// site below targets a selector that exists, so the throw guard is a
// strict upgrade over the old silent empty slice).
import { blockOf, readSource } from './helpers.js';

describe('progress bar scaleX contract', () => {
    it('settings .progress-bar-fill uses width:100% + transform (no width transition)', () => {
        const css = readSource('ui/settings.css');
        const block = blockOf(css, '.progress-bar-fill');
        expect(block).toContain('width: 100%');
        expect(block).toContain('transform: scaleX(0)');
        expect(block).toContain('transform-origin: left');
        expect(block).not.toContain('transition: width');
    });
    it('settings.html progress element carries no inline width', () => {
        const html = readSource('ui/settings.html');
        expect(html).not.toMatch(/progress-fill[^>]*style="width/);
        expect(html).toContain('id="progress-fill"');
    });
    it('transcribe .progress-fill uses width:100% + transform', () => {
        const css = readSource('ui/transcribe.css');
        const block = blockOf(css, '.progress-fill');
        expect(block).toContain('width: 100%');
        expect(block).toContain('transform: scaleX(0)');
        expect(block).toContain('transform-origin: left');
        expect(block).not.toContain('transition: width');
    });
    it('both windows use pill-shaped progress tracks (radius-full)', () => {
        const s = blockOf(readSource('ui/settings.css'), '.progress-bar-track');
        const t = blockOf(readSource('ui/transcribe.css'), '.progress-track');
        expect(s).toContain('border-radius: var(--radius-full)');
        expect(t).toContain('border-radius: var(--radius-full)');
    });
});
