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
