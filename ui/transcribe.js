// ui/transcribe.js
//
// Transcribe window orchestration (record-only mode): recording list,
// on-demand transcription with progress/cancel, segment-level editing,
// click-to-seek playback highlight, and text injection.
//
// State cross-check rules (design review §E):
//   1. Switching recordings stops playback and clears segments/edits/inject
//      state; a deleted selection is cleared.
//   2. While any operation is in flight (loading/transcribing/injecting),
//      list switching and the transcribe/inject buttons are disabled
//      (unified phase guard; transcribing-only before 2026-08).
//   3. Only the timestamp badge seeks; clicking the text area just focuses.
//   4. Highlight auto-scroll is suppressed while a segment input is focused.
//   5. Segment edits never write back into segments; they only feed the
//      merged text (backend persists it as final_text on inject).
//   6. Enter inside a segment input never triggers injection.
//   7. Badges (pending/done/failed + dropped-blocks warning), progress bar
//      with stage label, active-segment highlight, inject spinner, toasts.
//   8. Window refocus refreshes the list; done recordings render stored
//      segments without re-transcribing.

import { call, reportError } from './lib/api.js';
import { attachAudio, loadRecordings, releaseAudio } from './lib/recordings.js';
import {
    filterRecordOnly,
    findActiveSegmentIndex,
    formatTimestamp,
    injectTargetLabel,
    llmConfigured,
    mergeSegmentTexts,
    PHASE,
    shouldAutoScroll,
    statusBadge,
    uiFlags,
} from './lib/transcribe.js';

const { listen } = window.__TAURI__.event;

const state = {
    items: [],
    selected: null,
    segments: [],
    edits: new Map(),
    activeSegment: -1,
    // In-flight operation phase (Axis A, mutually exclusive). Recording
    // status and edits stay orthogonal axes rendered via uiFlags.
    phase: PHASE.IDLE,
    // Ownership token for transcription-* events: set when WE start a
    // transcription, cleared when it ends. Events arriving with no local
    // in-flight stem (e.g. window re-created mid-transcription) are ignored
    // so stale events never reset buttons or pop misleading toasts.
    transcribingStem: null,
    durationMs: 0,
    status: null,
    droppedBlocks: 0,
};

const $ = (id) => document.getElementById(id);

// --- Toast -----------------------------------------------------------------

let toastTimer = null;

function showToast(msg, isError = false) {
    const toast = $('toast');
    if (!toast) return;
    toast.textContent = msg;
    toast.classList.toggle('error', isError);
    toast.hidden = false;
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
        toast.hidden = true;
    }, 4000);
}

// --- Audio -----------------------------------------------------------------

function audioEl() {
    return $('audio');
}

// --- List ------------------------------------------------------------------

async function loadList() {
    const errBar = $('list-error');
    try {
        const resp = await loadRecordings({
            offset: 0,
            limit: 200,
            query: null,
        });
        state.items = filterRecordOnly(resp.items);
        // Rule 1: selection deleted elsewhere → clear it.
        if (
            state.selected &&
            !state.items.some((it) => it.filename === state.selected)
        ) {
            clearDetail();
            state.selected = null;
        }
        if (errBar) errBar.hidden = true;
        // Cap hint is based on the PRE-filter length: the 200 limit is applied
        // server-side across all recordings (classic + record_only), so a full
        // page means record-only items may have been cut even if the filtered
        // list below shows fewer than 200.
        const cap = $('list-cap');
        if (cap) cap.hidden = resp.items.length < 200;
        renderList();
    } catch (e) {
        if (errBar) {
            errBar.textContent = e?.message || '加载失败';
            errBar.hidden = false;
        }
    }
}

function renderList() {
    const list = $('rec-list');
    if (!list) return;
    list.textContent = '';
    for (const item of state.items) {
        list.appendChild(buildListRow(item));
    }
    const empty = $('rec-empty');
    if (empty) empty.hidden = state.items.length > 0;
}

function buildListRow(item) {
    const row = document.createElement('div');
    row.className = 'rec-row';
    row.dataset.filename = item.filename;
    if (item.filename === state.selected) row.classList.add('selected');
    // Rule 2 + BC(C5)#1: any in-flight phase locks list switching (visual
    // symmetry with the selectRecording entry guard).
    if (state.phase !== PHASE.IDLE) row.classList.add('disabled');

    const ts = document.createElement('span');
    ts.className = 'rec-row-ts';
    ts.textContent = formatStem(item.filename);
    row.appendChild(ts);

    const meta = document.createElement('span');
    meta.className = 'rec-row-meta';

    const badge = statusBadge(item.transcription_status);
    const badgeEl = document.createElement('span');
    badgeEl.className = badge.className;
    badgeEl.textContent = badge.text;
    meta.appendChild(badgeEl);

    if (item.dropped_blocks > 0) {
        const warn = document.createElement('span');
        warn.className = 'badge badge-warning';
        warn.textContent = '音频不完整';
        meta.appendChild(warn);
    }

    row.appendChild(meta);
    return row;
}

