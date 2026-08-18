import { call } from './lib/api.js';
import { MASKED_MARKER } from './lib/api-key-mask.js';
import { onFormChange } from './lib/form-state.js';
import { isConfigDirty, validateSettings } from './lib/settings-utils.js';
import { hideError, showError } from './lib/ui-utils.js';
import {
    getModelStatus,
    getSelectedModel,
    setSelectedModel,
} from './model-manager.js';

// DOM elements
const languageSelect = document.getElementById('language');
const hotkeySelect = document.getElementById('hotkey');
const llmToggle = document.getElementById('llm-toggle');
const llmFields = document.getElementById('llm-fields');
const apiUrlInput = document.getElementById('api-url');
const apiKeyInput = document.getElementById('api-key');
const modelInput = document.getElementById('model');
const toggleKeyBtn = document.getElementById('toggle-key');
const testBtn = document.getElementById('test-btn');
const testStatus = document.getElementById('test-status');
const saveBtn = document.getElementById('save-btn');
const saveStatus = document.getElementById('save-status');
const downloadMirrorSelect = document.getElementById('download-mirror');
const dataSavingToggle = document.getElementById('data-saving-toggle');
const dataSavingFields = document.getElementById('data-saving-fields');
const dataSavingPath = document.getElementById('data-saving-path');
const btnBrowsePath = document.getElementById('btn-browse-path');
const reviewToggle = document.getElementById('review-toggle');
const autostartToggle = document.getElementById('autostart-toggle');
const realtimeToggle = document.getElementById('realtime-transcription-toggle');
const recordOnlyToggle = document.getElementById('record-only-toggle');
const recordOnlyHotkeyGroup = document.getElementById(
    'record-only-hotkey-group',
);
const recordOnlyHotkeySelect = document.getElementById('record-only-hotkey');

// State
let loadedConfig = null;
let isDirty = false;
let dirtyCheckEnabled = false;
let loadedAutostart = false;

// Subscribe to form change events (model selection etc.) to recompute dirty state.
// Returns unsubscribe; we don't unsubscribe for the lifetime of the settings window.
onFormChange(updateDirtyState);

// --- Initialization ---

/**
 * Field descriptors for the settings form.
 * Single source of truth for: populateFields (config -> DOM),
 * getCurrentConfig (DOM -> config). Adding a new field = adding one entry.
 *
 * Toggle fields use classList.contains('active') + setAttribute('aria-checked').
 * Input/select fields use .value.
 * llm_api_key has masked-marker fallback in get (preserves existing behavior).
 * whisper_model delegates to model-manager getSelectedModel/setSelectedModel.
 */
const FIELDS = [
    {
        key: 'language',
        get: () => languageSelect.value,
        set: (v) => {
            languageSelect.value = v || 'zh';
        },
    },
    {
        key: 'hotkey',
        get: () => hotkeySelect.value,
        set: (v) => {
            hotkeySelect.value = v || 'RightAlt';
        },
    },
    { key: 'whisper_model', get: getSelectedModel, set: setSelectedModel },
    {
        key: 'llm_enabled',
        get: () => llmToggle.classList.contains('active'),
        set: (v) => {
            llmToggle.classList.toggle('active', !!v);
            llmToggle.setAttribute('aria-checked', String(!!v));
            updateLlmFieldsState(!!v);
        },
    },
    {
        key: 'llm_api_url',
        get: () => apiUrlInput.value.trim(),
        set: (v) => {
            apiUrlInput.value = v || '';
        },
    },
    {
        key: 'llm_api_key',
        get: () => {
            const v = apiKeyInput.value.trim();
            const hasExistingKey =
                loadedConfig && loadedConfig.llm_api_key === MASKED_MARKER;
            return v || (hasExistingKey ? MASKED_MARKER : '');
        },
        set: (v) => {
            // Handle masked API key: clear input, show placeholder
            if (v === MASKED_MARKER) {
                apiKeyInput.value = '';
                apiKeyInput.placeholder = 'API Key 已设置';
            } else {
                apiKeyInput.value = v || '';
                apiKeyInput.placeholder = 'sk-...';
            }
        },
    },
    {
        key: 'llm_model',
        get: () => modelInput.value.trim(),
        set: (v) => {
            modelInput.value = v || '';
        },
    },
    {
        key: 'download_mirror',
        get: () => downloadMirrorSelect.value,
        set: (v) => {
            downloadMirrorSelect.value = v || 'hf-mirror';
        },
    },
    {
        key: 'data_saving_enabled',
        get: () => dataSavingToggle.classList.contains('active'),
        set: (v) => {
            dataSavingToggle.classList.toggle('active', !!v);
            dataSavingToggle.setAttribute('aria-checked', String(!!v));
            updateDataSavingFieldsState(!!v);
        },
    },
    {
        key: 'data_saving_path',
        get: () => dataSavingPath.value.trim(),
        set: (v) => {
            dataSavingPath.value = v || '';
        },
    },
    {
        key: 'review_before_paste',
        get: () => reviewToggle.classList.contains('active'),
        set: (v) => {
            reviewToggle.classList.toggle('active', !!v);
            reviewToggle.setAttribute('aria-checked', String(!!v));
        },
    },
    {
        key: 'realtime_transcription',
        get: () => realtimeToggle.classList.contains('active'),
        set: (v) => {
            realtimeToggle.classList.toggle('active', !!v);
            realtimeToggle.setAttribute('aria-checked', String(!!v));
        },
    },
    {
        key: 'autostart',
        get: () => autostartToggle.classList.contains('active'),
        set: (v) => {
            loadedAutostart = !!v;
            autostartToggle.classList.toggle('active', loadedAutostart);
            autostartToggle.setAttribute(
                'aria-checked',
                String(loadedAutostart),
            );
        },
    },
    {
        key: 'record_only_enabled',
        get: () => recordOnlyToggle.classList.contains('active'),
        set: (v) => {
            recordOnlyToggle.classList.toggle('active', !!v);
            recordOnlyToggle.setAttribute('aria-checked', String(!!v));
            updateRecordOnlyHotkeyState(!!v);
        },
    },
    {
        key: 'record_only_hotkey',
        get: () => recordOnlyHotkeySelect.value,
        set: (v) => {
            recordOnlyHotkeySelect.value = v || 'RightAlt';
        },
    },
];

