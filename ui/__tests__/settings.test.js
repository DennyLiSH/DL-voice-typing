import { describe, it, expect } from 'vitest';
import { isConfigDirty, validateSettings } from '../lib/settings-utils.js';

const baseConfig = {
    language: 'zh',
    hotkey: 'RightCtrl',
    whisper_model: 'base',
    llm_enabled: false,
    llm_api_url: '',
    llm_api_key: '',
    llm_model: '',
    download_mirror: 'hf-mirror',
    data_saving_enabled: false,
    data_saving_path: '',
    review_before_paste: false,
    autostart: false,
};

describe('isConfigDirty', () => {
    it('returns false when configs are identical', () => {
        expect(isConfigDirty(baseConfig, baseConfig)).toBe(false);
    });

    it('returns true when language differs', () => {
        const current = { ...baseConfig, language: 'en' };
        expect(isConfigDirty(current, baseConfig)).toBe(true);
    });

    it('returns true when hotkey differs', () => {
        const current = { ...baseConfig, hotkey: 'F9' };
        expect(isConfigDirty(current, baseConfig)).toBe(true);
    });

    it('returns true when whisper_model differs', () => {
        const current = { ...baseConfig, whisper_model: 'small' };
        expect(isConfigDirty(current, baseConfig)).toBe(true);
    });

    it('returns true when llm_enabled differs', () => {
        const current = { ...baseConfig, llm_enabled: true };
        expect(isConfigDirty(current, baseConfig)).toBe(true);
    });

    it('returns false when API key is masked', () => {
        const loaded = { ...baseConfig, llm_api_key: '__MASKED__' };
        const current = { ...baseConfig, llm_api_key: '__MASKED__' };
        expect(isConfigDirty(current, loaded)).toBe(false);
    });

    it('returns true when API key is a new value', () => {
        const loaded = { ...baseConfig, llm_api_key: '__MASKED__' };
        const current = { ...baseConfig, llm_api_key: 'sk-new-key' };
        expect(isConfigDirty(current, loaded)).toBe(true);
    });

    it('returns true when download_mirror differs', () => {
        const current = { ...baseConfig, download_mirror: 'huggingface' };
        expect(isConfigDirty(current, baseConfig)).toBe(true);
    });

    it('returns true when data_saving_enabled differs', () => {
        const current = { ...baseConfig, data_saving_enabled: true };
        expect(isConfigDirty(current, baseConfig)).toBe(true);
    });

    it('returns true when review_before_paste differs', () => {
        const current = { ...baseConfig, review_before_paste: true };
        expect(isConfigDirty(current, baseConfig)).toBe(true);
    });

    it('returns true when autostart differs', () => {
        const current = { ...baseConfig, autostart: true };
        expect(isConfigDirty(current, baseConfig)).toBe(true);
    });
});

describe('validateSettings', () => {
    const modelStatus = { base: true, small: false };

    it('returns valid when all fields correct', () => {
        const result = validateSettings(baseConfig, modelStatus);
        expect(result.valid).toBe(true);
        expect(result.error).toBeNull();
    });

    it('returns invalid when LLM enabled but API URL missing', () => {
        const config = { ...baseConfig, llm_enabled: true, llm_api_url: '', llm_api_key: 'sk-test', llm_model: 'gpt-4' };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(false);
        expect(result.error).toBeTruthy();
    });

    it('returns invalid when LLM enabled but API key missing', () => {
        const config = { ...baseConfig, llm_enabled: true, llm_api_url: 'https://api.example.com', llm_api_key: '', llm_model: 'gpt-4' };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(false);
    });

    it('returns invalid when LLM enabled but model name missing', () => {
        const config = { ...baseConfig, llm_enabled: true, llm_api_url: 'https://api.example.com', llm_api_key: 'sk-test', llm_model: '' };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(false);
    });

    it('returns invalid when selected model not downloaded', () => {
        const config = { ...baseConfig, whisper_model: 'small' };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(false);
        expect(result.error).toContain('模型');
    });

    it('returns invalid when data saving enabled but path empty', () => {
        const config = { ...baseConfig, data_saving_enabled: true, data_saving_path: '' };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(false);
    });

    it('returns valid when LLM disabled (no API fields needed)', () => {
        const config = { ...baseConfig, llm_enabled: false, llm_api_url: '', llm_api_key: '', llm_model: '' };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(true);
    });

    it('returns valid when data saving disabled (no path needed)', () => {
        const config = { ...baseConfig, data_saving_enabled: false, data_saving_path: '' };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(true);
    });
});
