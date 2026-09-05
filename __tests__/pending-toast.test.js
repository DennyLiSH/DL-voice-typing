/**
 * @vitest-environment jsdom
 *
 * pending-toast.js lifecycle tests. The module dynamically injects its
 * own DOM (no fixture required) — every test starts with a clean body.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
    destroyPendingToast,
    showPendingToast,
} from '../ui/lib/pending-toast.js';

beforeEach(() => {
    document.body.innerHTML = '';
    vi.useFakeTimers();
});

afterEach(() => {
    destroyPendingToast();
    vi.useRealTimers();
});

function getToastEl() {
    return document.getElementById('pending-toast');
}

describe('pending toast lifecycle', () => {
    it('injects the toast with text + undo button on first show', () => {
        showPendingToast({ moved: 3, onUndo: vi.fn() });
        const toast = getToastEl();
        expect(toast).not.toBeNull();
        const text = toast.querySelector('#pending-toast-text');
        const undo = toast.querySelector('#pending-toast-undo');
        expect(text).not.toBeNull();
        expect(undo).not.toBeNull();
        expect(text.textContent).toContain('已删除 3 条');
        expect(text.textContent).toContain('5s');
        expect(undo.textContent).toBe('撤销');
    });

    it('clicking 撤销 invokes onUndo and tears down the toast', () => {
        const onUndo = vi.fn();
        showPendingToast({ moved: 2, onUndo });
        const undo = document.querySelector('#pending-toast-undo');
        undo.dispatchEvent(new MouseEvent('click', { bubbles: true }));
        expect(onUndo).toHaveBeenCalledTimes(1);
        expect(getToastEl()).toBeNull();
    });

    it('advancing timers past 5s destroys the toast and does NOT invoke onUndo', () => {
        const onUndo = vi.fn();
        showPendingToast({ moved: 1, onUndo });
        // The countdown ticks once per second; advancing by 5s emits 5 ticks
        // then triggers the destroy at remaining=0 (see tick()).
        vi.advanceTimersByTime(5000);
        expect(getToastEl()).toBeNull();
        expect(onUndo).not.toHaveBeenCalled();
    });

    it('countdown text reflects the seconds remaining on each tick', () => {
        showPendingToast({ moved: 4, onUndo: vi.fn() });
        const text = document.querySelector('#pending-toast-text');
        expect(text.textContent).toContain('5s');
        vi.advanceTimersByTime(1000);
        // After one tick: 4s remaining
        expect(text.textContent).toContain('4s');
        vi.advanceTimersByTime(1000);
        expect(text.textContent).toContain('3s');
    });

    it('second show replaces the first toast (single-instance slot)', () => {
        const onUndoA = vi.fn();
        const onUndoB = vi.fn();
        showPendingToast({ moved: 1, onUndo: onUndoA });
        const first = getToastEl();
        expect(first).not.toBeNull();
        showPendingToast({ moved: 7, onUndo: onUndoB });
        const second = getToastEl();
        expect(second).not.toBeNull();
        // Single-instance: only ONE toast in the DOM at any time.
        expect(document.querySelectorAll('#pending-toast').length).toBe(1);
        expect(
            second.querySelector('#pending-toast-text').textContent,
        ).toContain('已删除 7 条');
        // The first callback is no longer wired to the live button.
        const undo = second.querySelector('#pending-toast-undo');
        undo.dispatchEvent(new MouseEvent('click', { bubbles: true }));
        expect(onUndoB).toHaveBeenCalledTimes(1);
        expect(onUndoA).not.toHaveBeenCalled();
    });

    it('destroyPendingToast on a dead slot is a no-op', () => {
        destroyPendingToast();
        destroyPendingToast();
        expect(getToastEl()).toBeNull();
    });
});