export function populateFields(config) {
    loadedConfig = config;
    FIELDS.forEach(({ key, set }) => set(config[key]));

    // In dev builds without DL_AUTOSTART=1, gray out the autostart toggle.
    // Probe is in populateFields (not in FIELDS) because it is a one-shot
    // availability check, not a per-field set operation.
    (async () => {
        try {
            const autostartAvailable = await call('is_autostart_available');
            if (!autostartAvailable) {
                autostartToggle.classList.add('disabled');
                autostartToggle.setAttribute('aria-disabled', 'true');
                autostartToggle.parentElement.classList.add('disabled');
            }
        } catch (_e) {
            // Non-critical: just skip gray-out (error already reported via call)
        }
    })();
}

// --- Toggle helper ---

/**
 * Bind click + keydown handlers to a toggle button.
 *
 * @param {HTMLElement} el - the toggle button element
 * @param {Object} [opts]
 * @param {(isActive: boolean) => void} [opts.onToggle] - called after toggle state changes (e.g., disable dependent inputs)
 * @param {() => boolean} [opts.guard] - returns true to skip the click (e.g., disabled check)
 */
function bindToggle(el, { onToggle, guard } = {}) {
    el.addEventListener('click', () => {
        if (guard?.()) return;
        const isActive = el.classList.toggle('active');
        el.setAttribute('aria-checked', String(isActive));
        if (onToggle) onToggle(isActive);
        updateDirtyState();
    });
    el.addEventListener('keydown', (e) => {
        if (e.key === ' ') {
            e.preventDefault();
            el.click();
        }
    });
}

// --- LLM Toggle ---

function updateLlmFieldsState(enabled) {
    llmFields.classList.toggle('disabled', !enabled);
    for (const input of llmFields.querySelectorAll('input')) {
        input.disabled = !enabled;
    }
    testBtn.disabled = !enabled;
}

bindToggle(llmToggle, { onToggle: updateLlmFieldsState });

// --- Data Saving Toggle ---

function updateDataSavingFieldsState(enabled) {
    dataSavingFields.classList.toggle('disabled', !enabled);
    for (const input of dataSavingFields.querySelectorAll('input')) {
        input.disabled = !enabled;
    }
    btnBrowsePath.disabled = !enabled;
}

bindToggle(dataSavingToggle, { onToggle: updateDataSavingFieldsState });

// --- Review Before Paste Toggle ---

bindToggle(reviewToggle);

// --- Autostart Toggle ---

bindToggle(autostartToggle, {
    guard: () => autostartToggle.classList.contains('disabled'),
});

// --- Realtime Transcription Toggle ---

bindToggle(realtimeToggle);

// --- Record-Only Mode Toggle ---

function updateRecordOnlyHotkeyState(enabled) {
    recordOnlyHotkeyGroup.classList.toggle('disabled', !enabled);
    recordOnlyHotkeySelect.disabled = !enabled;
}

bindToggle(recordOnlyToggle, { onToggle: updateRecordOnlyHotkeyState });

// --- Folder Browser ---

