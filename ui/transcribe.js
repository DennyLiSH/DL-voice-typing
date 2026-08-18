// ui/transcribe.js
//
// Transcribe window orchestration (record-only mode): recording list,
// on-demand transcription with progress/cancel, segment-level editing,
// click-to-seek playback highlight, and text injection.
//
// State cross-check rules (design review §E):
//   1. Switching recordings stops playback and clears segments/edits/inject
//      state; a deleted selection is cleared.
//   2. While transcribing, list switching and the transcribe button are
//      disabled (in-flight conflict guard).
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
import {
    filterRecordOnly,
    findActiveSegmentIndex,
    formatTimestamp,
    mergeSegmentTexts,
    shouldAutoScroll,
    statusBadge,
} from './lib/transcribe.js';

const { listen } = window.__TAURI__.event;

const state = {
    items: [],
    selected: null,
    segments: [],
    edits: new Map(),
    activeSegment: -1,
    transcribing: false,
    injecting: false,
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

function releaseAudio() {
    const audio = audioEl();
    if (audio) {
        try {
            audio.pause();
        } catch (_e) {
            /* jsdom / detached element */
        }
        if (audio.dataset.blobUrl) {
            URL.revokeObjectURL(audio.dataset.blobUrl);
            delete audio.dataset.blobUrl;
        }
        audio.removeAttribute('src');
    }
}

// --- List ------------------------------------------------------------------

async function loadList() {
    const errBar = $('list-error');
    try {
        const resp = await call('list_saved_recordings', {
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
        renderList();
    } catch (e) {
        if (errBar) {
            errBar.textContent =
                typeof e === 'string' ? e : e?.message || '加载失败';
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
    // Rule 2: in-flight transcription locks list switching.
    if (state.transcribing) row.classList.add('disabled');

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
        warn.textContent = '录音有洞';
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
    releaseAudio();
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
    // Rule 2: no switching while a transcription is in flight.
    if (state.transcribing) return;
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

    try {
        const [segs, bytes] = await Promise.all([
            call('get_recording_segments', { filename }),
            call('read_recording_audio', { filename }),
        ]);
        // Stale guard: user switched again while loading.
        if (state.selected !== filename) return;
        state.segments = Array.isArray(segs.segments) ? segs.segments : [];
        state.durationMs = segs.duration_ms || 0;
        state.status = segs.transcription_status || 'pending';
        state.droppedBlocks = segs.dropped_blocks || 0;
        attachAudio(bytes);
        renderDetail();
    } catch (e) {
        if (state.selected !== filename) return;
        showToast(
            typeof e === 'string' ? e : e?.message || '加载录音失败',
            true,
        );
    }
}

function attachAudio(bytes) {
    const audio = audioEl();
    if (!audio) return;
    const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    const blob = new Blob([u8], { type: 'audio/wav' });
    const url = URL.createObjectURL(blob);
    audio.dataset.blobUrl = url;
    audio.src = url;
}

function renderDetail() {
    renderBadges();
    renderSegments();
    updateMerged();
    updateActionButtons();
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
        warn.textContent = '录音有洞';
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
}

// --- Transcription ---------------------------------------------------------

function updateActionButtons() {
    const btnT = $('btn-transcribe');
    if (btnT) {
        btnT.disabled = state.transcribing || !state.selected;
        btnT.textContent = state.status === 'done' ? '重新转录' : '转录';
    }
    const btnCancel = $('btn-cancel');
    if (btnCancel) btnCancel.hidden = !state.transcribing;
    const progress = $('progress-wrap');
    if (progress) progress.hidden = !state.transcribing;
    updateInjectButton();
}

function updateInjectButton() {
    const btn = $('btn-inject');
    if (btn) {
        btn.disabled =
            state.injecting ||
            state.transcribing ||
            mergeSegmentTexts(state.segments, state.edits).trim() === '';
    }
    const spinner = $('inject-spinner');
    if (spinner) spinner.hidden = !state.injecting;
}

async function startTranscription() {
    if (state.transcribing || !state.selected) return;
    state.transcribing = true;
    updateActionButtons();
    renderList(); // lock list rows (rule 2)
    setProgress(0, 'whisper');
    try {
        await call('transcribe_recording', {
            filename: state.selected,
            use_llm: Boolean($('chk-llm')?.checked),
        });
        // Outcome arrives via transcription-* events.
    } catch (e) {
        state.transcribing = false;
        updateActionButtons();
        renderList();
        showToast(
            typeof e === 'string' ? e : e?.message || '转录启动失败',
            true,
        );
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
    if (fill) fill.style.width = `${Math.min(100, Math.max(0, percent))}%`;
    const label = $('progress-label');
    if (label) {
        label.textContent =
            stage === 'llm' ? 'LLM 纠错中…' : `转录中 ${percent}%`;
    }
}

function finishTranscriptionUI() {
    state.transcribing = false;
    updateActionButtons();
    renderList();
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
    if (state.injecting || state.transcribing || !state.selected) return;
    const text = mergeSegmentTexts(state.segments, state.edits);
    if (text.trim() === '') return;
    state.injecting = true;
    updateInjectButton();
    try {
        await call('inject_transcript_text', {
            filename: state.selected,
            text,
        });
        showToast('注入成功');
    } catch (e) {
        // Backend messages include the clipboard-fallback guidance.
        showToast(typeof e === 'string' ? e : e?.message || '注入失败', true);
    } finally {
        state.injecting = false;
        updateInjectButton();
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

    // Rule 8: window re-shown (hidden → focused) refreshes the list.
    window.addEventListener('focus', loadList);
}

wireEvents();
loadList();

// --- Backend events --------------------------------------------------------

listen('transcription-progress', (event) => {
    const { percent, stage } = event.payload || {};
    if (state.transcribing) setProgress(percent ?? 0, stage ?? 'whisper');
});

listen('transcription-done', (event) => {
    finishTranscriptionUI();
    showToast('转录完成');
    if (event.payload?.filename === state.selected) {
        reloadSelectedDetail();
    }
});

listen('transcription-cancelled', () => {
    finishTranscriptionUI();
    showToast('转录已取消');
});

listen('transcription-error', (event) => {
    finishTranscriptionUI();
    const msg = event.payload?.message;
    showToast(typeof msg === 'string' && msg ? msg : '转录失败', true);
});

// A new record-only recording landed while the window is open.
listen('record-only-finished', () => {
    loadList();
});
