// @vitest-environment jsdom
//
// M3-a restore-defaults orchestration: danger confirm → populateFields with
// keepBaseline → CLEAR sentinel on the key get() → saveSettings normalizes
// the sentinel before submit. Covers both stored-key and no-key baselines
// plus failure paths and the input-event intent-withdraw edge case.

import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it, vi } from 'vitest';

const __dirname = dirname(fileURLToPath(import.meta.url));
const SETTINGS_HTML = readFileSync(
    resolve(__dirname, '../ui/settings.html'),
    'utf-8',
)
    .replace(/<script[\s\S]*?<\/script>/g, '')
    .replace(/<link[^>]*>/g, '');

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
    vi.stubGlobal('__TAURI__', {
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
    document.body.innerHTML = SETTINGS_HTML;
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
        // typing+delete leaves the key box empty — populate returned
        // MASKED for the baseline (validateSettings rejects the literal
        // sentinel per the Task 3.1 guard, so save is a no-op). The intent
        // is withdrawn (notice is empty), and the form is left untouched.
        expect(
            document.getElementById('save-status').textContent,
        ).not.toContain('已恢复默认值');
        const saveCalls = invocations.filter((i) => i.cmd === 'save_settings');
        expect(saveCalls).toHaveLength(0);
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
});
