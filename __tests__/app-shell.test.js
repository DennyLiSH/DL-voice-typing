// @vitest-environment jsdom
//
// Behavior tests for app-shell.js — the settings window shell that had no
// dedicated test file: feedback link (opener plugin + failure recovery),
// help-page recent-errors rendering (auto-expand / empty / failure /
// stale-response guard), sidebar navigation (page-enter hooks and the
// dirty-state confirm gate), and the PAGE_HOOKS leave-hook dispatch
// (switching to the SAME page must not fire the leave hook; beforeunload
// must fire it unconditionally).
//
// Real-entry mode (Round 6 P4 Task 3): stub window.__TAURI__, load the
// REAL ui/settings.html via loadSettingsDom() (helpers.js), dynamically
// import settings.js (the real entry — importing it registers every
// module's listeners and runs init()), then drive the real handlers.
// The real HTML includes everything the old hand-copied DOM faked
// (#feedback-link, the model sidebar page) plus everything it lacked.
//
// The confirm-dialog mock is mutable: dirty-nav tests flip `confirmCtl`
// per case (the form-behavior file hardcodes `false`, too coarse here).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { loadSettingsDom, stubTauri } from './helpers.js';

const confirmCtl = { result: false, open: false };
vi.mock('../ui/lib/confirm-dialog.js', () => ({
    confirmDialog: vi.fn(async () => confirmCtl.result),
    isDialogOpen: () => confirmCtl.open,
}));

// The data-page enter/leave hooks are spied so the PAGE_HOOKS dispatch can
// be pinned PER CALL (call counts, not just "was called"). ui/data-manager.js
// exports exactly these two functions and app-shell.js imports only them,
// so the mock is complete — the real module's behavior is not needed here
// (its own lifecycle is covered by __tests__/data-manager.test.js).
const dataHooks = {
    enter: vi.fn(),
    leave: vi.fn(),
};
vi.mock('../ui/data-manager.js', () => ({
    onDataPageEnter: dataHooks.enter,
    onDataPageLeave: dataHooks.leave,
}));

let invokeMock;
let listeners;

