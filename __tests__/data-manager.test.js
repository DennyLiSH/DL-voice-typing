// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';

// jsdom orchestration tests for ui/data-manager.js audio lifecycle (C6):
// blob-URL release on list rebuild, onDataPageLeave cleanup, and the
// in-flight fetch identity guard.
//
// The fixture must contain every element wireDataListEvents() touches
// (btn-refresh-data / data-search-input / data-list / data-select-all-cb /
// btn-batch-delete / btn-prev-page / btn-next-page) and must be injected
// BEFORE the import, because wiring runs once at module top level.

const FIXTURE = `
    <button id="btn-refresh-data"></button>
    <input id="data-search-input">
    <div id="data-error-bar" hidden></div>
    <div id="data-list"></div>
    <div id="data-empty-state" hidden></div>
    <span id="data-total-count"></span>
    <span id="data-total-size"></span>
    <div id="data-pagination" hidden>
        <span id="data-page-info"></span>
        <button id="btn-prev-page"></button>
        <button id="btn-next-page"></button>
    </div>
    <div id="data-batch-bar" hidden>
        <span id="data-selected-count"></span>
        <span id="data-visible-count"></span>
        <button id="btn-batch-delete"></button>
    </div>
    <input type="checkbox" id="data-select-all-cb">
`;

const ITEM = {
    filename: '2026-08-18_10-00-00',
    timestamp: '2026-08-18T10:00:00+08:00',
    source: 'record_only',
    transcription_status: 'done',
    dropped_blocks: 0,
    wav_size: 100,
    json_size: 50,
    language: 'zh',
    duration_seconds: 5,
    transcription: '你好',
};

let invokeMock;
let mod;

const flush = () => new Promise((r) => setTimeout(r, 0));

async function loadFresh(invokeImpl) {
    invokeMock = vi.fn(invokeImpl);
    vi.stubGlobal('__TAURI__', { core: { invoke: invokeMock } });
    URL.createObjectURL = vi.fn(() => 'blob:mock-url');
    URL.revokeObjectURL = vi.fn();
    document.body.innerHTML = FIXTURE;
    vi.resetModules();
    mod = await import('../ui/data-manager.js');
}

function defaultInvoke(cmd) {
    switch (cmd) {
        case 'list_saved_recordings':
            return Promise.resolve({
                items: [ITEM],
                total: 1,
                total_bytes: 150,
                offset: 0,
                limit: 50,
            });
        case 'read_recording_audio':
            return Promise.resolve([1, 2, 3]);
        case 'soft_delete_recordings':
            return Promise.resolve({ id: 42, moved: 1, failed: [] });
        case 'restore_pending_delete':
            return Promise.resolve({ restored: 1, failed: 0 });
        default:
            return Promise.resolve(null);
    }
}

async function enterAndPlay() {
    mod.onDataPageEnter();
    await flush();
    await flush();
    document
        .querySelector('.btn-play')
        .dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await flush();
    await flush();
}

afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
});

describe('audio blob lifecycle', () => {
    it('revokes the old blob URL when renderDataList rebuilds (expand toggle)', async () => {
        await loadFresh(defaultInvoke);
        await enterAndPlay();
        expect(URL.createObjectURL).toHaveBeenCalledTimes(1);
        expect(URL.revokeObjectURL).not.toHaveBeenCalled();

        // Row body click toggles expand → renderDataList rebuilds the list.
        document
            .querySelector('.data-row')
            .dispatchEvent(new MouseEvent('click', { bubbles: true }));
        await flush();
        await flush();

        expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:mock-url');
        // Player row still active → re-attached with a fresh URL.
        expect(URL.createObjectURL).toHaveBeenCalledTimes(2);
        expect(document.querySelector('audio')).not.toBeNull();
    });

    it('onDataPageLeave revokes and clears player state', async () => {
        await loadFresh(defaultInvoke);
        await enterAndPlay();
        expect(document.querySelector('audio')).not.toBeNull();

        mod.onDataPageLeave();
        expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:mock-url');

        // A later rebuild must not resurrect the player (state cleared).
        document
            .querySelector('.data-row')
            .dispatchEvent(new MouseEvent('click', { bubbles: true }));
        await flush();
        await flush();
        expect(document.querySelector('audio')).toBeNull();
    });

    it('drops in-flight fetch bytes when the element was replaced (identity guard)', async () => {
        let resolveAudio;
        await loadFresh((cmd) => {
            if (cmd === 'read_recording_audio') {
                return new Promise((r) => {
                    resolveAudio = r;
                });
            }
            return defaultInvoke(cmd);
        });
        mod.onDataPageEnter();
        await flush();
        await flush();

        // Start playback — fetch stays in flight.
        document
            .querySelector('.btn-play')
            .dispatchEvent(new MouseEvent('click', { bubbles: true }));
        await flush();
        // Toggle play off before the fetch resolves → element discarded.
        document
            .querySelector('.btn-play')
            .dispatchEvent(new MouseEvent('click', { bubbles: true }));
        await flush();

        resolveAudio([1, 2, 3]);
        await flush();
        await flush();

        // Orphan element must never receive a blob URL (bounded leak closed).
        expect(URL.createObjectURL).not.toHaveBeenCalled();
    });
});

