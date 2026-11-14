/**
 * The floating window's visual mode machine (Round 6 P6).
 *
 * A small decision layer — explicit mode + transition matrix + per-mode
 * inline cleanup matrix — replacing the comment-carried mode knowledge
 * and the hand-rolled inline-style cleanup sites in ui/floating.js.
 * Deliberately NOT a full transcribe-controller replica (plan 裁决:
 * 小决策层).
 *
 * Element-first args (lib/banner.js precedent): the module never queries
 * the DOM itself; callers pass the #indicator element in.
 *
 * Why the transition matrix is permissive — every mode lists IDLE, and
 * every non-record-only-family mode lists RECORDING + RECORD_ONLY +
 * ERROR:
 *
 * - Stale-webview reachability: backend escape hatches (watchdog/tray
 *   reset) OS-hide the window without a webview event, and hidden-window
 *   timer throttling can strand the webview in any mode. The next backend
 *   event can therefore be a session start (`recording-start`,
 *   `record-only-started`) or an error from any mode — hide (IDLE) is the
 *   universal reset.
 * - `record_only → recording` is a tested defensive path
 *   (floating.record-only.test.js cross-mode guard): a classic
 *   `recording-start` arriving mid-record-only-timer must clear the
 *   record-only state.
 * - Self-transitions are always accepted (no-op re-entry: e.g.
 *   `transcription-complete` then `llm-refining` both land in PROCESSING;
 *   a second error while the error display is up) — same rule as the
 *   transcribe-controller precedent.
 *
 * Rejections are the wiring-bug surface — a rejected transition means a
 * backend event arrived in a state the pipelines cannot produce:
 *
 * - record-only family → PROCESSING: classic transcription events during
 *   a record-only session cannot happen — the pipelines are mutually
 *   exclusive backend-side.
 * - IDLE → PROCESSING: processing only follows a classic recording.
 * - ERROR → PROCESSING: same, and it closes the pre-existing
 *   showProcessing settle-race (a mid-settle error's display must not be
 *   overwritten by the late settle flip).
 * - Substate re-entries are not listed (e.g. RECORD_ONLY_SAVED →
 *   RECORD_ONLY_DISCARDED): one release classifies as one or the other,
 *   never both.
 */

export const MODES = Object.freeze({
    IDLE: 'idle',
    RECORDING: 'recording',
    PROCESSING: 'processing',
    ERROR: 'error',
    RECORD_ONLY: 'record_only',
    RECORD_ONLY_SAVED: 'record_only_saved',
    RECORD_ONLY_DISCARDED: 'record_only_discarded',
});

// The legal mode transitions. Every transition's absence is justified in
// the module doc block above.
const MODE_TRANSITIONS = {
    [MODES.IDLE]: [MODES.RECORDING, MODES.RECORD_ONLY, MODES.ERROR],
    [MODES.RECORDING]: [
        MODES.IDLE,
        MODES.PROCESSING,
        MODES.ERROR,
        MODES.RECORD_ONLY,
    ],
    [MODES.PROCESSING]: [
        MODES.IDLE,
        MODES.RECORDING,
        MODES.RECORD_ONLY,
        MODES.ERROR,
    ],
    [MODES.ERROR]: [MODES.IDLE, MODES.RECORDING, MODES.RECORD_ONLY],
    [MODES.RECORD_ONLY]: [
        MODES.IDLE,
        MODES.RECORDING,
        MODES.RECORD_ONLY_SAVED,
        MODES.RECORD_ONLY_DISCARDED,
        MODES.ERROR,
    ],
    [MODES.RECORD_ONLY_SAVED]: [
        MODES.IDLE,
        MODES.RECORDING,
        MODES.RECORD_ONLY,
        MODES.ERROR,
    ],
    [MODES.RECORD_ONLY_DISCARDED]: [
        MODES.IDLE,
        MODES.RECORDING,
        MODES.RECORD_ONLY,
        MODES.ERROR,
    ],
};

// Mode slot — which visual mode the floating window is in. Starts at IDLE;
// mutated only by setMode.
let mode = MODES.IDLE;

/**
 * Whether `from → to` is a legal transition.
 *
 * Self-transitions (from === to) are always accepted for known modes;
 * unknown mode strings are rejected (either argument). Object.hasOwn
 * keeps prototype keys ('constructor', 'toString') out of the matrix
 * lookup, same hazard lib/banner.js guards against with Map.get.
 *
 * @param {string} from
 * @param {string} to
 * @returns {boolean}
 */
export function canTransition(from, to) {
    if (!Object.hasOwn(MODE_TRANSITIONS, from)) return false;
    if (from === to) return true;
    return MODE_TRANSITIONS[from].includes(to);
}

/**
 * The current mode; starts at MODES.IDLE.
 *
 * @returns {string}
 */
export function currentMode() {
    return mode;
}

/**
 * The single transition point. Returns false WITHOUT any mutation when
 * the transition is rejected; on acceptance it updates the mode slot and
 * syncs the record-only class — the class ⇔ mode === RECORD_ONLY
 * invariant is owned here and nowhere else (record-only class 仍由 mode
 * 驱动供 CSS 消费).
 *
 * @param {HTMLElement} el - the #indicator element (class sync target)
 * @param {string} next - target mode
 * @returns {boolean} true when the transition was accepted
 */
export function setMode(el, next) {
    if (!canTransition(mode, next)) return false;
    mode = next;
    el.classList.toggle('record-only', next === MODES.RECORD_ONLY);
    return true;
}

// The record-only look is CSS-driven (breathe keyframes + tint), so any
// inline background/shadow/transform left by the spring-driven modes must
// be cleared — inline styles beat class rules. This matrix is the single
// home for that knowledge: per mode, the inline properties #indicator
// must have cleared when the mode is entered.
//
// RECORDING only clears `filter`: bg/shadow/transform are SET right after
// by the handler.
const INLINE_CLEANUP = {
    [MODES.IDLE]: ['filter'],
    [MODES.RECORDING]: ['filter'], // bg/shadow/transform are SET right after by the handler
    [MODES.RECORD_ONLY]: ['background', 'boxShadow', 'transform', 'filter'],
    [MODES.PROCESSING]: ['background', 'boxShadow'],
    [MODES.ERROR]: ['background', 'boxShadow', 'filter'],
    [MODES.RECORD_ONLY_SAVED]: ['filter'],
    [MODES.RECORD_ONLY_DISCARDED]: ['filter'],
};

/**
 * Apply the per-mode inline cleanup matrix to `el`: for each property in
 * the mode's set, clear the inline style. An unknown mode throws BEFORE
 * any mutation (mirror of lib/banner.js's unknown-variant guard — a
 * programming error, not a runtime condition).
 *
 * @param {HTMLElement} el - the #indicator element
 * @param {string} mode - the mode being entered
 */
export function resetIndicatorInline(el, mode) {
    if (!Object.hasOwn(INLINE_CLEANUP, mode)) {
        throw new Error(`floating-mode: unknown mode "${mode}"`);
    }
    for (const prop of INLINE_CLEANUP[mode]) {
        el.style[prop] = '';
    }
}
