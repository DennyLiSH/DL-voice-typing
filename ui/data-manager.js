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
import { destroyPendingToast, showPendingToast } from './lib/pending-toast.js';
import {
    attachAudio,
    loadRecordings,
    releaseAudio as releaseAudioElement,
} from './lib/recordings.js';

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
    audioElement: null,
    // Rows whose audio failed to load (rendered as a persistent badge by
    // buildRecordingRow — the old DOM-patch badge was destroyed by the
    // renderDataList rebuild before it could ever paint).
    audioFailedFiles: new Set(),
    isLoading: false,
    lastReqId: 0,
    searchDebounceTimer: null,
};

// DOM refs (resolved lazily because the script may run before #page-data exists
// in some test environments).
function $data(id) {
    return document.getElementById(id);
}

/**
 * Release the current audio player: element-level cleanup (pause, revoke
 * blob URL, clear src) plus player state reset.
 *
 * Centralizes the cleanup that was previously duplicated at 5 sites
 * (toggle collapse, toggle switch, onAudioError, single-delete, batch-delete).
 */
function releaseAudio() {
    releaseAudioElement(dataState.audioElement);
    dataState.audioElement = null;
    dataState.audioPlayerRowId = null;
}

export function onDataPageEnter() {
    // Reset all state on entry (constraint #1 from design review).
    resetDataListState();
    loadRecordingsPage(0);
}

export function onDataPageLeave() {
    // Full release: pause + revoke blob URL + clear state (constraint #2),
    // plus drop any in-flight undo toast (window close / navigation —
    // backend timer keeps running and finalizes naturally).
    releaseAudio();
    destroyPendingToast();
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
        dataState.audioFailedFiles.clear();
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
    // row is re-assembled below and reassigns dataState.audioElement).
    releaseAudioElement(dataState.audioElement);
    dataState.audioElement = null;
    list.innerHTML = '';
    for (const entry of dataState.items) {
        const isSelected = dataState.selectedFiles.has(entry.filename);
        const isExpanded = dataState.expandedRowId === entry.filename;
        const row = buildRecordingRow(entry, {
            selected: isSelected,
            expanded: isExpanded,
            audioFailed: dataState.audioFailedFiles.has(entry.filename),
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
            // The actual src will be set by attachAudioSrc() once the bytes arrive.
            playerWrap.appendChild(audio);
            list.appendChild(playerWrap);
            dataState.audioElement = audio;
            audio.addEventListener('error', () => onAudioError(entry.filename));
            audio.addEventListener('ended', () => {
                // Auto-cleanup is optional; keep player visible until user closes.
            });
            // Fetch bytes asynchronously.
            attachAudioSrc(audio, entry.filename);
        }
    }
    updateBatchBar();
}

async function attachAudioSrc(audioEl, filename) {
    try {
        const bytes = await call('read_recording_audio', { filename });
        // Identity guard: the row may have been re-rendered (element replaced
        // or detached) while the fetch was in flight. Attaching a blob URL to
        // an orphan element would leak it — nobody can reach it for revoke.
        if (dataState.audioElement !== audioEl || !audioEl.isConnected) return;
        attachAudio(audioEl, bytes);
        dataState.audioFailedFiles.delete(filename);
    } catch (_e) {
        // Mark this row's audio as failed.
        onAudioError(filename);
    }
}

function onAudioError(filename) {
    if (dataState.audioPlayerRowId !== filename) return;
    // Record the failure in render state (NOT a DOM patch — the list rebuild
    // below would destroy a patched badge before it could ever paint, which
    // is exactly the dead-code bug this replaces).
    dataState.audioFailedFiles.add(filename);
    releaseAudio();
    renderDataList();
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
        bar.hidden = false;
    }
}

function hideDataError() {
    const bar = $data('data-error-bar');
    if (bar) bar.hidden = true;
}

function cssEscape(s) {
    if (
        typeof window.CSS !== 'undefined' &&
        typeof window.CSS.escape === 'function'
    ) {
        return window.CSS.escape(s);
    }
    return String(s).replace(/["\\]/g, '\\$&');
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

        // Keyboard activation for row expand (mirrors the click path; the
        // native controls inside the row keep their own key handling).
        list.addEventListener('keydown', (e) => {
            if (e.key !== 'Enter' && e.key !== ' ') return;
            const row = e.target.closest('.data-row');
            if (!row || row !== e.target) return; // only the row itself
            e.preventDefault();
            const filename = row.dataset.filename;
            if (!filename) return;
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
        if (result.moved > 0) {
            showPendingToast({
                moved: result.moved,
                onUndo: () => undoDelete(result.id),
            });
        }
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
        if (result.moved > 0) {
            showPendingToast({
                moved: result.moved,
                onUndo: () => undoDelete(result.id),
            });
        }
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
    try {
        const restored = await call('restore_pending_delete', { id });
        showDataError(`已撤销删除，恢复 ${restored} 条。`);
    } catch (e) {
        const msg = typeof e === 'string' ? e : e?.message || '未知错误';
        showDataError(`撤销失败：${msg}`);
    }
    // Refresh regardless — failed restore leaves the files in pending,
    // successful restore brings them back; either way the list needs reload.
    try {
        await loadRecordingsPage(dataState.offset);
    } catch (_e) {
        // loadRecordingsPage already surfaces its own error-bar message;
        // swallowing here avoids a double error.
    }
}

// Wire events after DOM is ready (script runs at end of body, so DOM is ready).
wireDataListEvents();
