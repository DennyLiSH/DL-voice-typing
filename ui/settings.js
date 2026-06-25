import {
    buildExpandedMetadata,
    buildRecordingRow,
    computeOffsetAfterDeletion,
    deleteConfirmMessage,
    formatBytes,
    getPageRange,
} from '../lib/data-management.js';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// DOM elements
const languageSelect = document.getElementById('language');
const hotkeySelect = document.getElementById('hotkey');
const whisperModelSelect = document.getElementById('whisper-model');
const modelStatusText = document.getElementById('model-status-text');
const btnDownloadModel = document.getElementById('btn-download-model');
const downloadProgress = document.getElementById('download-progress');
const progressFill = document.getElementById('progress-fill');
const progressPercent = document.getElementById('progress-percent');
const btnCancelDownload = document.getElementById('btn-cancel-download');
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
const errorBanner = document.getElementById('error-banner');
const downloadMirrorSelect = document.getElementById('download-mirror');
const dataSavingToggle = document.getElementById('data-saving-toggle');
const dataSavingFields = document.getElementById('data-saving-fields');
const dataSavingPath = document.getElementById('data-saving-path');
const btnBrowsePath = document.getElementById('btn-browse-path');
const reviewToggle = document.getElementById('review-toggle');
const autostartToggle = document.getElementById('autostart-toggle');
const realtimeToggle = document.getElementById('realtime-transcription-toggle');

// Sidebar elements
const sidebarItems = document.querySelectorAll('.sidebar-item');
const pageContents = document.querySelectorAll('.page-content');

// State
let loadedConfig = null;
let modelStatus = {}; // { tiny: true, base: false, ... }
let customModels = []; // ["my-model.bin", ...]
let selectedModel = 'base';
let activeDownload = null;
let isDirty = false;
let dirtyCheckEnabled = false;
let loadedAutostart = false;
let currentPage = 'general';

// --- Sidebar Navigation ---

function switchPage(pageName) {
    // Page leave hook: pause audio + clear audio state when leaving data sub-page.
    if (currentPage === 'data' && pageName !== 'data') {
        onDataPageLeave();
    }
    currentPage = pageName;
    sidebarItems.forEach((item) => {
        const isActive = item.dataset.page === pageName;
        item.classList.toggle('active', isActive);
        item.setAttribute('aria-selected', String(isActive));
        item.setAttribute('tabindex', isActive ? '0' : '-1');
    });
    pageContents.forEach((page) => {
        page.classList.toggle('active', page.id === `page-${pageName}`);
    });
    // Page enter hook: reset + reload data list when entering data sub-page.
    if (pageName === 'data') {
        onDataPageEnter();
    }
}

sidebarItems.forEach((item) => {
    item.addEventListener('click', () => switchPage(item.dataset.page));
    item.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            switchPage(item.dataset.page);
        }
    });
});

// Keyboard navigation: arrow keys in sidebar
document.querySelector('.sidebar').addEventListener('keydown', (e) => {
    if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return;
    e.preventDefault();
    const pages = Array.from(sidebarItems).map((item) => item.dataset.page);
    const idx = pages.indexOf(currentPage);
    const next =
        e.key === 'ArrowDown'
            ? pages[(idx + 1) % pages.length]
            : pages[(idx - 1 + pages.length) % pages.length];
    switchPage(next);
    document.querySelector(`.sidebar-item[data-page="${next}"]`).focus();
});

// --- Initialization ---

async function init() {
    try {
        const [config, modelsData] = await Promise.all([
            invoke('get_config'),
            invoke('get_whisper_models'),
        ]);
        loadedConfig = config;
        modelStatus = modelsData.built_in;
        customModels = modelsData.custom;
        selectedModel = config.whisper_model;
        populateFields(config);
        populateModelSelect();
        updateModelAction();
        loadComputeMode();
        updateDirtyState();
        dirtyCheckEnabled = true;

        // Load autostart state from config (source of truth).
        loadedAutostart = !!config.autostart;
        autostartToggle.classList.toggle('active', loadedAutostart);
        autostartToggle.setAttribute('aria-checked', String(loadedAutostart));

        // In dev builds without DL_AUTOSTART=1, gray out the autostart toggle.
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

        updateDirtyState();

        // Display version
        try {
            const version = await window.__TAURI__.app.getVersion();
            document.getElementById('version-info').textContent =
                `语文兔 v${version}`;
            document.getElementById('version-display').textContent =
                `v${version}`;
        } catch (_e) {
            // Version display is non-critical, silently ignore
        }
    } catch (_e) {
        showError('加载配置失败，请重试');
    }
}

