// @vitest-environment jsdom
//
// Unit tests for ui/lib/api.js wrapper.
// Mock pattern: vi.stubGlobal('__TAURI__', ...) — matches the project's
// existing pattern in __tests__/settings.error_forwarding.test.js:118.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

let invokeMock;

function setupTauriMock() {
    invokeMock = vi.fn();
    vi.stubGlobal('__TAURI__', {
        core: { invoke: invokeMock },
    });
}

describe('ui/lib/api.js wrapper', () => {
    beforeEach(() => {
        setupTauriMock();
        // resetModules BEFORE each import — required because ui/lib/api.js
        // captures `window.__TAURI__.core.invoke` at module load time, so
        // each test needs a fresh module re-execute bound to the new mock.
        // Pattern matches __tests__/settings.error_forwarding.test.js.
        vi.resetModules();
    });

    afterEach(() => {
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
    });

    describe('rawInvoke', () => {
        it('re-exports window.__TAURI__.core.invoke directly', async () => {
            const { rawInvoke } = await import('../../ui/lib/api.js');
            expect(rawInvoke).toBe(invokeMock);
        });
    });

    describe('call', () => {
        it('returns the invoke result on success', async () => {
            invokeMock.mockResolvedValueOnce('ok');
            const { call } = await import('../../ui/lib/api.js');
            const result = await call('some_cmd', { x: 1 });
            expect(result).toBe('ok');
            expect(invokeMock).toHaveBeenCalledWith('some_cmd', { x: 1 });
        });

        it('does not call log_frontend_error on success', async () => {
            invokeMock.mockResolvedValueOnce('ok');
            const { call } = await import('../../ui/lib/api.js');
            await call('some_cmd');
            expect(invokeMock).toHaveBeenCalledTimes(1);
        });

        it('reports error and re-throws on failure', async () => {
            const error = { message: 'fail' };
            invokeMock.mockRejectedValueOnce(error);
            invokeMock.mockResolvedValueOnce(undefined);

            const { call } = await import('../../ui/lib/api.js');
            await expect(
                call('save_settings', {}, 'save_settings'),
            ).rejects.toEqual(error);
            expect(invokeMock).toHaveBeenCalledTimes(2);
            expect(invokeMock).toHaveBeenNthCalledWith(
                2,
                'log_frontend_error',
                {
                    message: 'fail',
                    stack: null,
                    context: 'save_settings',
                },
            );
        });

        it('defaults context to cmd name when not provided', async () => {
            const error = new Error('boom');
            invokeMock.mockRejectedValueOnce(error);
            invokeMock.mockResolvedValueOnce(undefined);

            const { call } = await import('../../ui/lib/api.js');
            await expect(call('test_cmd')).rejects.toThrow('boom');
            expect(invokeMock).toHaveBeenNthCalledWith(
                2,
                'log_frontend_error',
                {
                    message: 'boom',
                    stack: error.stack,
                    context: 'test_cmd',
                },
            );
        });

        it('handles string errors (no .message property)', async () => {
            invokeMock.mockRejectedValueOnce('plain string error');
            invokeMock.mockResolvedValueOnce(undefined);

            const { call } = await import('../../ui/lib/api.js');
            await expect(call('cmd')).rejects.toBe('plain string error');
            expect(invokeMock).toHaveBeenNthCalledWith(
                2,
                'log_frontend_error',
                {
                    message: 'plain string error',
                    stack: null,
                    context: 'cmd',
                },
            );
        });

        it('uses rawInvoke for log_frontend_error call (no recursion if it fails)', async () => {
            invokeMock.mockRejectedValueOnce(new Error('original'));
            invokeMock.mockRejectedValueOnce(new Error('log write failed'));

            const { call } = await import('../../ui/lib/api.js');
            await expect(call('cmd')).rejects.toThrow('original');
            expect(invokeMock).toHaveBeenCalledTimes(2);
        });
    });

    describe('reportError', () => {
        it('swallows log_frontend_error failures silently', async () => {
            invokeMock.mockRejectedValueOnce(new Error('log write failed'));
            const { reportError } = await import('../../ui/lib/api.js');
            await expect(
                reportError(new Error('x'), 'ctx'),
            ).resolves.toBeUndefined();
        });

        it('passes Error.stack through', async () => {
            invokeMock.mockResolvedValueOnce(undefined);
            const err = new Error('boom');
            const { reportError } = await import('../../ui/lib/api.js');
            await reportError(err, 'ctx');
            expect(invokeMock).toHaveBeenCalledWith('log_frontend_error', {
                message: 'boom',
                stack: err.stack,
                context: 'ctx',
            });
        });

        it('handles null error gracefully', async () => {
            invokeMock.mockResolvedValueOnce(undefined);
            const { reportError } = await import('../../ui/lib/api.js');
            await reportError(null, 'ctx');
            expect(invokeMock).toHaveBeenCalledWith('log_frontend_error', {
                message: 'null',
                stack: null,
                context: 'ctx',
            });
        });
    });
});
