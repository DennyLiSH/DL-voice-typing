/**
 * Pure helpers for the transcribe window UI.
 *
 * Every function here is pure — same input → same output, no DOM access,
 * no side effects. The imperative glue lives in ui/transcribe.js.
 * Unit-tested in __tests__/transcribe.test.js.
 */

/**
 * Format a millisecond offset as a compact timestamp badge.
 *   - < 1h: "m:ss"  (minutes not zero-padded)
 *   - >= 1h: "h:mm:ss" (minutes/seconds zero-padded)
 * Sub-second precision is floored (59.9s → "0:59").
 *
 * @param {number} ms
 * @returns {string}
 */
export function formatTimestamp(ms) {
    if (!Number.isFinite(ms) || ms < 0) return '0:00';
    const totalSecs = Math.floor(ms / 1000);
    const h = Math.floor(totalSecs / 3600);
    const m = Math.floor((totalSecs % 3600) / 60);
    const s = totalSecs % 60;
    const ss = String(s).padStart(2, '0');
    if (h > 0) {
        return `${h}:${String(m).padStart(2, '0')}:${ss}`;
    }
    return `${m}:${ss}`;
}

/**
 * Find the index of the segment containing `timeMs`.
 * Containment is half-open: start_ms <= timeMs < end_ms.
 * Returns -1 when no segment matches (gaps, before first, at/after last end).
 *
 * @param {Array<{start_ms: number, end_ms: number}>} segments
 * @param {number} timeMs
 * @returns {number}
 */
export function findActiveSegmentIndex(segments, timeMs) {
    if (!Array.isArray(segments) || !Number.isFinite(timeMs)) return -1;
    for (let i = 0; i < segments.length; i++) {
        const seg = segments[i];
        if (seg.start_ms <= timeMs && timeMs < seg.end_ms) return i;
    }
    return -1;
}

/**
 * Merge segment texts into the final injectable text.
 * `edits` maps segment index → user-edited text, overriding the segment's
 * original text without writing back into the segment itself (timestamps
 * stay aligned with the original transcription).
 *
 * @param {Array<{text: string}>} segments
 * @param {Map<number, string>} [edits]
 * @returns {string}
 */
export function mergeSegmentTexts(segments, edits) {
    if (!Array.isArray(segments) || segments.length === 0) return '';
    let out = '';
    for (let i = 0; i < segments.length; i++) {
        const edited = edits instanceof Map ? edits.get(i) : undefined;
        out += edited !== undefined ? edited : segments[i].text;
    }
    return out;
}

/**
 * Badge presentation for a transcription status.
 *
 * @param {string|null|undefined} status - "pending" | "done" | "failed"
 * @returns {{text: string, className: string}}
 */
export function statusBadge(status) {
    switch (status) {
        case 'done':
            return { text: '已转录', className: 'badge badge-done' };
        case 'failed':
            return { text: '转录失败', className: 'badge badge-failed' };
        default:
            return { text: '待转录', className: 'badge badge-pending' };
    }
}

/**
 * Keep only record-only recordings (the transcribe window never lists
 * classic pipeline recordings).
 *
 * @param {Array<{source?: string}>} items
 * @returns {Array}
 */
export function filterRecordOnly(items) {
    if (!Array.isArray(items)) return [];
    return items.filter((it) => it && it.source === 'record_only');
}

/**
 * Whether a playback-highlight change may auto-scroll the segments list.
 * Scrolling is suppressed while the user is editing a segment so the list
 * never yanks the caret out of view mid-edit.
 *
 * @param {Element|null} activeElement - typically document.activeElement
 * @returns {boolean}
 */
export function shouldAutoScroll(activeElement) {
    if (!activeElement) return true;
    if (typeof activeElement.closest !== 'function') return true;
    return activeElement.closest('.segment-text') === null;
}
