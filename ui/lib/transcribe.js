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

/**
 * In-flight operation phases of the transcribe window (Axis A — mutually
 * exclusive). Recording status (pending/done/failed) and edit presence are
 * orthogonal axes consumed directly by uiFlags.
 */
export const PHASE = {
    IDLE: 'idle',
    LOADING: 'loading',
    TRANSCRIBING: 'transcribing',
    INJECTING: 'injecting',
};

/**
 * Table-driven UI sync: state snapshot → all UI flags (single authority).
 * Pure — DOM application lives in ui/transcribe.js syncUI().
 *
 * @param {{phase: string, selected: string|null, status: string|null, mergedEmpty: boolean}} s
 * @returns {{
 *   transcribeDisabled: boolean,   // phase!==IDLE || !selected
 *   transcribeLabel: string,       // status==='done' ? '重新转录' : '转录'
 *   cancelVisible: boolean,        // phase===TRANSCRIBING
 *   progressVisible: boolean,      // phase===TRANSCRIBING
 *   segmentsLoading: boolean,      // phase===LOADING (skeleton rows in detail pane)
 *   injectDisabled: boolean,       // phase!==IDLE || mergedEmpty
 *   injectSpinnerVisible: boolean, // phase===INJECTING
 *   listLocked: boolean,           // phase!==IDLE (visual symmetry with entry guards)
 * }}
 */
export function uiFlags({ phase, selected, status, mergedEmpty }) {
    const busy = phase !== PHASE.IDLE;
    return {
        transcribeDisabled: busy || !selected,
        transcribeLabel: status === 'done' ? '重新转录' : '转录',
        cancelVisible: phase === PHASE.TRANSCRIBING,
        progressVisible: phase === PHASE.TRANSCRIBING,
        segmentsLoading: phase === PHASE.LOADING,
        injectDisabled: busy || mergedEmpty,
        injectSpinnerVisible: phase === PHASE.INJECTING,
        listLocked: busy,
    };
}

/**
 * Presentation for the inject-target indicator (D2-b three states).
 * `title` is read live at each refresh point (init/focus/post-inject);
 * null means never captured / consumed by a previous inject / window gone
 * at the moment of reading.
 *
 * @param {string|null} title
 * @returns {{text: string, muted: boolean}}
 */
export function injectTargetLabel(title) {
    if (typeof title === 'string' && title.trim() !== '') {
        return { text: `将注入到：${title}`, muted: false };
    }
    return { text: '重新打开窗口以选择注入目标', muted: true };
}

/**
 * Whether LLM correction is usable for the transcribe window checkbox.
 * Mirrors the backend readiness check in transcribe_cmd.rs run_llm_correction
 * (llm_enabled + url + key + model all set). The masked key marker is truthy.
 *
 * @param {Object|null} cfg - AppConfig from get_config
 * @returns {boolean}
 */
export function llmConfigured(cfg) {
    return Boolean(
        cfg?.llm_enabled && cfg.llm_api_url && cfg.llm_api_key && cfg.llm_model,
    );
}
