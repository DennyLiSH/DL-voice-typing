import { describe, expect, it } from 'vitest';
import {
    COLOR_STOPS,
    ERROR_GUIDE,
    errorDisplayText,
    errorTextWithGuide,
    getColor,
    getShadow,
    lerpColor,
    remapRecordOnlyRms,
    remapRms,
} from '../ui/floating-utils.js';

// ---------------------------------------------------------------------------
// lerpColor
// ---------------------------------------------------------------------------
describe('lerpColor', () => {
    it('returns the first color when t = 0', () => {
        const a = [10, 20, 30, 0.5];
        const b = [100, 200, 300, 1.0];
        expect(lerpColor(a, b, 0)).toEqual(a);
    });

    it('returns the second color when t = 1', () => {
        const a = [10, 20, 30, 0.5];
        const b = [100, 200, 300, 1.0];
        expect(lerpColor(a, b, 1)).toEqual(b);
    });

    it('returns the exact midpoint when t = 0.5', () => {
        const a = [0, 0, 0, 0.0];
        const b = [100, 200, 300, 1.0];
        const result = lerpColor(a, b, 0.5);
        expect(result).toEqual([50, 100, 150, 0.5]);
    });

    it('interpolates all four channels linearly', () => {
        const a = [18, 40, 48, 0.82];
        const b = [30, 110, 120, 0.88];
        const t = 0.25;
        const result = lerpColor(a, b, t);
        // Verify channel-by-channel: a[i] + (b[i] - a[i]) * t
        for (let i = 0; i < 4; i++) {
            expect(result[i]).toBeCloseTo(a[i] + (b[i] - a[i]) * t, 10);
        }
    });

    it('works with identical colors (no-op)', () => {
        const c = [58, 186, 180, 0.92];
        expect(lerpColor(c, c, 0.73)).toEqual(c);
    });
});

// ---------------------------------------------------------------------------
// remapRms
// ---------------------------------------------------------------------------
describe('remapRms', () => {
    it('returns 0 for rms = 0', () => {
        expect(remapRms(0)).toBe(0);
    });

    it('returns 1 for rms = 1', () => {
        expect(remapRms(1)).toBeCloseTo(1, 10);
    });

    it('applies sqrt: remapRms(0.25) = 0.5', () => {
        expect(remapRms(0.25)).toBeCloseTo(0.5, 10);
    });

    it('applies sqrt: remapRms(0.04) = 0.2', () => {
        expect(remapRms(0.04)).toBeCloseTo(0.2, 10);
    });

    it('produces higher values than input for 0 < rms < 1 (sqrt compresses)', () => {
        for (const rms of [0.01, 0.1, 0.3, 0.5, 0.8, 0.99]) {
            expect(remapRms(rms)).toBeGreaterThan(rms);
        }
    });
});

// ---------------------------------------------------------------------------
// remapRecordOnlyRms (盲录电平增强 2026-09-29)
// ---------------------------------------------------------------------------
describe('remapRecordOnlyRms (盲录电平增强 2026-09-29)', () => {
    it('normal speech RMS spans most of [0,1] after knee normalization', () => {
        // Field data (v26.9.5 FAIL): speech ~0.01, silence ~0.001.
        // The old shared sqrt remap gave 0.1 vs 0.032 (brightness swing 6%,
        // measured 0.4% on screen). The knee'd curve must separate them hard.
        expect(remapRecordOnlyRms(0.01)).toBeCloseTo(Math.sqrt(0.5), 5); // 0.707
        expect(remapRecordOnlyRms(0.001)).toBeCloseTo(Math.sqrt(0.05), 5); // 0.224
    });

    it('clamps at the knee: louder-than-knee input saturates at 1', () => {
        expect(remapRecordOnlyRms(0.02)).toBe(1);
        expect(remapRecordOnlyRms(0.5)).toBe(1);
    });

    it('regression guard: field speech/silence pair yields a wide spread', () => {
        // The v26.9.5 failure signature was a spread of ~0.08 in visualRms
        // (brightness 1.0→1.06). The recalibrated curve must keep the
        // speech-vs-silence visualRms spread above 0.4 for the same pair.
        const spread = remapRecordOnlyRms(0.01) - remapRecordOnlyRms(0.001);
        expect(spread).toBeGreaterThan(0.4);
    });

    it('zero and negative input map to 0', () => {
        expect(remapRecordOnlyRms(0)).toBe(0);
        expect(remapRecordOnlyRms(-0.01)).toBe(0);
    });
});

