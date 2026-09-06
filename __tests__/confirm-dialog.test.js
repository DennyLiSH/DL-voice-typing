// @vitest-environment jsdom
//
// Unit tests for the shared in-app confirm dialog (ui/lib/confirm-dialog.js).
// Drives the real DOM: clicks the buttons, dispatches keydown, checks focus.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const overlay = () => document.querySelector('.dialog-overlay');
const dialog = () => document.querySelector('[role="alertdialog"]');
const confirmBtn = () => dialog()?.querySelector('.btn-primary, .btn-danger');
const cancelBtn = () => dialog()?.querySelector('.btn-secondary');

async function settled() {
    await new Promise((r) => setTimeout(r, 0));
}

describe('confirmDialog', () => {
    beforeEach(() => {
        document.body.innerHTML = '';
    });

    afterEach(() => {
        vi.restoreAllMocks();
    });

    it('injects the alertdialog into the body with the given texts', async () => {
        const { confirmDialog } = await import('../ui/lib/confirm-dialog.js');
        const p = confirmDialog({ title: '标题', message: '内容' });
        await settled();
        expect(dialog()).not.toBeNull();
        expect(dialog().getAttribute('aria-modal')).toBe('true');
        expect(dialog().getAttribute('aria-labelledby')).toBe('dialog-title');
        expect(dialog().getAttribute('aria-describedby')).toBe(
            'dialog-message',
        );
        expect(dialog().querySelector('.dialog-title').textContent).toBe(
            '标题',
        );
        expect(dialog().querySelector('.dialog-message').textContent).toBe(
            '内容',
        );
        confirmBtn().click();
        expect(await p).toBe(true);
    });

    it('resolves true on confirm click and removes the overlay', async () => {
        const { confirmDialog } = await import('../ui/lib/confirm-dialog.js');
        const p = confirmDialog({ title: 't', message: 'm' });
        await settled();
        confirmBtn().click();
        expect(await p).toBe(true);
        expect(overlay()).toBeNull();
    });

    it('resolves false on cancel click', async () => {
        const { confirmDialog } = await import('../ui/lib/confirm-dialog.js');
        const p = confirmDialog({ title: 't', message: 'm' });
        await settled();
        cancelBtn().click();
        expect(await p).toBe(false);
        expect(overlay()).toBeNull();
    });

    it('resolves false on Escape and on overlay click', async () => {
        const { confirmDialog } = await import('../ui/lib/confirm-dialog.js');
        const p1 = confirmDialog({ title: 't', message: 'm' });
        await settled();
        dialog().dispatchEvent(
            new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }),
        );
        expect(await p1).toBe(false);

        const p2 = confirmDialog({ title: 't', message: 'm' });
        await settled();
        overlay().dispatchEvent(new MouseEvent('click', { bubbles: true }));
        expect(await p2).toBe(false);
    });

    it('resolves true on Enter when focus is inside but not on a button', async () => {
        const { confirmDialog } = await import('../ui/lib/confirm-dialog.js');
        const p = confirmDialog({ title: 't', message: 'm' });
        await settled();
        dialog().dispatchEvent(
            new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }),
        );
        expect(await p).toBe(true);
    });

    it('restores focus to the previously focused element on close', async () => {
        const trigger = document.createElement('button');
        document.body.appendChild(trigger);
        trigger.focus();

        const { confirmDialog } = await import('../ui/lib/confirm-dialog.js');
        const p = confirmDialog({ title: 't', message: 'm' });
        await settled();
        expect(document.activeElement).toBe(confirmBtn());
        cancelBtn().click();
        await p;
        expect(document.activeElement).toBe(trigger);
    });

    it('danger: red confirm button and initial focus on CANCEL', async () => {
        const { confirmDialog } = await import('../ui/lib/confirm-dialog.js');
        const p = confirmDialog({ title: 't', message: 'm', danger: true });
        await settled();
        expect(confirmBtn().className).toContain('btn-danger');
        expect(document.activeElement).toBe(cancelBtn());
        cancelBtn().click();
        expect(await p).toBe(false);
    });

    it('keeps "\\n" in the message (rendered via CSS pre-line)', async () => {
        const { confirmDialog } = await import('../ui/lib/confirm-dialog.js');
        const p = confirmDialog({ title: 't', message: 'a\nb' });
        await settled();
        expect(dialog().querySelector('.dialog-message').textContent).toBe(
            'a\nb',
        );
        cancelBtn().click();
        await p;
    });

    it('isDialogOpen reflects the modal lifecycle', async () => {
        const { confirmDialog, isDialogOpen } = await import(
            '../ui/lib/confirm-dialog.js'
        );
        expect(isDialogOpen()).toBe(false);
        const p = confirmDialog({ title: 't', message: 'm' });
        await settled();
        expect(isDialogOpen()).toBe(true);
        cancelBtn().click();
        await p;
        expect(isDialogOpen()).toBe(false);
    });
});
