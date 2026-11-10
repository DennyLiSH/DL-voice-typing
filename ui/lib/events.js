/**
 * The event registry: one table for every backend event name, plus the
 * payload decoders and the listener lifecycle.
 *
 * Before this module the event names were bare string literals at 27
 * call sites in 6 files. A typo in a listener name fails SILENTLY —
 * Tauri does not warn about a listener for an event that is never
 * emitted — which is the same class of defect the invoke-registry
 * contract test kills on the command side (`get_default_config`
 * shipped unregistered).
 *
 * Three kinds of knowledge live here on purpose:
 *
 * - the NAME TABLE: which events exist (the values are pinned against
 *   the Rust emit sites by `__tests__/event-registry-contract.test.js`),
 * - the PAYLOAD DECODERS: the backend sends two payload shapes — bare
 *   strings (the error summaries) and objects (`{stem}`,
 *   `{stem, status}`, `{message}`, `{percent, stage}`, `{filename}`).
 *   That rule used to live in a comment in floating.js; now it is code.
 * - the LISTENER LIFECYCLE: Tauri 2's `event.listen` returns a
 *   `Promise<UnlistenFn>` that must be awaited before unlistening.
 *   Only one module knew that; `subscribe` makes it the default path.
 */

/**
 * Every backend event name.
 *
 * The table is the union of the events the frontend listens to and the
 * events the backend emits — a registry of the namespace, not just of
 * the listeners. The five entries the frontend never subscribes to
 * (`llmComplete`, `perfMetrics`, `reviewFinalText`, `trayReset`,
 * `watchdogReset`) are still part of the contract: the registry pins
 * both sides against the Rust source.
 */
export const EVENTS = {
    audioRms: 'audio-rms',
    downloadProgress: 'download-progress',
    hotkeyError: 'hotkey-error',
    injectionComplete: 'injection-complete',
    injectionError: 'injection-error',
    llmComplete: 'llm-complete',
    llmError: 'llm-error',
    llmRefining: 'llm-refining',
    modelLoaded: 'model-loaded',
    pendingDeletesFinalized: 'pending-deletes-finalized',
    perfMetrics: 'perf-metrics',
    pipelineCancelled: 'pipeline-cancelled',
    recordOnlyDiscarded: 'record-only-discarded',
    recordOnlyError: 'record-only-error',
    recordOnlyFinished: 'record-only-finished',
    recordOnlyStarted: 'record-only-started',
    recordingStart: 'recording-start',
    reviewFinalText: 'review-final-text',
    reviewShow: 'review-show',
    speechError: 'speech-error',
    transcriptionCancelled: 'transcription-cancelled',
    transcriptionComplete: 'transcription-complete',
    transcriptionDone: 'transcription-done',
    transcriptionError: 'transcription-error',
    transcriptionPartial: 'transcription-partial',
    transcriptionProgress: 'transcription-progress',
    trayReset: 'tray-reset',
    watchdogReset: 'watchdog-reset',
};

/**
 * Decode an OBJECT-shaped payload.
 *
 * The backend sends several object shapes; they are all normalized to
 * the same key set so a listener can destructure without guessing:
 * `{stem}`, `{stem, status}`, `{message}`, `{percent, stage}`,
 * `{filename}`. A non-object payload yields an empty object — the
 * "is it an object?" decision no longer belongs to each listener.
 */
export function decodeObjectPayload(payload) {
    if (payload === null || typeof payload !== 'object') return {};
    const { stem, status, message, percent, stage, filename } = payload;
    return { stem, status, message, percent, stage, filename };
}

/**
 * Decode a BARE-STRING payload (the error summaries).
 *
 * The backend sends the error payloads as bare JSON strings. The
 * ErrorHistory recorder also accepts a `{message}` object form, so both
 * shapes are accepted here and anything else yields an empty string.
 */
export function decodeBareString(payload) {
    if (typeof payload === 'string') return payload;
    if (
        payload !== null &&
        typeof payload === 'object' &&
        typeof payload.message === 'string'
    ) {
        return payload.message;
    }
    return '';
}

/**
 * Subscribe to an event and get a handle with an idempotent `unlisten`.
 *
 * Tauri 2's `event.listen` returns a `Promise<UnlistenFn>`; the
 * promise must be awaited before the listener can actually be removed.
 * `subscribe` owns that detail: `unlisten()` awaits it, is safe to
 * call twice, and never rejects (the window may already be gone when
 * the promise settles).
 *
 * Module-level listeners that live as long as the window do not need
 * this handle — they can call `listen(name, ...)` with a name from the
 * table directly.
 */
export function subscribe(name, callback) {
    const listenFn = window.__TAURI__?.event?.listen;
    if (!listenFn) {
        return { unlisten: async () => {} };
    }
    const pending = listenFn(name, callback);
    let released = false;
    return {
        async unlisten() {
            if (released) return;
            released = true;
            try {
                const unlisten = await pending;
                if (typeof unlisten === 'function') unlisten();
            } catch (_e) {
                // Window already gone — listen()'s promise may reject on
                // teardown. The listener dies with the window anyway.
            }
        },
    };
}
