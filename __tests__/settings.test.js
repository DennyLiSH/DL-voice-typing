import { describe, expect, it } from 'vitest';
import { MASKED_MARKER } from '../ui/lib/api-key-mask.js';
import { sameSpec } from '../ui/lib/hotkeys.js';
import {
    apiUrlWarningText,
    crossSlotConflicts,
    hasCredentialInUrl,
    hasUserinfoInUrl,
    hotkeyConflictWarning,
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

    it('returns true when llm_api_url differs', () => {
        const current = {
            ...baseConfig,
            llm_api_url: 'https://api.example.com/v1',
        };
        expect(isConfigDirty(current, baseConfig)).toBe(true);
    });

    it('returns true when llm_model differs', () => {
        const current = { ...baseConfig, llm_model: 'gpt-4o-mini' };
        expect(isConfigDirty(current, baseConfig)).toBe(true);
    });

    it('returns true when data_saving_path differs', () => {
        const current = { ...baseConfig, data_saving_path: 'D:\\recordings' };
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

    it('returns invalid when an open slot equals the main hotkey', () => {
        const config = {
            ...baseConfig,
            open_settings_hotkey: RIGHT_CTRL,
        };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(false);
        expect(result.error).toContain('冲突');
    });

    it('returns invalid when the two open slots are identical', () => {
        const config = {
            ...baseConfig,
            open_settings_hotkey: F9,
            open_transcribe_hotkey: F9,
        };
        const result = validateSettings(config, modelStatus);
        expect(result.valid).toBe(false);
        expect(result.error).toContain('冲突');
    });
});

// ---- M6-a cross-slot conflict pure function ----

describe('crossSlotConflicts (M6-a)', () => {
    const base = {
        hotkey: RIGHT_CTRL,
        record_only_hotkey: RIGHT_ALT,
        open_settings_hotkey: null,
        open_transcribe_hotkey: null,
    };

    it('returns [] when no two enabled slots collide (null slots exempt)', () => {
        expect(crossSlotConflicts(base)).toEqual([]);
        // Null open slots never conflict with any set spec.
        expect(crossSlotConflicts({ ...base, hotkey: F1 })).toEqual([]);
    });

    it('flags the open slot when it equals the primary hotkey (never the primary slot)', () => {
        // DR-2.3 order invariant: blame always points at the LATER slot —
        // the primary slot is never reported.
        const conflicts = crossSlotConflicts({
            ...base,
            open_settings_hotkey: RIGHT_CTRL,
        });
        expect(conflicts).toHaveLength(1);
        expect(conflicts[0].slot).toBe('open_settings_hotkey');
        expect(conflicts[0].slot).not.toBe('hotkey');
        expect(conflicts[0].message).toContain('语音输入键');
    });

    it('flags the later open slot when the two open slots collide', () => {
        const conflicts = crossSlotConflicts({
            ...base,
            open_settings_hotkey: F9,
            open_transcribe_hotkey: F9,
        });
        expect(conflicts).toHaveLength(1);
        expect(conflicts[0].slot).toBe('open_transcribe_hotkey');
    });

    it('flags the open slot when record_only collides with it', () => {
        const conflicts = crossSlotConflicts({
            ...base,
            open_settings_hotkey: RIGHT_ALT,
        });
        expect(conflicts).toHaveLength(1);
        expect(conflicts[0].slot).toBe('open_settings_hotkey');
        expect(conflicts[0].message).toContain('录音快捷键');
    });

    it('attributes every colliding pair to its later slot (multi-pair)', () => {
        const conflicts = crossSlotConflicts({
            ...base,
            open_settings_hotkey: RIGHT_CTRL,
            open_transcribe_hotkey: RIGHT_CTRL,
        });
        // Two open slots colliding with each other AND with the primary —
        // each colliding pair reports its later slot: one per pair.
        expect(conflicts).toHaveLength(3);
        const slots = conflicts.map((c) => c.slot);
        expect(slots).toContain('open_settings_hotkey');
        expect(slots).toContain('open_transcribe_hotkey');
        expect(slots).not.toContain('hotkey');
        expect(slots).not.toContain('record_only_hotkey');
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

describe('hotkeyConflictWarning', () => {
    it('flags F1 and F12 with actionable copy', () => {
        expect(hotkeyConflictWarning('F1')).toContain('帮助');
        expect(hotkeyConflictWarning('F12')).toContain('开发者工具');
    });

    it('returns null for other F-keys and letters', () => {
        expect(hotkeyConflictWarning('F5')).toBeNull();
        expect(hotkeyConflictWarning('B')).toBeNull();
        expect(hotkeyConflictWarning('')).toBeNull();
    });
});

describe('apiUrlWarningText', () => {
    const PERSISTENCE =
        '检测到地址中嵌有密钥参数，建议将密钥填入下方「API 密钥」字段，并从地址中移除密钥参数。';
    const PERSISTENCE_AND_TRANSPORT =
        '检测到地址中嵌有密钥参数，建议将密钥填入下方「API 密钥」字段，并从地址中移除密钥参数。该地址使用 http 明文连接，密钥将以明文形式在网络上传输，可能被同一网络中的设备截获，建议改用 https。';

    it('null without credential params, regardless of scheme/host', () => {
        expect(apiUrlWarningText('http://192.168.1.5:1234/v1')).toBeNull();
        expect(apiUrlWarningText('https://h.com/v1?model=gpt-4o')).toBeNull();
        expect(apiUrlWarningText('')).toBeNull();
    });

    it('persistence advice only for https + credential param', () => {
        expect(apiUrlWarningText('https://h.com/v1?key=sk-1')).toBe(
            PERSISTENCE,
        );
    });

    it('persistence advice only for scheme-less + credential param', () => {
        expect(apiUrlWarningText('h.com/v1?key=sk-1')).toBe(PERSISTENCE);
    });

    it('appends transport warning for http + non-loopback + credential param', () => {
        expect(apiUrlWarningText('http://192.168.1.5:1234/v1?key=sk-1')).toBe(
            PERSISTENCE_AND_TRANSPORT,
        );
    });

    it('transport warning fires case-insensitively (HTTP://, mixed-case host, port, fragment param)', () => {
        expect(apiUrlWarningText('HTTP://Example.COM:8080/v1#api_key=x')).toBe(
            PERSISTENCE_AND_TRANSPORT,
        );
    });

    it('transport warning fires for non-loopback IPv6 literal with port', () => {
        expect(apiUrlWarningText('http://[2001:db8::1]:1234/v1?key=x')).toBe(
            PERSISTENCE_AND_TRANSPORT,
        );
    });

    it('loopback hosts are exempt from the transport warning', () => {
        for (const url of [
            'http://localhost:1234/v1?key=x',
            'http://127.0.0.1:1234/v1?key=x',
            'http://127.1.2.3:1234/v1?key=x',
            'http://[::1]:1234/v1?key=x',
            'HTTP://LOCALHOST:1234/v1?key=x',
        ]) {
            expect(apiUrlWarningText(url)).toBe(PERSISTENCE);
        }
    });

    // --- Extended matrix (userinfo + dedicated-key transport) ---

    const USERINFO =
        '检测到地址中嵌有用户名密码，建议从地址中移除，密钥改用下方「API 密钥」字段。';
    const TRANSPORT =
        '该地址使用 http 明文连接，密钥将以明文形式在网络上传输，可能被同一网络中的设备截获，建议改用 https。';

    it('userinfo-only https → just the userinfo message', () => {
        expect(apiUrlWarningText('https://user:pass@h.com/v1')).toBe(USERINFO);
    });

    it('userinfo + http non-loopback → userinfo + transport', () => {
        expect(apiUrlWarningText('http://user:pass@192.168.1.5:1234/v1')).toBe(
            USERINFO + TRANSPORT,
        );
    });

    it('query + userinfo + http non-loopback → all three segments', () => {
        expect(apiUrlWarningText('http://user@h.com/v1?key=sk-1')).toBe(
            PERSISTENCE + USERINFO + TRANSPORT,
        );
    });

    it('dedicated key + http non-loopback + clean URL → just transport', () => {
        expect(apiUrlWarningText('http://192.168.1.5:1234/v1', true)).toBe(
            TRANSPORT,
        );
    });

    it('dedicated key + http loopback → null (loopback exempts transport)', () => {
        expect(apiUrlWarningText('http://127.0.0.1:1234/v1', true)).toBeNull();
        expect(apiUrlWarningText('http://localhost:1234/v1', true)).toBeNull();
    });

    it('no key + http non-loopback + clean URL → null', () => {
        expect(apiUrlWarningText('http://192.168.1.5/v1', false)).toBeNull();
    });
});

describe('hasUserinfoInUrl', () => {
    // Shared fixture table — must match the table embedded in
    // `src-tauri/src/llm/mod.rs::test_url_contains_userinfo_fixtures` and
    // in `__tests__/userinfo-detection-contract.test.js`.
    const POSITIVE = [
        'http://user:pass@host/v1',
        'https://user@h.com/v1',
        'http://u:p@[2001:db8::1]:8080/v1',
        'http://user:pass@host',
        'https://u:p@h.com/v1?key=abc#frag',
    ];
    const NEGATIVE = [
        'http://host/v1',
        'http://host/v1?next=@x',
        'https://h.com/v1#frag@ment',
        'mailto:user@host',
        'http://[::1]:8080/v1',
    ];

    it('matches positive fixtures', () => {
        for (const url of POSITIVE) {
            expect(hasUserinfoInUrl(url), url).toBe(true);
        }
    });

    it('does not match negative fixtures', () => {
        for (const url of NEGATIVE) {
            expect(hasUserinfoInUrl(url), url).toBe(false);
        }
    });
});
