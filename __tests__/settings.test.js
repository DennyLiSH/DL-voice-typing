import { describe, expect, it } from 'vitest';
import { MASKED_MARKER } from '../ui/lib/api-key-mask.js';
import { sameSpec } from '../ui/lib/hotkeys.js';
import {
    hasCredentialInUrl,
    isConfigDirty,
    validateSettings,
} from '../ui/lib/settings-utils.js';

// Hotkey specs use vk codes (0xA3 = RightCtrl, 0xA5 = RightAlt, 0x70-0x7B =
// F1-F12, 0x41-0x5A = A-Z). See backend hotkey/mod.rs NAMED_KEYS for the
// authoritative mapping. 0x70 = 112 (F1), 0x41 = 65 ('A').
const RIGHT_CTRL = { ctrl: false, shift: false, alt: false, vk: 0xa3 };
const RIGHT_ALT = { ctrl: false, shift: false, alt: false, vk: 0xa5 };
const F1 = { ctrl: false, shift: false, alt: false, vk: 0x70 };
const F9 = { ctrl: false, shift: false, alt: false, vk: 0x78 };

const baseConfig = {
    language: 'zh',
    hotkey: RIGHT_CTRL,
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
    realtime_transcription: false,
    record_only_enabled: false,
    record_only_hotkey: RIGHT_ALT,
};

describe('isConfigDirty', () => {
    it('returns false when configs are identical', () => {
        expect(isConfigDirty(baseConfig, baseConfig)).toBe(false);
    });

    it('returns true when language differs', () => {
        const current = { ...baseConfig, language: 'en' };
        expect(isConfigDirty(current, baseConfig)).toBe(true);
    });

    it('returns true when hotkey vk differs (same modifier bits)', () => {
        const current = { ...baseConfig, hotkey: F9 };
        expect(isConfigDirty(current, baseConfig)).toBe(true);
    });

    it('returns true when hotkey ctrl modifier differs', () => {
        const current = {
            ...baseConfig,
            hotkey: { ctrl: true, shift: false, alt: false, vk: 0xa3 },
        };
        expect(isConfigDirty(current, baseConfig)).toBe(true);
    });

    it('returns false when hotkey matches structurally with different property order', () => {
        // The form can produce specs with different property order than
        // the loaded config (e.g. setter copy). sameSpec is order-independent.
        const current = {
            ...baseConfig,
            hotkey: { vk: 0xa3, alt: false, shift: false, ctrl: false },
        };
        expect(isConfigDirty(current, baseConfig)).toBe(false);
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

    it('returns true when record_only_hotkey differs (vk change)', () => {
        const current = { ...baseConfig, record_only_hotkey: F9 };
        expect(isConfigDirty(current, baseConfig)).toBe(true);
    });
});

describe('hasCredentialInUrl', () => {
    it.each([
        ['https://h.com/v1?key=sk-123'],
        ['https://h.com/v1?api_key=abc'],
        ['https://h.com/v1?a=1&token=xyz'],
        ['https://h.com/v1?API_KEY=abc'],
        ['https://h.com/v1?access_token=t'],
        ['https://h.com/v1?client_secret=s'],
        ['https://h.com/v1?bearer_token=t'],
        ['https://h.com/v1?secret_key=k'],
        ['https://h.com/v1?api-key=k'],
        ['https://h.com/v1#access_token=t'],
        ['https://h.com/v1?sk=abc'],
    ])('detects credential param in %s', (url) => {
        expect(hasCredentialInUrl(url)).toBe(true);
    });

    it.each([
        ['https://api.openai.com/v1/chat/completions'],
        ['https://h.com/v1?model=gpt-4o'],
        ['https://h.com/v1?stream=true&a=1'],
        [''],
        ['https://h.com/v1?keyboard=logitech'],
        ['https://h.com/v1?monkey=1'],
        ['https://h.com/v1?author=denny'],
        ['https://h.com/v1?task=1'],
    ])('does not flag %s', (url) => {
        expect(hasCredentialInUrl(url)).toBe(false);
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

    it('returns invalid when record-only hotkey equals main hotkey (same vk, no mods)', () => {
        const config = {
            ...baseConfig,
            record_only_enabled: true,
            hotkey: RIGHT_CTRL,
            record_only_hotkey: RIGHT_CTRL,
            data_saving_path: 'D:\\recordings',
        };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(false);
        expect(result.error).toContain('录音快捷键');
    });

    it('returns invalid when record-only hotkey equals main hotkey (with mods)', () => {
        // Modifier bits make the same vk distinguishable by spec but sameSpec
        // ignores modifiers in the equality check (we use structural equality
        // across the whole spec). The check here is whether the two specs
        // are deeply equal — identical spec means the keys would collide.
        const config = {
            ...baseConfig,
            record_only_enabled: true,
            hotkey: { ctrl: true, shift: false, alt: false, vk: 0x41 },
            record_only_hotkey: {
                ctrl: true,
                shift: false,
                alt: false,
                vk: 0x41,
            },
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
            record_only_hotkey: RIGHT_ALT,
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
            record_only_hotkey: RIGHT_ALT,
            data_saving_path: 'D:\\recordings',
        };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(true);
    });
});

// sameSpec is the structural-equality primitive used by isConfigDirty and
// validateSettings. Tested separately so failures point to the right layer.
describe('sameSpec (sanity check — deep coverage lives in __tests__/hotkeys.test.js)', () => {
    it('compares two specs structurally', () => {
        expect(sameSpec(RIGHT_CTRL, RIGHT_CTRL)).toBe(true);
        expect(sameSpec(RIGHT_CTRL, RIGHT_ALT)).toBe(false);
        expect(sameSpec(F1, F9)).toBe(false);
    });
});
