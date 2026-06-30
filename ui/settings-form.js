import { hideError, showError } from './lib/ui-utils.js';
import { getModelStatus, getSelectedModel } from './model-manager.js';

const { invoke } = window.__TAURI__.core;

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

// State
let loadedConfig = null;
let isDirty = false;
let dirtyCheckEnabled = false;
let loadedAutostart = false;

// --- Initialization ---

export function populateFields(config) {
    loadedConfig = config;
    languageSelect.value = config.language || 'zh';
    hotkeySelect.value = config.hotkey || 'RightAlt';
    llmToggle.classList.toggle('active', config.llm_enabled);
    llmToggle.setAttribute('aria-checked', String(!!config.llm_enabled));
    updateLlmFieldsState(config.llm_enabled);
    apiUrlInput.value = config.llm_api_url || '';
    modelInput.value = config.llm_model || '';
    // Handle masked API key: clear input, show placeholder
    if (config.llm_api_key === '__MASKED__') {
        apiKeyInput.value = '';
        apiKeyInput.placeholder = 'API Key 已设置';
    } else {
        apiKeyInput.value = config.llm_api_key || '';
        apiKeyInput.placeholder = 'sk-...';
    }
    downloadMirrorSelect.value = config.download_mirror || 'hf-mirror';
    dataSavingToggle.classList.toggle('active', !!config.data_saving_enabled);
    dataSavingToggle.setAttribute(
        'aria-checked',
        String(!!config.data_saving_enabled),
    );
    updateDataSavingFieldsState(!!config.data_saving_enabled);
    dataSavingPath.value = config.data_saving_path || '';
    reviewToggle.classList.toggle('active', !!config.review_before_paste);
    reviewToggle.setAttribute(
        'aria-checked',
        String(!!config.review_before_paste),
    );
    realtimeToggle.classList.toggle('active', !!config.realtime_transcription);
    realtimeToggle.setAttribute(
        'aria-checked',
        String(!!config.realtime_transcription),
    );

    // Load autostart state from config (source of truth).
    loadedAutostart = !!config.autostart;
    autostartToggle.classList.toggle('active', loadedAutostart);
    autostartToggle.setAttribute('aria-checked', String(loadedAutostart));

    // In dev builds without DL_AUTOSTART=1, gray out the autostart toggle.
    (async () => {
        try {
            const autostartAvailable = await invoke('is_autostart_available');
            if (!autostartAvailable) {
                autostartToggle.classList.add('disabled');
                autostartToggle.setAttribute('aria-disabled', 'true');
                autostartToggle.parentElement.classList.add('disabled');
            }
        } catch (_e) {
            // Non-critical: just skip gray-out
        }
    })();
}

// --- LLM Toggle ---

function updateLlmFieldsState(enabled) {
    llmFields.classList.toggle('disabled', !enabled);
    for (const input of llmFields.querySelectorAll('input')) {
        input.disabled = !enabled;
    }
    testBtn.disabled = !enabled;
}

llmToggle.addEventListener('click', () => {
    const isActive = llmToggle.classList.toggle('active');
    llmToggle.setAttribute('aria-checked', String(isActive));
    updateLlmFieldsState(isActive);
    updateDirtyState();
});

llmToggle.addEventListener('keydown', (e) => {
    if (e.key === ' ') {
        e.preventDefault();
        llmToggle.click();
    }
});

// --- Data Saving Toggle ---

function updateDataSavingFieldsState(enabled) {
    dataSavingFields.classList.toggle('disabled', !enabled);
    for (const input of dataSavingFields.querySelectorAll('input')) {
        input.disabled = !enabled;
    }
    btnBrowsePath.disabled = !enabled;
}

dataSavingToggle.addEventListener('click', () => {
    const isActive = dataSavingToggle.classList.toggle('active');
    dataSavingToggle.setAttribute('aria-checked', String(isActive));
    updateDataSavingFieldsState(isActive);
    updateDirtyState();
});

dataSavingToggle.addEventListener('keydown', (e) => {
    if (e.key === ' ') {
        e.preventDefault();
        dataSavingToggle.click();
    }
});

// --- Review Before Paste Toggle ---

reviewToggle.addEventListener('click', () => {
    const isActive = reviewToggle.classList.toggle('active');
    reviewToggle.setAttribute('aria-checked', String(isActive));
    updateDirtyState();
});

reviewToggle.addEventListener('keydown', (e) => {
    if (e.key === ' ') {
        e.preventDefault();
        reviewToggle.click();
    }
});

// --- Autostart Toggle ---

autostartToggle.addEventListener('click', () => {
    if (autostartToggle.classList.contains('disabled')) return;
    const isActive = autostartToggle.classList.toggle('active');
    autostartToggle.setAttribute('aria-checked', String(isActive));
    updateDirtyState();
});

autostartToggle.addEventListener('keydown', (e) => {
    if (e.key === ' ') {
        e.preventDefault();
        autostartToggle.click();
    }
});

// --- Realtime Transcription Toggle ---

realtimeToggle.addEventListener('click', () => {
    const isActive = realtimeToggle.classList.toggle('active');
    realtimeToggle.setAttribute('aria-checked', String(isActive));
    updateDirtyState();
});