function formatStem(stem) {
    if (typeof stem !== 'string' || stem.length !== 19) return stem || '';
    return `${stem.slice(0, 10)} ${stem.slice(11).replace(/-/g, ':')}`;
}

// --- Detail ----------------------------------------------------------------

function clearDetail() {
    releaseAudio(audioEl());
    state.segments = [];
    state.edits.clear();
    state.activeSegment = -1;
    state.durationMs = 0;
    state.status = null;
    state.droppedBlocks = 0;
    const detail = $('detail');
    if (detail) detail.hidden = true;
    const placeholder = $('detail-empty');
    if (placeholder) placeholder.hidden = false;
}

async function selectRecording(filename) {
    // Unified entry guard (BC(C5)#1): no switching while any op is in flight.
    if (state.phase !== PHASE.IDLE) return;
    if (state.selected === filename) return;
    // Rule 1: full state reset on switch.
    clearDetail();
    state.selected = filename;
    renderList();

    const detail = $('detail');
    const placeholder = $('detail-empty');
    if (placeholder) placeholder.hidden = true;
    if (detail) detail.hidden = false;

    const title = $('detail-title');
    if (title) title.textContent = formatStem(filename);

    setPhase(PHASE.LOADING);
    try {
        const [segs, bytes] = await Promise.all([
            call('get_recording_segments', { filename }),
            call('read_recording_audio', { filename }),
        ]);
        // Stale guard: selection cleared/changed while loading.
        if (state.selected !== filename) return;
        state.segments = Array.isArray(segs.segments) ? segs.segments : [];
        state.durationMs = segs.duration_ms || 0;
        state.status = segs.transcription_status || 'pending';
        state.droppedBlocks = segs.dropped_blocks || 0;
        attachAudio(audioEl(), bytes);
        renderDetail();
    } catch (e) {
        if (state.selected !== filename) return;
        showToast(e?.message || '加载录音失败', true);
    } finally {
        // DR-1.2: phase reset covers every exit (success / stale ×2 / toast)
        // — a leftover LOADING phase would lock list switching forever.
        setPhase(PHASE.IDLE);
    }
}

function renderDetail() {
    renderBadges();
    renderSegments();
    updateMerged();
    syncUI();
}

function renderBadges() {
    const wrap = $('detail-badges');
    if (!wrap) return;
    wrap.textContent = '';
    const badge = statusBadge(state.status);
    const el = document.createElement('span');
    el.className = badge.className;
    el.textContent = badge.text;
    wrap.appendChild(el);
    if (state.droppedBlocks > 0) {
        const warn = document.createElement('span');
        warn.className = 'badge badge-warning';
        warn.textContent = '音频不完整';
        wrap.appendChild(warn);
    }
}

function renderSegments() {
    const wrap = $('segments');
    if (!wrap) return;
    wrap.textContent = '';
    state.activeSegment = -1;
    for (let i = 0; i < state.segments.length; i++) {
        wrap.appendChild(buildSegmentRow(i, state.segments[i]));
    }
    const empty = $('segments-empty');
    if (empty) {
        // Only meaningful after a completed transcription returned nothing.
        empty.hidden = !(
            state.status === 'done' && state.segments.length === 0
        );
    }
}

function buildSegmentRow(index, seg) {
    const row = document.createElement('div');
    row.className = 'segment-row';
    row.dataset.index = String(index);

    // Rule 3: only the timestamp badge seeks.
    const ts = document.createElement('button');
    ts.type = 'button';
    ts.className = 'segment-ts';
    ts.textContent = formatTimestamp(seg.start_ms);
    ts.setAttribute('aria-label', `跳转到 ${formatTimestamp(seg.start_ms)}`);
    ts.addEventListener('click', () => seekTo(seg.start_ms));
    row.appendChild(ts);

    const input = document.createElement('input');
    input.type = 'text';
    input.className = 'segment-text';
    input.value = state.edits.has(index) ? state.edits.get(index) : seg.text;
    input.setAttribute('aria-label', `段落 ${index + 1}`);
    // Rule 5: edits feed the merged text only, never the segment.
    input.addEventListener('input', () => {
        state.edits.set(index, input.value);
        updateMerged();
    });
    row.appendChild(input);

    return row;
}

function seekTo(startMs) {
    const audio = audioEl();
    if (!audio) return;
    audio.currentTime = startMs / 1000;
    try {
        const p = audio.play();
        if (p && typeof p.catch === 'function') p.catch(() => {});
    } catch (_e) {
        /* jsdom: play() not implemented */
    }
}