function populateFields(config) {
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
}

// --- Model Select ---

const MODEL_SIZES = [
    { id: 'tiny', name: 'Tiny', size: '75MB' },
    { id: 'tiny-q8_0', name: 'Tiny Q8_0', size: '~40MB', tag: '量化' },
    { id: 'base', name: 'Base', size: '142MB' },
    { id: 'base-q8_0', name: 'Base Q8_0', size: '~75MB', tag: '量化' },
    { id: 'small', name: 'Small', size: '466MB' },
    { id: 'small-q8_0', name: 'Small Q8_0', size: '~250MB', tag: '量化' },
    { id: 'medium', name: 'Medium', size: '1.5GB' },
    { id: 'medium-q8_0', name: 'Medium Q8_0', size: '~800MB', tag: '量化' },
];

function populateModelSelect() {
    whisperModelSelect.innerHTML = '';

    // Built-in group
    const builtInGroup = document.createElement('optgroup');
    builtInGroup.label = '内置模型';
    for (const m of MODEL_SIZES) {
        const opt = document.createElement('option');
        opt.value = m.id;
        opt.textContent = m.tag
            ? `${m.name} (${m.size}) [${m.tag}]`
            : `${m.name} (${m.size})`;
        if (m.id === selectedModel) opt.selected = true;
        builtInGroup.appendChild(opt);
    }
    whisperModelSelect.appendChild(builtInGroup);

    // Custom group (only if there are custom models)
    if (customModels.length > 0) {
        const customGroup = document.createElement('optgroup');
        customGroup.label = '自定义模型';
        for (const name of customModels) {
            const opt = document.createElement('option');
            opt.value = `custom:${name}`;
            opt.textContent = name;
            if (`custom:${name}` === selectedModel) opt.selected = true;
            customGroup.appendChild(opt);
        }
        whisperModelSelect.appendChild(customGroup);
    }
}

function updateModelAction() {
    const isDownloading = activeDownload !== null;
    const downloadingThis = activeDownload === selectedModel;
    const isCustom = selectedModel.startsWith('custom:');

    // Hide all action elements first
    modelStatusText.style.display = 'none';
    btnDownloadModel.style.display = 'none';
    downloadProgress.style.display = 'none';
    btnDownloadModel.textContent = '下载';
    btnDownloadModel.className = 'btn-download-model';

    if (isCustom && !isDownloading) {
        // Custom model — show delete button
        btnDownloadModel.textContent = '删除';
        btnDownloadModel.className = 'btn-download-model btn-delete-model';
        btnDownloadModel.style.display = 'inline-block';
        btnDownloadModel.disabled = false;
        whisperModelSelect.disabled = false;
    } else if (isDownloading && downloadingThis) {
        downloadProgress.style.display = 'block';
        whisperModelSelect.disabled = true;
        btnDownloadModel.disabled = true;
    } else if (isDownloading) {
        btnDownloadModel.style.display = 'inline-block';
        btnDownloadModel.disabled = true;
        whisperModelSelect.disabled = true;
    } else if (isCustom) {
        // Custom model during download of another model
        btnDownloadModel.textContent = '删除';
        btnDownloadModel.className = 'btn-download-model btn-delete-model';
        btnDownloadModel.style.display = 'inline-block';
        btnDownloadModel.disabled = true;
        whisperModelSelect.disabled = true;
    } else if (modelStatus[selectedModel]) {
        modelStatusText.style.display = 'inline';
        whisperModelSelect.disabled = false;
    } else {
        btnDownloadModel.style.display = 'inline-block';
        btnDownloadModel.disabled = false;
        whisperModelSelect.disabled = false;
    }
}