// ---------------------------------------------------------------------------
// getColor
// ---------------------------------------------------------------------------
describe('getColor', () => {
    it('returns first stop color when visualRms = 0', () => {
        const result = getColor(0);
        expect(result).toEqual(COLOR_STOPS[0].color);
    });

    it('returns first stop color when visualRms is negative', () => {
        const result = getColor(-0.5);
        expect(result).toEqual(COLOR_STOPS[0].color);
    });

    it('returns last stop color when visualRms = 1', () => {
        const result = getColor(1);
        expect(result).toEqual(COLOR_STOPS[COLOR_STOPS.length - 1].color);
    });

    it('returns last stop color when visualRms > 1', () => {
        const result = getColor(2.0);
        expect(result).toEqual(COLOR_STOPS[COLOR_STOPS.length - 1].color);
    });

    it('returns exact stop color at boundary 0.25', () => {
        const result = getColor(0.25);
        expect(result).toEqual(COLOR_STOPS[1].color);
    });

    it('returns exact stop color at boundary 0.6', () => {
        const result = getColor(0.6);
        expect(result).toEqual(COLOR_STOPS[2].color);
    });

    it('interpolates between first two stops at midpoint (0.125)', () => {
        const result = getColor(0.125);
        // Midpoint between stop 0 (at=0.0) and stop 1 (at=0.25)
        const expected = lerpColor(
            COLOR_STOPS[0].color,
            COLOR_STOPS[1].color,
            0.5,
        );
        expect(result[0]).toBeCloseTo(expected[0], 10);
        expect(result[1]).toBeCloseTo(expected[1], 10);
        expect(result[2]).toBeCloseTo(expected[2], 10);
        expect(result[3]).toBeCloseTo(expected[3], 10);
    });

    it('interpolates between middle stops at 0.425', () => {
        const result = getColor(0.425);
        // Between stop 1 (at=0.25) and stop 2 (at=0.6)
        const t = (0.425 - 0.25) / (0.6 - 0.25);
        const expected = lerpColor(
            COLOR_STOPS[1].color,
            COLOR_STOPS[2].color,
            t,
        );
        expect(result[0]).toBeCloseTo(expected[0], 10);
        expect(result[1]).toBeCloseTo(expected[1], 10);
        expect(result[2]).toBeCloseTo(expected[2], 10);
        expect(result[3]).toBeCloseTo(expected[3], 10);
    });

    it('produces monotonically increasing red channel from 0 to 1', () => {
        const steps = 20;
        const reds = [];
        for (let i = 0; i <= steps; i++) {
            const c = getColor(i / steps);
            reds.push(c[0]);
        }
        for (let i = 1; i < reds.length; i++) {
            expect(reds[i]).toBeGreaterThanOrEqual(reds[i - 1]);
        }
    });
});

