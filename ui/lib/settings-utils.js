import { sameSpec } from './hotkeys.js';
import { SETTINGS_FIELDS } from './settings-schema.js';

/**
 * Compare current config against loaded config to determine dirty state.
 * Derived from the SETTINGS_FIELDS schema — the field list and per-field
 * comparison semantics live in one place (settings-schema.js).
 * Special handling: masked API key is never considered dirty.
 */
export function isConfigDirty(current, loaded) {
    return SETTINGS_FIELDS.some(({ key, equal }) =>
        equal
            ? !equal(current[key], loaded[key])
            : current[key] !== loaded[key],
    );
}

export const CREDENTIAL_WORDS = new Set([
    'key',
    'apikey',
    'token',
    'secret',
    'password',
    'signature',
    'auth',
    'authorization',
    'bearer',
    'credential',
    'sk',
]);

/**
 * Detect credential-like query/fragment params embedded in the LLM API URL.
 * Such URLs are conditionally DPAPI-encrypted at save time (backend
 * `url_contains_credential_query`); the warning still recommends moving
 * the key to the dedicated field.
 * Param names are split on non-alphanumerics, then matched exactly —
 * `keyboard`/`monkey`/`author` must NOT match.
 * Word-level counterpart of the backend redaction list
 * `src-tauri/src/llm/mod.rs::CREDENTIAL_QUERY_WORDS` (sync guarded by
 * `__tests__/credential-words-contract.test.js`).
 */
export function hasCredentialInUrl(url) {
    const segments = url.split(/[?&#]/).slice(1);
    for (const segment of segments) {
        const name = segment.split('=')[0].toLowerCase();
        const words = name.split(/[^a-z0-9]+/);
        if (words.some((w) => CREDENTIAL_WORDS.has(w))) {
            return true;
        }
    }
    return false;
}

/**
 * Validate settings before save.
 * Returns { valid: boolean, error: string|null }.
 */
export function validateSettings(config, modelStatus) {
    if (
        config.llm_enabled &&
        (!config.llm_api_url || !config.llm_api_key || !config.llm_model)
    ) {
        return {
            valid: false,
            error: '启用 LLM 时，API 地址、密钥和模型名称不能为空',
        };
    }
    const isCustomModel = config.whisper_model?.startsWith('custom:');
    if (!isCustomModel && !modelStatus[config.whisper_model]) {
        return { valid: false, error: '请先下载所选的 Whisper 模型' };
    }
    if (config.data_saving_enabled && !config.data_saving_path) {
        return { valid: false, error: '启用数据保存时，必须设置保存路径' };
    }
    if (
        config.record_only_enabled &&
        sameSpec(config.record_only_hotkey, config.hotkey)
    ) {
        return {
            valid: false,
            error: '录音快捷键不能与语音输入快捷键相同',
        };
    }
    if (config.record_only_enabled && !config.data_saving_path) {
        return {
            valid: false,
            error: '开启「录音模式」时，必须设置数据保存路径（录音文件保存在此）',
        };
    }
    return { valid: true, error: null };
}

/**
 * Conflict warning for combo-form main keys that other software commonly
 * claims (Round 5 Minor: the select offered F1/F12 with no warning).
 * Returns null when the key has no known conflict risk.
 */
export function hotkeyConflictWarning(keyName) {
    const risks = {
        F1: '多数程序用作「帮助」键',
        F12: '浏览器的「开发者工具」键',
    };
    const risk = risks[keyName];
    return risk
        ? `「${keyName}」是${risk}，容易被其他程序抢占，建议更换主键`
        : null;
}