whisperModelSelect.addEventListener('change', () => {
    selectedModel = whisperModelSelect.value;
    updateModelAction();
    updateDirtyState();
});

// --- Compute Mode ---

async function loadComputeMode() {
    const badge = document.getElementById('compute-mode-badge');
    try {
        const mode = await invoke('get_compute_mode');
        if (mode === 'gpu') {
            badge.textContent = 'GPU 加速';
            badge.className = 'mode-badge gpu';
        } else if (mode === 'cpu') {
            badge.textContent = 'CPU 模式（未检测到 GPU）';
            badge.className = 'mode-badge cpu';
        } else {
            badge.textContent = '模型未加载';
            badge.className = 'mode-badge unloaded';
        }
    } catch (_e) {
        badge.textContent = '检测失败';
        badge.className = 'mode-badge unloaded';
    }
}

// --- Download ---

btnDownloadModel.addEventListener('click', async () => {
    const isCustom = selectedModel.startsWith('custom:');
    if (isCustom) {
        const filename = selectedModel.replace(/^custom:/, '');
        if (!confirm(`确认删除模型 ${filename}？`)) return;
        try {
            await invoke('delete_custom_model', { filename });
            // Refresh model list
            const modelsData = await invoke('get_whisper_models');
            modelStatus = modelsData.built_in;
            customModels = modelsData.custom;
            // If deleted was selected, reset to base
            if (!customModels.some((n) => `custom:${n}` === selectedModel)) {
                selectedModel = 'base';
            }
            populateModelSelect();
            updateModelAction();
            updateDirtyState();
        } catch (e) {
            const message =
                typeof e === 'object' && e?.message ? e.message : String(e);
            invoke('log_frontend_error', {
                message,
                stack: null,
                context: 'delete_custom_model',
            }).catch(() => {});
            showError('删除失败，请重试');
        }
    } else {
        startDownload(selectedModel);
    }
});

btnCancelDownload.addEventListener('click', () => {
    cancelDownload();
});

async function startDownload(size) {
    activeDownload = size;
    progressFill.style.width = '0%';
    progressPercent.textContent = '0%';
    updateModelAction();

    try {
        await invoke('download_whisper_model', { size });
        activeDownload = null;
        modelStatus[size] = true;
        updateModelAction();
        updateDirtyState();
    } catch (e) {
        activeDownload = null;
        // F6 fix: defensive check matches both raw string and CommandError-object shapes
        const isCancel =
            e === 'download cancelled' || e?.message === 'download cancelled';
        if (isCancel) {
            updateModelAction();
        } else {
            const message =
                typeof e === 'object' && e?.message ? e.message : String(e);
            invoke('log_frontend_error', {
                message,
                stack: null,
                context: 'download_whisper_model',
            }).catch(() => {});
            showError('下载失败，请重试');
            updateModelAction();
        }
    }
}

async function cancelDownload() {
    try {
        await invoke('cancel_download');
    } catch (_e) {
        progressPercent.textContent = '取消下载失败';
    }
}

// Listen for download progress events
listen('download-progress', (event) => {
    const { size, percent } = event.payload;
    if (size !== activeDownload) return;

    progressFill.style.width = `${percent}%`;
    progressPercent.textContent = `${percent}%`;
});

// Listen for hotkey errors
listen('hotkey-error', (event) => {
    showError(event.payload);
});

// Listen for background model loading completion
listen('model-loaded', () => {
    loadComputeMode();
});

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

// --- Password Toggle ---

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

