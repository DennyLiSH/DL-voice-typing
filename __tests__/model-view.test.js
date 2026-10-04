// Pure-function branch matrix tests for ui/lib/model-view.js — no jsdom
// needed. Mirrors the transcribe uiFlags precedent: the state→view
// derivation is testable without any DOM.
import { describe, expect, it } from 'vitest';
import { modelActionView } from '../ui/lib/model-view.js';

const STATUS = { tiny: true, base: false };

describe('modelActionView', () => {
    it('custom model, idle: enabled danger delete button', () => {
        const v = modelActionView('custom:my.bin', null, STATUS);
        expect(v).toEqual({
            action: 'delete',
            actionDisabled: false,
            actionDanger: true,
            progressVisible: false,
            selectDisabled: false,
            statusVisible: false,
        });
    });

    it('downloading THIS model: progress bar, everything else locked', () => {
        const v = modelActionView('base', 'base', STATUS);
        expect(v.action).toBe('none');
        expect(v.actionDisabled).toBe(true);
        expect(v.progressVisible).toBe(true);
        expect(v.selectDisabled).toBe(true);
        expect(v.statusVisible).toBe(false);
    });

    it('downloading THIS model wins over custom (priority parity with else-if chain)', () => {
        const v = modelActionView('custom:my.bin', 'custom:my.bin', STATUS);
        expect(v.action).toBe('none');
        expect(v.progressVisible).toBe(true);
    });

    it('downloading ANOTHER model, builtin selected: disabled download button', () => {
        const v = modelActionView('base', 'tiny', STATUS);
        expect(v.action).toBe('download');
        expect(v.actionDisabled).toBe(true);
        expect(v.actionDanger).toBe(false);
        expect(v.selectDisabled).toBe(true);
    });

    // BehaviorChange #3 (registered in Task 4 Step 2): the current
    // else-if chain's `else if (isCustom)` arm (delete-danger during
    // another model's download) is UNREACHABLE dead code — the preceding
    // `else if (isDownloading)` arm already swallows every isDownloading
    // case without splitting custom/builtin. modelActionView revives the
    // designed intent; the state is UI-unreachable either way (select is
    // disabled during downloads), so no user-visible delta.
    it('downloading ANOTHER model, custom selected: disabled danger delete button (BehaviorChange #3: dead-branch revival)', () => {
        const v = modelActionView('custom:my.bin', 'tiny', STATUS);
        expect(v.action).toBe('delete');
        expect(v.actionDisabled).toBe(true);
        expect(v.actionDanger).toBe(true);
        expect(v.selectDisabled).toBe(true);
    });

    it('downloaded builtin: status text shown, no button', () => {
        const v = modelActionView('tiny', null, STATUS);
        expect(v.action).toBe('none');
        expect(v.statusVisible).toBe(true);
        expect(v.selectDisabled).toBe(false);
    });

    it('not-downloaded builtin: enabled download button', () => {
        const v = modelActionView('base', null, STATUS);
        expect(v.action).toBe('download');
        expect(v.actionDisabled).toBe(false);
        expect(v.actionDanger).toBe(false);
        expect(v.selectDisabled).toBe(false);
        expect(v.statusVisible).toBe(false);
    });
});
