/**
 * Compare current config against loaded config to determine dirty state.
 * Special handling: masked API key is never considered dirty.
 */
export function isConfigDirty(current, loaded) {
    const apiKeyDirty = current.llm_api_key !== '__MASKED__' && current.llm_api_key !== loaded.llm_api_key;
    return (
        current.language !== loaded.language ||
        current.hotkey !== loaded.hotkey ||
        current.whisper_model !== loaded.whisper_model ||
        current.llm_enabled !== loaded.llm_enabled ||
        current.llm_api_url !== loaded.llm_api_url ||
        apiKeyDirty ||
        current.llm_model !== loaded.llm_model ||
        current.download_mirror !== loaded.download_mirror ||
        current.data_saving_enabled !== loaded.data_saving_enabled ||
        current.data_saving_path !== loaded.data_saving_path ||
        current.review_before_paste !== loaded.review_before_paste ||
        current.autostart !== loaded.autostart
    );
}

/**
 * Validate settings before save.
 * Returns { valid: boolean, error: string|null }.
 */
export function validateSettings(config, modelStatus) {
    if (config.llm_enabled && (!config.llm_api_url || !config.llm_api_key || !config.llm_model)) {
        return { valid: false, error: '启用 LLM 时，API 地址、密钥和模型名称不能为空' };
    }
    if (!modelStatus[config.whisper_model]) {
        return { valid: false, error: '请先下载所选的 Whisper 模型' };
    }
    if (config.data_saving_enabled && !config.data_saving_path) {
        return { valid: false, error: '启用数据保存时，必须设置保存路径' };
    }
    return { valid: true, error: null };
}
