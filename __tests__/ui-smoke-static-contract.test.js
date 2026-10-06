// Static text-scrape contract for the 2026-10-06 smoke-list triage: pins the
// six static criteria carved out of the manual UI smoke checklist (TODO.md)
// so they cannot silently regress. Dynamic-behavior items stay manual — see
// _Project/superpowers/specs/20261006_0833_ui-smoke-static-contract-design.md.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const read = (rel) =>
    readFileSync(fileURLToPath(new URL(`../${rel}`, import.meta.url)), 'utf8');

describe('common.css static state contract', () => {
    const css = read('ui/common.css');

    it('reduced-motion disables dialog animations', () => {
        expect(css).toMatch(
            /@media \(prefers-reduced-motion: reduce\)\s*\{\s*\.dialog-overlay,\s*\.dialog\s*\{[^}]*animation:\s*none/,
        );
    });

    it('confirm-dialog action row is right-aligned via flex-end', () => {
        const block = css.match(/\.dialog-actions\s*\{[^}]*\}/);
        expect(block).not.toBeNull();
        expect(block[0]).toContain('justify-content: flex-end');
    });
});

describe('settings.css static state contract', () => {
    const css = read('ui/settings.css');

    it('disabled hotkey group dims label and hint (opacity 0.5)', () => {
        const block = css.match(
            /\.form-group\.disabled label,\s*\.form-group\.disabled \.hint\s*\{[^}]*\}/,
        );
        expect(block).not.toBeNull();
        expect(block[0]).toContain('opacity: 0.5');
    });

    it('batch-bar delete button keeps right alignment scoped to its context', () => {
        const block = css.match(/\.data-batch-bar \.btn-danger\s*\{[^}]*\}/);
        expect(block).not.toBeNull();
        expect(block[0]).toContain('margin-left: auto');
    });
});

describe('settings-form hotkey disabled wiring', () => {
    it('record-only hotkey group toggles the disabled class', () => {
        const js = read('ui/settings-form.js');
        expect(js).toContain(
            "recordOnlyHotkeyGroup.classList.toggle('disabled', !enabled)",
        );
    });
});

describe('static copy contract', () => {
    it('transcribe empty state copy with hidden initial value', () => {
        const html = read('ui/transcribe.html');
        expect(html).toMatch(
            /id="rec-empty"[^>]*hidden[^>]*>暂无录音，按住录音快捷键开始第一条</,
        );
    });

    it('model delete confirmation warns the action is irreversible', () => {
        const js = read('ui/model-manager.js');
        expect(js).toContain('此操作不可恢复。');
    });
});