btnBrowsePath.addEventListener('click', async () => {
    try {
        const selected = await window.__TAURI__.dialog.open({
            directory: true,
            multiple: false,
            title: '选择数据保存路径',
        });
        if (selected) {
            dataSavingPath.value = selected;
            updateDirtyState();
        }
    } catch (_e) {
        showError('打开文件夹选择器失败');
    }
});

dataSavingPath.addEventListener('input', updateDirtyState);

toggleKeyBtn.addEventListener('click', () => {
    const input = apiKeyInput;
    if (input.type === 'password') {
        input.type = 'text';
        toggleKeyBtn.textContent = '🔒';
    } else {
        input.type = 'password';
        toggleKeyBtn.textContent = '👁';
    }
});

// --- Test Connection ---

testBtn.addEventListener('click', async () => {
    const apiUrl = apiUrlInput.value.trim();
    const apiKeyRaw = apiKeyInput.value.trim();
    const model = modelInput.value.trim();

    // If key input is empty and a key was previously set, use masked marker.
    // Otherwise, require the user to enter a key.
    const hasExistingKey =
        loadedConfig && loadedConfig.llm_api_key === MASKED_MARKER;
    const apiKey = apiKeyRaw || (hasExistingKey ? MASKED_MARKER : '');

    if (!apiUrl || !apiKey || !model) {
        setTestStatus('请填写所有字段', 'error');
        return;
    }

    testBtn.disabled = true;
    testBtn.textContent = '测试中...';
    testStatus.textContent = '';

    try {
        await call('test_llm_connection', { apiUrl, apiKey, model });
        setTestStatus('✓ 连接成功', 'success');
    } catch (_e) {
        setTestStatus('✗ 连接失败，请检查配置', 'error');
    } finally {
        testBtn.disabled = false;
        testBtn.textContent = '测试连接';
    }
});

function setTestStatus(message, type) {
    testStatus.textContent = message;
    testStatus.className = `status ${type}`;
}

// --- Dirty State ---

export function updateDirtyState() {
    if (!loadedConfig || !dirtyCheckEnabled) return;

    isDirty = isConfigDirty(getCurrentConfig(), loadedConfig);

    saveBtn.disabled = !isDirty;
    saveStatus.textContent = '';
    saveStatus.className = 'status';
}

export function getCurrentConfig() {
    return Object.fromEntries(FIELDS.map(({ key, get }) => [key, get()]));
}

// Track changes on all inputs
languageSelect.addEventListener('change', updateDirtyState);
hotkeySelect.addEventListener('change', updateDirtyState);
recordOnlyHotkeySelect.addEventListener('change', updateDirtyState);
downloadMirrorSelect.addEventListener('change', updateDirtyState);
apiUrlInput.addEventListener('input', updateDirtyState);
apiKeyInput.addEventListener('input', updateDirtyState);
modelInput.addEventListener('input', updateDirtyState);

// --- Save ---

saveBtn.addEventListener('click', async () => {
    hideError();
    const config = getCurrentConfig();

    const validation = validateSettings(config, getModelStatus());
    if (!validation.valid) {
        showError(validation.error);
        return;
    }

    saveBtn.disabled = true;
    saveBtn.textContent = '保存中...';
    saveBtn.classList.add('saving');

    try {
        await call('save_settings', { config });
        loadedConfig = config;

        // Sync autostart state with OS (skip in dev builds without DL_AUTOSTART=1).
        let saveMsg = '✓ 已保存';
        let saveMsgType = 'success';
        try {
            const wantAutostart = autostartToggle.classList.contains('active');
            const autostartAvailable = await call('is_autostart_available');
            if (autostartAvailable) {
                if (wantAutostart) {
                    await window.__TAURI__.autostart.enable();
                } else {
                    await window.__TAURI__.autostart.disable();
                }
            }
            loadedAutostart = wantAutostart;
        } catch (_e) {
            saveMsg = '⚠ 已保存，开机自启同步失败';
            saveMsgType = 'error';
        }
        isDirty = false;
        setSaveStatus(saveMsg, saveMsgType);
        setTimeout(() => {
            saveStatus.textContent = '';
        }, 1500);
    } catch (_e) {
        setSaveStatus('✗ 保存失败，请重试', 'error');
        showError('保存失败，请重试');
    } finally {
        saveBtn.textContent = '保存';
        saveBtn.classList.remove('saving');
        saveBtn.disabled = !isDirty;
    }
});

function setSaveStatus(message, type) {
    saveStatus.textContent = message;
    saveStatus.className = `status ${type}`;
}

export function setDirtyCheckEnabled(enabled) {
    dirtyCheckEnabled = enabled;
}

export function isDirtyState() {
    return isDirty;
}

export function getLoadedAutostart() {
    return loadedAutostart;
}
