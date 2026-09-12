import { call } from './lib/api.js';
import { MASKED_MARKER } from './lib/api-key-mask.js';
import { isFormDirty, onFormChange, setFormDirty } from './lib/form-state.js';
import {
    DEFAULT_PRIMARY_SPEC,
    DEFAULT_RECORD_ONLY_SPEC,
    sameSpec,
    specFromUI,
    specLabel,
    writeSpecToUI,
} from './lib/hotkeys.js';
import { SETTINGS_FIELDS } from './lib/settings-schema.js';
import {
    hasCredentialInUrl,
    isConfigDirty,
    validateSettings,
} from './lib/settings-utils.js';
import { hideError, showError } from './lib/ui-utils.js';
import {
    getModelStatus,
    getSelectedModel,
    setSelectedModel,
} from './model-manager.js';

// Icon constants (static SVG markup, no untrusted data is ever interpolated).
const EYE_SVG =
    '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>';
const LOCK_SVG =
    '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="11" width="18" height="11" rx="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/></svg>';

// DOM elements
const languageSelect = document.getElementById('language');
const llmToggle = document.getElementById('llm-toggle');
const llmFields = document.getElementById('llm-fields');
const apiUrlInput = document.getElementById('api-url');
const apiUrlWarning = document.getElementById('api-url-warning');
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

// Initial icon for the API key toggle: input starts as type="password",
// which maps to the EYE (reveal) affordance — replaces the emoji placeholder
// in the HTML so first paint matches the click-handler semantics.
toggleKeyBtn.innerHTML = EYE_SVG;

// State
let loadedConfig = null;
let dirtyCheckEnabled = false;
let loadedAutostart = false;

// Subscribe to form change events (model selection etc.) to recompute dirty state.
// Returns unsubscribe; we don't unsubscribe for the lifetime of the settings window.
onFormChange(updateDirtyState);

// --- Initialization ---

/**
 * DOM wiring for each SETTINGS_FIELDS key (get: DOM → value,
 * set: value → DOM). Keyed by field name — the key LIST lives in
 * lib/settings-schema.js (single source); this map only wires DOM.
 * Sync is guarded by __tests__/settings-schema.test.js.
 *
 * Toggle fields use classList.contains('active') + setAttribute('aria-checked').
 * Input/select fields use .value.
 * llm_api_key has masked-marker fallback in get (preserves existing behavior).
 * whisper_model delegates to model-manager getSelectedModel/setSelectedModel.
 */
