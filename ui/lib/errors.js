// Shared frontend user-facing error strings (frontend counterpart of the
// backend's fixed Chinese summary constants). The floating window and the
// review window must render the SAME injection-failure sentence — a vitest
// contract asserts the two imports stay identical.

/**
 * Mirrors `INJECTION_ERROR_USER_MSG` in
 * `src-tauri/src/commands/delivery_controller.rs`. States the real
 * consequence honestly (the transcribed text was NOT saved — only the
 * tracing log keeps it) and the clipboard trade-off (the previous clipboard
 * content wins; the transcript is deliberately not put back).
 */
export const INJECTION_ERROR_MSG = '粘贴失败，本次文字未保存，原剪贴板已恢复';

/**
 * Normalize a caught error into a user-facing message.
 *
 * The same failure reaches the frontend in three shapes: a bare string
 * (the backend's fixed Chinese summaries — Tauri rejects plain string
 * errors as strings), a `CommandError` object (`{code, message}` — the
 * serializable error the backend returns), and an `Error` instance
 * (thrown by the frontend itself). Every call site used to re-derive
 * that discrimination on its own, with two different variants — one
 * extracted `error.message`, the other showed a local fallback for
 * anything that was not a string and therefore DROPPED the backend's
 * message.
 *
 * @param {unknown} error - The caught value.
 * @param {string} fallback - Message to use when the error carries none.
 * @returns {string}
 */
export function errorMessage(error, fallback) {
    if (typeof error === 'string') return error;
    if (
        error !== null &&
        typeof error === 'object' &&
        typeof error.message === 'string' &&
        error.message
    ) {
        return error.message;
    }
    return fallback;
}
