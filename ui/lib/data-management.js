/**
 * Pure helpers for the data management UI.
 *
 * These functions are extracted from settings.js so they can be unit-tested
 * in isolation via vitest (see __tests__/data-management.test.js).
 *
 * Convention: every function here is pure — same input → same output, no DOM
 * access, no side effects. The imperative glue lives in settings.js.
 */

import { statusBadge } from './transcribe.js';

/**
 * Format a byte count as a compact human-readable string.
 * Uses binary units (KB = 1024) and 1 decimal place above 1024.
 *
 * @param {number} bytes
 * @returns {string}
 */
export function formatBytes(bytes) {
    if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024)
        return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    if (bytes < 1024 * 1024 * 1024 * 1024)
        return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
    return `${(bytes / (1024 * 1024 * 1024 * 1024)).toFixed(1)} TB`;
}

/**
 * Format a duration in seconds. Output:
 *   - < 60s: "3.2s"
 *   - < 1h:  "1m 23s"
 *   - >= 1h: "1h 5m"
 *
 * @param {number} seconds
 * @returns {string}
 */
export function formatDuration(seconds) {
    if (!Number.isFinite(seconds) || seconds < 0) return '0s';
    if (seconds < 60)
        return `${seconds % 1 === 0 ? seconds : seconds.toFixed(1)}s`;
    const totalSecs = Math.floor(seconds);
    const h = Math.floor(totalSecs / 3600);
    const m = Math.floor((totalSecs % 3600) / 60);
    const s = totalSecs % 60;
    if (h > 0) return `${h}h ${m}m`;
    return `${m}m ${s}s`;
}

/**
 * Truncate `text` to `maxLen` characters, appending '…' when truncated.
 *
 * @param {string} text
 * @param {number} [maxLen=30]
 * @returns {string}
 */
export function truncateText(text, maxLen = 30) {
    if (typeof text !== 'string') return '';
    if (text.length <= maxLen) return text;
    return `${text.slice(0, maxLen)}…`;
}

/**
 * Build a DOM element for one recording row.
 *
 * Returns the populated `.data-row` element. The caller attaches it to the
 * list container. The row contains:
 *   - checkbox (always focusable, tabindex=0)
 *   - timestamp span
 *   - language badge
 *   - duration span
 *   - transcription preview (truncated)
 *   - play button (hidden if wav_size === 0, replaced with "音频缺失" badge)
 *   - delete button
 *
 * Per design-review constraint F1, this uses DOM API (createElement + textContent)
 * rather than innerHTML — never concatenate untrusted text into HTML.
 *
 * @param {Object} entry - recording entry from list_saved_recordings
 * @param {Object} [opts]
 * @param {boolean} [opts.selected=false]
 * @param {boolean} [opts.expanded=false]
 * @returns {HTMLElement}
 */
export function buildRecordingRow(entry, opts = {}) {
    const row = document.createElement('div');
    row.className = 'data-row';
    row.dataset.filename = entry.filename;
    if (opts.selected) row.classList.add('selected');
    if (opts.expanded) row.classList.add('expanded');

    // Checkbox (focusable)
    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.className = 'data-row-cb';
    cb.checked = Boolean(opts.selected);
    cb.setAttribute('aria-label', `选择 ${entry.filename}`);
    row.appendChild(cb);

    // Timestamp (formatted for display: 2026-06-24 14:30:25)
    const tsSpan = document.createElement('span');
    tsSpan.className = 'data-row-ts';
    tsSpan.textContent = formatStemForDisplay(entry.filename);
    row.appendChild(tsSpan);

    // Language badge
    const langSpan = document.createElement('span');
    langSpan.className = 'data-row-lang';
    langSpan.textContent = entry.language || '—';
    row.appendChild(langSpan);

    // Source badge (classic pipeline vs record-only mode)
    const srcSpan = document.createElement('span');
    if (entry.source === 'record_only') {
        srcSpan.className = 'badge badge-source-record';
        srcSpan.textContent = '录音';
    } else {
        srcSpan.className = 'badge badge-source-classic';
        srcSpan.textContent = '经典';
    }
    row.appendChild(srcSpan);

    // Transcription status + dropped-blocks badges (record-only only)
    if (entry.source === 'record_only') {
        const st = statusBadge(entry.transcription_status);
        const stSpan = document.createElement('span');
        stSpan.className = st.className;
        stSpan.textContent = st.text;
        row.appendChild(stSpan);

        if (entry.dropped_blocks > 0) {
            const warnSpan = document.createElement('span');
            warnSpan.className = 'badge badge-warning';
            warnSpan.textContent = '录音有洞';
            row.appendChild(warnSpan);
        }
    }

    // Duration
    const durSpan = document.createElement('span');
    durSpan.className = 'data-row-dur';
    durSpan.textContent =
        entry.duration_seconds != null
            ? formatDuration(entry.duration_seconds)
            : '—';
    row.appendChild(durSpan);

    // Transcription preview
    const previewSpan = document.createElement('span');
    previewSpan.className = 'data-row-preview';
    previewSpan.textContent = truncateText(
        entry.transcription || entry.final_text || entry.llm_corrected || '',
        30,
    );
    row.appendChild(previewSpan);

    // Play button OR "音频缺失" badge
    if (entry.wav_size > 0) {
        const playBtn = document.createElement('button');
        playBtn.type = 'button';
        playBtn.className = 'btn-icon btn-play';
        playBtn.textContent = '▶';
        playBtn.setAttribute('aria-label', `播放 ${entry.filename}`);
        row.appendChild(playBtn);
    } else {
        const badge = document.createElement('span');
        badge.className = 'audio-missing-badge';
        badge.textContent = '音频缺失';
        row.appendChild(badge);
    }

    // Delete button (always present)
    const delBtn = document.createElement('button');
    delBtn.type = 'button';
    delBtn.className = 'btn-icon btn-delete';
    delBtn.textContent = '🗑';
    delBtn.setAttribute('aria-label', `删除 ${entry.filename}`);
    row.appendChild(delBtn);

    return row;
}