// ---------------------------------------------------------------------------
// getShadow
// ---------------------------------------------------------------------------
describe('getShadow', () => {
    it('returns a string starting with base shadow format', () => {
        const result = getShadow(0);
        expect(result).toMatch(/^0 4px \d+px rgba\(\d+,\d+,\d+,[\d.]+\)$/);
    });

    it('does not include glow at visualRms = 0.35 (threshold, no glow)', () => {
        const result = getShadow(0.35);
        expect(result).not.toContain(', 0 0');
    });

    it('includes glow layer when visualRms > 0.35', () => {
        const result = getShadow(0.5);
        expect(result).toContain('rgba(58,186,180,');
        // Two shadows separated by comma
        const parts = result.split('),');
        expect(parts.length).toBe(2);
    });

    it('increases spread as visualRms increases', () => {
        const lo = getShadow(0);
        const hi = getShadow(1);
        // Extract the first spread value (number before "px rgba")
        const spreadOf = (s) => {
            const m = s.match(/0 4px (\d+)px/);
            return m ? Number.parseInt(m[1], 10) : -1;
        };
        expect(spreadOf(hi)).toBeGreaterThan(spreadOf(lo));
    });

    it('computes correct rgba values at visualRms = 0', () => {
        const result = getShadow(0);
        // r=30, g=120, b=140, alpha=(0.15).toFixed(2)='0.15', spread=18
        expect(result).toContain('rgba(30,120,140,0.15)');
        expect(result).toContain('18px');
    });

    it('computes correct rgba values at visualRms = 1', () => {
        const result = getShadow(1);
        // r=58, g=186, b=180, alpha=(0.15+0.12)='0.27', spread=26
        expect(result).toContain('rgba(58,186,180,0.27)');
        expect(result).toContain('26px');
        // Glow: glowAlpha = (1-0.35)*0.2 = 0.13 → '0.13', blur = 22+15 = 37
        expect(result).toContain('rgba(58,186,180,0.13)');
        expect(result).toContain('37px');
    });

    it('glow blur radius increases with visualRms above threshold', () => {
        const lo = getShadow(0.5);
        const hi = getShadow(0.9);
        const glowBlurOf = (s) => {
            // Match the glow part: "0 0 NNpx rgba(58,186,180,...)"
            const m = s.match(/0 0 (\d+)px rgba\(58,186,180/);
            return m ? Number.parseInt(m[1], 10) : -1;
        };
        expect(glowBlurOf(hi)).toBeGreaterThan(glowBlurOf(lo));
    });
});

// ---------------------------------------------------------------------------
// errorDisplayText
// ---------------------------------------------------------------------------
describe('errorDisplayText', () => {
    it('returns the payload string when non-empty', () => {
        expect(errorDisplayText('模型加载中，请稍候...', '默认')).toBe(
            '模型加载中，请稍候...',
        );
    });
    it('returns the fallback for non-string payloads (serialized AppError object)', () => {
        expect(errorDisplayText({ code: 'X', message: 'y' }, '默认')).toBe(
            '默认',
        );
    });
    it('returns the fallback for empty or whitespace strings', () => {
        expect(errorDisplayText('', '默认')).toBe('默认');
        expect(errorDisplayText('   ', '默认')).toBe('默认');
    });
    it('returns the fallback for null/undefined', () => {
        expect(errorDisplayText(null, '默认')).toBe('默认');
        expect(errorDisplayText(undefined, '默认')).toBe('默认');
    });
    it('truncates payloads longer than 40 chars (default maxLen) with …', () => {
        const long = 'x'.repeat(60);
        const out = errorDisplayText(long, '默认');
        expect(out.length).toBe(41);
        expect(out.endsWith('…')).toBe(true);
        expect(out.startsWith('x'.repeat(40))).toBe(true);
    });
    it('keeps a 40-char payload untruncated', () => {
        expect(errorDisplayText('y'.repeat(40), '默认')).toBe('y'.repeat(40));
    });
    it('never truncates the fallback (capping applies to the payload only)', () => {
        const longFallback = '兜'.repeat(60);
        expect(errorDisplayText('', longFallback)).toBe(longFallback);
    });
});

describe('errorTextWithGuide', () => {
    it('appends the help-page guide to a short fallback', () => {
        expect(errorTextWithGuide(undefined, '语音识别失败')).toBe(
            `语音识别失败${ERROR_GUIDE}`,
        );
    });

    it('tightens the payload cap to 24 chars so base + guide fits the 40-char area', () => {
        const long = 'a'.repeat(30);
        const out = errorTextWithGuide(long, 'fallback');
        expect(out.endsWith(ERROR_GUIDE)).toBe(true);
        expect(out.length).toBeLessThanOrEqual(40);
        // The payload itself was capped at 24 before the guide was appended.
        expect(out.startsWith('a'.repeat(24))).toBe(true);
    });

    it('guide is 14 chars and does not double-count', () => {
        expect([...ERROR_GUIDE].length).toBe(14);
        expect(ERROR_GUIDE).toBe(' · 详情见 帮助→最近错误');
    });
});
