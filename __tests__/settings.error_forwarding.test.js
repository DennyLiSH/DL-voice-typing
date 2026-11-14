// @vitest-environment jsdom
//
// Integration tests for the 4 frontend error-forwarding catch blocks added
// in ui/settings.js (save_settings, test_llm_connection, download_whisper_model,
// delete_custom_model) plus the F6 fix (cancel-shape compatibility).
//
// Real-entry mode (Round 6 P4 Task 3): stub window.__TAURI__, load the
// REAL ui/settings.html via loadSettingsDom() (helpers.js), dynamically
// import settings.js. The old hand-copied fixture carried a comment
// warning that missing data-manager DOM made wireDataListEvents() skip
// binding — with the real HTML every element exists, so that risk class
// is gone at the source.
//
// Coverage note (per plan §前端测试改动 降级策略):
// - test_llm_connection + delete_custom_model: jsdom-integration triggered
//   via real DOM events (click handlers exercise the catch blocks end-to-end).
// - save_settings + download_whisper_model: covered indirectly via static
//   contract assertions (the log_frontend_error call + F6 fix expression
//   must be present in settings.js source). Triggering these paths requires
//   complex UI state (isDirty + full validation pass / activeDownload state),
//   so manual E2E remains authoritative for them.
// - F6 cancel-shape: pure contract test on the isCancel expression.
import {
    afterEach,
    beforeAll,
    beforeEach,
    describe,
    expect,
    it,
    vi,
} from 'vitest';

import { loadSettingsDom, stubTauri } from './helpers.js';

// Deletion confirmations go through the shared in-app dialog.
vi.mock('../ui/lib/confirm-dialog.js', () => ({
    confirmDialog: vi.fn(async () => false),
    isDialogOpen: () => false,
}));

let invokeMock;
let listeners;

async function loadFresh() {
    listeners = {};
    // Default mock returns plausible values for init() chain:
    //   get_config → minimal config; get_whisper_models → empty;
    //   is_autostart_available → true; otherwise null.
    // Per-test overrides use mockImplementationOnce / mockRejectedValueOnce.
    invokeMock = vi.fn(async (cmd) => {
        if (cmd === 'get_config') {
            return {
                language: 'zh',
                hotkey: {
                    ctrl: false,
                    shift: false,
                    alt: false,
                    vk: 163,
                },
                whisper_model: 'base',
                llm_enabled: false,
                llm_api_url: '',
                llm_api_key: '',
                llm_model: '',
                // Backend default (schema.rs DownloadMirror::HfMirror) — must
                // match a real <option> in settings.html or the dirty baseline
                // would light on first paint.
                download_mirror: 'hf-mirror',
                data_saving_enabled: false,
                data_saving_path: '',
                review_before_paste: false,
                autostart: false,
                realtime_transcription: false,
                record_only_enabled: false,
                record_only_hotkey: {
                    ctrl: false,
                    shift: false,
                    alt: false,
                    vk: 165,
                },
            };
        }
        if (cmd === 'get_whisper_models') {
            return { built_in: { base: true }, custom: [] };
        }
        if (cmd === 'is_autostart_available') return true;
        return null;
    });

    // stubTauri (helpers.js) is the suite's default __TAURI__ stub path;
    // direct `global.window.__TAURI__` assignment stays fragile under
    // jsdom 25/29 (global.window may not exist when entry modules load).
    stubTauri({
        event: {
            listen: vi.fn((evt, cb) => {
                listeners[evt] = cb;
                return () => {};
            }),
        },
        core: { invoke: invokeMock },
        app: { getVersion: vi.fn(async () => '0.0.0-test') },
    });

    loadSettingsDom();

    vi.resetModules();
    await import('../ui/settings.js');

    // Allow the top-level init() IIFE to settle so its invokes don't leak
    // across tests as unhandled rejections.
    await new Promise((r) => setTimeout(r, 10));
}

beforeEach(async () => {
    await loadFresh();
});

afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
});

/** Find the first invoke call matching cmd name; returns undefined if absent. */
function findInvokeCall(cmd) {
    return invokeMock.mock.calls.find(([c]) => c === cmd);
}