function updateMerged() {
    const merged = $('merged');
    if (merged) merged.value = mergeSegmentTexts(state.segments, state.edits);
    // BC(C5)#2: inject disabled refreshes live while editing (e.g. clearing
    // all text grays the button immediately).
    syncUI();
}

// --- Phase / UI sync -------------------------------------------------------

// Table-driven UI sync — uiFlags() is the single authority for what each
// phase means visually. Cheap; safe to call on every keystroke.
function syncUI() {
    const flags = uiFlags({
        phase: state.phase,
        selected: state.selected,
        status: state.status,
        mergedEmpty:
            mergeSegmentTexts(state.segments, state.edits).trim() === '',
    });
    const btnT = $('btn-transcribe');
    if (btnT) {
        btnT.disabled = flags.transcribeDisabled;
        btnT.textContent = flags.transcribeLabel;
    }
    const btnCancel = $('btn-cancel');
    if (btnCancel) btnCancel.hidden = !flags.cancelVisible;
    const progress = $('progress-wrap');
    if (progress) progress.hidden = !flags.progressVisible;
    const btnInject = $('btn-inject');
    if (btnInject) btnInject.disabled = flags.injectDisabled;
    const spinner = $('inject-spinner');
    if (spinner) spinner.hidden = !flags.injectSpinnerVisible;
}

// Single transition point for the phase state machine. Phase changes are
// rare events, so re-rendering the list (row .disabled ↔ listLocked) here
// is acceptable and keeps the two in sync by construction.
function setPhase(phase) {
    state.phase = phase;
    syncUI();
    renderList();
}

// --- Transcription ---------------------------------------------------------

async function startTranscription() {
    if (state.phase !== PHASE.IDLE || !state.selected) return;
    state.transcribingStem = state.selected;
    setPhase(PHASE.TRANSCRIBING);
    setProgress(0, 'whisper');
    try {
        await call('transcribe_recording', {
            filename: state.selected,
            use_llm: Boolean($('chk-llm')?.checked),
        });
        // Outcome arrives via transcription-* events.
    } catch (e) {
        // Stem cleared at the same site as the phase reset (DR-1.2).
        state.transcribingStem = null;
        setPhase(PHASE.IDLE);
        showToast(e?.message || '转录启动失败', true);
    }
}

async function cancelTranscription() {
    try {
        await call('cancel_transcription');
    } catch (e) {
        showToast(typeof e === 'string' ? e : e?.message || '取消失败', true);
    }
}

function setProgress(percent, stage) {
    const fill = $('progress-fill');
    if (fill)
        fill.style.transform = `scaleX(${Math.min(100, Math.max(0, percent)) / 100})`;
    const label = $('progress-label');
    if (label) {
        label.textContent =
            stage === 'llm' ? 'LLM 纠错中…' : `转录中 ${percent}%`;
    }
}

function finishTranscriptionUI() {
    // Stem cleared at the same site as the phase reset (DR-1.2).
    state.transcribingStem = null;
    setPhase(PHASE.IDLE);
}

// Rule 8: after done, re-read stored segments (single source of truth).
async function reloadSelectedDetail() {
    if (!state.selected) return;
    try {
        const segs = await call('get_recording_segments', {
            filename: state.selected,
        });
        state.segments = Array.isArray(segs.segments) ? segs.segments : [];
        state.durationMs = segs.duration_ms || 0;
        state.status = segs.transcription_status || 'pending';
        state.droppedBlocks = segs.dropped_blocks || 0;
        state.edits.clear();
        renderDetail();
    } catch (e) {
        reportError(e, 'reload_segments');
    }
}

// --- Injection -------------------------------------------------------------

async function injectText() {
    if (state.phase !== PHASE.IDLE || !state.selected) return;
    const text = mergeSegmentTexts(state.segments, state.edits);
    if (text.trim() === '') return;
    setPhase(PHASE.INJECTING);
    try {
        await call('inject_transcript_text', {
            filename: state.selected,
            text,
        });
        showToast('注入成功');
    } catch (e) {
        // Backend messages include the clipboard-fallback guidance.
        showToast(e?.message || '注入失败', true);
    } finally {
        setPhase(PHASE.IDLE);
        refreshInjectTarget();
    }
}

// --- Playback highlight ----------------------------------------------------

