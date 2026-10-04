import { call, rawInvoke, reportError } from './lib/api.js';
import { confirmDialog } from './lib/confirm-dialog.js';
import { hideIfVisible } from './lib/first-run-banner.js';
import { notifyFormChange } from './lib/form-state.js';
import { modelActionView } from './lib/model-view.js';
import { MODELS } from './lib/transcribe-models.js';
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

// Single source of truth: MODELS in ui/lib/transcribe-models.js,
// mirrored with src-tauri/src/config/schema.rs BUILT_IN_MODELS — drift
// is pinned by __tests__/model-table-contract.test.js.

export function populateModelSelect(converge = true) {
    whisperModelSelect.innerHTML = '';

    // Built-in group
    const builtInGroup = document.createElement('optgroup');
    builtInGroup.label = '内置模型';
    for (const m of MODELS) {
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

    if (converge && whisperModelSelect.value !== selectedModel) {
        selectedModel = whisperModelSelect.value;
    }
}

export function updateModelAction() {
    const view = modelActionView(selectedModel, activeDownload, modelStatus);

    modelStatusText.style.display = view.statusVisible ? 'inline' : 'none';
    downloadProgress.style.display = view.progressVisible ? 'block' : 'none';

    btnDownloadModel.textContent = view.action === 'delete' ? '删除' : '下载';
    btnDownloadModel.className = view.actionDanger
        ? 'btn-primary btn-download-model btn-danger'
        : 'btn-primary btn-download-model';
    btnDownloadModel.style.display =
        view.action === 'none' ? 'none' : 'inline-block';
    btnDownloadModel.disabled = view.actionDisabled;

    whisperModelSelect.disabled = view.selectDisabled;
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
        badge.title = '';
        const mode = await call('get_compute_mode');
        if (mode === 'gpu') {
            badge.textContent = 'GPU 加速';
            badge.className = 'badge mode-badge gpu';
        } else if (mode === 'cpu') {
            badge.textContent = 'CPU 模式（未检测到 GPU）';
            badge.className = 'badge mode-badge cpu';
        } else {
            badge.textContent = '模型未加载';
            badge.className = 'badge mode-badge unloaded';
        }
    } catch (_e) {
        badge.textContent = '检测失败';
        badge.className = 'badge mode-badge unloaded';
        badge.title = '检测失败：切到其他设置页再切回本页可重试';
    }
}

// --- Download ---

btnDownloadModel.addEventListener('click', async () => {
    const isCustom = selectedModel.startsWith('custom:');
    if (isCustom) {
        const filename = selectedModel.replace(/^custom:/, '');
        const ok = await confirmDialog({
            title: '删除模型',
            message: `确认删除模型 ${filename}？此操作不可恢复。`,
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
        hideIfVisible();
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
    // Intermediate-state rebuild: refresh the DOM selected marker WITHOUT
    // the convergence guard — the custom group may not be loaded yet
    // (app-shell init calls this before setCustomModels). Callers own the
    // final populate.
    populateModelSelect(false);
}

// The module variable is the single source of truth; the DOM select is a
// projection (change events write back into it, populateModelSelect
// renders from it, and its convergence guard re-syncs both after every
// final-state rebuild). Never read the DOM here — a DOM-first read
// resurrects the dual-source divergence this refactor removed.
export function getSelectedModel() {
    return selectedModel;
}

export function getModelStatus() {
    return modelStatus;
}

export function getActiveDownload() {
    return activeDownload;
}