const DOM_DEFS = {
    language: {
        get: () => languageSelect.value,
        set: (v) => {
            languageSelect.value = v || 'zh';
        },
    },
    hotkey: {
        get: () => specFromUI('hotkey'),
        set: (v) => {
            writeSpecToUI('hotkey', v ?? DEFAULT_PRIMARY_SPEC);
            updateHotkeyPreview('hotkey');
        },
    },
    whisper_model: {
        get: getSelectedModel,
        set: setSelectedModel,
    },
    llm_enabled: {
        get: () => llmToggle.classList.contains('active'),
        set: (v) => {
            llmToggle.classList.toggle('active', !!v);
            llmToggle.setAttribute('aria-checked', String(!!v));
            updateLlmFieldsState(!!v);
        },
    },
    llm_api_url: {
        get: () => apiUrlInput.value.trim(),
        set: (v) => {
            apiUrlInput.value = v || '';
        },
    },
    llm_api_key: {
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
    llm_model: {
        get: () => modelInput.value.trim(),
        set: (v) => {
            modelInput.value = v || '';
        },
    },
    download_mirror: {
        get: () => downloadMirrorSelect.value,
        set: (v) => {
            downloadMirrorSelect.value = v || 'hf-mirror';
        },
    },
    data_saving_enabled: {
        get: () => dataSavingToggle.classList.contains('active'),
        set: (v) => {
            dataSavingToggle.classList.toggle('active', !!v);
            dataSavingToggle.setAttribute('aria-checked', String(!!v));
            updateDataSavingFieldsState(!!v);
        },
    },
    data_saving_path: {
        get: () => dataSavingPath.value.trim(),
        set: (v) => {
            dataSavingPath.value = v || '';
        },
    },
    review_before_paste: {
        get: () => reviewToggle.classList.contains('active'),
        set: (v) => {
            reviewToggle.classList.toggle('active', !!v);
            reviewToggle.setAttribute('aria-checked', String(!!v));
        },
    },
    realtime_transcription: {
        get: () => realtimeToggle.classList.contains('active'),
        set: (v) => {
            realtimeToggle.classList.toggle('active', !!v);
            realtimeToggle.setAttribute('aria-checked', String(!!v));
        },
    },
    autostart: {
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
    record_only_enabled: {
        get: () => recordOnlyToggle.classList.contains('active'),
        set: (v) => {
            recordOnlyToggle.classList.toggle('active', !!v);
            recordOnlyToggle.setAttribute('aria-checked', String(!!v));
            updateRecordOnlyHotkeyState(!!v);
        },
    },
    record_only_hotkey: {
        get: () => specFromUI('record-only-hotkey'),
        set: (v) => {
            writeSpecToUI('record-only-hotkey', v ?? DEFAULT_RECORD_ONLY_SPEC);
            updateHotkeyPreview('record-only-hotkey');
        },
    },
};

const FIELDS = SETTINGS_FIELDS.map(({ key }) => ({ key, ...DOM_DEFS[key] }));

export function populateFields(config) {
    loadedConfig = config;
    FIELDS.forEach(({ key, set }) => set(config[key]));
    updateApiUrlWarning();

    // In dev builds without DL_AUTOSTART=1, gray out the autostart toggle.
    // Probe is in populateFields (not in FIELDS) because it is a one-shot
    // availability check, not a per-field set operation.
    (async () => {
        try {
            const autostartAvailable = await call('is_autostart_available');
            if (!autostartAvailable) {
                autostartToggle.classList.add('disabled');
                autostartToggle.setAttribute('aria-disabled', 'true');
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
    document.getElementById('record-only-hotkey-ctrl').disabled = !enabled;
    document.getElementById('record-only-hotkey-shift').disabled = !enabled;
    document.getElementById('record-only-hotkey-alt').disabled = !enabled;
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
        toggleKeyBtn.innerHTML = LOCK_SVG;
    } else {
        input.type = 'password';
        toggleKeyBtn.innerHTML = EYE_SVG;
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
    } catch (e) {
        setTestStatus(`✗ ${e?.message || '连接失败，请检查配置'}`, 'error');
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

    const dirty = isConfigDirty(getCurrentConfig(), loadedConfig);
    setFormDirty(dirty);

    saveBtn.disabled = !dirty;
    saveStatus.textContent = '';
    saveStatus.className = 'status';
}

export function getCurrentConfig() {
    return Object.fromEntries(FIELDS.map(({ key, get }) => [key, get()]));
}

// Track changes on all inputs
languageSelect.addEventListener('change', updateDirtyState);
downloadMirrorSelect.addEventListener('change', updateDirtyState);
apiUrlInput.addEventListener('input', updateDirtyState);
apiKeyInput.addEventListener('input', updateDirtyState);
modelInput.addEventListener('input', updateDirtyState);

// Hotkey combo controls: 3 modifier checkboxes + main-key <select> for each
// of the two combos. The form fetches the spec via specFromUI(); any of
// these 8 inputs being toggled must mark the form dirty AND refresh the
// live preview label (spec: 预览文本 "Ctrl+Shift+A").
for (const prefix of ['hotkey', 'record-only-hotkey']) {
    document
        .getElementById(prefix)
        .addEventListener('change', onHotkeyComboChange);
    for (const mod of ['ctrl', 'shift', 'alt']) {
        document
            .getElementById(`${prefix}-${mod}`)
            .addEventListener('change', onHotkeyComboChange);
    }
}

function onHotkeyComboChange() {
    updateDirtyState();
    for (const prefix of ['hotkey', 'record-only-hotkey']) {
        updateHotkeyPreview(prefix);
    }
}

/**
 * Render the live combo label under the form controls. An empty main-key
 * select (unknown vk from a prior config) reads as 未选择主键 rather than
 * a misleading "VK0x0" — the dirty/save validation flags it separately.
 */
function updateHotkeyPreview(prefix) {
    const el = document.getElementById(`${prefix}-preview`);
    if (!el) return;
    const spec = specFromUI(prefix);
    el.textContent =
        spec.vk === 0 ? '未选择主键' : `当前组合：${specLabel(spec)}`;
}

// Show the plaintext-persistence warning when the URL embeds credential-like
// query params. Intentionally stays visible while the LLM toggle is off:
// api_url is persisted to config.json regardless of llm_enabled, so the
// exposure does not depend on the toggle.
function updateApiUrlWarning() {
    apiUrlWarning.hidden = !hasCredentialInUrl(apiUrlInput.value.trim());
}

apiUrlInput.addEventListener('input', updateApiUrlWarning);

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

    const prevHotkey = loadedConfig?.hotkey;
    try {
        await call('save_settings', { config });
        loadedConfig = config;

        // Sync autostart state with OS (skip in dev builds without DL_AUTOSTART=1).
        let saveMsg = '✓ 已保存';
        let saveMsgType = 'success';
        if (!sameSpec(config.hotkey, prevHotkey)) {
            saveMsg = '✓ 已保存（新热键重启应用后生效）';
        }
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
        updateDirtyState();
        setSaveStatus(saveMsg, saveMsgType);
        if (saveMsg === '✓ 已保存') {
            // Pure acknowledgement — auto-clear after 1.5s.
            setTimeout(() => {
                saveStatus.textContent = '';
            }, 1500);
        } else {
            // Instructional message (restart hint / autostart failure) —
            // must survive until the user's next interaction, not evaporate.
            armClearStatusOnInteract();
        }
    } catch (e) {
        disarmClearStatusOnInteract();
        const msg = e?.message || '保存失败，请重试';
        setSaveStatus(`✗ ${msg}`, 'error');
        showError(msg);
    } finally {
        saveBtn.textContent = '保存';
        saveBtn.classList.remove('saving');
        saveBtn.disabled = !isFormDirty();
    }
});

// One-shot listeners that clear an instructional save message on the next
// user interaction (input OR click, whichever comes first). All arming
// paths detach previous listeners first so a success→failure sequence can
// never leave a stale listener clearing the error status.
const contentArea = document.querySelector('.content-area');
let detachClearListeners = null;

function armClearStatusOnInteract() {
    disarmClearStatusOnInteract();
    if (!contentArea) return;
    const clear = () => {
        saveStatus.textContent = '';
        disarmClearStatusOnInteract();
    };
    contentArea.addEventListener('input', clear, { once: true });
    contentArea.addEventListener('click', clear, { once: true });
    detachClearListeners = () => {
        contentArea.removeEventListener('input', clear);
        contentArea.removeEventListener('click', clear);
    };
}

function disarmClearStatusOnInteract() {
    if (detachClearListeners) {
        detachClearListeners();
        detachClearListeners = null;
    }
}

function setSaveStatus(message, type) {
    saveStatus.textContent = message;
    saveStatus.className = `status ${type}`;
}

export function setDirtyCheckEnabled(enabled) {
    dirtyCheckEnabled = enabled;
}

export function getLoadedAutostart() {
    return loadedAutostart;
}
