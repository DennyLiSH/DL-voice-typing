/**
 * The transcribe window's decision layer: the phase state machine, the
 * transcription ownership token, the merged-edit lock and the selection
 * guard.
 *
 * These are the accident areas of the transcribe window — its file
 * comment carries a bug history (a phase-reset leak that locked list
 * switching forever, and a guard that fired twice). They used to be
 * welded into `ui/transcribe.js`'s module-level `state` with no seam, so
 * that file had NO behaviour tests at all: the only test file imported
 * the pure `ui/lib/transcribe.js`.
 *
 * The controller owns the decisions; the DOM rendering stays in
 * `ui/transcribe.js`. The wiring is: DOM event → controller method →
 * `onRender(state)` → render.
 *
 * No framework, no dependency injection beyond the render callback: the
 * point is a seam, not an architecture.
 */
import { PHASE } from './transcribe.js';

/**
 * The legal phase transitions.
 *
 * The phases are mutually exclusive (Axis A): while an operation is in
 * flight, no other operation may start. A transition that is not listed
 * is REJECTED — the caller keeps its state and the operation it wanted
 * to start does not start.
 */
const ALLOWED_TRANSITIONS = {
    [PHASE.IDLE]: [PHASE.LOADING, PHASE.TRANSCRIBING, PHASE.INJECTING],
    [PHASE.LOADING]: [PHASE.IDLE],
    [PHASE.TRANSCRIBING]: [PHASE.IDLE],
    [PHASE.INJECTING]: [PHASE.IDLE],
};

/**
 * @param {{onRender?: (state: object) => void}} [deps]
 */
export function createTranscribeController({ onRender = () => {} } = {}) {
    const state = {
        phase: PHASE.IDLE,
        selectedStem: null,
        // Ownership token for transcription-* events: set when WE start a
        // transcription, cleared when it ends. Events arriving with no
        // local in-flight stem (e.g. the window was re-created
        // mid-transcription) are ignored so stale events never reset
        // buttons or pop misleading toasts.
        transcribingStem: null,
        // The user is editing the full merged text (segments locked).
        mergedEditing: false,
        // The user has unsaved segment edits (re-transcribing would wipe
        // them — the caller confirms first).
        dirtyEdit: false,
    };

    function snapshot() {
        return { ...state };
    }

    function render() {
        onRender(snapshot());
    }

    /**
     * The single transition point for the phase state machine.
     *
     * @param {string} next - the target phase
     * @returns {boolean} true when the transition was accepted
     */
    function setPhase(next) {
        if (next !== state.phase) {
            const allowed = ALLOWED_TRANSITIONS[state.phase] || [];
            if (!allowed.includes(next)) return false;
            state.phase = next;
        }
        render();
        return true;
    }

    function phase() {
        return state.phase;
    }

    // --- selection ------------------------------------------------------

    /** Unified entry guard (BC(C5)#1): no switching while an op is in flight. */
    function canSelect() {
        return state.phase === PHASE.IDLE;
    }

    /**
     * Select a recording.
     *
     * @param {string} stem
     * @returns {boolean} true when the selection changed
     */
    function select(stem) {
        if (!canSelect()) return false;
        if (state.selectedStem === stem) return false;
        state.selectedStem = stem;
        render();
        return true;
    }

    /** The stale guard: the selection changed while the load was in flight. */
    function isSelected(stem) {
        return state.selectedStem === stem;
    }

    function selectedStem() {
        return state.selectedStem;
    }

    /**
     * Clear the selection — the recording was deleted elsewhere.
     *
     * @returns {boolean} true when a selection was actually cleared
     */
    function clearSelection() {
        if (state.selectedStem === null) return false;
        state.selectedStem = null;
        render();
        return true;
    }

    // --- ownership token ------------------------------------------------

    /**
     * Claim the ownership token for a transcription of `stem`.
     *
     * @param {string} stem
     * @returns {boolean} true when the transcription may start
     */
    function beginTranscription(stem) {
        if (state.phase !== PHASE.IDLE) return false;
        state.transcribingStem = stem;
        return setPhase(PHASE.TRANSCRIBING);
    }

    /** Release the token — the transcription ended (or failed to start). */
    function endTranscription() {
        state.transcribingStem = null;
        return setPhase(PHASE.IDLE);
    }

    /**
     * Does this window own the incoming backend event?
     *
     * `filename` is null for the events that carry no filename
     * (progress / error) — there, the token's presence is the ownership
     * proof. For the events that do carry one (done / cancelled), the
     * filename must match the token.
     */
    function ownsEvent(filename = null) {
        if (state.transcribingStem === null) return false;
        if (filename === null || filename === undefined) return true;
        return filename === state.transcribingStem;
    }

    function transcribingStem() {
        return state.transcribingStem;
    }

    // --- merged-edit lock -----------------------------------------------

    function enterMergedEdit() {
        state.mergedEditing = true;
        render();
        return true;
    }

    function exitMergedEdit() {
        state.mergedEditing = false;
        render();
        return true;
    }

    function isMergedEditing() {
        return state.mergedEditing;
    }

    /**
     * Segment edits are locked while the merged text is being edited: a
     * segment change would otherwise clobber the user's full-text edit.
     */
    function canEditSegment() {
        return !state.mergedEditing;
    }

    // --- unsaved edits ---------------------------------------------------

    function markEditsDirty() {
        state.dirtyEdit = true;
    }

    function clearEdits() {
        state.dirtyEdit = false;
    }

    function hasEdits() {
        return state.dirtyEdit;
    }

    // --- injection --------------------------------------------------------

    function canInject() {
        return state.phase === PHASE.IDLE && state.selectedStem !== null;
    }

    /**
     * The text to inject.
     *
     * While the user edits the merged text, the textarea holds their
     * authoritative version — the segments and their edits may be stale
     * relative to it. Otherwise the text is rebuilt from the segments
     * (which respects the per-segment edits).
     *
     * @param {string} mergedValue - the textarea's current value
     * @param {string} mergedText - the text rebuilt from segments + edits
     */
    function textForInjection(mergedValue, mergedText) {
        return state.mergedEditing ? (mergedValue ?? '') : mergedText;
    }

    return {
        snapshot,
        setPhase,
        phase,
        canSelect,
        select,
        isSelected,
        selectedStem,
        clearSelection,
        beginTranscription,
        endTranscription,
        ownsEvent,
        transcribingStem,
        enterMergedEdit,
        exitMergedEdit,
        isMergedEditing,
        canEditSegment,
        markEditsDirty,
        clearEdits,
        hasEdits,
        canInject,
        textForInjection,
    };
}