describe('settings.js error forwarding to log_frontend_error', () => {
    describe('test_llm_connection catch block (integration via DOM click)', () => {
        it('forwards to log_frontend_error with context=test_llm_connection', async () => {
            // Pre-fill LLM fields so the click handler's validation passes
            // and reaches the invoke('test_llm_connection') call.
            document.getElementById('api-url').value =
                'http://invalid.example.com';
            document.getElementById('api-key').value = 'fake-key';
            document.getElementById('model').value = 'fake-model';

            // Override: next invoke matching 'test_llm_connection' rejects.
            // Then 'log_frontend_error' should resolve (fire-and-forget).
            const originalImpl = invokeMock.getMockImplementation();
            invokeMock.mockImplementation(async (cmd) => {
                if (cmd === 'test_llm_connection') {
                    throw new Error('connect ECONNREFUSED');
                }
                if (cmd === 'log_frontend_error') return null;
                return originalImpl(cmd);
            });

            document
                .getElementById('test-btn')
                .dispatchEvent(new Event('click'));
            await new Promise((r) => setTimeout(r, 10));

            const logCall = findInvokeCall('log_frontend_error');
            expect(logCall).toBeDefined();
            expect(logCall[1]).toMatchObject({
                context: 'test_llm_connection',
            });
            expect(logCall[1].message).toContain('connect ECONNREFUSED');
        });

        it('UI feedback (setTestStatus error) still fires alongside forwarding', async () => {
            document.getElementById('api-url').value =
                'http://invalid.example.com';
            document.getElementById('api-key').value = 'fake-key';
            document.getElementById('model').value = 'fake-model';

            invokeMock.mockImplementation(async (cmd) => {
                if (cmd === 'test_llm_connection') throw new Error('net');
                if (cmd === 'log_frontend_error') return null;
                return null;
            });

            document
                .getElementById('test-btn')
                .dispatchEvent(new Event('click'));
            await new Promise((r) => setTimeout(r, 10));

            const status = document.getElementById('test-status');
            // Error detail passthrough: the backend message ('net') is
            // surfaced instead of a fixed '连接失败' string.
            expect(status.textContent).toContain('✗ net');
            expect(status.className).toContain('error');
        });
    });

    describe('F6 fix — defensive cancel detection (contract)', () => {
        // The F6 fix in settings.js (~line 357) reads:
        //   const isCancel = e === 'download cancelled' || e?.message === 'download cancelled';
        // These tests verify that expression matches both raw-string and
        // CommandError-object shapes. If Tauri 2.x changes how CommandError
        // is serialized to the frontend, these tests will flag the regression.
        const isCancel = (e) =>
            e === 'download cancelled' || e?.message === 'download cancelled';

        it('matches raw string shape', () => {
            expect(isCancel('download cancelled')).toBe(true);
        });

        it('matches CommandError object shape { message }', () => {
            expect(isCancel({ message: 'download cancelled' })).toBe(true);
        });

        it('matches Error instance with .message', () => {
            expect(isCancel(new Error('download cancelled'))).toBe(true);
        });

        it('does NOT match real failure (different message)', () => {
            expect(isCancel('network error')).toBe(false);
            expect(isCancel({ message: 'ECONNREFUSED' })).toBe(false);
        });

        it('does NOT match null / undefined / empty', () => {
            expect(isCancel(null)).toBe(false);
            expect(isCancel(undefined)).toBe(false);
            expect(isCancel({})).toBe(false);
        });
    });

    describe('static contract — call/rawInvoke shapes after api.js wrapper adoption', () => {
        // Static source assertions: after routing invoke through ui/lib/api.js,
        // the 4 user-initiated catch sites no longer contain log_frontend_error
        // boilerplate. Instead, error forwarding is centralized in api.js's
        // call() / reportError(). These tests verify the new call shapes.
        let settingsFormSrc;
        let modelManagerSrc;
        beforeAll(async () => {
            const { readFileSync } = await import('node:fs');
            const { fileURLToPath } = await import('node:url');
            const path = await import('node:path');
            const here = fileURLToPath(import.meta.url);
            settingsFormSrc = readFileSync(
                path.resolve(
                    path.dirname(here),
                    '..',
                    'ui',
                    'settings-form.js',
                ),
                'utf8',
            );
            modelManagerSrc = readFileSync(
                path.resolve(
                    path.dirname(here),
                    '..',
                    'ui',
                    'model-manager.js',
                ),
                'utf8',
            );
        });

        it('save_settings invoked via call(cmd, args)', () => {
            expect(settingsFormSrc).toMatch(/call\(['"]save_settings['"]/);
        });

        it('test_llm_connection invoked via call', () => {
            expect(settingsFormSrc).toMatch(
                /call\(['"]test_llm_connection['"]/,
            );
        });

        it('delete_custom_model invoked via call', () => {
            expect(modelManagerSrc).toMatch(
                /call\(['"]delete_custom_model['"]/,
            );
        });

        it('download_whisper_model uses rawInvoke + reportError (Option B)', () => {
            // rawInvoke preserves the cancel-silent invariant; reportError only
            // fires in the error branch (semantically equivalent to the old
            // log_frontend_error boilerplate).
            expect(modelManagerSrc).toMatch(
                /rawInvoke\(['"]download_whisper_model['"]/,
            );
            expect(modelManagerSrc).toMatch(
                /reportError\([^,]+,\s*['"]download_whisper_model['"]\)/,
            );
        });

        it('F6 fix expression preserved', () => {
            expect(modelManagerSrc).toContain(
                "e === 'download cancelled' || e?.message === 'download cancelled'",
            );
        });

        it('fire-and-forget .catch(() => {}) removed from settings-form + model-manager', () => {
            // The boilerplate catch (() => {}) was the mark of inline
            // log_frontend_error forwarding; with call()/reportError() handling
            // it centrally, no inline sites should remain.
            const combined = settingsFormSrc + modelManagerSrc;
            const matches = combined.match(/\.catch\(\(\) => \{\}\)/g) || [];
            expect(matches.length).toBe(0);
        });
    });

    describe('data-manager event binding smoke test', () => {
        // Historical regression guard: an early hand-copied fixture lacked
        // the data-page elements, so wireDataListEvents() silently skipped
        // binding (real bugs slipped through). The real settings.html always
        // carries them — a click on btn-refresh-data must trigger an
        // invoke('list_saved_recordings').
        it('clicking #btn-refresh-data triggers list_saved_recordings invoke', async () => {
            // Reset mock to clear init() noise.
            invokeMock.mockClear();
            // Provide a plausible response so the load does not error out.
            invokeMock.mockImplementation(async (cmd) => {
                if (cmd === 'list_saved_recordings') {
                    return { items: [], total: 0, total_bytes: 0 };
                }
                return null;
            });

            document
                .getElementById('btn-refresh-data')
                .dispatchEvent(new Event('click'));

            // loadRecordingsPage is async; let the invoke promise settle.
            await new Promise((r) => setTimeout(r, 10));

            const calls = invokeMock.mock.calls.map((c) => c[0]);
            expect(calls).toContain('list_saved_recordings');
        });
    });
});
