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

// Icon constants (static SVG markup, no untrusted data is ever interpolated).
const PLAY_SVG =
    '<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M8 5v14l11-7z"/></svg>';
const TRASH_SVG =
    '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" aria-hidden="true"><path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2m3 0v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/></svg>';

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
 * Preview text for a collapsed recording row.
 *
 * Returns `{ text, placeholder }`: `placeholder` marks the text as a status
 * hint ("未转录" / "转录失败") that should render in tertiary color — used
 * when the row has no transcription text to show. Status badges moved to the
 * expanded section (560px row budget), so this is the only collapsed-state
 * status cue for record-only rows.
 *
 * done + all-empty is deliberately NOT a placeholder (rare edge, no hint is
 * less misleading than a wrong one); classic rows have no status concept.
 *
 * @param {Object} entry - recording entry from list_saved_recordings
 * @returns {{text: string, placeholder: boolean}}
 */
export function previewText(entry) {
    const text =
        entry.transcription || entry.final_text || entry.llm_corrected || '';
    if (text) return { text: truncateText(text, 30), placeholder: false };
    if (entry.source === 'record_only') {
        if (entry.transcription_status === 'failed')
            return { text: '转录失败', placeholder: true };
        if (entry.transcription_status === 'pending')
            return { text: '未转录', placeholder: true };
    }
    return { text: '', placeholder: false };
}

/**
 * Build a DOM element for one recording row.
 *
 * Returns the populated `.data-row` element. The caller attaches it to the
 * list container. The row contains:
 *   - checkbox (always focusable, tabindex=0)
 *   - timestamp span
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
    // Keyboard-reachable expand affordance: the row itself acts as a button
    // (tabindex stop; Enter/Space toggle handled by data-manager's delegated
    // keydown). The chevron ::after (settings.css) is the visual cue.
    row.setAttribute('role', 'listitem');
    row.tabIndex = 0;
    row.setAttribute('aria-expanded', String(Boolean(opts.expanded)));
    row.dataset.filename = entry.filename;
    if (opts.selected) row.classList.add('selected');
    if (opts.expanded) row.classList.add('expanded');

    // Checkbox (focusable)
    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.className = 'data-row-cb';
    cb.checked = Boolean(opts.selected);
    cb.setAttribute(
        'aria-label',
        `选择录音 ${formatStemForDisplay(entry.filename)}`,
    );
    row.appendChild(cb);

    // Timestamp (formatted for display: 2026-06-24 14:30:25)
    const tsSpan = document.createElement('span');
    tsSpan.className = 'data-row-ts';
    tsSpan.textContent = formatStemForDisplay(entry.filename);
    row.appendChild(tsSpan);

    // Source badge (classic pipeline vs record-only mode)
    const srcSpan = document.createElement('span');
    if (entry.source === 'record_only') {
        srcSpan.className = 'badge badge-source-record';
        srcSpan.textContent = '录音';
    } else {
        srcSpan.className = 'badge badge-source-classic';
        srcSpan.textContent = '语音输入';
    }
    row.appendChild(srcSpan);

    // Duration
    const durSpan = document.createElement('span');
    durSpan.className = 'data-row-dur';
    durSpan.textContent =
        entry.duration_seconds != null
            ? formatDuration(entry.duration_seconds)
            : '—';
    row.appendChild(durSpan);

    // Transcription preview (placeholder text when record-only rows have no
    // transcription yet — the status badge lives in the expanded section)
    const previewSpan = document.createElement('span');
    previewSpan.className = 'data-row-preview';
    const { text: preview, placeholder } = previewText(entry);
    previewSpan.textContent = preview;
    if (placeholder) previewSpan.classList.add('placeholder');
    row.appendChild(previewSpan);

    // Play button OR "音频缺失" badge
    if (entry.wav_size > 0 && opts.audioFailed) {
        // Load-failure state: kept in render state (dataState.audioFailedFiles)
        // so the badge survives list rebuilds — the old DOM-patch badge was
        // destroyed by the rebuild before it could ever paint. The badge
        // replaces the play button (442px row budget — both would overflow);
        // retry = the refresh button, which clears the failure set.
        const badge = document.createElement('span');
        badge.className = 'audio-error-badge';
        badge.textContent = '音频加载失败';
        row.appendChild(badge);
    } else if (entry.wav_size > 0) {
        const playBtn = document.createElement('button');
        playBtn.type = 'button';
        playBtn.className = 'btn-icon btn-play';
        playBtn.innerHTML = PLAY_SVG;
        playBtn.setAttribute(
            'aria-label',
            `播放录音 ${formatStemForDisplay(entry.filename)}`,
        );
        row.appendChild(playBtn);
    } else {
        // Variant hook: the text badge is ~28px wider than the 28px play
        // button it replaces, which overflows the 442px row budget together
        // with the preview floor. The CSS drops the floor for this variant.
        row.classList.add('audio-missing');
        const badge = document.createElement('span');
        badge.className = 'audio-missing-badge';
        badge.textContent = '音频缺失';
        row.appendChild(badge);
    }

    // Delete button (always present)
    const delBtn = document.createElement('button');
    delBtn.type = 'button';
    delBtn.className = 'btn-icon btn-delete';
    delBtn.innerHTML = TRASH_SVG;
    delBtn.setAttribute(
        'aria-label',
        `删除录音 ${formatStemForDisplay(entry.filename)}`,
    );
    row.appendChild(delBtn);

    return row;
}

/**
 * Build the expanded metadata section (language / transcription / LLM / final text).
 *
 * @param {Object} entry
 * @returns {HTMLElement}
 */
export function buildExpandedMetadata(entry) {
    const container = document.createElement('div');
    container.className = 'data-row-expanded';

    if (entry.source === 'record_only') {
        const statusLine = document.createElement('div');
        statusLine.className = 'data-row-meta-line';
        const lbl = document.createElement('span');
        lbl.className = 'data-row-meta-label';
        lbl.textContent = '状态：';
        const val = document.createElement('span');
        val.className = 'data-row-meta-value';
        const st = statusBadge(entry.transcription_status);
        const stSpan = document.createElement('span');
        stSpan.className = st.className;
        stSpan.textContent = st.text;
        val.appendChild(stSpan);
        if (entry.dropped_blocks > 0) {
            const warnSpan = document.createElement('span');
            warnSpan.className = 'badge badge-warning';
            warnSpan.textContent = '音频不完整';
            val.appendChild(warnSpan);
        }
        statusLine.appendChild(lbl);
        statusLine.appendChild(val);
        container.appendChild(statusLine);
    }

    const fields = [
        ['语言', entry.language],
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
 * Confirmation message for delete operations. Per P2 用户控制权专项,
 * the copy reflects the actual semantics: soft-delete with a 5-second undo
 * window after the dialog closes. "应用退出则无法撤销" makes the lifetime
 * of the pending window explicit (a crash kills the timer; startup sweep
 * reaps residuals).
 *
 * @param {number} count - how many recordings will be deleted
 * @returns {string}
 */
export function deleteConfirmMessage(count) {
    if (count === 1) {
        return '确定删除这条录音？删除后 5 秒内可撤销，应用退出则无法撤销。';
    }
    return `确定删除选中的 ${count} 条录音？删除后 5 秒内可撤销，应用退出则无法撤销。`;
}
