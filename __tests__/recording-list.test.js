// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// recordings.js transitively imports api.js, which evaluates
// `window.__TAURI__.core` at module load. Stub it before the import so
// the controller's import graph doesn't blow up under jsdom.
vi.stubGlobal('__TAURI__', { core: { invoke: vi.fn() } });
URL.createObjectURL = vi.fn(() => 'blob:mock-url');
URL.revokeObjectURL = vi.fn();

const { createRovingList, createAudioSlot } = await import(
    '../ui/lib/recording-list.js'
);

function buildRows(container, names, rowClass) {
    for (const n of names) {
        const row = document.createElement('div');
        row.className = rowClass;
        row.dataset.filename = n;
        row.tabIndex = -1;
        container.appendChild(row);
    }
    return Array.from(container.children);
}

afterEach(() => {
    vi.restoreAllMocks();
});

describe('createRovingList', () => {
    let container;
    beforeEach(() => {
        document.body.innerHTML = '';
        container = document.createElement('div');
        document.body.appendChild(container);
    });

    it('sync() makes exactly one row the tab stop (last focused, else first)', () => {
        const [a, b] = buildRows(container, ['a', 'b'], 'rec-row');
        const list = createRovingList({
            container,
            rowSelector: '.rec-row',
            onActivate: () => {},
        });
        list.sync();
        expect(a.tabIndex).toBe(0);
        expect(b.tabIndex).toBe(-1);
        b.dispatchEvent(new FocusEvent('focusin', { bubbles: true }));
        list.sync();
        expect(a.tabIndex).toBe(-1);
        expect(b.tabIndex).toBe(0);
    });

    it('ArrowDown/ArrowUp move focus with wrap-around', () => {
        const [a, b] = buildRows(container, ['a', 'b'], 'rec-row');
        createRovingList({
            container,
            rowSelector: '.rec-row',
            onActivate: () => {},
        });
        a.focus();
        a.dispatchEvent(
            new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }),
        );
        expect(document.activeElement).toBe(b);
        b.dispatchEvent(
            new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }),
        );
        expect(document.activeElement).toBe(a); // wraps
    });

    it('Enter activates the row itself, NOT interactive children (DR-2.2)', () => {
        const [a] = buildRows(container, ['a'], 'data-row');
        const btn = document.createElement('button');
        a.appendChild(btn);
        const onActivate = vi.fn();
        createRovingList({ container, rowSelector: '.data-row', onActivate });
        a.focus();
        a.dispatchEvent(
            new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }),
        );
        expect(onActivate).toHaveBeenCalledWith('a');
        onActivate.mockClear();
        btn.focus();
        btn.dispatchEvent(
            new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }),
        );
        expect(onActivate).not.toHaveBeenCalled();
    });

    it('destroy() removes listeners', () => {
        const [a, b] = buildRows(container, ['a', 'b'], 'rec-row');
        const list = createRovingList({
            container,
            rowSelector: '.rec-row',
            onActivate: () => {},
        });
        a.focus();
        list.destroy();
        a.dispatchEvent(
            new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }),
        );
        // Listener removed → focus must NOT move to b (with the listener
        // alive it would — falsifiable both ways). `b` is part of the
        // fixture (two-row list); its presence proves the listener was
        // the only path to a different focus target.
        void b;
        expect(document.activeElement).toBe(a);
    });
});

describe('createAudioSlot', () => {
    it('attaches bytes when not stale; marks failure and calls onFail', async () => {
        const el = document.createElement('audio');
        document.body.appendChild(el);
        const bytes = new Uint8Array([1, 2, 3]);
        const slot = createAudioSlot({
            fetchBytes: async () => bytes,
            onFail: () => {},
        });
        await slot.bind(el, 'a', { isStale: () => false });
        expect(el.dataset.blobUrl).toBeTruthy();
        expect(slot.failedFiles.has('a')).toBe(false);

        const onFail = vi.fn();
        const slot2 = createAudioSlot({
            fetchBytes: async () => {
                throw new Error('x');
            },
            onFail,
        });
        await slot2.bind(el, 'b', { isStale: () => false });
        expect(slot2.failedFiles.has('b')).toBe(true);
        expect(onFail).toHaveBeenCalledWith('b');
    });

    it('stale responses (isStale / element replaced) never attach (DR-1.2)', async () => {
        const el = document.createElement('audio');
        document.body.appendChild(el);
        const slot = createAudioSlot({
            fetchBytes: async () => new Uint8Array([1]),
            onFail: () => {},
        });
        await slot.bind(el, 'a', { isStale: () => true });
        expect(el.dataset.blobUrl).toBeUndefined();

        const el2 = document.createElement('audio');
        document.body.appendChild(el2);
        const p = slot.bind(el2, 'b', { isStale: () => false });
        slot.release(); // element replaced mid-flight
        await p;
        expect(el2.dataset.blobUrl).toBeUndefined();
    });

    it('rethrows fetch errors when no onFail given (transcribe shape)', async () => {
        const el = document.createElement('audio');
        document.body.appendChild(el);
        const slot = createAudioSlot({
            fetchBytes: async () => {
                throw new Error('net');
            },
        });
        await expect(
            slot.bind(el, 'a', { isStale: () => false }),
        ).rejects.toThrow('net');
        expect(slot.failedFiles.has('a')).toBe(true);
    });

    it('persistent fetch failure does not loop: onFail consumer clears active row (审查 S5-F1 回归)', async () => {
        // Simulates the data page wiring: bind fails -> onFail -> consumer
        // releases (clears the active-row marker) -> re-render does NOT
        // re-bind. A wiring that only re-renders without clearing would
        // re-bind forever (fetch -> fail -> render -> fetch ...).
        let activeRow = 'a';
        let binds = 0;
        const el = document.createElement('audio');
        document.body.appendChild(el);
        const slot = createAudioSlot({
            fetchBytes: async () => {
                binds += 1;
                throw new Error('down');
            },
            onFail: () => {
                activeRow = null; // consumer's release (onAudioError path)
            },
        });
        await slot.bind(el, 'a', { isStale: () => activeRow !== 'a' });
        expect(binds).toBe(1);
        // Re-render gate: activeRow cleared -> next bind call is stale-guarded.
        await slot.bind(el, 'a', { isStale: () => activeRow !== 'a' });
        expect(binds).toBe(1, 'stale guard must prevent the second fetch');
    });
});
