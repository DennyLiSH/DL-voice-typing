import { onDataPageEnter, onDataPageLeave } from './data-manager.js';
import { call } from './lib/api.js';
import { confirmDialog, isDialogOpen } from './lib/confirm-dialog.js';
import { isFormDirty } from './lib/form-state.js';
import { populateMainKeySelects } from './lib/hotkeys.js';
import { showError } from './lib/ui-utils.js';
import {
    loadComputeMode,
    populateModelSelect,
    setCustomModels,
    setModelStatus,
    setSelectedModel,
    updateModelAction,
} from './model-manager.js';
import {
    populateFields,
    setDirtyCheckEnabled,
    updateDirtyState,
} from './settings-form.js';

const { listen } = window.__TAURI__.event;

// DOM elements
const sidebarItems = document.querySelectorAll('.sidebar-item');
const pageContents = document.querySelectorAll('.page-content');

// State
let currentPage = 'general';

// --- Sidebar Navigation ---

export async function switchPage(pageName) {
    // Modal short-circuit: while a dialog is open, further navigation
    // requests are dropped — the queued question's premise (dirty state)
    // may already have been changed by the pending confirm.
    if (isDialogOpen()) return;
    // Dirty state check: warn user about unsaved changes before switching.
    // Must run before the data-leave hook so cancel also skips side effects
    // (semantics: not leaving = not cleaning). isFormDirty is imported above.
    if (currentPage !== pageName && isFormDirty()) {
        const ok = await confirmDialog({
            title: '未保存的更改',
            message: '有未保存的更改，确定要离开此页吗？',
        });
        if (!ok) {
            return; // user cancelled — stay on current page
        }
    }
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
    } else if (pageName === 'help') {
        refreshErrorHistory();
    }
}

// --- Recent Errors (help page) ---

const CHANNEL_LABELS = {
    'speech-error': '语音识别',
    'llm-error': 'LLM 纠错',
    'injection-error': '文本粘贴',
    'record-only-error': '录音模式',
    'transcription-error': '转录',
    'hotkey-error': '热键注册',
};

// Generation guard: a stale response from a previous help-page visit must
// not overwrite a newer render (help → away → help rapid switching).
let errorHistorySeq = 0;

async function refreshErrorHistory() {
    const list = document.getElementById('error-history-list');
    if (!list) return;
    const seq = ++errorHistorySeq;
    let records;
    try {
        records = await call('get_last_errors', { n: 3 });
    } catch (_e) {
        if (seq === errorHistorySeq) {
            renderErrorHistoryHint(list, '无法加载错误记录');
        }
        return;
    }
    if (seq !== errorHistorySeq) return;
    if (!Array.isArray(records) || records.length === 0) {
        renderErrorHistoryHint(list, '暂无错误记录');
        return;
    }
    list.textContent = '';
    for (const record of records) {
        const item = document.createElement('div');
        item.className = 'error-history-item';

        const time = document.createElement('div');
        time.className = 'error-history-time';
        time.textContent = record.timestamp;
        item.appendChild(time);

        const head = document.createElement('div');
        head.className = 'error-history-head';
        const badge = document.createElement('span');
        badge.className = 'badge badge-warning';
        badge.textContent = CHANNEL_LABELS[record.event] || record.event;
        head.appendChild(badge);
        item.appendChild(head);

        const message = document.createElement('div');
        message.className = 'error-history-message';
        message.textContent = record.message || '（无详情）';
        item.appendChild(message);

        list.appendChild(item);
    }
}

function renderErrorHistoryHint(list, text) {
    list.textContent = '';
    const hint = document.createElement('div');
    hint.className = 'hint';
    hint.textContent = text;
    list.appendChild(hint);
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

// --- Window Close ---

// Closing the settings window hides it to tray (lib.rs:on_window_event
// prevents CloseRequested and calls window.hide). The app keeps running,
// so an in-flight model download must NOT be cancelled here — that would
// destroy progress just because the user clicked the X. Audio playback,
// however, should stop: a hidden window playing audio is wasted resources
// and contradicts switchPage's onDataPageLeave contract.
// NOTE: beforeunload's browser-native close confirmation is a platform
// limitation — it cannot be replaced by the in-app confirm dialog (the
// dialog dies with the page before the user could answer it).
window.addEventListener('beforeunload', (e) => {
    if (currentPage === 'data') {
        onDataPageLeave();
    }
    if (isFormDirty()) {
        e.preventDefault();
        e.returnValue = '';
    }
});

// --- Shared Tauri Event Listeners ---

listen('hotkey-error', (event) => {
    showError(event.payload);
});

// --- Initialization ---

export async function init() {
    try {
        const [config, modelsData] = await Promise.all([
            call('get_config'),
            call('get_whisper_models'),
        ]);
        // Fill the two hotkey <select> elements with one <option> per
        // MAIN_KEYS entry. Must run BEFORE populateFields so the
        // writeSpecToUI call inside populateFields can locate the right
        // <option> for the loaded config's vk.
        populateMainKeySelects();
        setDirtyCheckEnabled(false);
        setSelectedModel(config.whisper_model);
        setModelStatus(modelsData.built_in);
        setCustomModels(modelsData.custom);
        populateModelSelect();
        updateModelAction();
        loadComputeMode();
        populateFields(config);
        updateDirtyState();
        setDirtyCheckEnabled(true);
        updateDirtyState();

        // Display version
        try {
            const version = await window.__TAURI__.app.getVersion();
            document.getElementById('version-display').textContent =
                `v${version}`;
        } catch (_e) {
            // Version display is non-critical, silently ignore
        }
    } catch (_e) {
        showError('加载配置失败，请重试');
    }
}
