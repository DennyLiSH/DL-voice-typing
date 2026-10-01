// This marker must stay in sync with the backend constant
// `config::presentation::MASKED_MARKER` in `src-tauri/src/config/presentation.rs`.
// When changing it, update both sides and the contract test.
export const MASKED_MARKER = '__MASKED__';

// Backend: config::presentation::CLEAR_MARKER. Only the restore-defaults
// flow sends this; an empty input means "keep the stored key".
export const CLEAR_MARKER = '__CLEAR__';
