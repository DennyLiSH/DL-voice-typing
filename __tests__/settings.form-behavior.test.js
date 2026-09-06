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
  <div id="container" class="content-area"></div>
  <div class="sidebar"></div>
  <div class="sidebar-item" data-page="general"></div>
  <div class="sidebar-item" data-page="help"></div>
  <div class="page-content" id="page-general"></div>
  <div class="page-content" id="page-help"></div>
  <div id="error-history-list"></div>
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
    invokeMock = vi.fn(async (cmd) => {
        if (cmd === 'get_config') {
            return {
                language: 'zh',
                // HotkeySpec objects (mirror backend HotkeySpec shape):
                //   hotkey           = {ctrl, shift, alt, vk} — RightCtrl = 0xA3 = 163
                //   record_only_hotkey             — RightAlt  = 0xA5 = 165
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

    it('hotkey preview renders the loaded spec and follows combo edits', () => {
        // Spec: 预览文本 "Ctrl+Shift+A" — live label under the combo
        // controls, initialized from the loaded config.
        const preview = document.getElementById('hotkey-preview');
        const roPreview = document.getElementById('record-only-hotkey-preview');
        expect(preview.textContent).toBe('当前组合：RightCtrl');
        expect(roPreview.textContent).toBe('当前组合：RightAlt');

        // Toggling modifiers + main key updates the label live.
        const ctrl = document.getElementById('hotkey-ctrl');
        ctrl.checked = true;
        ctrl.dispatchEvent(new Event('change'));
        const sel = document.getElementById('hotkey');
        sel.value = 'F9';
        sel.dispatchEvent(new Event('change'));
        expect(preview.textContent).toBe('当前组合：Ctrl+F9');
    });

    it('hotkey preview shows 未选择主键 when the main-key select is empty', () => {
        const preview = document.getElementById('hotkey-preview');
        const sel = document.getElementById('hotkey');
        sel.value = '';
        sel.dispatchEvent(new Event('change'));
        expect(preview.textContent).toBe('未选择主键');
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
        expect(
            items[0].querySelector('.error-history-message').textContent,
        ).toBe('模型未下载，请在 设置→模型 下载');
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
        expect(list.querySelector('.hint').textContent).toBe(
            '无法加载错误记录',
        );
    });
});

describe('save status clearing semantics', () => {
    beforeEach(async () => {
        // Fake timers AFTER loadFresh — its settle wait uses a real
        // setTimeout that frozen timers would never fire.
        await loadFresh();
        vi.useFakeTimers();
    });

    afterEach(() => {
        vi.useRealTimers();
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
    });

    function makeDirty() {
        setInputValue('api-url', 'https://api.example.com/v1');
    }

    function saveWith({ autostartAvailable = false, saveFails = false } = {}) {
        const originalImpl = invokeMock.getMockImplementation();
        invokeMock.mockImplementation(async (cmd, args) => {
            if (cmd === 'save_settings' && saveFails) {
                throw new Error('disk full');
            }
            if (cmd === 'is_autostart_available') return autostartAvailable;
            return originalImpl(cmd, args);
        });
        clickSave();
    }

    it('restart-hint message survives 1.5s and clears on the next input', async () => {
        makeDirty();
        // Change the hotkey so the save message becomes instructional.
        // Hotkey is now a combo: 3 modifier checkboxes + main <select>.
        // Toggling a checkbox changes the persisted HotkeySpec shape, which
        // triggers the "新热键重启应用后生效" message branch.
        const hotkeyCtrl = document.getElementById('hotkey-ctrl');
        hotkeyCtrl.checked = true;
        hotkeyCtrl.dispatchEvent(new Event('change'));

        saveWith();
        await vi.advanceTimersByTimeAsync(10);
        expect(document.getElementById('save-status').textContent).toContain(
            '重启应用后生效',
        );

        await vi.advanceTimersByTimeAsync(2000);
        expect(document.getElementById('save-status').textContent).toContain(
            '重启应用后生效',
        );

        document
            .querySelector('.content-area')
            .dispatchEvent(new Event('input', { bubbles: true }));
        expect(document.getElementById('save-status').textContent).toBe('');
    });

    it('baseline ✓ 已保存 still auto-clears after 1.5s', async () => {
        makeDirty();
        saveWith();
        await vi.advanceTimersByTimeAsync(10);
        expect(document.getElementById('save-status').textContent).toContain(
            '已保存',
        );

        await vi.advanceTimersByTimeAsync(1600);
        expect(document.getElementById('save-status').textContent).toBe('');
    });

    it('autostart-failure ⚠ message also survives until interaction', async () => {
        makeDirty();
        // wantAutostart=false (config autostart off) + available + disable
        // throwing drives the ⚠ branch.
        vi.stubGlobal('__TAURI__', {
            ...window.__TAURI__,
            autostart: {
                enable: vi.fn(),
                disable: vi.fn(async () => {
                    throw new Error('denied');
                }),
            },
        });
        const originalImpl = invokeMock.getMockImplementation();
        invokeMock.mockImplementation(async (cmd, args) => {
            if (cmd === 'is_autostart_available') return true;
            return originalImpl(cmd, args);
        });
        clickSave();
        await vi.advanceTimersByTimeAsync(20);

        const status = document.getElementById('save-status');
        expect(status.textContent).toContain('开机自启同步失败');
        await vi.advanceTimersByTimeAsync(2000);
        expect(status.textContent).toContain('开机自启同步失败');

        document
            .querySelector('.content-area')
            .dispatchEvent(new Event('click', { bubbles: true }));
        expect(status.textContent).toBe('');
    });

    it('a failed save after a successful one never has its error cleared by interaction', async () => {
        makeDirty();
        saveWith();
        await vi.advanceTimersByTimeAsync(10);
        expect(document.getElementById('save-status').textContent).toContain(
            '已保存',
        );

        // Make dirty again, then fail the second save.
        makeDirty();
        saveWith({ saveFails: true });
        await vi.advanceTimersByTimeAsync(10);
        expect(document.getElementById('save-status').textContent).toContain(
            'disk full',
        );

        // Baseline success auto-clear timer from save #1 already fired or
        // was for a different message; the interaction must NOT clear the ✗.
        document
            .querySelector('.content-area')
            .dispatchEvent(new Event('input', { bubbles: true }));
        await vi.advanceTimersByTimeAsync(10);
        expect(document.getElementById('save-status').textContent).toContain(
            'disk full',
        );
    });
});

describe('hotkey combo (Task 8: arbitrary combinations)', () => {
    beforeEach(async () => {
        // Real timers for these tests — the interactions are pure DOM
        // checks that do not need fake-timer plumbing.
        await loadFresh();
    });

    afterEach(() => {
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
    });

    function getCurrentConfigFromForm() {
        // settings-form.js does not export getCurrentConfig (only updateDirtyState
        // and a few others). We re-import dynamically to call the real function.
        return import('../ui/settings-form.js').then((m) =>
            m.getCurrentConfig(),
        );
    }

    it('populates the main <select> from MAIN_KEYS via settings.js init', () => {
        // After loadFresh, app-shell.js init() runs populateMainKeySelects()
        // before populateFields(); verify the select has 54 options
        // (6 modifier + 12 F-keys + 26 letters + 10 digits).
        const sel = document.getElementById('hotkey');
        // The MINIMAL_DOM seed has 3 options; settings.js init() should
        // have populated them with all MAIN_KEYS (54 entries).
        expect(sel.options.length).toBeGreaterThanOrEqual(54);
        // RightCtrl is in MAIN_KEYS and must be present.
        const hasRightCtrl = Array.from(sel.options).some(
            (o) => o.value === 'RightCtrl',
        );
        expect(hasRightCtrl).toBe(true);
    });

    it('populateFields reflects the loaded HotkeySpec into checkboxes + select', async () => {
        // loadFresh loaded hotkey = RightCtrl (vk 163, no mods). The select
        // option value should be 'RightCtrl' after init.
        const sel = document.getElementById('hotkey');
        expect(sel.value).toBe('RightCtrl');
        expect(document.getElementById('hotkey-ctrl').checked).toBe(false);
        expect(document.getElementById('hotkey-shift').checked).toBe(false);
        expect(document.getElementById('hotkey-alt').checked).toBe(false);
    });

    it('getCurrentConfig reads back the loaded spec structurally', async () => {
        const config = await getCurrentConfigFromForm();
        expect(config.hotkey).toEqual({
            ctrl: false,
            shift: false,
            alt: false,
            vk: 163,
        });
        expect(config.record_only_hotkey).toEqual({
            ctrl: false,
            shift: false,
            alt: false,
            vk: 165,
        });
    });

    it('toggling a modifier checkbox does not produce a dirty form (already loaded = no change)', async () => {
        // Loaded spec has ctrl=false. Toggling the checkbox ON then OFF
        // must not leave the form dirty (round-tripped to same shape).
        const cb = document.getElementById('hotkey-ctrl');
        cb.checked = true;
        cb.dispatchEvent(new Event('change'));
        cb.checked = false;
        cb.dispatchEvent(new Event('change'));
        const { isFormDirty } = await import('../ui/lib/form-state.js');
        expect(isFormDirty()).toBe(false);
    });

    it('toggling a modifier checkbox ON marks the form dirty', async () => {
        const cb = document.getElementById('hotkey-ctrl');
        cb.checked = true;
        cb.dispatchEvent(new Event('change'));
        const { isFormDirty } = await import('../ui/lib/form-state.js');
        expect(isFormDirty()).toBe(true);
    });

    it('changing the main <select> value marks the form dirty', async () => {
        const sel = document.getElementById('hotkey');
        sel.value = 'F9';
        sel.dispatchEvent(new Event('change'));
        const { isFormDirty } = await import('../ui/lib/form-state.js');
        expect(isFormDirty()).toBe(true);
    });

    it('unknown vk in loaded config leaves the select empty AND does NOT silently rewrite to a canonical name', async () => {
        // Reload with an unknown vk (e.g. 0xDEAD) — the select stays empty,
        // and the form must NOT silently rewrite to RightCtrl (the dangerous
        // config-rewrite chain the plan calls out as a regression).
        //
        // Note on round-trip semantics: specFromUI reads from the DOM, so
        // when the select is empty the spec returned is `{ctrl:..., vk:0}`
        // — the unknown vk is lost in the DOM direction. The protection
        // here is structural: the SPEC OBJECT passed into writeSpecToUI
        // is NOT mutated (the loaded vk 0xDEAD stays in memory until the
        // user changes the form). On save, validateSettings rejects
        // vk=0, forcing the user to fix the config rather than silently
        // persisting RightCtrl.
        const originalImpl = invokeMock.getMockImplementation();
        invokeMock.mockImplementation(async (cmd, args) => {
            if (cmd === 'get_config') {
                const config = await originalImpl(cmd, args);
                return {
                    ...config,
                    hotkey: {
                        ctrl: true,
                        shift: false,
                        alt: false,
                        vk: 0xdead,
                    },
                };
            }
            return originalImpl(cmd, args);
        });

        vi.resetModules();
        await import('../ui/settings.js');
        await new Promise((r) => setTimeout(r, 10));

        // Select value is empty (no matching option for 0xDEAD).
        expect(document.getElementById('hotkey').value).toBe('');
        // The <select> value did NOT silently fall back to RightCtrl —
        // a real bug class this guards against (specLabel would still
        // display "Ctrl+VK0xdead" via the loaded spec object).
        expect(document.getElementById('hotkey').value).not.toBe('RightCtrl');
        // vk read back is 0 (no option matches), not the unknown 0xDEAD —
        // the DOM is lossy in this direction, but validateSettings would
        // reject save, so the bad config never gets persisted silently.
        const config = await getCurrentConfigFromForm();
        expect(config.hotkey.vk).toBe(0);
    });
});
