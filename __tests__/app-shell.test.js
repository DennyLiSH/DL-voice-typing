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
  <div class="sidebar-item" data-page="general"></div>
  <div class="sidebar-item" data-page="help"></div>
  <div class="sidebar-item" data-page="model"></div>
  <div class="page-content" id="page-general"></div>
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
