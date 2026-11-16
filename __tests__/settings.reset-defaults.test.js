// @vitest-environment jsdom
//
// M3-a restore-defaults orchestration: danger confirm → populateFields with
// keepBaseline → CLEAR sentinel on the key get() → saveSettings normalizes
// the sentinel before submit. Covers both stored-key and no-key baselines
// plus failure paths and the input-event intent-withdraw edge case.
//
// Real-entry fixture (Round 6 P4 Task 3): loads the REAL ui/settings.html
// via loadSettingsDom() — this file previously inlined the exact same
// read+strip logic the helper encapsulates.

import { afterEach, describe, expect, it, vi } from 'vitest';

import { HOTKEY_SLOTS } from '../ui/lib/settings-schema.js';
import { loadSettingsDom, stubTauri } from './helpers.js';

vi.mock('../ui/model-manager.js', () => ({
    getModelStatus: () => ({
        tiny: true,
        base: true,
        small: true,
        medium: true,
        'tiny-q8_0': true,
        'base-q8_0': true,
        'small-q8_0': true,
        'medium-q8_0': true,
    }),
    getSelectedModel: () => 'base',
    setSelectedModel: () => {},
    populateModelSelect: () => {},
    setModelStatus: () => {},
    setCustomModels: () => {},
    loadComputeMode: () => Promise.resolve(),
}));

// Bypass confirm-dialog.js module-level openCount state across tests.
vi.mock('../ui/lib/confirm-dialog.js', () => ({
    confirmDialog: () => Promise.resolve(true),
    isDialogOpen: () => false,
}));

let invocations;

const baseConfig = {
    hotkey: { ctrl: false, shift: false, alt: false, vk: 163 },
    record_only_hotkey: { ctrl: false, shift: false, alt: false, vk: 165 },
    language: 'en',
    whisper_model: 'medium',
    llm_enabled: true,
    llm_api_url: 'http://127.0.0.1:9/v1',
    llm_api_key: '__MASKED__',
    llm_model: 'm',
    download_mirror: 'hf-mirror',
    data_saving_enabled: true,
    data_saving_path: 'D:\\some\\path',
    review_before_paste: false,
    autostart: false,
    realtime_transcription: false,
    record_only_enabled: false,
};

const defaults = {
    hotkey: { ctrl: false, shift: false, alt: false, vk: 163 },
    record_only_hotkey: { ctrl: false, shift: false, alt: false, vk: 165 },
    language: 'zh',
    whisper_model: 'base',
    llm_enabled: false,
    llm_api_url: '',
    llm_api_key: '',
    llm_model: '',
    download_mirror: 'hf-mirror',
    data_saving_enabled: false,
    data_saving_path: '',
    review_before_paste: false,
    autostart: false,
    realtime_transcription: false,
    record_only_enabled: false,
};

async function stubInvoke(extraFn, baseline = {}) {
    invocations = [];
    const cfg = { ...baseConfig, ...baseline };
    stubTauri({
        event: {
            listen: vi.fn(() => Promise.resolve(() => {})),
        },
        dialog: { open: vi.fn(() => Promise.resolve(null)) },
        core: {
            invoke: vi.fn((cmd, args) => {
                invocations.push({ cmd, args });
                if (extraFn) return extraFn(cmd, args);
                if (cmd === 'get_config') return Promise.resolve(cfg);
                if (cmd === 'get_default_config')
                    return Promise.resolve(defaults);
                if (cmd === 'save_settings') return Promise.resolve(null);
                if (cmd === 'is_autostart_available')
                    return Promise.resolve(true);
                if (cmd === 'get_compute_mode') return Promise.resolve('gpu');
                if (cmd === 'get_whisper_models')
                    return Promise.resolve({
                        built_in: {
                            tiny: true,
                            base: true,
                            small: true,
                            medium: true,
                        },
                        custom: [],
                    });
                if (cmd === 'check_ui_imports') return Promise.resolve([]);
                if (cmd === 'check_model_exists') return Promise.resolve(true);
                return Promise.resolve(null);
            }),
        },
    });
}

async function loadFresh(loadedConfig) {
    loadSettingsDom();
    const mod = await import('../ui/settings-form.js');
    await new Promise((r) => setTimeout(r, 0));
    // settings-form.js disables the save button until setDirtyCheckEnabled(true).
    mod.setDirtyCheckEnabled(true);
    // populateFields is normally called by app-shell.js init — invoke it
    // here to set loadedConfig + render initial form values.
    mod.populateFields(loadedConfig ?? baseConfig);
    mod.updateDirtyState();
}

