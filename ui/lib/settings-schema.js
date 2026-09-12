import { MASKED_MARKER } from './api-key-mask.js';
import { sameSpec } from './hotkeys.js';

/**
 * Single source of truth for the settings form's field list.
 *
 * Each descriptor: { key, equal? }. `equal(current, loaded)` returns true
 * when the field is UNCHANGED (default: `===`). Custom comparators:
 *  - hotkey / record_only_hotkey: HotkeySpec objects — structural equality
 *    via sameSpec (shallow `!==` is always true on objects).
 *  - llm_api_key: masked marker is never dirty (DPAPI-masked roundtrip).
 *
 * Consumers:
 *  - settings-utils.js :: isConfigDirty — derives the dirty check
 *  - settings-form.js :: FIELDS — injects DOM get/set wiring per key
 *
 * Adding a setting = one entry here + one DOM_DEFS wiring in
 * settings-form.js (sync guarded by __tests__/settings-schema.test.js).
 */
// equal 槽位是 UNCHANGED 谓词（与 sameSpec 同极性）：掩码标记或与 loaded
// 相同 → 未变更。dirty = !equal ≡ 现行 apiKeyDirty
// (`current !== MASKED_MARKER && current !== loaded`)，逐字等价。
// 注意：不要把现行 apiKeyDirty 的 dirty 谓词原样放进 equal 槽位——那会
// 双重取反导致 llm_api_key 脏态判定完全反转（review Iteration 1 S4-M1）。
const maskedKeyEqual = (current, loaded) =>
    current === MASKED_MARKER || current === loaded;

export const SETTINGS_FIELDS = [
    { key: 'language' },
    { key: 'hotkey', equal: sameSpec },
    { key: 'whisper_model' },
    { key: 'llm_enabled' },
    { key: 'llm_api_url' },
    { key: 'llm_api_key', equal: maskedKeyEqual },
    { key: 'llm_model' },
    { key: 'download_mirror' },
    { key: 'data_saving_enabled' },
    { key: 'data_saving_path' },
    { key: 'review_before_paste' },
    { key: 'realtime_transcription' },
    { key: 'autostart' },
    { key: 'record_only_enabled' },
    { key: 'record_only_hotkey', equal: sameSpec },
];
