import { call } from './lib/api.js';
import { confirmDialog } from './lib/confirm-dialog.js';
import {
    buildExpandedMetadata,
    buildRecordingRow,
    computeOffsetAfterDeletion,
    deleteConfirmMessage,
    formatBytes,
    getPageRange,
} from './lib/data-management.js';
import {
    bindFinalizeListener,
    destroyPendingToast,
    showPendingToast,
    unbindFinalizeListener,
} from './lib/pending-toast.js';
import { createAudioSlot, createRovingList } from './lib/recording-list.js';
import { loadRecordings } from './lib/recordings.js';

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
    // Rows whose audio failed to load live on audioSlot.failedFiles —
    // a render-state Set that survives renderDataList rebuilds (the old
    // DOM-patch badge was destroyed by the rebuild before it could ever
    // paint, d903b9e fix territory). DR-6: persistence across renders is
    // load-bearing — the badge replaces the play button on the next render.
    isLoading: false,
    lastReqId: 0,
    searchDebounceTimer: null,
};

// DOM refs (resolved lazily because the script may run before #page-data exists
// in some test environments).
function $data(id) {
    return document.getElementById(id);
}

// List / audio controllers — created lazily in onDataPageEnter and torn
// down in onDataPageLeave. `let` (not const) so the init-time assignment
// works; if init never ran, onDataPageLeave's optional-chaining skips the
// destroy() call.
let listNav = null;
let audioSlot = null;

/**
 * Release the current audio player: element-level cleanup (pause, revoke
 * blob URL, clear src) plus player state reset. Centralizes the cleanup
 * that was previously duplicated at 5 sites (toggle collapse, toggle
 * switch, onAudioError, single-delete, batch-delete). All 6 call sites
 * MUST go through this helper — direct audio element access would leak
 * blob URLs (审查 S5-3/S5-F3).
 */
function releaseAudio() {
    if (audioSlot) audioSlot.release();
    dataState.audioPlayerRowId = null;
}

/**
 * Audio error handler — shared between the DOM `error` event (decode
 * failure on an attached audio element) and the audioSlot's fetch-failure
 * onFail (network/disk failure). Both paths converge here:
 *   1. Guard: only the active player row can fail (decoded by both the
 *      element identity and the audioPlayerRowId match).
 *   2. Mark the filename in the slot's failedFiles Set so the next render
 *      paints the persistent badge (the old DOM-patch badge was destroyed
 *      by the rebuild before it could paint — d903b9e).
 *   3. Release + re-render. The releaseAudio() call is what terminates the
 *      fetch-failure loop (审查 S5-F1): without it, renderDataList would
 *      re-bind and the slot would re-fetch — a livelock + log flood.
 */
function onAudioError(filename) {
    if (dataState.audioPlayerRowId !== filename) return;
    if (audioSlot) audioSlot.failedFiles.add(filename);
    releaseAudio();
    renderDataList();
}

export function onDataPageEnter() {
    // Reset all state on entry (constraint #1 from design review).
    resetDataListState();
    // Idempotent: installs the pending-deletes-finalized listener at most
    // once. Pairs with the unbind in onDataPageLeave so we never pile up
    // duplicate listeners across page navigation.
    bindFinalizeListener();
    // Wire controllers after DOM is known to exist (the page's content
    // area is hidden when other sidebar pages are active, so the list
    // element might not be queryable yet). Done once per page entry.
    const list = $data('data-list');
    if (list && !listNav) {
        listNav = createRovingList({
            container: list,
            rowSelector: '.data-row',
            onActivate: (filename) => {
                dataState.expandedRowId =
                    dataState.expandedRowId === filename ? null : filename;
                renderDataList();
            },
        });
        audioSlot = createAudioSlot({
            fetchBytes: (filename) =>
                call('read_recording_audio', { filename }),
            // onFail (not raw renderDataList) — it releases the player row
            // via releaseAudio(), which is the loop terminator for a
            // persistently failing fetch: without clearing audioPlayerRowId,
            // renderDataList would rebuild the row, re-bind, re-fetch, and
            // log-spam forever (审查 S5-F1).
            onFail: (filename) => onAudioError(filename),
        });
    }
    loadRecordingsPage(0);
}