afterEach(() => {
    vi.unstubAllGlobals();
    vi.resetModules();
    document.body.innerHTML = '';
});

// --- Hotkey preview refresh (Round 6 P10 Task 5a) ----------------------
//
// The restore-defaults flow must refresh ALL FOUR hotkey-slot previews.
// The slot prefixes come from HOTKEY_SLOTS (settings-schema.js — the single
// source for the four slots); updateHotkeyPreview(prefix) renders into
// `${prefix}-preview`.
const SLOT_EXPECTATIONS = [
    {
        key: 'hotkey',
        loaded: '当前组合：Ctrl+Shift+A',
        default: '当前组合：RightCtrl',
    },
    {
        key: 'record_only_hotkey',
        loaded: '当前组合：B',
        default: '当前组合：RightAlt',
    },
    {
        key: 'open_settings_hotkey',
        loaded: '当前组合：Alt+C',
        default: '当前组合：未启用',
    },
    {
        key: 'open_transcribe_hotkey',
        loaded: '当前组合：D',
        default: '当前组合：未启用',
    },
];

function previewText(key) {
    const slot = HOTKEY_SLOTS.find((s) => s.key === key);
    if (!slot) throw new Error(`unknown hotkey slot: ${key}`);
    return document.getElementById(`${slot.prefix}-preview`)?.textContent;
}

