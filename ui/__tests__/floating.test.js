import { describe, it, expect } from 'vitest';
import { lerpColor, remapRms, getColor, getShadow, COLOR_STOPS } from '../lib/floating-utils.js';

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
    const expected = lerpColor(COLOR_STOPS[0].color, COLOR_STOPS[1].color, 0.5);
    expect(result[0]).toBeCloseTo(expected[0], 10);
    expect(result[1]).toBeCloseTo(expected[1], 10);
    expect(result[2]).toBeCloseTo(expected[2], 10);
    expect(result[3]).toBeCloseTo(expected[3], 10);
  });

  it('interpolates between middle stops at 0.425', () => {
    const result = getColor(0.425);
    // Between stop 1 (at=0.25) and stop 2 (at=0.6)
    const t = (0.425 - 0.25) / (0.6 - 0.25);
    const expected = lerpColor(COLOR_STOPS[1].color, COLOR_STOPS[2].color, t);
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
      return m ? parseInt(m[1], 10) : -1;
    };
    expect(spreadOf(hi)).toBeGreaterThan(spreadOf(lo));
  });

  it('computes correct rgba values at visualRms = 0', () => {
    const result = getShadow(0);
    // r=30, g=120, b=140, alpha=(0.2).toFixed(2)='0.20', spread=18
    expect(result).toContain('rgba(30,120,140,0.20)');
    expect(result).toContain('18px');
  });

  it('computes correct rgba values at visualRms = 1', () => {
    const result = getShadow(1);
    // r=58, g=186, b=180, alpha=(0.35).toFixed(2)='0.35', spread=26
    expect(result).toContain('rgba(58,186,180,0.35)');
    expect(result).toContain('26px');
    // Glow: glowAlpha = (1-0.35)*0.25 = 0.1625 → '0.16', blur = 22+15 = 37
    expect(result).toContain('rgba(58,186,180,0.16)');
    expect(result).toContain('37px');
  });

  it('glow blur radius increases with visualRms above threshold', () => {
    const lo = getShadow(0.5);
    const hi = getShadow(0.9);
    const glowBlurOf = (s) => {
      // Match the glow part: "0 0 NNpx rgba(58,186,180,...)"
      const m = s.match(/0 0 (\d+)px rgba\(58,186,180/);
      return m ? parseInt(m[1], 10) : -1;
    };
    expect(glowBlurOf(hi)).toBeGreaterThan(glowBlurOf(lo));
  });
});
