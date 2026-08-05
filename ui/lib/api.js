// ui/lib/api.js
//
// Unified Tauri invoke wrapper with automatic error forwarding.
// All user-initiated commands should go through `call()` — errors are
// reported to log_frontend_error (fire-and-forget) before re-throwing,
// so callers can focus on UI feedback (showError/setStatus) without
// re-implementing the error-reporting boilerplate.
//
// Use `rawInvoke` for fire-and-forget calls or commands whose failure
// should NOT be reported (e.g., log_frontend_error itself — prevents
// infinite recursion; cancel_download — see Task 3 Step 5).
//
// Access pattern: window.__TAURI__.core (matches ui/settings-form.js:5
// and all other existing ui/*.js files; @tauri-apps/api npm package is
// NOT installed in this repo).

const { invoke } = window.__TAURI__.core;

/**
 * Re-exported raw invoke for fire-and-forget scenarios.
 * Use this for: log_frontend_error itself, cancel_download (silent cancel path),
 * or any call whose failure is silently ignored by design.
 */
export const rawInvoke = invoke;

/**
 * Report an error to the backend tracing log. Fire-and-forget.
 *
 * @param {unknown} e - Error value (any shape; typically Error or Tauri CommandError)
 * @param {string} context - Operation name for filtering in logs
 */
export function reportError(e, context) {
    const message =
        typeof e === 'object' && e !== null && 'message' in e
            ? String(e.message)
            : String(e);
    const stack = e instanceof Error ? e.stack : null;
    // rawInvoke (NOT call) — prevents infinite recursion if log_frontend_error itself fails.
    // Return the promise chain so callers CAN await if desired; the .catch() guarantees
    // the returned promise never rejects, preserving the fire-and-forget contract.
    return rawInvoke('log_frontend_error', {
        message,
        stack,
        context,
    }).catch(() => {
        // Swallow: log write failures are non-critical (tracing is fire-and-forget by design).
    });
}

/**
 * Invoke a Tauri command with automatic error reporting.
 *
 * On success: returns the command's result.
 * On error: forwards the error to log_frontend_error (fire-and-forget)
 *          and re-throws the original error so the caller can do UI feedback.
 *
 * @param {string} cmd - Command name (e.g., 'save_settings')
 * @param {Record<string, unknown>} [args] - Arguments object (optional)
 * @param {string} [context] - Operation name for log filtering (defaults to cmd)
 * @returns {Promise<unknown>}
 */
export async function call(cmd, args = {}, context = cmd) {
    try {
        return await invoke(cmd, args);
    } catch (e) {
        reportError(e, context);
        throw e;
    }
}
