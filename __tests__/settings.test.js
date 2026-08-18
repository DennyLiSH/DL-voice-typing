import { describe, expect, it } from 'vitest';
import { MASKED_MARKER } from '../ui/lib/api-key-mask.js';
import { isConfigDirty, validateSettings } from '../ui/lib/settings-utils.js';

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
        const loaded = { ...baseConfig, llm_api_key: MASKED_MARKER };
        const current = { ...baseConfig, llm_api_key: MASKED_MARKER };
        expect(isConfigDirty(current, loaded)).toBe(false);
    });

    it('returns true when API key is a new value', () => {
        const loaded = { ...baseConfig, llm_api_key: MASKED_MARKER };
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

    it('returns true when realtime_transcription differs', () => {
        const current = { ...baseConfig, realtime_transcription: true };
        expect(
            isConfigDirty(current, {
                ...baseConfig,
                realtime_transcription: false,
            }),
        ).toBe(true);
    });

    it('returns false when realtime_transcription matches', () => {
        const current = { ...baseConfig, realtime_transcription: true };
        expect(
            isConfigDirty(current, {
                ...baseConfig,
                realtime_transcription: true,
            }),
        ).toBe(false);
    });

    it('returns true when record_only_enabled differs', () => {
        const current = { ...baseConfig, record_only_enabled: true };
        expect(isConfigDirty(current, baseConfig)).toBe(true);
    });

    it('returns true when record_only_hotkey differs', () => {
        const current = { ...baseConfig, record_only_hotkey: 'F9' };
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
        const config = {
            ...baseConfig,
            llm_enabled: true,
            llm_api_url: '',
            llm_api_key: 'sk-test',
            llm_model: 'gpt-4',
        };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(false);
        expect(result.error).toBeTruthy();
    });

    it('returns invalid when LLM enabled but API key missing', () => {
        const config = {
            ...baseConfig,
            llm_enabled: true,
            llm_api_url: 'https://api.example.com',
            llm_api_key: '',
            llm_model: 'gpt-4',
        };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(false);
    });

    it('returns invalid when LLM enabled but model name missing', () => {
        const config = {
            ...baseConfig,
            llm_enabled: true,
            llm_api_url: 'https://api.example.com',
            llm_api_key: 'sk-test',
            llm_model: '',
        };
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
        const config = {
            ...baseConfig,
            data_saving_enabled: true,
            data_saving_path: '',
        };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(false);
    });

    it('returns valid when LLM disabled (no API fields needed)', () => {
        const config = {
            ...baseConfig,
            llm_enabled: false,
            llm_api_url: '',
            llm_api_key: '',
            llm_model: '',
        };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(true);
    });

    it('returns valid when data saving disabled (no path needed)', () => {
        const config = {
            ...baseConfig,
            data_saving_enabled: false,
            data_saving_path: '',
        };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(true);
    });

    it('returns valid for custom model regardless of modelStatus', () => {
        const config = { ...baseConfig, whisper_model: 'custom:my-model.bin' };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(true);
    });

    it('returns valid for custom model with empty modelStatus', () => {
        const config = {
            ...baseConfig,
            whisper_model: 'custom:path/to/model.bin',
        };
        const result = validateSettings(config, {});
        expect(result.valid).toBe(true);
    });

    it('returns invalid when record-only hotkey equals main hotkey', () => {
        const config = {
            ...baseConfig,
            record_only_enabled: true,
            record_only_hotkey: 'RightCtrl',
            data_saving_path: 'D:\\recordings',
        };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(false);
        expect(result.error).toContain('录音快捷键');
    });

    it('returns invalid when record-only enabled but data saving path empty', () => {
        const config = {
            ...baseConfig,
            record_only_enabled: true,
            record_only_hotkey: 'RightAlt',
            data_saving_path: '',
        };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(false);
        expect(result.error).toContain('数据保存路径');
    });

    it('returns valid when record-only enabled with distinct hotkey and path', () => {
        const config = {
            ...baseConfig,
            record_only_enabled: true,
            record_only_hotkey: 'RightAlt',
            data_saving_path: 'D:\\recordings',
        };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(true);
    });
});