function updateDirtyState() {
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

function getCurrentConfig() {
    const apiKeyValue = apiKeyInput.value.trim();
    // Send masked marker only if user hasn't typed anything AND a key was previously set
    const hasExistingKey =
        loadedConfig && loadedConfig.llm_api_key === '__MASKED__';
    return {
        language: languageSelect.value,
        hotkey: hotkeySelect.value,
        whisper_model: selectedModel,
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
    if (!isCustom && !modelStatus[config.whisper_model]) {
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

// --- Error Banner ---

function showError(msg) {
    errorBanner.textContent = msg;
    errorBanner.classList.add('visible');
}

function hideError() {
    errorBanner.classList.remove('visible');
}

// --- Window Close ---

window.addEventListener('beforeunload', (e) => {
    if (activeDownload) {
        invoke('cancel_download');
    }
    if (isDirty) {
        e.preventDefault();
        e.returnValue = '';
    }
});

// ============================================================
// Data management — saved recordings list
// (Constraints F1-F11 from plan review)
// ============================================================

const dataState = {
    offset: 0,
    limit: 50,
    query: '',
    total: 0,
    items: [],
    selectedFiles: new Set(),
    expandedRowId: null,
    audioPlayerRowId: null,
    audioElement: null,
    isLoading: false,
    lastReqId: 0,
    searchDebounceTimer: null,
};

// DOM refs (resolved lazily because the script may run before #page-data exists
// in some test environments).
function $data(id) {
    return document.getElementById(id);
}

function onDataPageEnter() {
    // Reset all state on entry (constraint #1 from design review).
    resetDataListState();
    loadRecordingsPage(0);
}

function onDataPageLeave() {
    // Pause audio + clear audio state (constraint #2).
    if (dataState.audioElement) {
        try {
            dataState.audioElement.pause();
        } catch (_e) {
            /* ignore */
        }
    }
    dataState.audioPlayerRowId = null;
}

function resetDataListState() {
    dataState.offset = 0;
    dataState.query = '';
    dataState.total = 0;
    dataState.items = [];
    dataState.selectedFiles.clear();
    dataState.expandedRowId = null;
    if (dataState.audioElement) {
        try {
            dataState.audioElement.pause();
        } catch (_e) {
            /* ignore */
        }
        dataState.audioElement = null;
    }
    dataState.audioPlayerRowId = null;
    dataState.isLoading = false;
    dataState.lastReqId = 0;

    const searchInput = $data('data-search-input');
    if (searchInput) searchInput.value = '';
    const errBar = $data('data-error-bar');
    if (errBar) errBar.hidden = true;
}

async function loadRecordingsPage(offset) {
    if (dataState.isLoading) return; // race guard (constraint #8)
    dataState.isLoading = true;
    const reqId = ++dataState.lastReqId; // out-of-order response guard (constraint F9)
    const refreshBtn = $data('btn-refresh-data');
    if (refreshBtn) {
        refreshBtn.disabled = true;
        refreshBtn.classList.add('loading');
    }
    try {
        const resp = await invoke('list_saved_recordings', {
            offset,
            limit: dataState.limit,
            query: dataState.query || null,
        });
        // Stale response guard — discard if a newer request superseded us.
        if (reqId !== dataState.lastReqId) return;
        dataState.offset = resp.offset;
        dataState.total = resp.total;
        dataState.items = resp.items;
        renderDataList();
        renderStats(resp.total, resp.total_bytes);
        renderPagination();
        renderEmptyState();
        hideDataError();
    } catch (e) {
        if (reqId !== dataState.lastReqId) return;
        showDataError(typeof e === 'string' ? e : e?.message || '加载失败');
        // Keep previous list contents intact (constraint F2).
    } finally {
        if (reqId === dataState.lastReqId) {
            dataState.isLoading = false;
            if (refreshBtn) {
                refreshBtn.disabled = false;
                refreshBtn.classList.remove('loading');
            }
        }
    }
}

function renderDataList() {
    const list = $data('data-list');
    if (!list) return;
    list.innerHTML = '';
    for (const entry of dataState.items) {
        const isSelected = dataState.selectedFiles.has(entry.filename);
        const isExpanded = dataState.expandedRowId === entry.filename;
        const row = buildRecordingRow(entry, {
            selected: isSelected,
            expanded: isExpanded,
        });
        list.appendChild(row);
        if (isExpanded) {
            const meta = buildExpandedMetadata(entry);
            meta.classList.add('data-row-meta-wrapper');
            list.appendChild(meta);
        }
        if (dataState.audioPlayerRowId === entry.filename) {
            const playerWrap = document.createElement('div');
            playerWrap.className = 'data-row-player';
            const audio = document.createElement('audio');
            audio.controls = true;
            audio.autoplay = true;
            // The actual src will be set by attachAudioSrc() once the bytes arrive.
            playerWrap.appendChild(audio);
            list.appendChild(playerWrap);
            dataState.audioElement = audio;
            audio.addEventListener('error', () => onAudioError(entry.filename));
            audio.addEventListener('ended', () => {
                // Auto-cleanup is optional; keep player visible until user closes.
            });
            // Fetch bytes asynchronously.
            attachAudioSrc(audio, entry.filename);
        }
    }
    updateBatchBar();
}

async function attachAudioSrc(audioEl, filename) {
    try {
        const bytes = await invoke('read_recording_audio', { filename });
        // Tauri returns ArrayLike<number>; convert to Uint8Array for Blob.
        const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
        const blob = new Blob([u8], { type: 'audio/wav' });
        const url = URL.createObjectURL(blob);
        audioEl.dataset.blobUrl = url;
        audioEl.src = url;
    } catch (_e) {
        // Mark this row's audio as failed.
        onAudioError(filename);
    }
}

function onAudioError(filename) {
    if (dataState.audioPlayerRowId !== filename) return;
    const row = document.querySelector(
        `.data-row[data-filename="${cssEscape(filename)}"]`,
    );
    if (row) {
        // Replace play button area with a temporary error badge.
        const existing = row.querySelector('.audio-error-badge');
        if (!existing) {
            const badge = document.createElement('span');
            badge.className = 'audio-error-badge';
            badge.textContent = '音频加载失败';
            row.appendChild(badge);
            // Auto-remove after 3 seconds (constraint F4).
            setTimeout(() => badge.remove(), 3000);
        }
    }
    // Collapse the player.
    dataState.audioPlayerRowId = null;
    dataState.audioElement = null;
    renderDataList();
}

function renderStats(total, totalBytes) {
    const countEl = $data('data-total-count');
    const sizeEl = $data('data-total-size');
    if (countEl) countEl.textContent = String(total);
    if (sizeEl) sizeEl.textContent = formatBytes(totalBytes);
}

function renderPagination() {
    const pagination = $data('data-pagination');
    if (!pagination) return;
    const { currentPage, totalPages, hasNext, hasPrev } = getPageRange(
        dataState.total,
        dataState.offset,
        dataState.limit,
    );
    if (dataState.total === 0) {
        pagination.hidden = true;
        return;
    }
    pagination.hidden = totalPages <= 1;
    const info = $data('data-page-info');
    if (info) info.textContent = `${currentPage} / ${totalPages}`;
    const prev = $data('btn-prev-page');
    const next = $data('btn-next-page');
    if (prev) prev.disabled = !hasPrev;
    if (next) next.disabled = !hasNext;
}

function renderEmptyState() {
    const empty = $data('data-empty-state');
    if (!empty) return;
    if (dataState.items.length > 0) {
        empty.hidden = true;
        return;
    }
    empty.hidden = false;
    // Distinguish two empty states (constraint #7).
    if (dataState.query) {
        empty.textContent = '未找到匹配的录音';
    } else {
        empty.textContent = '暂无录音数据';
    }
}

function updateBatchBar() {
    const bar = $data('data-batch-bar');
    if (!bar) return;
    bar.hidden = dataState.selectedFiles.size === 0;
    const selCountEl = $data('data-selected-count');
    const visCountEl = $data('data-visible-count');
    if (selCountEl)
        selCountEl.textContent = String(dataState.selectedFiles.size);
    if (visCountEl) visCountEl.textContent = String(dataState.items.length);
}

function showDataError(msg) {
    const bar = $data('data-error-bar');
    if (bar) {
        bar.textContent = msg;
        bar.hidden = false;
    }
}

function hideDataError() {
    const bar = $data('data-error-bar');
    if (bar) bar.hidden = true;
}

function cssEscape(s) {
    if (
        typeof window.CSS !== 'undefined' &&
        typeof window.CSS.escape === 'function'
    ) {
        return window.CSS.escape(s);
    }
    return String(s).replace(/["\\]/g, '\\$&');
}

// --- Event wiring ---

function wireDataListEvents() {
    const refreshBtn = $data('btn-refresh-data');
    if (refreshBtn) {
        refreshBtn.addEventListener('click', () =>
            loadRecordingsPage(dataState.offset),
        );
    }

    const searchInput = $data('data-search-input');
    if (searchInput) {
        // Esc clears search (constraint #5)
        searchInput.addEventListener('keydown', (e) => {
            if (e.key === 'Escape') {
                searchInput.value = '';
                dataState.query = '';
                loadRecordingsPage(0);
            }
        });
        // Debounced search trigger (300ms)
        searchInput.addEventListener('input', () => {
            if (dataState.searchDebounceTimer) {
                clearTimeout(dataState.searchDebounceTimer);
            }
            dataState.searchDebounceTimer = setTimeout(() => {
                dataState.query = searchInput.value.trim();
                loadRecordingsPage(0);
            }, 300);
        });
    }

    const list = $data('data-list');
    if (list) {
        // Delegated click handler for the whole list.
        list.addEventListener('click', (e) => {
            const row = e.target.closest('.data-row');
            if (!row) return;
            const filename = row.dataset.filename;
            if (!filename) return;

            // Checkbox toggle
            if (e.target.classList.contains('data-row-cb')) {
                if (e.target.checked) {
                    dataState.selectedFiles.add(filename);
                } else {
                    dataState.selectedFiles.delete(filename);
                }
                row.classList.toggle(
                    'selected',
                    dataState.selectedFiles.has(filename),
                );
                updateBatchBar();
                return;
            }

            // Play button
            if (e.target.classList.contains('btn-play')) {
                e.stopPropagation();
                // Toggle: clicking again collapses.
                if (dataState.audioPlayerRowId === filename) {
                    dataState.audioPlayerRowId = null;
                    if (dataState.audioElement) {
                        try {
                            dataState.audioElement.pause();
                        } catch (_e) {
                            /* ignore */
                        }
                        if (dataState.audioElement.dataset.blobUrl) {
                            URL.revokeObjectURL(
                                dataState.audioElement.dataset.blobUrl,
                            );
                        }
                        dataState.audioElement = null;
                    }
                } else {
                    if (dataState.audioElement) {
                        try {
                            dataState.audioElement.pause();
                        } catch (_e) {
                            /* ignore */
                        }
                        if (dataState.audioElement.dataset.blobUrl) {
                            URL.revokeObjectURL(
                                dataState.audioElement.dataset.blobUrl,
                            );
                        }
                        dataState.audioElement = null;
                    }
                    dataState.audioPlayerRowId = filename;
                }
                renderDataList();
                return;
            }

            // Delete button
            if (e.target.classList.contains('btn-delete')) {
                e.stopPropagation();
                handleSingleDelete(filename);
                return;
            }

            // Row body click → toggle expand (single-row expand, constraint implied)
            if (dataState.expandedRowId === filename) {
                dataState.expandedRowId = null;
            } else {
                dataState.expandedRowId = filename;
            }
            renderDataList();
        });
    }

    const selectAllCb = $data('data-select-all-cb');
    if (selectAllCb) {
        // Select-all only affects current page (constraint #6).
        selectAllCb.addEventListener('change', () => {
            if (selectAllCb.checked) {
                for (const item of dataState.items) {
                    dataState.selectedFiles.add(item.filename);
                }
            } else {
                for (const item of dataState.items) {
                    dataState.selectedFiles.delete(item.filename);
                }
            }
            renderDataList();
        });
    }

    const batchBtn = $data('btn-batch-delete');
    if (batchBtn) {
        batchBtn.addEventListener('click', handleBatchDelete);
    }

    const prevBtn = $data('btn-prev-page');
    if (prevBtn) {
        prevBtn.addEventListener('click', () => {
            // Page change clears selection (constraint #6).
            dataState.selectedFiles.clear();
            dataState.expandedRowId = null;
            loadRecordingsPage(Math.max(0, dataState.offset - dataState.limit));
        });
    }
    const nextBtn = $data('btn-next-page');
    if (nextBtn) {
        nextBtn.addEventListener('click', () => {
            dataState.selectedFiles.clear();
            dataState.expandedRowId = null;
            loadRecordingsPage(dataState.offset + dataState.limit);
        });
    }
}

async function handleSingleDelete(filename) {
    if (!confirm(deleteConfirmMessage(1))) return;
    try {
        await invoke('delete_recording', { filename });
        // Clear audio state if it was this row (constraint #10a).
        if (dataState.audioPlayerRowId === filename) {
            if (dataState.audioElement) {
                try {
                    dataState.audioElement.pause();
                } catch (_e) {
                    /* ignore */
                }
                if (dataState.audioElement.dataset.blobUrl) {
                    URL.revokeObjectURL(dataState.audioElement.dataset.blobUrl);
                }
                dataState.audioElement = null;
            }
            dataState.audioPlayerRowId = null;
        }
        dataState.selectedFiles.delete(filename);
        if (dataState.expandedRowId === filename) {
            dataState.expandedRowId = null;
        }
        // Auto-navigate to last valid page if current becomes empty (constraint #9).
        const itemsOnPage = dataState.items.length;
        if (itemsOnPage === 1 && dataState.offset > 0) {
            const newOffset = computeOffsetAfterDeletion(
                dataState.total,
                1,
                dataState.limit,
                dataState.offset,
            );
            await loadRecordingsPage(newOffset);
        } else {
            await loadRecordingsPage(dataState.offset);
        }
    } catch (e) {
        alert(
            `删除失败：${typeof e === 'string' ? e : e?.message || '未知错误'}`,
        );
    }
}

async function handleBatchDelete() {
    const count = dataState.selectedFiles.size;
    if (count === 0) return;
    if (!confirm(deleteConfirmMessage(count))) return;
    const filenames = Array.from(dataState.selectedFiles);
    try {
        const result = await invoke('delete_recordings', { filenames });
        dataState.selectedFiles.clear();
        // Clear audio if it was a selected row.
        if (
            dataState.audioPlayerRowId &&
            filenames.includes(dataState.audioPlayerRowId)
        ) {
            if (dataState.audioElement) {
                try {
                    dataState.audioElement.pause();
                } catch (_e) {
                    /* ignore */
                }
                if (dataState.audioElement.dataset.blobUrl) {
                    URL.revokeObjectURL(dataState.audioElement.dataset.blobUrl);
                }
                dataState.audioElement = null;
            }
            dataState.audioPlayerRowId = null;
        }
        if (
            dataState.expandedRowId &&
            filenames.includes(dataState.expandedRowId)
        ) {
            dataState.expandedRowId = null;
        }
        // Compute new offset using fresh total (constraint #9 + F6).
        const deletedCount = filenames.length;
        const newOffset = computeOffsetAfterDeletion(
            dataState.total,
            deletedCount,
            dataState.limit,
            dataState.offset,
        );
        await loadRecordingsPage(newOffset);
        // Show partial success message (constraint F3).
        if (result.failed && result.failed.length > 0) {
            const failedList = result.failed
                .map((f) => `${f.filename}：${f.error}`)
                .join('\n');
            alert(
                `已删除 ${result.deleted} 条，失败 ${result.failed.length} 条：\n${failedList}`,
            );
        }
    } catch (e) {
        alert(
            `批量删除失败：${typeof e === 'string' ? e : e?.message || '未知错误'}`,
        );
    }
}

// Wire events after DOM is ready (script runs at end of body, so DOM is ready).
wireDataListEvents();

// --- Start ---
init();
