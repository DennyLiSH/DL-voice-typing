// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';

// Contract tests for the shared recording-list module (C6).
// jsdom lacks URL.createObjectURL/revokeObjectURL — stub before import,
// following the loadFresh pattern in transcribe.test.js.

let invokeMock;
let mod;

async function loadFresh(invokeImpl) {
    invokeMock = vi.fn(invokeImpl ?? (() => Promise.resolve(null)));
    vi.stubGlobal('__TAURI__', { core: { invoke: invokeMock } });
    URL.createObjectURL = vi.fn(() => 'blob:mock-url');
    URL.revokeObjectURL = vi.fn();
    vi.resetModules();
    mod = await import('../ui/lib/recordings.js');
}

afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
});

describe('loadRecordings', () => {
    it('passes offset/limit/query through and returns the response', async () => {
        const resp = { items: [{ filename: 'a' }], total: 1 };
        await loadFresh(() => Promise.resolve(resp));
        const out = await mod.loadRecordings({
            offset: 10,
            limit: 50,
            query: '会议',
        });
        expect(out).toBe(resp);
        expect(invokeMock).toHaveBeenCalledWith('list_saved_recordings', {
            offset: 10,
            limit: 50,
            query: '会议',
        });
    });

    it('defaults query to null', async () => {
        await loadFresh();
        await mod.loadRecordings({ offset: 0, limit: 200 });
        expect(invokeMock).toHaveBeenCalledWith('list_saved_recordings', {
            offset: 0,
            limit: 200,
            query: null,
        });
    });

    it('normalizes a raw-string rejection to an Error with that message', async () => {
        await loadFresh((cmd) =>
            cmd === 'list_saved_recordings'
                ? Promise.reject('磁盘读取失败')
                : Promise.resolve(null),
        );
        const err = await mod
            .loadRecordings({ offset: 0, limit: 50 })
            .catch((e) => e);
        expect(err).toBeInstanceOf(Error);
        expect(err.message).toBe('磁盘读取失败');
    });

    it('normalizes a CommandError-shaped rejection to its message', async () => {
        await loadFresh((cmd) =>
            cmd === 'list_saved_recordings'
                ? Promise.reject({ code: 'io', message: '读取失败' })
                : Promise.resolve(null),
        );
        await expect(
            mod.loadRecordings({ offset: 0, limit: 50 }),
        ).rejects.toThrow('读取失败');
    });

    it('falls back to a Chinese message for unknown error shapes', async () => {
        await loadFresh((cmd) =>
            cmd === 'list_saved_recordings'
                ? Promise.reject(42)
                : Promise.resolve(null),
        );
        await expect(
            mod.loadRecordings({ offset: 0, limit: 50 }),
        ).rejects.toThrow('加载失败');
    });
});

describe('attachAudio', () => {
    it('accepts Uint8Array and sets dataset.blobUrl + src', async () => {
        await loadFresh();
        const el = document.createElement('audio');
        mod.attachAudio(el, new Uint8Array([1, 2, 3]));
        expect(URL.createObjectURL).toHaveBeenCalledTimes(1);
        expect(el.dataset.blobUrl).toBe('blob:mock-url');
        expect(el.getAttribute('src')).toBe('blob:mock-url');
    });

    it('accepts ArrayLike<number> (Tauri serde shape)', async () => {
        await loadFresh();
        const el = document.createElement('audio');
        mod.attachAudio(el, [1, 2, 3]);
        expect(el.dataset.blobUrl).toBe('blob:mock-url');
    });

    it('is a no-op for a null element', async () => {
        await loadFresh();
        expect(() => mod.attachAudio(null, [1])).not.toThrow();
        expect(URL.createObjectURL).not.toHaveBeenCalled();
    });
});

describe('releaseAudio', () => {
    it('pauses, revokes the blob URL, clears dataset and src', async () => {
        await loadFresh();
        const el = document.createElement('audio');
        const pauseSpy = vi.spyOn(el, 'pause').mockImplementation(() => {});
        mod.attachAudio(el, [1, 2]);
        mod.releaseAudio(el);
        expect(pauseSpy).toHaveBeenCalled();
        expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:mock-url');
        expect(el.dataset.blobUrl).toBeUndefined();
        expect(el.getAttribute('src')).toBeNull();
    });

    it('is idempotent — a second call neither throws nor revokes again', async () => {
        await loadFresh();
        const el = document.createElement('audio');
        mod.attachAudio(el, [1, 2]);
        mod.releaseAudio(el);
        expect(() => mod.releaseAudio(el)).not.toThrow();
        expect(URL.revokeObjectURL).toHaveBeenCalledTimes(1);
    });

    it('tolerates a missing blobUrl and a null element', async () => {
        await loadFresh();
        const el = document.createElement('audio');
        expect(() => mod.releaseAudio(el)).not.toThrow();
        expect(() => mod.releaseAudio(null)).not.toThrow();
        expect(URL.revokeObjectURL).not.toHaveBeenCalled();
    });

    it('still revokes when pause() throws (detached element)', async () => {
        await loadFresh();
        const el = document.createElement('audio');
        el.pause = () => {
            throw new Error('detached');
        };
        mod.attachAudio(el, [1, 2]);
        expect(() => mod.releaseAudio(el)).not.toThrow();
        expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:mock-url');
    });
});
