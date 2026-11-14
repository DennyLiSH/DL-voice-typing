import { describe, expect, it } from 'vitest';

// readSource is repo-root-relative; blockOf throws on a missing selector
// (both shared from helpers.js — this file's former inline `read` +
// silent blockOf variants consolidated in Round 6 P4 Task 4; every call
// site below targets a selector that exists, so the throw guard is a
// strict upgrade over the old silent empty slice).
//
// Round 6 P5 Task 2: the per-window recipe mirrors (settings
// .progress-bar-fill / transcribe .progress-fill scaleX blocks, both-window
// pill assertions) are retired — the recipe is single-sourced in
// common.css (`.progress[hidden]` guard + `.progress-track` capsule +
// `.progress-fill` accent scaleX) and this file pins OWNERSHIP, not
// per-window copies (same shape as the .banner/.toast pins in the
// uniformity contract).
import { blockOf, readSource } from './helpers.js';

describe('progress bar contract', () => {
    it('common.css owns the .progress recipe (guard + track capsule + fill scaleX)', () => {
        const css = readSource('ui/common.css');
        // [hidden] guard: an author display on a consumer class (transcribe
        // .progress-wrap is flex) must not beat the UA hidden rule — same
        // pattern as .badge[hidden].
        expect(blockOf(css, '.progress[hidden] {')).toContain('display: none');
        // 6px pill-shaped track.
        const track = blockOf(css, '.progress-track {');
        expect(track).toContain('height: 6px');
        expect(track).toContain('border-radius: var(--radius-full)');
        expect(track).toContain('background: var(--progress-bg)');
        expect(track).toContain('overflow: hidden');
        // accent scaleX fill (JS writes only the inline transform value —
        // the recipe itself owns scaleX(0) + transform-origin, and a width
        // transition would fight the transform animation).
        const fill = blockOf(css, '.progress-fill {');
        expect(fill).toContain('width: 100%');
        expect(fill).toContain('transform: scaleX(0)');
        expect(fill).toContain('transform-origin: left');
        expect(fill).not.toContain('transition: width');
    });
    it('settings.html progress element carries no inline width', () => {
        const html = readSource('ui/settings.html');
        expect(html).not.toMatch(/progress-fill[^>]*style="width/);
        expect(html).toContain('id="progress-fill"');
    });
});
