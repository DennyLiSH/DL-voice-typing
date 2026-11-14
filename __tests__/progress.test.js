// @vitest-environment jsdom
//
// Unit tests for ui/lib/progress.js — the shared percent → DOM write
// primitive (Round 6 P5 Task 2). Both former call sites (settings
// model-manager.js, transcribe window setProgress) delegate here; the
// single-source triple is clamp + aria-valuenow + scaleX transform, and the
// transform string must stay byte-identical to the old hand-rolled output
// (`String(clamped / 100)` template semantics — pinned end-to-end by
// model-manager.test.js 'scaleX(0.42)' and transcribe.test.js 'scaleX(0.42)').
//
// Label text is deliberately NOT a percent formatter here: settings' label
// shows a bare percent, transcribe's shows stage text. Call sites own the
// text format (see the module header); these tests lock the guarded
// label+labelText write contract instead.
import { describe, expect, it } from 'vitest';

import { setProgressBar } from '../ui/lib/progress.js';

function buildBar() {
    const wrap = document.createElement('div');
    wrap.setAttribute('role', 'progressbar');
    const fill = document.createElement('div');
    const label = document.createElement('span');
    wrap.appendChild(fill);
    wrap.appendChild(label);
    return { wrap, fill, label };
}

describe('setProgressBar', () => {
    it('writes the byte-identical transform for in-range percents', () => {
        const { fill, wrap } = buildBar();
        setProgressBar({ fill, wrap, percent: 42 });
        expect(fill.style.transform).toBe('scaleX(0.42)');
        setProgressBar({ fill, wrap, percent: 50 });
        expect(fill.style.transform).toBe('scaleX(0.5)');
        setProgressBar({ fill, wrap, percent: 7 });
        expect(fill.style.transform).toBe('scaleX(0.07)');
    });

    it('clamps below 0 and above 100 (both writes)', () => {
        const { fill, wrap } = buildBar();
        setProgressBar({ fill, wrap, percent: -20 });
        expect(fill.style.transform).toBe('scaleX(0)');
        expect(wrap.getAttribute('aria-valuenow')).toBe('0');
        setProgressBar({ fill, wrap, percent: 150 });
        expect(fill.style.transform).toBe('scaleX(1)');
        expect(wrap.getAttribute('aria-valuenow')).toBe('100');
    });

    it('boundaries 0 and 100 produce the exact reset/full strings', () => {
        const { fill, wrap } = buildBar();
        setProgressBar({ fill, wrap, percent: 0 });
        expect(fill.style.transform).toBe('scaleX(0)');
        expect(wrap.getAttribute('aria-valuenow')).toBe('0');
        setProgressBar({ fill, wrap, percent: 100 });
        expect(fill.style.transform).toBe('scaleX(1)');
        expect(wrap.getAttribute('aria-valuenow')).toBe('100');
    });

    it('writes aria-valuenow on the wrap, not the fill', () => {
        const { fill, wrap } = buildBar();
        setProgressBar({ fill, wrap, percent: 42 });
        expect(wrap.getAttribute('aria-valuenow')).toBe('42');
        expect(fill.getAttribute('aria-valuenow')).toBe(null);
    });

    it('writes labelText into the label element when both are given', () => {
        const { fill, wrap, label } = buildBar();
        setProgressBar({ fill, wrap, percent: 42, label, labelText: '42%' });
        expect(label.textContent).toBe('42%');
    });

    it('leaves the label untouched without labelText, and tolerates a missing label', () => {
        const { fill, wrap, label } = buildBar();
        label.textContent = 'keep me';
        setProgressBar({ fill, wrap, percent: 42, label });
        expect(label.textContent).toBe('keep me');
        setProgressBar({ fill, wrap, percent: 7, labelText: 'orphan text' });
        expect(label.textContent).toBe('keep me');
    });

    it('null-safe: no fill, wrap, or label does not throw', () => {
        expect(() =>
            setProgressBar({ fill: null, wrap: null, percent: 42 }),
        ).not.toThrow();
        const { label } = buildBar();
        expect(() =>
            setProgressBar({
                fill: null,
                wrap: null,
                percent: 0,
                label,
                labelText: '0%',
            }),
        ).not.toThrow();
    });
});