describe('restore defaults (M3-a)', () => {
    it('danger confirm resolves: populates defaults, marks dirty, does NOT save', async () => {
        invocations = [];
        stubInvoke();
        await loadFresh();
        document.getElementById('reset-defaults-btn').click();
        await new Promise((r) => setTimeout(r, 0));
        const saveCalls = invocations.filter((i) => i.cmd === 'save_settings');
        expect(saveCalls).toHaveLength(0);
        expect(document.getElementById('language').value).toBe('zh');
        expect(document.getElementById('save-status').textContent).toContain(
            '已恢复默认值',
        );
        expect(document.getElementById('save-btn').disabled).toBe(false);
    });

    it('reset then save sends CLEAR_MARKER when a key was stored', async () => {
        invocations = [];
        stubInvoke();
        await loadFresh();
        document.getElementById('reset-defaults-btn').click();
        await new Promise((r) => setTimeout(r, 50));
        const saveBtn = document.getElementById('save-btn');
        saveBtn.click();
        await new Promise((r) => setTimeout(r, 50));
        const saveCall = invocations.find((i) => i.cmd === 'save_settings');
        expect(saveCall).toBeDefined();
        expect(saveCall.args.config.llm_api_key).toBe('__CLEAR__');
    });

    it('reset then save sends empty key when none was stored', async () => {
        await stubInvoke(null, { llm_api_key: '' });
        await loadFresh({ ...baseConfig, llm_api_key: '' });
        document.getElementById('reset-defaults-btn').click();
        await new Promise((r) => setTimeout(r, 0));
        document.getElementById('save-btn').click();
        await new Promise((r) => setTimeout(r, 0));
        const saveCall = invocations.find((i) => i.cmd === 'save_settings');
        expect(saveCall.args.config.llm_api_key).toBe('');
    });

    it('typing a new key after reset cancels the CLEAR intent', async () => {
        invocations = [];
        stubInvoke();
        await loadFresh();
        document.getElementById('reset-defaults-btn').click();
        await new Promise((r) => setTimeout(r, 0));
        const input = document.getElementById('api-key');
        input.value = 'sk-new';
        input.dispatchEvent(new Event('input', { bubbles: true }));
        document.getElementById('save-btn').click();
        await new Promise((r) => setTimeout(r, 0));
        const saveCall = invocations.find((i) => i.cmd === 'save_settings');
        expect(saveCall.args.config.llm_api_key).toBe('sk-new');
    });

    it('get_default_config rejection shows error and leaves the form untouched', async () => {
        await stubInvoke((cmd) => {
            if (cmd === 'get_default_config')
                return Promise.reject(new Error('boom'));
            return null;
        });
        await loadFresh();
        document.getElementById('reset-defaults-btn').click();
        await new Promise((r) => setTimeout(r, 10));
        expect(document.getElementById('save-status').textContent).toContain(
            '恢复默认失败',
        );
        const saveCalls = invocations.filter((i) => i.cmd === 'save_settings');
        expect(saveCalls).toHaveLength(0);
    });

    it('after reset+save, a second save sends empty key (no sentinel residue)', async () => {
        invocations = [];
        stubInvoke();
        await loadFresh();
        document.getElementById('reset-defaults-btn').click();
        await new Promise((r) => setTimeout(r, 0));
        document.getElementById('save-btn').click();
        await new Promise((r) => setTimeout(r, 0));
        // 基线归一化断言（原 plan 用例 7）：CLEAR 哨兵不得残留基线，
        // 否则 maskedKeyEqual('', '__CLEAR__') = false → 永久 dirty。
        expect(document.getElementById('save-btn').disabled).toBe(true);
        // The clean form disables the save button (disabled = clean), so
        // jsdom suppresses the second click — force-enable to reach
        // saveSettings() (same precedent as the retry tests below).
        document.getElementById('save-btn').disabled = false;
        document.getElementById('save-btn').click();
        await new Promise((r) => setTimeout(r, 0));
        const saveCalls = invocations.filter((i) => i.cmd === 'save_settings');
        expect(saveCalls.length).toBeGreaterThanOrEqual(1);
        const last = saveCalls[saveCalls.length - 1];
        expect(last.args.config.llm_api_key).toBe('');
    });

    it('reset lights dirty for a no-key user with non-default settings', async () => {
        await stubInvoke(null, { llm_api_key: '', language: 'en' });
        await loadFresh({ ...baseConfig, llm_api_key: '', language: 'en' });
        document.getElementById('reset-defaults-btn').click();
        await new Promise((r) => setTimeout(r, 0));
        expect(document.getElementById('save-btn').disabled).toBe(false);
    });

    it('typing then deleting in the key box cancels the wipe and withdraws the notice', async () => {
        await stubInvoke();
        await loadFresh();
        document.getElementById('reset-defaults-btn').click();
        await new Promise((r) => setTimeout(r, 0));
        expect(document.getElementById('save-status').textContent).toContain(
            '已恢复默认值',
        );
        const input = document.getElementById('api-key');
        input.value = 'sk-x';
        input.dispatchEvent(new Event('input', { bubbles: true }));
        expect(
            document.getElementById('save-status').textContent,
        ).not.toContain('已恢复默认值');
        input.value = '';
        input.dispatchEvent(new Event('input', { bubbles: true }));
        document.getElementById('save-btn').click();
        await new Promise((r) => setTimeout(r, 0));
        expect(
            document.getElementById('save-status').textContent,
        ).not.toContain('已恢复默认值');
        // Plan 用例 8 原裁决：input 全删后 get() 走末行返回 '__MASKED__'
        // （keepBaseline 下基线仍 MASKED），save 发送 '__MASKED__'，后端
        // unmask_or_keep 保留旧 key——两种形态均 = 保留，行为等价于未动。
        const saveCalls = invocations.filter((i) => i.cmd === 'save_settings');
        const saveCall = saveCalls[saveCalls.length - 1];
        expect(saveCall).toBeDefined();
        expect(saveCall.args.config.llm_api_key).toBe('__MASKED__');
    });

    it('save-failure retry chain keeps sending CLEAR on repeated resets', async () => {
        let saveCount = 0;
        await stubInvoke((cmd) => {
            if (cmd === 'get_config')
                return Promise.resolve({
                    ...baseConfig,
                    llm_api_key: '__MASKED__',
                });
            if (cmd === 'save_settings') {
                saveCount++;
                if (saveCount === 1)
                    return Promise.reject(new Error('transient'));
                return Promise.resolve(null);
            }
            return null;
        });
        await loadFresh({ ...baseConfig, llm_api_key: '__MASKED__' });
        const saveBtn = document.getElementById('save-btn');
        document.getElementById('reset-defaults-btn').click();
        await new Promise((r) => setTimeout(r, 50));
        saveBtn.click();
        await new Promise((r) => setTimeout(r, 50));
        document.getElementById('reset-defaults-btn').click();
        await new Promise((r) => setTimeout(r, 50));
        // The first save rejected and left the save button disabled; in the
        // real product a successful save re-enables. Force-enable here so the
        // second click reaches saveSettings() for the retry.
        saveBtn.disabled = false;
        saveBtn.click();
        await new Promise((r) => setTimeout(r, 50));
        const saveCalls = invocations.filter((i) => i.cmd === 'save_settings');
        expect(saveCalls.length).toBeGreaterThanOrEqual(2);
        const lastSave = saveCalls[saveCalls.length - 1];
        expect(lastSave.args.config.llm_api_key).toBe('__CLEAR__');
    });

    it('retrying save directly after a failure still sends CLEAR (intent survives)', async () => {
        let saveCount = 0;
        await stubInvoke((cmd) => {
            if (cmd === 'get_config')
                return Promise.resolve({
                    ...baseConfig,
                    llm_api_key: '__MASKED__',
                });
            if (cmd === 'save_settings') {
                saveCount++;
                if (saveCount === 1)
                    return Promise.reject(new Error('transient'));
                return Promise.resolve(null);
            }
            return null;
        });
        await loadFresh({ ...baseConfig, llm_api_key: '__MASKED__' });
        const saveBtn = document.getElementById('save-btn');
        document.getElementById('reset-defaults-btn').click();
        await new Promise((r) => setTimeout(r, 50));
        saveBtn.click();
        await new Promise((r) => setTimeout(r, 50));
        // DR-1.2：失败后直接重试（不再 reset）——CLEAR 意图必须在失败的
        // save 后存活（双清只在成功路径执行），否则第二次 save 发 MASKED
        // = 保留 key，用户以为在清 key。
        saveBtn.click();
        await new Promise((r) => setTimeout(r, 50));
        const saveCalls = invocations.filter((i) => i.cmd === 'save_settings');
        expect(saveCalls.length).toBeGreaterThanOrEqual(2);
        const secondSaveCall = saveCalls[saveCalls.length - 1];
        expect(secondSaveCall.args.config.llm_api_key).toBe('__CLEAR__');
    });

    it('hand-typing a sentinel literal is neutralized (never sent to backend)', async () => {
        invocations = [];
        stubInvoke();
        await loadFresh();
        const input = document.getElementById('api-key');
        // 手输 '__CLEAR__' —— 哨兵不得经手输路径触达后端（否则 = 意外清 key）
        input.value = '__CLEAR__';
        input.dispatchEvent(new Event('input', { bubbles: true }));
        document.getElementById('save-btn').click();
        await new Promise((r) => setTimeout(r, 0));
        let saveCalls = invocations.filter((i) => i.cmd === 'save_settings');
        let saveCall = saveCalls[saveCalls.length - 1];
        expect(saveCall).toBeDefined();
        expect(saveCall.args.config.llm_api_key).toBe('__MASKED__');
        // 手输 '__MASKED__' —— 同样无害化为保留语义（后端保留旧 key）
        input.value = '__MASKED__';
        input.dispatchEvent(new Event('input', { bubbles: true }));
        document.getElementById('save-btn').click();
        await new Promise((r) => setTimeout(r, 0));
        saveCalls = invocations.filter((i) => i.cmd === 'save_settings');
        saveCall = saveCalls[saveCalls.length - 1];
        expect(saveCall).toBeDefined();
        expect(saveCall.args.config.llm_api_key).toBe('__MASKED__');
    });

    it('refreshes ALL FOUR hotkey-slot previews to the default labels', async () => {
        // Every slot of the LOADED config holds a non-default spec, so a
        // preview the reset failed to refresh stays visibly stale.
        const loaded = {
            ...baseConfig,
            hotkey: { ctrl: true, shift: true, alt: false, vk: 65 },
            record_only_hotkey: {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 66,
            },
            open_settings_hotkey: {
                ctrl: false,
                shift: false,
                alt: true,
                vk: 67,
            },
            open_transcribe_hotkey: {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 68,
            },
        };
        // Backend defaults (src-tauri/src/config/schema.rs): RightCtrl /
        // RightAlt for the two required slots, None for the optional ones.
        await stubInvoke((cmd) => {
            if (cmd === 'get_default_config') {
                return Promise.resolve({
                    ...defaults,
                    open_settings_hotkey: null,
                    open_transcribe_hotkey: null,
                });
            }
            return null;
        });
        await loadFresh(loaded);

        // Bind the expectations to the single source of slots — a fifth
        // slot added upstream must fail here loudly, not be skipped.
        expect(SLOT_EXPECTATIONS).toHaveLength(HOTKEY_SLOTS.length);

        // Before the reset the previews show the LOADED specs.
        for (const { key, loaded: label } of SLOT_EXPECTATIONS) {
            expect(previewText(key)).toBe(label);
        }

        document.getElementById('reset-defaults-btn').click();
        await new Promise((r) => setTimeout(r, 10));

        // After the reset every preview shows the DEFAULT label — including
        // the two optional slots, whose default is "disabled".
        for (const { key, default: label } of SLOT_EXPECTATIONS) {
            expect(previewText(key)).toBe(label);
        }
    });
});
