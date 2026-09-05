// @vitest-environment jsdom
//
// Behavior tests for the settings save flow's dirty-state recalculation.
//
// Regression target: the save success path used to call setFormDirty(false),
// which force-cleared the dirty flag even when the user edited an input
// during the autostart await window — the switchPage/beforeunload guards
// would then let those edits be lost. The fix recomputes via
// updateDirtyState() (current DOM vs the just-saved snapshot).
//
// Follows the loadFresh pattern from settings.error_forwarding.test.js:
// stub window.__TAURI__, inject a minimal DOM subtree, dynamically import
// settings.js, then drive the real save click handler.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// switchPage's dirty guard goes through the shared in-app dialog.
vi.mock('../ui/lib/confirm-dialog.js', () => ({
    confirmDialog: vi.fn(async () => false),
    isDialogOpen: () => false,
}));

let invokeMock;
let listeners;

const MINIMAL_DOM = `
  <div id="container"></div>
  <div class="sidebar"></div>
  <div class="sidebar-item" data-page="general"></div>
  <div class="sidebar-item" data-page="help"></div>
  <div class="page-content" id="page-general"></div>
  <div class="page-content" id="page-help"></div>
  <div id="error-history-list"></div>
  <select id="language"><option value="zh"></option></select>
  <select id="hotkey"><option value="RightCtrl"></option></select>
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
  <div id="record-only-hotkey-group"></div>
  <select id="record-only-hotkey"><option value="RightAlt"></option></select>
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
    invokeMock = vi.fn(async (cmd) => {
        if (cmd === 'get_config') {
            return {
                language: 'zh',
                hotkey: 'RightCtrl',
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
                record_only_hotkey: 'RightAlt',
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

    document.body.innerHTML = MINIMAL_DOM;

    vi.resetModules();
    await import('../ui/settings.js');

    await new Promise((r) => setTimeout(r, 10));
}

function setInputValue(id, value) {
    const el = document.getElementById(id);
    el.value = value;
    el.dispatchEvent(new Event('input'));
}

function clickSave() {
    document.getElementById('save-btn').dispatchEvent(new Event('click'));
}

const flush = () => new Promise((r) => setTimeout(r, 10));

describe('settings save flow dirty-state recalculation', () => {
    beforeEach(async () => {
        await loadFresh();
    });

    afterEach(() => {
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
    });

    it('edits during the autostart await window keep the form dirty (race fix)', async () => {
        // Make the form dirty so the save button is actionable.
        setInputValue('api-url', 'https://api.example.com/v1');

        // Hold the autostart-availability invoke open so we can edit inside
        // the await window that used to force-clear the dirty flag.
        let releaseAutostart;
        const autostartGate = new Promise((resolve) => {
            releaseAutostart = () => resolve(false);
        });
        const originalImpl = invokeMock.getMockImplementation();
        invokeMock.mockImplementation(async (cmd, args) => {
            if (cmd === 'is_autostart_available') return autostartGate;
            return originalImpl(cmd, args);
        });

        clickSave();
        // Let the handler pass save_settings and reach the autostart await.
        await flush();

        // User keeps typing while the save pipeline is still in flight.
        setInputValue('api-key', 'sk-edited-during-save');

        releaseAutostart();
        await flush();

        const { isFormDirty } = await import('../ui/lib/form-state.js');
        expect(isFormDirty()).toBe(true);

        // The save itself succeeded — the success message must survive the
        // updateDirtyState() call (which clears the status line internally).
        expect(document.getElementById('save-status').textContent).toContain(
            '已保存',
        );
    });

    it('no edits during save → form is clean afterwards (regression guard)', async () => {
        setInputValue('api-url', 'https://api.example.com/v1');

        const originalImpl = invokeMock.getMockImplementation();
        invokeMock.mockImplementation(async (cmd, args) => {
            if (cmd === 'is_autostart_available') return false;
            return originalImpl(cmd, args);
        });

        clickSave();
        await flush();

        const { isFormDirty } = await import('../ui/lib/form-state.js');
        expect(isFormDirty()).toBe(false);
        expect(document.getElementById('save-status').textContent).toContain(
            '已保存',
        );
    });

    it('save_settings rejection keeps the form dirty and shows the error status', async () => {
        setInputValue('api-url', 'https://api.example.com/v1');

        invokeMock.mockImplementation(async (cmd) => {
            if (cmd === 'save_settings') throw new Error('disk full');
            return null;
        });

        clickSave();
        await flush();

        const { isFormDirty } = await import('../ui/lib/form-state.js');
        expect(isFormDirty()).toBe(true);
        // Message passthrough (Task 8): the backend reason surfaces instead
        // of a fixed "保存失败" string.
        expect(document.getElementById('save-status').textContent).toContain(
            'disk full',
        );
        expect(document.getElementById('save-status').textContent).toContain(
            '✗',
        );
    });
});

describe('api-url credential warning hint visibility', () => {
    beforeEach(async () => {
        await loadFresh();
    });

    afterEach(() => {
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
    });

    const warning = () => document.getElementById('api-url-warning');

    it('appears when the URL embeds a credential query param', () => {
        setInputValue('api-url', 'https://h.com/v1?key=sk-123');
        expect(warning().hidden).toBe(false);
    });

    it('disappears again when the param is removed', () => {
        setInputValue('api-url', 'https://h.com/v1?key=sk-123');
        setInputValue('api-url', 'https://h.com/v1/chat/completions');
        expect(warning().hidden).toBe(true);
    });

    it('stays hidden for harmless query params', () => {
        setInputValue('api-url', 'https://h.com/v1?model=gpt-4o');
        expect(warning().hidden).toBe(true);
    });

    it('shows on load when the saved config already embeds a key', async () => {
        // loadFresh already ran with a clean URL (warning hidden); reload
        // with a saved config whose api_url contains a credential param —
        // populateFields must surface the warning without any user input.
        const originalImpl = invokeMock.getMockImplementation();
        invokeMock.mockImplementation(async (cmd, args) => {
            if (cmd === 'get_config') {
                const config = await originalImpl(cmd, args);
                return { ...config, llm_api_url: 'https://h.com/v1?api_key=x' };
            }
            return originalImpl(cmd, args);
        });

        vi.resetModules();
        await import('../ui/settings.js');
        await flush();

        expect(warning().hidden).toBe(false);
    });
});

describe('help page recent-errors section', () => {
    afterEach(() => {
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
    });

    async function loadWithErrors(cmd, records) {
        invokeMock = vi.fn(async (c) => {
            if (c === 'get_config') {
                return {
                    language: 'zh',
                    hotkey: 'RightCtrl',
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
                    record_only_hotkey: 'RightAlt',
                };
            }
            if (c === 'get_whisper_models') {
                return { built_in: { base: true }, custom: [] };
            }
            if (c === 'get_last_errors') {
                if (cmd === 'fail') throw new Error('backend gone');
                return records;
            }
            return null;
        });
        vi.stubGlobal('__TAURI__', {
            event: { listen: vi.fn(() => () => {}) },
            core: { invoke: invokeMock },
            app: { getVersion: vi.fn(async () => '0.0.0-test') },
            autostart: { enable: vi.fn(), disable: vi.fn() },
        });
        document.body.innerHTML = MINIMAL_DOM;
        vi.resetModules();
        await import('../ui/settings.js');
        await new Promise((r) => setTimeout(r, 10));
        // Navigate to the help page (switchPage is async + exported).
        const { switchPage } = await import('../ui/app-shell.js');
        await switchPage('help');
        await new Promise((r) => setTimeout(r, 10));
    }

    it('renders the most recent errors with channel labels', async () => {
        await loadWithErrors('ok', [
            {
                timestamp: '2026-09-05T10:00:00+08:00',
                event: 'speech-error',
                message: '模型未下载，请在 设置→模型 下载',
            },
            {
                timestamp: '2026-09-05T11:00:00+08:00',
                event: 'injection-error',
                message: '粘贴失败，本次文字未保存，原剪贴板已恢复',
            },
        ]);
        const items = document.querySelectorAll('.error-history-item');
        expect(items.length).toBe(2);
        expect(items[0].querySelector('.badge').textContent).toBe('语音识别');
        expect(items[0].querySelector('.error-history-message').textContent)
            .toBe('模型未下载，请在 设置→模型 下载');
        expect(items[1].querySelector('.badge').textContent).toBe('文本粘贴');
    });

    it('shows the empty-state hint when history is empty', async () => {
        await loadWithErrors('ok', []);
        const list = document.getElementById('error-history-list');
        expect(list.querySelector('.hint').textContent).toBe('暂无错误记录');
        expect(document.querySelectorAll('.error-history-item').length).toBe(0);
    });

    it('shows the failure hint when the invoke rejects (no error popup)', async () => {
        await loadWithErrors('fail', null);
        const list = document.getElementById('error-history-list');
        expect(list.querySelector('.hint').textContent).toBe('无法加载错误记录');
    });
});
