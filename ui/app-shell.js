import { onDataPageEnter, onDataPageLeave } from './data-manager.js';
import { call } from './lib/api.js';
import {
    loadComputeMode,
    populateModelSelect,
    setCustomModels,
    setModelStatus,
    setSelectedModel,
    updateModelAction,
} from './model-manager.js';
import {
    isDirtyState,
    populateFields,
    setDirtyCheckEnabled,
    updateDirtyState,
} from './settings-form.js';

const { listen } = window.__TAURI__.event;

// DOM elements
const sidebarItems = document.querySelectorAll('.sidebar-item');
const pageContents = document.querySelectorAll('.page-content');
const errorBanner = document.getElementById('error-banner');

// State
let currentPage = 'general';

// --- Sidebar Navigation ---

export function switchPage(pageName) {
    // Dirty state check: warn user about unsaved changes before switching.
    // Must run before the data-leave hook so cancel also skips side effects
    // (semantics: not leaving = not cleaning). isDirtyState is imported above.
    if (currentPage !== pageName && isDirtyState()) {
        if (!window.confirm('有未保存的更改，确定要离开此页吗？')) {
            return; // user cancelled or confirm disabled — stay on current page
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

// --- Error Banner ---

export function showError(msg) {
    errorBanner.textContent = msg;
    errorBanner.classList.add('visible');
}

export function hideError() {
    errorBanner.classList.remove('visible');
}

// --- Window Close ---

// Closing the settings window hides it to tray (lib.rs:on_window_event
// prevents CloseRequested and calls window.hide). The app keeps running,
// so an in-flight model download must NOT be cancelled here — that would
// destroy progress just because the user clicked the X. Audio playback,
// however, should stop: a hidden window playing audio is wasted resources
// and contradicts switchPage's onDataPageLeave contract.
window.addEventListener('beforeunload', (e) => {
    if (currentPage === 'data') {
        onDataPageLeave();
    }
    if (isDirtyState()) {
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
