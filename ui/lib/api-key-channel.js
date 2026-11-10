/**
 * The API-key reset channel: the state machine behind "wipe the stored
 * key".
 *
 * Wiping a stored key is a three-channel protocol, and the three
 * channels used to live as module-level variables in
 * `ui/settings-form.js`:
 *
 * 1. the PENDING flag — the restore-defaults flow asked for a wipe,
 * 2. the SNAPSHOT flag — whether a key existed when the wipe was armed
 *    (defense-in-depth: it survives a populate that forgets to reset
 *    the pending flag),
 * 3. the SENTINELS (`__CLEAR__` / `__MASKED__`) — the wire form.
 *
 * Adding a second sentinel field meant cloning the whole apparatus into
 * the next state. The channel owns the flags and the sentinel
 * discrimination; the DOM stays in settings-form.js.
 */
import { CLEAR_MARKER, MASKED_MARKER } from './api-key-mask.js';

export function createApiKeyChannel() {
    // The wipe intent is armed (the user asked to restore the defaults
    // and the stored key must go).
    let resetPending = false;
    // Snapshot channel: did a key exist when the intent was armed?
    let hadExistingAtReset = false;

    /**
     * The value to save for the API key field.
     *
     * @param {string} currentInputValue - what the user sees in the input
     * @param {object} [loadedConfig] - the dirty baseline
     * @returns {string} the value to send to the backend
     */
    function getForSave(currentInputValue, loadedConfig) {
        const typed = (currentInputValue ?? '').trim();
        // Sentinel neutralization: a sentinel hand-typed into the input
        // is indistinguishable from the legit roundtrip form at the
        // validate layer, so the guard lives HERE at the DOM source — the
        // only place that can tell "user typed" from "masked roundtrip
        // echo". A typed sentinel degrades to the empty-input semantics
        // (keep the stored key); sentinels can never reach the backend
        // by hand-typing.
        const v =
            typed === CLEAR_MARKER || typed === MASKED_MARKER ? '' : typed;
        // Dual channel: the LIVE check reads the pre-reset baseline; the
        // SNAPSHOT channel remains as defense-in-depth (and covers any
        // future populate caller that forgets to reset the intent).
        const hasExistingKey =
            loadedConfig?.llm_api_key === MASKED_MARKER ||
            (resetPending && hadExistingAtReset);
        if (v) return v;
        // Restore-defaults intent: wiping a stored key needs the explicit
        // sentinel (empty input alone KEEPS the stored key — backend
        // unmask_or_keep semantics).
        if (resetPending && hasExistingKey) return CLEAR_MARKER;
        return loadedConfig?.llm_api_key === MASKED_MARKER ? MASKED_MARKER : '';
    }

    /**
     * Arm the wipe intent.
     *
     * @param {boolean} hadExisting - whether a key existed at that moment
     */
    function armReset(hadExisting) {
        hadExistingAtReset = Boolean(hadExisting);
        resetPending = true;
    }

    /**
     * Abandon the wipe intent completely (the config was reloaded from
     * the backend, or a save succeeded).
     */
    function disarm() {
        resetPending = false;
        hadExistingAtReset = false;
    }

    /**
     * Cancel the pending intent but KEEP the snapshot channel — the user
     * took over (typed something) while the snapshot stays as
     * defense-in-depth.
     */
    function cancelPending() {
        resetPending = false;
    }

    function isResetPending() {
        return resetPending;
    }

    return {
        getForSave,
        armReset,
        disarm,
        cancelPending,
        isResetPending,
    };
}
