const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// DOM elements
const languageSelect = document.getElementById('language');
const hotkeySelect = document.getElementById('hotkey');
const modelCards = document.getElementById('model-cards');
const modelWarning = document.getElementById('model-warning');
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

// State
let loadedConfig = null;
let modelStatus = {};  // { tiny: true, base: false, ... }
let selectedModel = 'base';
let activeDownload = null;  // currently downloading model size
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
        renderModelCards();
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
}

// --- Model Cards ---

const MODEL_SIZES = [
    { id: 'tiny', name: 'tiny', size: '75MB' },
    { id: 'base', name: 'base', size: '142MB' },
    { id: 'small', name: 'small', size: '466MB' },
];

function renderModelCards() {
    modelCards.innerHTML = '';
    const anyDownloaded = Object.values(modelStatus).some(v => v);

    // Show warning if no models downloaded
    modelWarning.style.display = anyDownloaded ? 'none' : 'block';

    for (const m of MODEL_SIZES) {
        const downloaded = modelStatus[m.id] || false;
        const isSelected = selectedModel === m.id;
        const isDownloading = activeDownload === m.id;

        const card = document.createElement('div');
        card.className = 'model-card' + (isSelected ? ' selected' : '') + (isDownloading ? ' disabled' : '');
        card.setAttribute('role', 'radio');
        card.setAttribute('aria-checked', String(isSelected));
        card.setAttribute('aria-label', `${m.name} model, ${m.size}${downloaded ? ', downloaded' : ''}`);
        card.setAttribute('tabindex', '0');
        card.dataset.size = m.id;

        card.innerHTML = `
            <div class="model-radio"></div>
            <div class="model-info">
                <span class="model-name">${m.name}</span>
                <span class="model-size">— ${m.size}</span>
            </div>
            ${isDownloading ? renderDownloadProgress(m.id) : downloaded ? '<span class="model-status downloaded">✓ 已下载</span>' : renderDownloadButton(m.id)}
        `;

        // Click to select
        card.addEventListener('click', () => {
            if (isDownloading) return;
            if (!downloaded) return; // must download first
            selectedModel = m.id;
            renderModelCards();
            updateDirtyState();
        });

        // Keyboard support
        card.addEventListener('keydown', (e) => {
            if (e.key === ' ' || e.key === 'Enter') {
                e.preventDefault();
                card.click();
            }
            if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                e.preventDefault();
                const cards = [...modelCards.querySelectorAll('.model-card')];
                const idx = cards.indexOf(card);
                const next = e.key === 'ArrowDown' ? idx + 1 : idx - 1;
                if (cards[next]) cards[next].focus();
            }
        });

        modelCards.appendChild(card);
    }

    // Attach download button listeners
    for (const btn of modelCards.querySelectorAll('.btn-download')) {
        btn.addEventListener('click', (e) => {
            e.stopPropagation();
            startDownload(btn.dataset.size);
        });
    }

    // Attach cancel button listeners
    for (const btn of modelCards.querySelectorAll('.btn-cancel-download')) {
        btn.addEventListener('click', (e) => {
            e.stopPropagation();
            cancelDownload();
        });
    }
}

function renderDownloadButton(size) {
    return `<button class="btn-download" data-size="${size}">下载</button>`;
}

function renderDownloadProgress(size) {
    return `
        <div class="download-progress">
            <div class="progress-bar-track">
                <div class="progress-bar-fill" id="progress-fill-${size}" style="width: 0%"></div>
            </div>
            <div class="progress-info">
                <span id="progress-percent-${size}">0%</span>
                <button class="btn-cancel-download">取消</button>
            </div>
        </div>
    `;
}

// --- Download ---

async function startDownload(size) {
    activeDownload = size;
    renderModelCards();

    try {
        await invoke('download_whisper_model', { size });
        // Download complete
        activeDownload = null;
        modelStatus[size] = true;
        selectedModel = size; // auto-select newly downloaded model
        renderModelCards();
        updateDirtyState();
    } catch (e) {
        activeDownload = null;
        if (e === 'download cancelled') {
            renderModelCards();
        } else {
            showError('下载失败: ' + e);
            renderModelCards();
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

    const fill = document.getElementById(`progress-fill-${size}`);
    const pct = document.getElementById(`progress-percent-${size}`);
    if (fill) fill.style.width = percent + '%';
    if (pct) pct.textContent = percent + '%';
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
        current.llm_model !== loadedConfig.llm_model
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
    };
}

// Track changes on all inputs
languageSelect.addEventListener('change', updateDirtyState);
hotkeySelect.addEventListener('change', updateDirtyState);
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
    // Cancel active download
    if (activeDownload) {
        invoke('cancel_download');
    }
    // Unsaved changes warning
    if (isDirty) {
        e.preventDefault();
        e.returnValue = '';
    }
});

// --- Start ---
init();
