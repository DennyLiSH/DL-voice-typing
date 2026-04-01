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

// State
let loadedConfig = null;
let modelStatus = {};  // { tiny: true, base: false, ... }
let selectedModel = 'base';
let activeDownload = null;
let isDirty = false;
let dirtyCheckEnabled = false;

// --- Initialization ---

async function init() {
    try {
        const [config, models] = await Promise.all([
            invoke('get_config'),
            invoke('get_whisper_models'),
        ]);
        loadedConfig = config;
        modelStatus = models;
        selectedModel = config.whisper_model;
        populateFields(config);
        populateModelSelect();
        updateModelAction();
        updateDirtyState();
        dirtyCheckEnabled = true;
    } catch (e) {
        showError('加载配置失败: ' + e);
    }
}

function populateFields(config) {
    languageSelect.value = config.language || 'zh';
    hotkeySelect.value = config.hotkey || 'RightAlt';
    llmToggle.classList.toggle('active', config.llm_enabled);
    llmToggle.setAttribute('aria-checked', String(!!config.llm_enabled));
    updateLlmFieldsState(config.llm_enabled);
    apiUrlInput.value = config.llm_api_url || '';
    apiKeyInput.value = config.llm_api_key || '';
    modelInput.value = config.llm_model || '';
    downloadMirrorSelect.value = config.download_mirror || 'hf-mirror';
}

// --- Model Select ---

const MODEL_SIZES = [
    { id: 'tiny', name: 'tiny', size: '75MB' },
    { id: 'base', name: 'base', size: '142MB' },
    { id: 'small', name: 'small', size: '466MB' },
    { id: 'medium', name: 'medium', size: '1.5GB' },
];

function populateModelSelect() {
    whisperModelSelect.innerHTML = '';
    for (const m of MODEL_SIZES) {
        const opt = document.createElement('option');
        opt.value = m.id;
        opt.textContent = `${m.name} (${m.size})`;
        if (m.id === selectedModel) opt.selected = true;
        whisperModelSelect.appendChild(opt);
    }
}

function updateModelAction() {
    const isDownloading = activeDownload !== null;
    const downloadingThis = activeDownload === selectedModel;

    // Hide all action elements first
    modelStatusText.style.display = 'none';
    btnDownloadModel.style.display = 'none';
    downloadProgress.style.display = 'none';

    if (isDownloading && downloadingThis) {
        // Show progress bar
        downloadProgress.style.display = 'block';
        whisperModelSelect.disabled = true;
        btnDownloadModel.disabled = true;
    } else if (isDownloading) {
        // Another model is downloading — show download button but disabled
        btnDownloadModel.style.display = 'inline-block';
        btnDownloadModel.disabled = true;
        whisperModelSelect.disabled = true;
    } else if (modelStatus[selectedModel]) {
        // Already downloaded
        modelStatusText.style.display = 'inline';
        whisperModelSelect.disabled = false;
    } else {
        // Not downloaded — show download button
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

// --- Download ---

btnDownloadModel.addEventListener('click', () => {
    startDownload(selectedModel);
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
        if (e === 'download cancelled') {
            updateModelAction();
        } else {
            showError('下载失败: ' + e);
            updateModelAction();
        }
    }
}

async function cancelDownload() {
    try {
        await invoke('cancel_download');
    } catch (e) {
        console.error('cancel failed:', e);
    }
}

// Listen for download progress events
listen('download-progress', (event) => {
    const { size, percent } = event.payload;
    if (size !== activeDownload) return;

    progressFill.style.width = percent + '%';
    progressPercent.textContent = percent + '%';
});

// Listen for hotkey errors
listen('hotkey-error', (event) => {
    showError(event.payload);
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
    const apiKey = apiKeyInput.value.trim();
    const model = modelInput.value.trim();

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
        setTestStatus('✗ 连接失败: ' + e, 'error');
    } finally {
        testBtn.disabled = false;
        testBtn.textContent = '测试连接';
    }
});

function setTestStatus(message, type) {
    testStatus.textContent = message;
    testStatus.className = 'status ' + type;
}

// --- Dirty State ---

function updateDirtyState() {
    if (!loadedConfig || !dirtyCheckEnabled) return;

    const current = getCurrentConfig();
    isDirty = (
        current.language !== loadedConfig.language ||
        current.hotkey !== loadedConfig.hotkey ||
        current.whisper_model !== loadedConfig.whisper_model ||
        current.llm_enabled !== loadedConfig.llm_enabled ||
        current.llm_api_url !== loadedConfig.llm_api_url ||
        current.llm_api_key !== loadedConfig.llm_api_key ||
        current.llm_model !== loadedConfig.llm_model ||
        current.download_mirror !== loadedConfig.download_mirror
    );

    saveBtn.disabled = !isDirty;
    saveStatus.textContent = '';
    saveStatus.className = 'status';
}

function getCurrentConfig() {
    return {
        language: languageSelect.value,
        hotkey: hotkeySelect.value,
        whisper_model: selectedModel,
        llm_enabled: llmToggle.classList.contains('active'),
        llm_api_url: apiUrlInput.value.trim(),
        llm_api_key: apiKeyInput.value.trim(),
        llm_model: modelInput.value.trim(),
        download_mirror: downloadMirrorSelect.value,
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
    if (config.llm_enabled && (!config.llm_api_url || !config.llm_api_key || !config.llm_model)) {
        showError('启用 LLM 时，API 地址、密钥和模型名称不能为空');
        return;
    }

    // Validate: selected model must be downloaded
    if (!modelStatus[config.whisper_model]) {
        showError('请先下载所选的 Whisper 模型');
        return;
    }

    saveBtn.disabled = true;
    saveBtn.textContent = '保存中...';

    try {
        await invoke('save_settings', { config });
        loadedConfig = config;
        isDirty = false;
        setSaveStatus('✓ 已保存', 'success');
        setTimeout(() => { saveStatus.textContent = ''; }, 1500);
    } catch (e) {
        setSaveStatus('✗ 保存失败: ' + e, 'error');
        showError(e);
    } finally {
        saveBtn.textContent = '保存';
        saveBtn.disabled = !isDirty;
    }
});

// Ctrl+S shortcut
document.addEventListener('keydown', (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key === 's') {
        e.preventDefault();
        if (!saveBtn.disabled) saveBtn.click();
    }
});

function setSaveStatus(message, type) {
    saveStatus.textContent = message;
    saveStatus.className = 'status ' + type;
}

// --- Error Banner ---

function showError(msg) {
    errorBanner.textContent = msg;
    errorBanner.style.display = 'block';
}

function hideError() {
    errorBanner.style.display = 'none';
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

// --- Start ---
init();
