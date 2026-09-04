import { call, rawInvoke, reportError } from './lib/api.js';
import { notifyFormChange } from './lib/form-state.js';
import { confirmDialog } from './lib/confirm-dialog.js';
import { showError } from './lib/ui-utils.js';

const { listen } = window.__TAURI__.event;

// DOM elements
const whisperModelSelect = document.getElementById('whisper-model');
const modelStatusText = document.getElementById('model-status-text');
const btnDownloadModel = document.getElementById('btn-download-model');
const downloadProgress = document.getElementById('download-progress');
const progressFill = document.getElementById('progress-fill');
const progressPercent = document.getElementById('progress-percent');
const btnCancelDownload = document.getElementById('btn-cancel-download');

// State
let modelStatus = {}; // { tiny: true, base: false, ... }
let customModels = []; // ["my-model.bin", ...]
let selectedModel = 'base';
let activeDownload = null;

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

export function populateModelSelect() {
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

export function updateModelAction() {
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
        btnDownloadModel.className = 'btn-download-model btn-danger';
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
        btnDownloadModel.className = 'btn-download-model btn-danger';
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
    notifyFormChange();
});

// --- Compute Mode ---

export async function loadComputeMode() {
    const badge = document.getElementById('compute-mode-badge');
    try {
        const mode = await call('get_compute_mode');
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
        const ok = await confirmDialog({
            title: '删除模型',
            message: `确认删除模型 ${filename}？`,
            danger: true,
        });
        if (!ok) return;
        try {
            await call('delete_custom_model', { filename });
            // Refresh model list
            const modelsData = await call('get_whisper_models');
            modelStatus = modelsData.built_in;
            customModels = modelsData.custom;
            // If deleted was selected, reset to base
            if (!customModels.some((n) => `custom:${n}` === selectedModel)) {
                selectedModel = 'base';
            }
            populateModelSelect();
            updateModelAction();
            notifyFormChange();
        } catch (e) {
            showError(e?.message || '删除失败，请重试');
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
    progressFill.style.transform = 'scaleX(0)';
    progressPercent.textContent = '0%';
    downloadProgress.setAttribute('aria-valuenow', '0');
    updateModelAction();

    try {
        await rawInvoke('download_whisper_model', { size });
        activeDownload = null;
        modelStatus[size] = true;
        updateModelAction();
        notifyFormChange();
    } catch (e) {
        activeDownload = null;
        // F6 fix: defensive check matches both raw string and CommandError-object shapes
        const isCancel =
            e === 'download cancelled' || e?.message === 'download cancelled';
        if (isCancel) {
            updateModelAction();
        } else {
            reportError(e, 'download_whisper_model');
            showError(e?.message || '下载失败，请重试');
            updateModelAction();
        }
    }
}

async function cancelDownload() {
    try {
        await rawInvoke('cancel_download');
    } catch (_e) {
        progressPercent.textContent = '取消下载失败';
    }
}

// Listen for download progress events
listen('download-progress', (event) => {
    const { size, percent } = event.payload;
    if (size !== activeDownload) return;

    progressFill.style.transform = `scaleX(${percent / 100})`;
    progressPercent.textContent = `${percent}%`;
    downloadProgress.setAttribute('aria-valuenow', String(percent));
});

// Listen for background model loading completion
listen('model-loaded', () => {
    loadComputeMode();
});

// --- State getters / setters ---

export function setModelStatus(status) {
    modelStatus = status;
}

export function setCustomModels(models) {
    customModels = models;
}

export function setSelectedModel(model) {
    selectedModel = model;
    // Keep DOM <select> in sync with the variable. populateModelSelect
    // rebuilds options with `selected` set on the matching one.
    populateModelSelect();
}

export function getSelectedModel() {
    return whisperModelSelect ? whisperModelSelect.value : selectedModel;
}

export function getModelStatus() {
    return modelStatus;
}

export function getActiveDownload() {
    return activeDownload;
}
