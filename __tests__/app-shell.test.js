// @vitest-environment jsdom
//
// Behavior tests for app-shell.js — the settings window shell that had no
// dedicated test file: feedback link (opener plugin + failure recovery),
// help-page recent-errors rendering (auto-expand / empty / failure /
// stale-response guard), and sidebar navigation (page-enter hooks and the
// dirty-state confirm gate).
//
// Pattern follows settings.form-behavior.test.js: stub window.__TAURI__,
// inject a DOM subtree, dynamically import settings.js (the real entry —
// importing it registers every module's listeners and runs init()), then
// drive the real handlers.
//
// The confirm-dialog mock is mutable: dirty-nav tests flip `confirmCtl`
// per case (the form-behavior file hardcodes `false`, too coarse here).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const confirmCtl = { result: false, open: false };
vi.mock('../ui/lib/confirm-dialog.js', () => ({
    confirmDialog: vi.fn(async () => confirmCtl.result),
    isDialogOpen: () => confirmCtl.open,
}));

let invokeMock;
let listeners;

// Base = MINIMAL_DOM from settings.form-behavior.test.js, plus:
//   #feedback-link (app-shell feedback handler)
//   sidebar-item/page for "model" (page-enter hook tests)
const DOM = `
  <div id="container" class="content-area"></div>
  <div class="sidebar"></div>
  <div class="sidebar-item active" data-page="general"></div>
  <div class="sidebar-item" data-page="help"></div>
  <div class="sidebar-item" data-page="model"></div>
  <div class="page-content active" id="page-general"></div>
  <div class="page-content" id="page-help"></div>
  <div class="page-content" id="page-model"></div>
  <details class="help-section" id="help-errors-section"><summary>最近错误</summary><div class="help-section-body"><div id="error-history-list" aria-live="polite"></div></div></details>
  <a id="feedback-link" href="#">反馈问题</a>
  <select id="language"><option value="zh"></option></select>
  <div id="hotkey-combo">
    <input type="checkbox" id="hotkey-ctrl" />
    <input type="checkbox" id="hotkey-shift" />
    <input type="checkbox" id="hotkey-alt" />
    <select id="hotkey">
      <option value="RightCtrl">RightCtrl</option>
      <option value="RightAlt">RightAlt</option>
      <option value="F9">F9</option>
    </select>
  </div>
  <div id="hotkey-preview" aria-live="polite"></div>
  <select id="whisper-model"><option value="base"></option></select>
  <div id="model-status-text"></div>
  <button id="btn-download-model"></button>
  <div id="download-progress"></div>
  <div id="progress-fill"></div>
  <div id="progress-percent"></div>
  <button id="btn-cancel-download"></button>
  <div id="llm-toggle"></div>
  <div id="llm-fields"></div>
  <input id="api-url" />
  <div id="api-url-warning" hidden></div>
  <input id="api-key" />
  <input id="model" />
  <button id="toggle-key"></button>
  <button id="test-btn"></button>
  <div id="test-status"></div>
  <button id="save-btn"></button>
  <div id="save-status"></div>
  <div id="error-banner"></div>
  <select id="download-mirror"><option value="Official"></option></select>
  <div id="data-saving-toggle"></div>
  <div id="data-saving-fields"></div>
  <input id="data-saving-path" />
  <button id="btn-browse-path"></button>
  <div id="review-toggle"></div>
  <div id="autostart-toggle"></div>
  <div id="realtime-transcription-toggle"></div>
  <div id="record-only-toggle"></div>
  <div id="record-only-hotkey-group">
    <input type="checkbox" id="record-only-hotkey-ctrl" />
    <input type="checkbox" id="record-only-hotkey-shift" />
    <input type="checkbox" id="record-only-hotkey-alt" />
    <select id="record-only-hotkey">
      <option value="RightAlt">RightAlt</option>
      <option value="F9">F9</option>
    </select>
  </div>
  <div id="record-only-hotkey-preview" aria-live="polite"></div>
  <div id="version-display"></div>
  <div id="compute-mode-badge"></div>
  <div id="data-error-bar"></div>
  <button id="btn-refresh-data"></button>
  <input id="data-search-input" />
  <div id="data-list"></div>
  <button id="btn-batch-delete"></button>
  <input type="checkbox" id="data-select-all-cb" />
  <button id="btn-prev-page"></button>
  <button id="btn-next-page"></button>
  <span id="data-total-count"></span>
  <span id="data-total-size"></span>
  <span id="data-range-start"></span>
  <span id="data-range-end"></span>
`;

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
                download_mirror: 'Official',
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

    vi.stubGlobal('__TAURI__', {
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

    document.body.innerHTML = DOM;

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
