import { call, reportError } from './lib/api.js';
import { createApiKeyChannel } from './lib/api-key-channel.js';
import { CLEAR_MARKER, MASKED_MARKER } from './lib/api-key-mask.js';
import { confirmDialog } from './lib/confirm-dialog.js';
import { errorMessage } from './lib/errors.js';
import { isFormDirty, onFormChange, setFormDirty } from './lib/form-state.js';
import {
    DEFAULT_PRIMARY_SPEC,
    DEFAULT_RECORD_ONLY_SPEC,
    hotkeyPreviewLabel,
    optionalSpecFromUI,
    populateMainKeySelects,
    sameSpec,
    specFromUI,
    writeOptionalSpecToUI,
    writeSpecToUI,
} from './lib/hotkeys.js';
import {
    HOTKEY_SLOTS,
    hotkeySpecsOf,
    SETTINGS_FIELDS,
} from './lib/settings-schema.js';
import {
    apiUrlWarningText,
    crossSlotConflicts,
    hotkeyConflictWarning,
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
const resetDefaultsBtn = document.getElementById('reset-defaults-btn');
// The API-key reset channel owns the wipe intent (pending flag +
// snapshot channel + sentinel discrimination) — see lib/api-key-channel.js.
const apiKeyChannel = createApiKeyChannel();
// M6-a: populate the four main-key <select> elements (hotkey,
// record-only-hotkey, open-settings-hotkey, open-transcribe-hotkey) as
// part of module init — production wires this in app-shell.js; the test
// loadFresh path imports settings-form.js directly and never calls
// app-shell.js, so calling it here keeps the two paths consistent.
populateMainKeySelects(HOTKEY_SLOTS);
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
// which maps to the EYE (reveal) affordance — the SVG is now inlined in
// settings.html so first paint matches without a JS replacement pass.

// State
let loadedConfig = null;
let dirtyCheckEnabled = false;
// The autostart state read back from the OS after a save — a
// different source than the toggle's DOM state (the third channel).
let savedAutostartState = false;

// Subscribe to form change events (model selection etc.) to recompute dirty state.
// Returns unsubscribe; we don't unsubscribe for the lifetime of the settings window.
onFormChange(updateDirtyState);

// --- Initialization ---

/**
 * Factory for the four hotkey-slot DOM_DEFS entries. Centralises the
 * `required: writeSpecToUI(prefix, v ?? fallback)` vs
 * `optional: writeOptionalSpecToUI(prefix, v)` split, plus the matching
 * get branch (optionalSpecFromUI returns null when the slot is
 * disabled, mirroring the backend Option<HotkeySpec>).
 *
 * `fallback` is the default spec used when the backend supplied null
 * (e.g. fresh install where the field has no server value yet). Only
 * required slots take a fallback — optional slots stay null and the UI
 * shows the "未启用" preview.
 */
const hotkeyField = ({ prefix, optional, fallback }) => ({
    get: () => (optional ? optionalSpecFromUI(prefix) : specFromUI(prefix)),
    set: (v) => {
        if (optional) writeOptionalSpecToUI(prefix, v);
        else writeSpecToUI(prefix, v ?? fallback);
        updateHotkeyPreview(prefix);
    },
});

// --- Toggles -----------------------------------------------------------
//
// The handles are created here (DOM_DEFS below needs their get/set) and
// the sections keep their original order.
// --- LLM Toggle ---

function updateLlmFieldsState(enabled) {
    llmFields.classList.toggle('disabled', !enabled);
    for (const input of llmFields.querySelectorAll('input')) {
        input.disabled = !enabled;
    }
    testBtn.disabled = !enabled;
}

const llmToggleState = bindToggle(llmToggle, {
    onToggle: updateLlmFieldsState,
});

// --- Data Saving Toggle ---

function updateDataSavingFieldsState(enabled) {
    dataSavingFields.classList.toggle('disabled', !enabled);
    for (const input of dataSavingFields.querySelectorAll('input')) {
        input.disabled = !enabled;
    }
    btnBrowsePath.disabled = !enabled;
}

const dataSavingToggleState = bindToggle(dataSavingToggle, {
    onToggle: updateDataSavingFieldsState,
});

// --- Review Before Paste Toggle ---

const reviewToggleState = bindToggle(reviewToggle);

// --- Autostart Toggle ---

const autostartToggleState = bindToggle(autostartToggle, {
    guard: () => autostartToggle.classList.contains('disabled'),
});

// --- Realtime Transcription Toggle ---

const realtimeToggleState = bindToggle(realtimeToggle);

// --- Record-Only Mode Toggle ---

function updateRecordOnlyHotkeyState(enabled) {
    recordOnlyHotkeyGroup.classList.toggle('disabled', !enabled);
    recordOnlyHotkeySelect.disabled = !enabled;
    document.getElementById('record-only-hotkey-ctrl').disabled = !enabled;
    document.getElementById('record-only-hotkey-shift').disabled = !enabled;
    document.getElementById('record-only-hotkey-alt').disabled = !enabled;
}

const recordOnlyToggleState = bindToggle(recordOnlyToggle, {
    onToggle: updateRecordOnlyHotkeyState,
});

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
    hotkey: hotkeyField({
        prefix: 'hotkey',
        optional: false,
        fallback: DEFAULT_PRIMARY_SPEC,
    }),
    whisper_model: {
        get: getSelectedModel,
        set: setSelectedModel,
    },
    llm_enabled: {
        get: () => llmToggleState.get(),
        set: (v) => {
            llmToggleState.set(v);
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
        // The sentinel discrimination and the wipe intent live in the
        // channel — this is the DOM source, the only place that can tell
        // "user typed" from "masked roundtrip echo".
        get: () => apiKeyChannel.getForSave(apiKeyInput.value, loadedConfig),
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
        get: () => dataSavingToggleState.get(),
        set: (v) => {
            dataSavingToggleState.set(v);
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
        get: () => reviewToggleState.get(),
        set: (v) => reviewToggleState.set(v),
    },
    realtime_transcription: {
        get: () => realtimeToggleState.get(),
        set: (v) => realtimeToggleState.set(v),
    },
    autostart: {
        get: () => autostartToggleState.get(),
        set: (v) => {
            // The third channel stays: `savedAutostartState` carries the
            // state READ BACK FROM THE OS after a save, which is a
            // different source than the toggle's DOM state — merging
            // them would couple the form to the OS.
            savedAutostartState = !!v;
            autostartToggleState.set(v);
        },
    },
    record_only_enabled: {
        get: () => recordOnlyToggleState.get(),
        set: (v) => {
            recordOnlyToggleState.set(v);
            updateRecordOnlyHotkeyState(!!v);
        },
    },
    record_only_hotkey: hotkeyField({
        prefix: 'record-only-hotkey',
        optional: false,
        fallback: DEFAULT_RECORD_ONLY_SPEC,
    }),
    open_settings_hotkey: hotkeyField({
        prefix: 'open-settings-hotkey',
        optional: true,
    }),
    open_transcribe_hotkey: hotkeyField({
        prefix: 'open-transcribe-hotkey',
        optional: true,
    }),
};

const FIELDS = SETTINGS_FIELDS.map(({ key }) => ({ key, ...DOM_DEFS[key] }));

export function populateFields(config) {
    loadedConfig = config;
    applyFieldsToForm(config);
    // Re-loading the real config invalidates any in-flight CLEAR intent.
    apiKeyChannel.disarm();
}

/**
 * Fill the form with `config` WITHOUT replacing the dirty baseline.
 *
 * The restore-defaults flow uses it: the baseline must stay the REAL
 * saved config, so the dirty check lights up for ANY divergence between
 * the defaults and what is saved (including no-key users with
 * non-default settings).
 */
export function populateFieldsKeepingBaseline(config) {
    applyFieldsToForm(config);
}

/**
 * Push every field value into the form and refresh the derived warnings.
 *
 * Optional slot spec passthrough relies on `config[key]` being exactly
 * what the backend sent (null for disabled, spec for enabled). The
 * dirty-check side (isConfigDirty) handles the undefined→null
 * normalisation for the optional slots when the user is on a pre-M6
 * install and just upgraded — no duplicate guard here.
 */
function applyFieldsToForm(config) {
    FIELDS.forEach(({ key, set }) => {
        set(config[key]);
    });
    updateApiUrlWarning();
    // Initial conflict-warning refresh on EVERY populate call (including
    // the restore-defaults path — DR-1.2: hooking only the page-load
    // entry would leave a stale conflict div up after reset, and a
    // legacy config that already carries a conflicting pair shows the
    // warning immediately on load).
    updateHotkeyWarnings();

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
 * Bind click + keydown handlers to a toggle button and return its
 * handle.
 *
 * A toggle's state lives in TWO DOM places — the `active` class and the
 * `aria-checked` attribute — and both used to be written by hand in two
 * places per toggle (the click handler and the populate path). The
 * handle's `set` is now the only writer and its `get` the only reader,
 * so the two can never drift.
 *
 * @param {HTMLElement} el - the toggle button element
 * @param {Object} [opts]
 * @param {(isActive: boolean) => void} [opts.onToggle] - called after toggle state changes (e.g., disable dependent inputs)
 * @param {() => boolean} [opts.guard] - returns true to skip the click (e.g., disabled check)
 * @returns {{get: () => boolean, set: (value: boolean) => void, el: HTMLElement}}
 */
function bindToggle(el, { onToggle, guard } = {}) {
    function get() {
        return el.classList.contains('active');
    }
    /** The only writer of the toggle's DOM state (class + aria). */
    function set(value) {
        const active = !!value;
        el.classList.toggle('active', active);
        el.setAttribute('aria-checked', String(active));
    }
    el.addEventListener('click', () => {
        if (guard?.()) return;
        const isActive = !get();
        set(isActive);
        if (onToggle) onToggle(isActive);
        updateDirtyState();
    });
    el.addEventListener('keydown', (e) => {
        if (e.key === ' ') {
            e.preventDefault();
            el.click();
        }
    });
    return { get, set, el };
}

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
    } catch (e) {
        // dialog.open is a plugin direct call bypassing the call() wrapper
        // — forward manually so the failure leaves a tracing audit trail.
        reportError(e, 'browse-data-path');
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
    testBtn.textContent = '测试中…';
    testStatus.textContent = '';

    try {
        await call('test_llm_connection', { apiUrl, apiKey, model });
        setTestStatus('✓ 连接成功', 'success');
    } catch (e) {
        setTestStatus(`✗ ${errorMessage(e, '连接失败，请检查配置')}`, 'error');
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
// The slot list is derived from SETTINGS_FIELDS (single source) — a
// fifth slot would otherwise be missing from these loops while the
// schema and the preview path already knew about it.
for (const { prefix } of HOTKEY_SLOTS) {
    // Optional-chaining matches populateMainKeySelects' missing-element
    // tolerance: module-init must not crash when a host DOM lacks the
    // open-* controls (test fixtures, embedded reuse).
    document
        .getElementById(prefix)
        ?.addEventListener('change', onHotkeyComboChange);
    for (const mod of ['ctrl', 'shift', 'alt']) {
        document
            .getElementById(`${prefix}-${mod}`)
            ?.addEventListener('change', onHotkeyComboChange);
    }
}

function onHotkeyComboChange() {
    updateDirtyState();
    for (const { prefix } of HOTKEY_SLOTS) {
        updateHotkeyPreview(prefix);
    }
    updateHotkeyWarnings();
}

/**
 * Cross-slot conflict warnings (M6-a): one dedicated conflict div per slot
 * (`#<prefix>-conflict-warning`), fully cleared and rewritten on every
 * call — this function is their ONLY writer, so the F1/F12 advisory
 * channel (`#<prefix>-warning`, written by updateHotkeyWarning) is never
 * touched and the two warning kinds coexist without swallowing each
 * other. Blame direction follows crossSlotConflicts' DR-2.3 order
 * invariant (never the primary slot).
 */
function updateHotkeyWarnings() {
    const config = getCurrentConfig();
    const conflicts = crossSlotConflicts(hotkeySpecsOf(config));
    // First conflict message per slot (a slot colliding with several
    // others shows its first pair — enough to direct the user).
    const hit = new Map();
    for (const c of conflicts) {
        if (!hit.has(c.slot)) hit.set(c.slot, c.message);
    }
    for (const { key, prefix } of HOTKEY_SLOTS) {
        const el = document.getElementById(`${prefix}-conflict-warning`);
        if (!el) continue;
        const message = hit.get(key);
        el.hidden = !message;
        el.textContent = message ?? '';
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
    // The optional flag is derived from HOTKEY_SLOTS (settings-schema.js)
    // rather than hard-coded id comparisons — adding a slot only needs
    // a SETTINGS_FIELDS entry, no change here.
    const slot = HOTKEY_SLOTS.find((s) => s.prefix === prefix);
    const optional = slot?.optional ?? false;
    const spec = specFromUI(prefix);
    const label = hotkeyPreviewLabel(spec, { optional });
    // '未选择主键' (required + empty main-key) is the lone label that
    // drops the '当前组合：' prefix — keeps the old form wording stable
    // so the byte-identical preview text contract holds.
    el.textContent = label === '未选择主键' ? label : `当前组合：${label}`;
    updateHotkeyWarning(prefix);
}

/**
 * F1/F12 conflict warning under the combo preview. Refreshed from the same
 * funnel as the preview label (init populateFields + the 8 change
 * handlers), so a config that loads F12 shows the warning immediately.
 */
function updateHotkeyWarning(prefix) {
    const warn = document.getElementById(`${prefix}-warning`);
    if (!warn) return;
    const warning = hotkeyConflictWarning(
        document.getElementById(prefix)?.value ?? '',
    );
    warn.hidden = !warning;
    warn.textContent = warning ?? '';
}

// Show the API-URL advisory: embedded credential param (recommend the
// dedicated key field), plus the plaintext-transport sentence when the URL
// is http:// to a non-loopback host. Intentionally stays visible while the
// LLM toggle is off: api_url is persisted to config.json regardless of
// llm_enabled, so the exposure does not depend on the toggle. The
// transport branch also fires when a dedicated key is set (typed or
// saved via MASKED_MARKER) — the key still travels the network in the
// `Authorization` header regardless of whether the URL itself carries
// credentials.
function updateApiUrlWarning() {
    const hasKey =
        apiKeyInput.value.trim() !== '' ||
        loadedConfig?.llm_api_key === MASKED_MARKER;
    const text = apiUrlWarningText(apiUrlInput.value.trim(), hasKey);
    apiUrlWarning.hidden = !text;
    apiUrlWarning.textContent = text ?? '';
}

apiUrlInput.addEventListener('input', updateApiUrlWarning);
// The API-key field gates the transport-only warning (dedicated key + http
// non-loopback URL with no embedded credential). Refresh on every keystroke
// so the warning appears / disappears as the user types or clears the key.
apiKeyInput.addEventListener('input', updateApiUrlWarning);

// --- Save ---

let saveInFlight = false;

export async function saveSettings() {
    // Shared re-entry guard for click + Ctrl+S. Note: we can't use
    // saveBtn.disabled here — it's also true when the form is clean (no
    // edits), which would block a redundant but valid save the test
    // suite relies on. Use a dedicated in-flight flag instead.
    if (saveInFlight) return;

    hideError();
    const config = getCurrentConfig();

    const validation = validateSettings(config, getModelStatus());
    if (!validation.valid) {
        showError(validation.error);
        return;
    }

    saveBtn.disabled = true;
    saveBtn.textContent = '保存中…';
    saveBtn.classList.add('saving');
    saveInFlight = true;

    const prevRecordOnly = loadedConfig?.record_only_hotkey;
    try {
        await call('save_settings', { config });
        // Baseline normalization (M3-a fix): the CLEAR sentinel must never
        // persist in the dirty baseline — maskedKeyEqual only treats the
        // MASKED marker as "unchanged", a '__CLEAR__' residue keeps the
        // form dirty forever. Normalize to '' (the post-save backend truth).
        loadedConfig = {
            ...config,
            llm_api_key:
                config.llm_api_key === CLEAR_MARKER ? '' : config.llm_api_key,
        };
        // Double-clear runs on the SUCCESS path only — clearing before the
        // call would silently drop the CLEAR intent on a failed save (the
        // retry would send MASKED = keep-key while the user meant to wipe).
        apiKeyChannel.disarm();

        // Sync autostart state with OS (skip in dev builds without DL_AUTOSTART=1).
        let saveMsg = '✓ 已保存';
        let saveMsgType = 'success';
        // M6-a: only the record-only slot still needs a restart — primary and
        // the optional open-* slots are live-re-registered on save. Old wording
        // ("新热键重启应用后生效") fired for any primary change, which was
        // stale after live re-registration landed.
        if (!sameSpec(config.record_only_hotkey, prevRecordOnly)) {
            saveMsg = '✓ 已保存（录音快捷键重启应用后生效）';
        }
        if (!(await syncAutostart())) {
            saveMsg = '⚠ 已保存，开机自启同步失败';
            saveMsgType = 'error';
        }
        updateDirtyState();
        revealSaveStatus(saveMsg, saveMsgType);
    } catch (e) {
        disarmClearStatusOnInteract();
        const msg = errorMessage(e, '保存失败，请重试');
        setSaveStatus(`✗ ${msg}`, 'error');
        showError(msg);
    } finally {
        saveBtn.textContent = '保存';
        saveBtn.classList.remove('saving');
        saveInFlight = false;
        saveBtn.disabled = !isFormDirty();
    }
}

saveBtn.addEventListener('click', saveSettings);

// Ctrl+S / Cmd+S saves (review.js precedent for keydown pattern).
// IME guard: don't intercept while a composition session is active
// (prevents stealing "select all" mid-pinyin entry).
document.addEventListener('keydown', (e) => {
    if (
        (e.ctrlKey || e.metaKey) &&
        e.key.toLowerCase() === 's' &&
        !e.isComposing
    ) {
        e.preventDefault();
        saveSettings();
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

// Sync the OS autostart state with the toggle. Returns false when the
// sync failed (caller downgrades the save message to the autostart
// warning). Dev builds without DL_AUTOSTART=1 skip via
// is_autostart_available → false. Two catch layers: the inner one owns
// the plugin direct calls (forwarded manually — they bypass the call()
// wrapper); the outer one only sees wrapper-reported probe failures.
async function syncAutostart() {
    try {
        const wantAutostart = autostartToggleState.get();
        const autostartAvailable = await call('is_autostart_available');
        if (autostartAvailable) {
            try {
                if (wantAutostart) {
                    await window.__TAURI__.autostart.enable();
                } else {
                    await window.__TAURI__.autostart.disable();
                }
            } catch (e) {
                // Plugin calls bypass the call() wrapper — forward
                // manually. The outer catch stays wrapper-only: probe
                // failures are already reported by call() with the
                // command-name context; reporting here too would
                // double-log them.
                reportError(e, 'sync-autostart');
                return false;
            }
        }
        savedAutostartState = wantAutostart;
        return true;
    } catch (_e) {
        return false;
    }
}

// Show the save message. Pure acknowledgements ("✓ 已保存") auto-clear
// after 1.5s; instructional messages (restart hint / autostart failure)
// survive until the user's next interaction instead of evaporating.
function revealSaveStatus(saveMsg, saveMsgType) {
    setSaveStatus(saveMsg, saveMsgType);
    if (saveMsg === '✓ 已保存') {
        // Pure acknowledgement — auto-clear after 1.5s.
        setTimeout(() => {
            saveStatus.textContent = '';
        }, 1500);
    } else {
        armClearStatusOnInteract();
    }
}

export function setDirtyCheckEnabled(enabled) {
    dirtyCheckEnabled = enabled;
}

export function getLoadedAutostart() {
    return savedAutostartState;
}

if (resetDefaultsBtn) {
    resetDefaultsBtn.addEventListener('click', async () => {
        const ok = await confirmDialog({
            title: '恢复全部默认设置？',
            message:
                '将清空 API Key、接口地址与数据保存路径，密钥需重新获取粘贴（不可找回）。恢复后不会自动保存，请核对后手动保存。',
            danger: true,
        });
        if (!ok) return;
        try {
            const defaults = await call('get_default_config', {});
            const hadExisting = loadedConfig?.llm_api_key === MASKED_MARKER;
            // The dirty baseline (loadedConfig) stays = the REAL saved
            // config, so dirty = "defaults vs saved" lights for ANY
            // divergence (incl. no-key users with non-default settings).
            populateFieldsKeepingBaseline({ ...defaults, llm_api_key: '' });
            apiKeyChannel.armReset(hadExisting);
            updateDirtyState();
            // All four hotkey previews are refreshed by
            // populateFieldsKeepingBaseline → applyFieldsToForm → the
            // hotkeyField `set` (which calls updateHotkeyPreview per slot).
            // An extra refresh here would only cover 2 of the 4 slots.
            setSaveStatus('已恢复默认值，请核对后保存', 'success');
            armClearStatusOnInteract();
        } catch (_e) {
            // get_default_config failures are already forwarded by the
            // call() wrapper (context = command name); this catch only
            // owns the UI fallback text — no manual reportError here.
            setSaveStatus('✗ 恢复默认失败，请重试', 'error');
        }
    });
}

// Any user edit (including type-then-delete-all) takes control away from
// the reset intent — otherwise silent fallback to "keep the old key" while
// the reset notice still promises a wipe.
if (apiKeyInput) {
    apiKeyInput.addEventListener('input', () => {
        // The user took over — the pending intent is cancelled, but the
        // snapshot channel stays as defense-in-depth.
        apiKeyChannel.cancelPending();
        if (saveStatus.textContent.includes('已恢复默认值')) {
            saveStatus.textContent = '';
        }
    });
}