/**
 * Build the expanded metadata section (transcription / LLM / final text).
 *
 * @param {Object} entry
 * @returns {HTMLElement}
 */
export function buildExpandedMetadata(entry) {
    const container = document.createElement('div');
    container.className = 'data-row-expanded';

    const fields = [
        ['转录', entry.transcription],
        ['LLM', entry.llm_corrected],
        ['最终', entry.final_text],
    ];
    for (const [label, text] of fields) {
        const line = document.createElement('div');
        line.className = 'data-row-meta-line';
        const lbl = document.createElement('span');
        lbl.className = 'data-row-meta-label';
        lbl.textContent = `${label}：`;
        const val = document.createElement('span');
        val.className = 'data-row-meta-value';
        val.textContent = text || '（无）';
        line.appendChild(lbl);
        line.appendChild(val);
        container.appendChild(line);
    }
    return container;
}

/**
 * Format a filename stem "2026-06-24_14-30-25" as "2026-06-24 14:30:25".
 *
 * @param {string} stem
 * @returns {string}
 */
export function formatStemForDisplay(stem) {
    if (typeof stem !== 'string' || stem.length !== 19) return stem || '';
    return `${stem.slice(0, 10)} ${stem.slice(11).replace(/-/g, ':')}`;
}

/**
 * Pagination math.
 *
 * @param {number} total - total matching items
 * @param {number} offset - current offset (0-based)
 * @param {number} limit - page size
 * @returns {{currentPage: number, totalPages: number, hasNext: boolean, hasPrev: boolean}}
 */
export function getPageRange(total, offset, limit) {
    const totalPages = total === 0 ? 1 : Math.ceil(total / limit);
    const currentPage = total === 0 ? 1 : Math.floor(offset / limit) + 1;
    return {
        currentPage,
        totalPages,
        hasNext: currentPage < totalPages,
        hasPrev: currentPage > 1,
    };
}

/**
 * Compute the offset to use after a deletion, when the current page becomes
 * empty (i.e., the user deleted the last item(s) on the last page).
 *
 * Returns the new offset to load. Stays on current page if it still has items,
 * otherwise jumps back to the previous valid page.
 *
 * @param {number} oldTotal - total count BEFORE deletion
 * @param {number} deletedCount - how many items were deleted
 * @param {number} limit - page size
 * @param {number} currentOffset - current offset (0-based)
 * @returns {number} - new offset to load
 */
export function computeOffsetAfterDeletion(
    oldTotal,
    deletedCount,
    limit,
    currentOffset,
) {
    const newTotal = Math.max(0, oldTotal - deletedCount);
    // Last valid offset (start of the last page that has items).
    const lastValidOffset =
        newTotal === 0 ? 0 : Math.floor((newTotal - 1) / limit) * limit;
    // If current offset is past the new last page, jump back.
    return Math.min(currentOffset, lastValidOffset);
}

/**
 * Confirmation message for delete operations. Per design-review F5, the text
 * explicitly says "永久删除" and "不可恢复" / "不会进入回收站".
 *
 * @param {number} count - how many recordings will be deleted
 * @returns {string}
 */
export function deleteConfirmMessage(count) {
    if (count === 1) {
        return '确定永久删除这条录音？此操作不可恢复（不会进入回收站）。';
    }
    return `确定永久删除选中的 ${count} 条录音？此操作不可恢复（不会进入回收站）。`;
}