async function loadFresh() {
    listeners = {};
    confirmCtl.result = false;
    confirmCtl.open = false;
    invokeMock = vi.fn(async (cmd) => {
        if (cmd === 'get_config') {
            return {
                language: 'zh',
                hotkey: { ctrl: false, shift: false, alt: false, vk: 163 },
                whisper_model: 'base',
                llm_enabled: false,
                llm_api_url: '',
                llm_api_key: '',
                llm_model: '',
                // Backend default (schema.rs DownloadMirror::HfMirror) — must
                // match a real <option> in settings.html or the dirty baseline
                // would light on first paint and gate every switchPage call
                // behind the (cancelled) confirm dialog.
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
        // Backend contract: get_last_errors always returns a (possibly
        // empty) list — never null. Navigation tests that don't care
        // about the history still trigger refreshErrorHistory on entry.
        if (cmd === 'get_last_errors') return [];
        return null;
    });

    stubTauri({
        event: {
            listen: vi.fn((evt, cb) => {
                listeners[evt] = cb;
                return () => {};
            }),
        },
        core: { invoke: invokeMock },
        app: { getVersion: vi.fn(async () => '0.0.0-test') },
        autostart: { enable: vi.fn(), disable: vi.fn() },
    });

    loadSettingsDom();

    vi.resetModules();
    await import('../ui/settings.js');

    await new Promise((r) => setTimeout(r, 10));
}

// Import the cached app-shell module after settings.js has loaded it, so
// tests can drive switchPage directly (same instance the sidebar uses).
// Introduced alongside the first describe that uses it.

const flush = () => new Promise((r) => setTimeout(r, 10));

async function shell() {
    return import('../ui/app-shell.js');
}

describe('app-shell: feedback link (help footer)', () => {
    beforeEach(async () => {
        await loadFresh();
    });

    afterEach(() => {
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
        vi.useRealTimers();
    });

    it('opens the system browser via the opener plugin and leaves the link untouched', async () => {
        const link = document.getElementById('feedback-link');
        link.dispatchEvent(new Event('click'));
        await flush();
        expect(invokeMock).toHaveBeenCalledWith('plugin:opener|open_url', {
            url: 'https://github.com/DennyLiSH/DL-voice-typing/issues',
        });
        expect(link.textContent).toBe('反馈问题');
    });

    it('failure swaps the text, recovers after 3s, and re-entry during recovery is a no-op', async () => {
        // loadFresh settles with REAL timers (its internal 10ms sleep) —
        // fake timers must be armed afterwards.
        vi.useFakeTimers();
        invokeMock.mockRejectedValueOnce(new Error('opener denied'));
        const link = document.getElementById('feedback-link');
        link.dispatchEvent(new Event('click'));
        await vi.advanceTimersByTimeAsync(0);
        expect(link.textContent).toBe('打开失败，请手动访问 GitHub Issues');

        // A second failure while the 3s recovery window is pending must
        // not re-arm the swap nor reset the timer (recovering guard).
        invokeMock.mockRejectedValueOnce(new Error('again'));
        link.dispatchEvent(new Event('click'));
        await vi.advanceTimersByTimeAsync(0);
        expect(link.textContent).toBe('打开失败，请手动访问 GitHub Issues');

        await vi.advanceTimersByTimeAsync(3000);
        expect(link.textContent).toBe('反馈问题');
        expect(link.dataset.recovering).toBeUndefined();
    });
});

describe('app-shell: help-page recent errors', () => {
    beforeEach(async () => {
        await loadFresh();
    });

    afterEach(() => {
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
    });

    it('renders 5 records and auto-expands the section', async () => {
        const events = [
            'speech-error',
            'llm-error',
            'injection-error',
            'record-only-error',
            'hotkey-error',
        ];
        invokeMock.mockResolvedValueOnce(
            events.map((event, i) => ({
                event,
                message: `错误 ${i + 1}`,
                timestamp: `2026-10-19 10:0${i}:00+08:00`,
            })),
        );
        const { switchPage } = await shell();
        await switchPage('help');
        await flush(); // switchPage does not await refreshErrorHistory

        const section = document.getElementById('help-errors-section');
        expect(section.open).toBe(true);
        const items = document.querySelectorAll(
            '#error-history-list .error-history-item',
        );
        expect(items).toHaveLength(5);
        expect(items[0].querySelector('.badge').textContent).toBe('语音识别');
        expect(items[1].querySelector('.badge').textContent).toBe('LLM 纠错');
        expect(items[4].querySelector('.badge').textContent).toBe('热键注册');
        expect(
            items[0].querySelector('.error-history-message').textContent,
        ).toBe('错误 1');
    });

    it('empty history shows the hint and stays collapsed', async () => {
        invokeMock.mockResolvedValueOnce([]);
        const { switchPage } = await shell();
        await switchPage('help');
        await flush(); // switchPage does not await refreshErrorHistory
        const section = document.getElementById('help-errors-section');
        expect(section.open).toBe(false);
        expect(
            document.getElementById('error-history-list').textContent,
        ).toContain('暂无错误记录');
    });

    it('backend failure shows the load-failure hint', async () => {
        invokeMock.mockRejectedValueOnce(new Error('backend down'));
        const { switchPage } = await shell();
        await switchPage('help');
        await flush(); // switchPage does not await refreshErrorHistory
        expect(
            document.getElementById('error-history-list').textContent,
        ).toContain('无法加载错误记录');
        expect(document.getElementById('help-errors-section').open).toBe(false);
    });

    it('stale response from a previous help visit cannot overwrite a newer render', async () => {
        // Visit 1 starts a gated fetch (A); user leaves and returns —
        // visit 2's fetch (B) resolves first. When A finally resolves it
        // must be discarded (errorHistorySeq guard).
        let releaseA;
        const gateA = new Promise((resolve) => {
            releaseA = () =>
                resolve([
                    {
                        event: 'speech-error',
                        message: 'STALE',
                        timestamp: 't',
                    },
                ]);
        });
        invokeMock.mockImplementationOnce(() => gateA);
        const { switchPage } = await shell();
        switchPage('help'); // A pending
        await switchPage('general'); // away
        invokeMock.mockResolvedValueOnce([]);
        switchPage('help'); // B (fast)
        await flush();
        expect(
            document.getElementById('error-history-list').textContent,
        ).toContain('暂无错误记录'); // B's render stands

        releaseA();
        await flush();
        expect(
            document.getElementById('error-history-list').textContent,
        ).toContain('暂无错误记录'); // A did not overwrite
    });
});

const activePage = () =>
    document.querySelector('.sidebar-item.active')?.dataset.page;

const computeModeCalls = () =>
    invokeMock.mock.calls.filter(([cmd]) => cmd === 'get_compute_mode').length;

describe('app-shell: sidebar navigation', () => {
    beforeEach(async () => {
        await loadFresh();
    });

    afterEach(() => {
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
    });

    it('entering the model page re-runs compute-mode detection (leave and return)', async () => {
        // init() already ran one detection — route every get_compute_mode
        // to 'gpu' without disturbing the config commands.
        const originalImpl = invokeMock.getMockImplementation();
        invokeMock.mockImplementation(async (cmd, args) => {
            if (cmd === 'get_compute_mode') return 'gpu';
            return originalImpl(cmd, args);
        });
        const { switchPage } = await shell();
        expect(computeModeCalls()).toBe(1); // init baseline

        await switchPage('model');
        expect(computeModeCalls()).toBe(2);

        await switchPage('general');
        expect(computeModeCalls()).toBe(2); // leaving does not re-run

        await switchPage('model');
        expect(computeModeCalls()).toBe(3); // re-enter re-runs
        expect(document.getElementById('compute-mode-badge').textContent).toBe(
            'GPU 加速',
        );
    });

    it('switching with unsaved edits prompts; cancel stays, confirm proceeds', async () => {
        const input = document.getElementById('api-url');
        input.value = 'https://api.example.com/v1';
        input.dispatchEvent(new Event('input')); // form is now dirty
        const { switchPage } = await shell();

        confirmCtl.result = false; // user cancels the confirm dialog
        await switchPage('help');
        expect(activePage()).toBe('general');

        confirmCtl.result = true; // user confirms leaving
        await switchPage('help');
        expect(activePage()).toBe('help');
        expect(
            document
                .querySelector('.sidebar-item[data-page="help"]')
                .classList.contains('active'),
        ).toBe(true);
    });

    it('navigation requests are dropped while a dialog is open (modal short-circuit)', async () => {
        confirmCtl.open = true;
        const { switchPage } = await shell();
        await switchPage('help');
        expect(activePage()).toBe('general'); // dropped, no dialog, no switch
    });
});

describe('app-shell: PAGE_HOOKS leave-hook dispatch', () => {
    // The leave hook has TWO dispatch sites with DIFFERENT guards:
    //   - switchPage: `currentPage !== pageName` — switching to the SAME
    //     page is not leaving, so the leave hook must NOT fire;
    //   - beforeunload: no guard — leaving the window always leaves the
    //     current page, so the leave hook MUST fire.
    // Both are pinned here per call (call counts), because a refactor that
    // drops either guard would silently change the semantics.

    beforeEach(async () => {
        await loadFresh();
        dataHooks.enter.mockClear();
        dataHooks.leave.mockClear();
    });

    afterEach(async () => {
        // Normalize back to a page WITHOUT a leave hook. The settings
        // entry registers a `beforeunload` listener on the SHARED jsdom
        // window at import time, and vi.resetModules() re-evaluates
        // app-shell.js WITHOUT removing the previous instance's listener.
        // A stale instance left on the data page would fire the leave hook
        // again during a later beforeunload dispatch and inflate the call
        // count — so every instance is parked on 'general' here.
        const { switchPage } = await shell();
        await switchPage('general');
        dataHooks.enter.mockClear();
        dataHooks.leave.mockClear();
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
    });

    it('switching to the SAME page does not fire the leave hook', async () => {
        const { switchPage } = await shell();
        await switchPage('data');
        expect(dataHooks.enter).toHaveBeenCalledTimes(1);
        expect(dataHooks.leave).toHaveBeenCalledTimes(0);

        // Same page again: the `currentPage !== pageName` guard must
        // suppress the leave hook (the enter hook still runs).
        await switchPage('data');
        expect(dataHooks.leave).toHaveBeenCalledTimes(0);
        expect(dataHooks.enter).toHaveBeenCalledTimes(2);
    });

    it('leaving the data page fires the leave hook exactly once', async () => {
        const { switchPage } = await shell();
        await switchPage('data');
        dataHooks.leave.mockClear();
        await switchPage('general');
        expect(dataHooks.leave).toHaveBeenCalledTimes(1);
    });

    it('beforeunload fires the leave hook of the current page', async () => {
        const { switchPage } = await shell();
        await switchPage('data');
        dataHooks.leave.mockClear();
        window.dispatchEvent(new Event('beforeunload'));
        expect(dataHooks.leave).toHaveBeenCalledTimes(1);
    });

    it('beforeunload on a page without a leave hook fires nothing', async () => {
        const { switchPage } = await shell();
        await switchPage('help');
        dataHooks.leave.mockClear();
        window.dispatchEvent(new Event('beforeunload'));
        expect(dataHooks.leave).toHaveBeenCalledTimes(0);
    });
});
