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

let invokeMock;
let listeners;

const MINIMAL_DOM = `
  <div id="container"></div>
  <div class="sidebar"></div>
  <div class="sidebar-item" data-page="general"></div>
  <div class="page-content" id="page-general"></div>
  <select id="language"></select>
  <select id="hotkey"></select>
  <select id="whisper-model"></select>
  <div id="model-status-text"></div>
  <button id="btn-download-model"></button>
  <div id="download-progress"></div>
  <div id="progress-fill"></div>
  <div id="progress-percent"></div>
  <button id="btn-cancel-download"></button>
  <div id="llm-toggle"></div>
  <div id="llm-fields"></div>
  <input id="api-url" />
  <input id="api-key" />
  <input id="model" />
  <button id="toggle-key"></button>
  <button id="test-btn"></button>
  <div id="test-status"></div>
  <button id="save-btn"></button>
  <div id="save-status"></div>
  <div id="error-banner"></div>
  <select id="download-mirror"></select>
  <div id="data-saving-toggle"></div>
  <div id="data-saving-fields"></div>
  <input id="data-saving-path" />
  <button id="btn-browse-path"></button>
  <div id="review-toggle"></div>
  <div id="autostart-toggle"></div>
  <div id="realtime-transcription-toggle"></div>
  <div id="record-only-toggle"></div>
  <div id="record-only-hotkey-group"></div>
  <select id="record-only-hotkey"></select>
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

    vi.stubGlobal('confirm', () => false);

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
        expect(document.getElementById('save-status').textContent).toContain(
            '保存失败',
        );
    });
});