describe('row expand keyboard activation', () => {
    it('Enter on the row toggles the expanded state (delegated keydown)', async () => {
        await loadFresh(defaultInvoke);
        mod.onDataPageEnter();
        await flush();
        await flush();

        const row = document.querySelector('.data-row');
        expect(row).not.toBeNull();
        expect(row.getAttribute('aria-expanded')).toBe('false');

        row.dispatchEvent(
            new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }),
        );
        await flush();

        const after = document.querySelector('.data-row');
        expect(after.getAttribute('aria-expanded')).toBe('true');
    });

    it('Space collapses an expanded row; keys on inner controls are ignored', async () => {
        await loadFresh(defaultInvoke);
        mod.onDataPageEnter();
        await flush();
        await flush();

        // renderDataList rebuilds rows on each toggle — re-query before
        // every dispatch (the detached old row would not bubble to the list).
        document
            .querySelector('.data-row')
            .dispatchEvent(
                new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }),
            );
        await flush();
        expect(
            document.querySelector('.data-row').getAttribute('aria-expanded'),
        ).toBe('true');

        document
            .querySelector('.data-row')
            .dispatchEvent(
                new KeyboardEvent('keydown', { key: ' ', bubbles: true }),
            );
        await flush();
        expect(
            document.querySelector('.data-row').getAttribute('aria-expanded'),
        ).toBe('false');

        // Key events originating on an inner control must not toggle.
        const cb = document.querySelector('.data-row-cb');
        cb.dispatchEvent(
            new KeyboardEvent('keydown', { key: ' ', bubbles: true }),
        );
        await flush();
        expect(
            document.querySelector('.data-row').getAttribute('aria-expanded'),
        ).toBe('false');
    });
});

describe('audio load failure badge (P1 fix: render-state driven)', () => {
    it('failed audio renders a persistent badge that survives list rebuilds', async () => {
        await loadFresh((cmd) => {
            if (cmd === 'read_recording_audio') {
                return Promise.reject(new Error('file locked'));
            }
            return defaultInvoke(cmd);
        });
        mod.onDataPageEnter();
        await flush();
        await flush();

        // Click play → fetch fails → badge replaces the play button.
        document
            .querySelector('.btn-play')
            .dispatchEvent(new MouseEvent('click', { bubbles: true }));
        await flush();
        await flush();

        const row = document.querySelector('.data-row');
        expect(row.querySelector('.audio-error-badge').textContent).toBe(
            '音频加载失败',
        );
        expect(row.querySelector('.btn-play')).toBeNull();
    });
});

describe('undo restore report (restore_pending_delete {restored, failed})', () => {
    async function clickUndoWithReport(report) {
        await loadFresh((cmd) => {
            if (cmd === 'restore_pending_delete') {
                return Promise.resolve(report);
            }
            return defaultInvoke(cmd);
        });
        mod.onDataPageEnter();
        await flush();
        await flush();
        // Single-row delete → confirm dialog (danger) → soft-delete →
        // pending toast → 撤销 click → restore report renders in the bar.
        document
            .querySelector('.btn-delete')
            .dispatchEvent(new MouseEvent('click', { bubbles: true }));
        await flush();
        const confirmBtn = document.querySelector(
            '.dialog-actions .btn-danger',
        );
        confirmBtn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
        await flush();
        await flush();
        const undoBtn = document.getElementById('pending-toast-undo');
        undoBtn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
        await flush();
        await flush();
        return document.getElementById('data-error-bar');
    }

    it('full restore shows the neutral notice variant (not error red)', async () => {
        const bar = await clickUndoWithReport({ restored: 2, failed: 0 });
        expect(bar.textContent).toContain('已撤销删除，恢复 2 条');
        expect(bar.classList.contains('notice')).toBe(true);
    });

    it('partial restore surfaces the failed count in the error bar', async () => {
        const bar = await clickUndoWithReport({ restored: 1, failed: 1 });
        expect(bar.textContent).toContain('恢复 1 条');
        expect(bar.textContent).toContain('1 条恢复失败');
        expect(bar.classList.contains('notice')).toBe(false);
    });
});
