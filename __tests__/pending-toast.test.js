/**
 * @vitest-environment jsdom
 *
 * pending-toast.js lifecycle tests. The module dynamically injects its
 * own DOM (no fixture required) — every test starts with a clean body.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

beforeEach(() => {
    document.body.innerHTML = '';
    vi.useFakeTimers();
});

afterEach(async () => {
    const mod = await import('../ui/lib/pending-toast.js');
    await mod.unbindFinalizeListener();
    mod.destroyPendingToast();
    vi.useRealTimers();
    vi.unstubAllGlobals();
});

async function loadModule() {
    vi.resetModules();
    return import('../ui/lib/pending-toast.js');
}

/**
 * Stub Tauri 2's event.listen so tests can capture the listener that
 * pending-toast.js installs and fire events through it. Tauri 2 returns
 * `Promise<UnlistenFn>` from listen — the helper preserves that shape.
 */
function stubTauriListen() {
    let emit;
    let unlistenCalled = false;
    vi.stubGlobal('__TAURI__', {
        event: {
            listen: (_name, cb) => {
                emit = cb;
                return Promise.resolve(() => {
                    unlistenCalled = true;
                    emit = null;
                });
            },
        },
    });
    return {
        getEmit: () => emit,
        isUnlistenCalled: () => unlistenCalled,
    };
}

function getToastEl() {
    return document.getElementById('pending-toast');
}

describe('pending toast lifecycle', () => {
    it('injects the toast with text + undo button on first show', async () => {
        const { showPendingToast } = await loadModule();
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

    it('clicking 撤销 invokes onUndo and tears down the toast', async () => {
        const { showPendingToast } = await loadModule();
        const onUndo = vi.fn();
        showPendingToast({ moved: 2, onUndo });
        const undo = document.querySelector('#pending-toast-undo');
        undo.dispatchEvent(new MouseEvent('click', { bubbles: true }));
        expect(onUndo).toHaveBeenCalledTimes(1);
        expect(getToastEl()).toBeNull();
    });

    it('countdown hitting zero shows 已永久删除 with disabled undo, then auto-dismisses', async () => {
        const { showPendingToast } = await loadModule();
        const onUndo = vi.fn();
        showPendingToast({ moved: 2, onUndo });
        // The countdown ticks once per second; at remaining=0 the toast
        // enters the finalized state (spec: 倒计时归零触发"已永久删除"状态,
        // toast 替换文案) instead of vanishing — the user sees the deletion
        // became permanent.
        vi.advanceTimersByTime(5000);
        const toast = getToastEl();
        expect(toast).not.toBeNull();
        const text = toast.querySelector('#pending-toast-text');
        const undo = toast.querySelector('#pending-toast-undo');
        expect(text.textContent).toBe('已永久删除 2 条');
        // The backend entry is taken-once by the finalize timer — a late
        // undo click must be impossible (disabled + guarded).
        expect(undo.disabled).toBe(true);
        undo.dispatchEvent(new MouseEvent('click', { bubbles: true }));
        expect(onUndo).not.toHaveBeenCalled();
        // The finalized state auto-dismisses 2s later.
        vi.advanceTimersByTime(2000);
        expect(getToastEl()).toBeNull();
        expect(onUndo).not.toHaveBeenCalled();
    });

    it('countdown text reflects the seconds remaining on each tick', async () => {
        const { showPendingToast } = await loadModule();
        showPendingToast({ moved: 4, onUndo: vi.fn() });
        const text = document.querySelector('#pending-toast-text');
        expect(text.textContent).toContain('5s');
        vi.advanceTimersByTime(1000);
        // After one tick: 4s remaining
        expect(text.textContent).toContain('4s');
        vi.advanceTimersByTime(1000);
        expect(text.textContent).toContain('3s');
    });

    it('second show replaces the first toast (single-instance slot)', async () => {
        const { showPendingToast } = await loadModule();
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

    it('destroyPendingToast on a dead slot is a no-op', async () => {
        const { destroyPendingToast } = await loadModule();
        destroyPendingToast();
        destroyPendingToast();
        expect(getToastEl()).toBeNull();
    });
});

describe('pending toast backend finalize event', () => {
    it('backend finalize event enters finalized state immediately', async () => {
        const { bindFinalizeListener, showPendingToast } = await loadModule();
        const { getEmit } = stubTauriListen();
        bindFinalizeListener();
        showPendingToast({ moved: 2, id: 7, undoSecs: 5, onUndo: () => {} });
        const emit = getEmit();
        expect(emit).toBeTypeOf('function');
        emit({ payload: { id: 7 } });
        const btn = document.getElementById('pending-toast-undo');
        expect(btn.disabled).toBe(true);
    });

    it('event for a different batch id is ignored', async () => {
        const { bindFinalizeListener, showPendingToast } = await loadModule();
        const { getEmit } = stubTauriListen();
        bindFinalizeListener();
        showPendingToast({ moved: 1, id: 7, undoSecs: 5, onUndo: () => {} });
        getEmit()({ payload: { id: 99 } });
        const btn = document.getElementById('pending-toast-undo');
        expect(btn.disabled).toBe(false);
    });

    it('unbindFinalizeListener actually unlistens', async () => {
        const {
            bindFinalizeListener,
            showPendingToast,
            unbindFinalizeListener,
        } = await loadModule();
        const handle = stubTauriListen();
        bindFinalizeListener();
        await unbindFinalizeListener();
        expect(handle.isUnlistenCalled()).toBe(true);

        // A re-bind must install a fresh listener (the previous slot is
        // cleared so events do not pile up across navigations).
        showPendingToast({ moved: 1, id: 7, undoSecs: 5, onUndo: () => {} });
        const fire = handle.getEmit();
        expect(fire).toBeNull();
        bindFinalizeListener();
        handle.getEmit()({ payload: { id: 7 } });
        expect(document.getElementById('pending-toast-undo').disabled).toBe(
            true,
        );
    });
});