export function onDataPageLeave() {
    // Full release: pause + revoke blob URL + clear state (constraint #2),
    // plus drop any in-flight undo toast (window close / navigation —
    // backend timer keeps running and finalizes naturally).
    releaseAudio();
    destroyPendingToast();
    unbindFinalizeListener();
    if (listNav) {
        listNav.destroy();
        listNav = null;
    }
}

function resetDataListState() {
    dataState.offset = 0;
    dataState.query = '';
    dataState.total = 0;
    dataState.items = [];
    dataState.selectedFiles.clear();
    dataState.expandedRowId = null;
    releaseAudio();
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
        const resp = await loadRecordings({
            offset,
            limit: dataState.limit,
            query: dataState.query || null,
        });
        // Stale response guard — discard if a newer request superseded us.
        if (reqId !== dataState.lastReqId) return;
        dataState.offset = resp.offset;
        dataState.total = resp.total;
        dataState.items = resp.items;
        // Fresh data: reset per-page transient failure states (the refresh
        // button is the retry path for audio-load failures).
        if (audioSlot) audioSlot.failedFiles.clear();
        renderDataList();
        renderStats(resp.total, resp.total_bytes);
        renderPagination();
        renderEmptyState();
        hideDataError();
    } catch (e) {
        if (reqId !== dataState.lastReqId) return;
        showDataError(e?.message || '加载失败');
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
    // Release the old player element before it is discarded, so its blob URL
    // is revoked rather than leaked (element-level only — an active player
    // row is re-assembled below and reassigned via audioSlot.bind).
    if (audioSlot) audioSlot.release();
    list.innerHTML = '';
    for (const entry of dataState.items) {
        const isSelected = dataState.selectedFiles.has(entry.filename);
        const isExpanded = dataState.expandedRowId === entry.filename;
        const audioFailed = audioSlot
            ? audioSlot.failedFiles.has(entry.filename)
            : false;
        const row = buildRecordingRow(entry, {
            selected: isSelected,
            expanded: isExpanded,
            audioFailed,
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
            // No autoplay: sound starts only on an explicit play click —
            // matches the transcribe window's behavior (no surprise audio
            // when a row is expanded).
            audio.autoplay = false;
            playerWrap.appendChild(audio);
            list.appendChild(playerWrap);
            audio.addEventListener('error', () => onAudioError(entry.filename));
            audio.addEventListener('ended', () => {
                // Auto-cleanup is optional; keep player visible until user closes.
            });
            // Fetch bytes asynchronously through the shared slot (identity
            // guard + caller isStale predicate live in the controller).
            audioSlot.bind(audio, entry.filename, {
                isStale: () => dataState.audioPlayerRowId !== entry.filename,
            });
        }
    }
    // Re-apply roving tabindex against the freshly rendered rows.
    if (listNav) listNav.sync();
    updateBatchBar();
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
        bar.classList.remove('notice');
        bar.hidden = false;
    }
}

/** Neutral (non-error) feedback in the same bar slot — success/partial
 *  outcomes that still deserve an in-page message. */
function showDataNotice(msg) {
    const bar = $data('data-error-bar');
    if (bar) {
        bar.textContent = msg;
        bar.classList.add('notice');
        bar.hidden = false;
    }
}