function onTimeUpdate() {
    const audio = audioEl();
    if (!audio || state.segments.length === 0) return;
    const idx = findActiveSegmentIndex(
        state.segments,
        audio.currentTime * 1000,
    );
    if (idx === state.activeSegment) return;
    state.activeSegment = idx;
    const wrap = $('segments');
    if (!wrap) return;
    const rows = wrap.querySelectorAll('.segment-row');
    rows.forEach((row, i) => {
        row.classList.toggle('active', i === idx);
    });
    // Rule 4: never scroll while the user is editing a segment.
    if (idx >= 0 && shouldAutoScroll(document.activeElement)) {
        rows[idx]?.scrollIntoView?.({ block: 'nearest' });
    }
}

// --- Event wiring ----------------------------------------------------------

// --- Inject target indicator (D2-a/D2-b) -----------------------------------

function renderInjectTarget(title) {
    const el = $('inject-target');
    if (!el) return;
    const { text, muted } = injectTargetLabel(title);
    el.textContent = text;
    el.classList.toggle('muted', muted);
    el.hidden = false;
}

async function refreshInjectTarget() {
    let title = null;
    try {
        title = await call('get_inject_target');
    } catch (_e) {
        // Command failed (transient) — keep the previously rendered state
        // instead of falsely showing the "no target" hint (reportError
        // already ran inside call()).
        return;
    }
    renderInjectTarget(typeof title === 'string' ? title : null);
}

// --- LLM checkbox availability (D3-b/D3-c) ----------------------------------

async function refreshLlmAvailability() {
    const chk = $('chk-llm');
    if (!chk) return;
    let configured = false;
    try {
        configured = llmConfigured(await call('get_config'));
    } catch (_e) {
        // Keep the previous state; call() already reported the error.
        return;
    }
    if (!configured) {
        chk.checked = false;
    }
    chk.disabled = !configured;
    chk.title = configured ? '' : '未配置 LLM（设置 → LLM 纠错）';
}

function wireEvents() {
    $('btn-refresh')?.addEventListener('click', loadList);
    $('btn-transcribe')?.addEventListener('click', startTranscription);
    $('btn-cancel')?.addEventListener('click', cancelTranscription);
    $('btn-inject')?.addEventListener('click', injectText);

    const list = $('rec-list');
    if (list) {
        list.addEventListener('click', (e) => {
            const row = e.target.closest('.rec-row');
            if (!row || row.classList.contains('disabled')) return;
            const filename = row.dataset.filename;
            if (filename) selectRecording(filename);
        });
    }

    // Rule 6: Enter inside a segment input never injects.
    $('segments')?.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' && e.target.closest?.('.segment-text')) {
            e.preventDefault();
        }
    });

    const audio = audioEl();
    if (audio) {
        audio.addEventListener('timeupdate', onTimeUpdate);
        audio.addEventListener('error', () => {
            const badge = $('audio-error');
            if (badge) badge.hidden = false;
        });
        audio.addEventListener('loadedmetadata', () => {
            const badge = $('audio-error');
            if (badge) badge.hidden = true;
        });
    }

    // Rule 8 + D2-a: window re-shown refreshes the list and inject target
    // (the target HWND is a snapshot; a hide/show cycle may have re-captured it).
    // D3-c: LLM availability too (the user may have just configured LLM in
    // the settings window and come back).
    window.addEventListener('focus', () => {
        loadList();
        refreshInjectTarget();
        refreshLlmAvailability();
    });
}

wireEvents();
loadList();
refreshInjectTarget();
refreshLlmAvailability();

// --- Backend events --------------------------------------------------------
//
// Ownership guard: transcription-* events are app-global. If this window did
// not start the in-flight transcription (transcribingStem === null — e.g. the
// window was re-created while a previous session's task is still running),
// every handler ignores the event instead of resetting UI it never set up or
// popping a misleading toast. error/progress payloads carry no filename
// (transcribe_cmd.rs), so the stem's presence is the ownership proof there.

listen('transcription-progress', (event) => {
    if (state.transcribingStem === null) return;
    const { percent, stage } = event.payload || {};
    setProgress(percent ?? 0, stage ?? 'whisper');
});

listen('transcription-done', (event) => {
    if (state.transcribingStem === null) return;
    if (event.payload?.filename !== state.transcribingStem) return;
    finishTranscriptionUI();
    showToast('转录完成');
    if (event.payload?.filename === state.selected) {
        reloadSelectedDetail();
    }
});

listen('transcription-cancelled', (event) => {
    if (state.transcribingStem === null) return;
    if (event.payload?.filename !== state.transcribingStem) return;
    finishTranscriptionUI();
    showToast('转录已取消');
});

listen('transcription-error', (event) => {
    if (state.transcribingStem === null) return;
    finishTranscriptionUI();
    const msg = event.payload?.message;
    showToast(typeof msg === 'string' && msg ? msg : '转录失败', true);
});

// A new record-only recording landed while the window is open.
listen('record-only-finished', () => {
    loadList();
});