realtimeToggle.addEventListener('keydown', (e) => {
    if (e.key === ' ') {
        e.preventDefault();
        realtimeToggle.click();
    }
});

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
        loadedConfig && loadedConfig.llm_api_key === '__MASKED__';
    const apiKey = apiKeyRaw || (hasExistingKey ? '__MASKED__' : '');

    if (!apiUrl || !apiKey || !model) {
        setTestStatus('请填写所有字段', 'error');
        return;
    }

    testBtn.disabled = true;
    testBtn.textContent = '测试中...';
    testStatus.textContent = '';

    try {
        await invoke('test_llm_connection', { apiUrl, apiKey, model });
        setTestStatus('✓ 连接成功', 'success');
    } catch (e) {
        const message =
            typeof e === 'object' && e?.message ? e.message : String(e);
        invoke('log_frontend_error', {
            message,
            stack: null,
            context: 'test_llm_connection',
        }).catch(() => {});
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

    const current = getCurrentConfig();
    // API key dirty check: only dirty if user typed a new key (non-empty, non-masked)
    const hasExistingKey = loadedConfig.llm_api_key === '__MASKED__';
    const apiKeyDirty = hasExistingKey
        ? current.llm_api_key !== '__MASKED__'
        : current.llm_api_key !== loadedConfig.llm_api_key;
    isDirty =
        current.language !== loadedConfig.language ||
        current.hotkey !== loadedConfig.hotkey ||
        current.whisper_model !== loadedConfig.whisper_model ||
        current.llm_enabled !== loadedConfig.llm_enabled ||
        current.llm_api_url !== loadedConfig.llm_api_url ||
        apiKeyDirty ||
        current.llm_model !== loadedConfig.llm_model ||
        current.download_mirror !== loadedConfig.download_mirror ||
        current.data_saving_enabled !== loadedConfig.data_saving_enabled ||
        current.data_saving_path !== loadedConfig.data_saving_path ||
        current.review_before_paste !== loadedConfig.review_before_paste ||
        current.autostart !== loadedConfig.autostart ||
        current.realtime_transcription !== loadedConfig.realtime_transcription;

    saveBtn.disabled = !isDirty;
    saveStatus.textContent = '';
    saveStatus.className = 'status';
}

export function getCurrentConfig() {
    const apiKeyValue = apiKeyInput.value.trim();
    // Send masked marker only if user hasn't typed anything AND a key was previously set
    const hasExistingKey =
        loadedConfig && loadedConfig.llm_api_key === '__MASKED__';
    return {
        language: languageSelect.value,
        hotkey: hotkeySelect.value,
        whisper_model: getSelectedModel(),
        llm_enabled: llmToggle.classList.contains('active'),
        llm_api_url: apiUrlInput.value.trim(),
        llm_api_key: apiKeyValue || (hasExistingKey ? '__MASKED__' : ''),
        llm_model: modelInput.value.trim(),
        download_mirror: downloadMirrorSelect.value,
        data_saving_enabled: dataSavingToggle.classList.contains('active'),
        data_saving_path: dataSavingPath.value.trim(),
        review_before_paste: reviewToggle.classList.contains('active'),
        autostart: autostartToggle.classList.contains('active'),
        realtime_transcription: realtimeToggle.classList.contains('active'),
    };
}

// Track changes on all inputs
languageSelect.addEventListener('change', updateDirtyState);
hotkeySelect.addEventListener('change', updateDirtyState);
downloadMirrorSelect.addEventListener('change', updateDirtyState);
apiUrlInput.addEventListener('input', updateDirtyState);
apiKeyInput.addEventListener('input', updateDirtyState);
modelInput.addEventListener('input', updateDirtyState);

// --- Save ---

saveBtn.addEventListener('click', async () => {
    hideError();
    const config = getCurrentConfig();

    // Validate: LLM fields when enabled
    if (
        config.llm_enabled &&
        (!config.llm_api_url || !config.llm_api_key || !config.llm_model)
    ) {
        showError('启用 LLM 时，API 地址、密钥和模型名称不能为空');
        return;
    }

    // Validate: selected model must be downloaded (built-in) or exist (custom)
    const isCustom = config.whisper_model.startsWith('custom:');
    if (!isCustom && !getModelStatus()[config.whisper_model]) {
        showError('请先下载所选的 Whisper 模型');
        return;
    }

    // Validate: data saving path when enabled
    if (config.data_saving_enabled && !config.data_saving_path) {
        showError('启用数据保存时，必须设置保存路径');
        return;
    }

    saveBtn.disabled = true;
    saveBtn.textContent = '保存中...';
    saveBtn.classList.add('saving');

    try {
        await invoke('save_settings', { config });
        loadedConfig = config;

        // Sync autostart state with OS (skip in dev builds without DL_AUTOSTART=1).
        let saveMsg = '✓ 已保存';
        let saveMsgType = 'success';
        try {
            const wantAutostart = autostartToggle.classList.contains('active');
            const autostartAvailable = await invoke('is_autostart_available');
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
    } catch (e) {
        const message =
            typeof e === 'object' && e?.message ? e.message : String(e);
        invoke('log_frontend_error', {
            message,
            stack: null,
            context: 'save_settings',
        }).catch(() => {});
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