function hideDataError() {
    const bar = $data('data-error-bar');
    if (bar) {
        bar.classList.remove('notice');
        bar.hidden = true;
    }
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
                    releaseAudio();
                } else {
                    releaseAudio();
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

/** Shared soft-delete feedback: show the undo toast when anything moved. */
function showUndoToast(result) {
    if (result.moved > 0) {
        showPendingToast({
            moved: result.moved,
            id: result.id,
            undoSecs: result.undo_window_secs,
            onUndo: () => undoDelete(result.id),
        });
    }
}

async function handleSingleDelete(filename) {
    const ok = await confirmDialog({
        title: '删除录音',
        message: deleteConfirmMessage(1),
        danger: true,
    });
    if (!ok) return;
    try {
        // Soft-delete → 5-second undo window. The pending-toast replaces the
        // old hard-delete (which had no recourse once confirmed).
        const result = await call('soft_delete_recordings', {
            filenames: [filename],
        });
        showUndoToast(result);
        // Clear audio state if it was this row (constraint #10a).
        if (dataState.audioPlayerRowId === filename) {
            releaseAudio();
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
        if (result.failed && result.failed.length > 0) {
            const failedList = result.failed
                .map((f) => `${f.filename}：${f.error}`)
                .join('\n');
            showDataError(`部分删除失败：\n${failedList}`);
        }
    } catch (e) {
        showDataError(
            `删除失败：${typeof e === 'string' ? e : e?.message || '未知错误'}`,
        );
    }
}

async function handleBatchDelete() {
    const count = dataState.selectedFiles.size;
    if (count === 0) return;
    const ok = await confirmDialog({
        title: '删除录音',
        message: deleteConfirmMessage(count),
        danger: true,
    });
    if (!ok) return;
    const filenames = Array.from(dataState.selectedFiles);
    try {
        const result = await call('soft_delete_recordings', { filenames });
        showUndoToast(result);
        dataState.selectedFiles.clear();
        // Clear audio if it was a selected row.
        if (
            dataState.audioPlayerRowId &&
            filenames.includes(dataState.audioPlayerRowId)
        ) {
            releaseAudio();
        }
        if (
            dataState.expandedRowId &&
            filenames.includes(dataState.expandedRowId)
        ) {
            dataState.expandedRowId = null;
        }
        // Compute new offset using fresh total (constraint #9 + F6).
        // `moved` is the count of stems that actually transitioned to
        // pending; `filenames.length` is the user's selection (used for
        // page math since the user no longer expects those rows to exist).
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
            showDataError(
                `已移动 ${result.moved} 条到待撤销，失败 ${result.failed.length} 条：\n${failedList}`,
            );
        }
    } catch (e) {
        showDataError(
            `批量删除失败：${typeof e === 'string' ? e : e?.message || '未知错误'}`,
        );
    }
}

/**
 * Undo handler wired into the pending-toast's 撤销 button. Always refreshes
 * the list afterwards (success or failure) so the user sees the final
 * state of the recordings.
 *
 * @param {number} id - the soft-delete batch id returned by the backend
 */
async function undoDelete(id) {
    let report = null;
    let failure = null;
    try {
        report = await call('restore_pending_delete', { id });
    } catch (e) {
        const msg = typeof e === 'string' ? e : e?.message || '未知错误';
        failure = `撤销失败：${msg}`;
    }
    // Refresh regardless — failed restore leaves the files in pending,
    // successful restore brings them back; either way the list needs reload.
    // NOTE: the status message renders AFTER the refresh, because a
    // successful loadRecordingsPage calls hideDataError() and would wipe
    // a message shown before it.
    try {
        await loadRecordingsPage(dataState.offset);
    } catch (_e) {
        // loadRecordingsPage already surfaces its own error-bar message;
        // swallowing here avoids a double error.
    }
    if (failure) {
        showDataError(failure);
    } else if (report && report.failed > 0) {
        // report = { restored, failed }: a partial restore must be visible —
        // failed pairs stay in pending until the next startup sweep.
        showDataError(
            `已撤销删除，恢复 ${report.restored} 条；${report.failed} 条恢复失败（已保留在待删除目录，重启后清理）。`,
        );
    } else {
        const n = report?.restored ?? 0;
        showDataNotice(`已撤销删除，恢复 ${n} 条。`);
    }
}

// Wire events after DOM is ready (script runs at end of body, so DOM is ready).
wireDataListEvents();
