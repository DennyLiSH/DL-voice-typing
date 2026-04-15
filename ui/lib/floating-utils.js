// Pure functions extracted from floating.js for testability.
// These are copied verbatim from the original file — floating.js retains its own
// inline copies since the Tauri WebView has no module bundler.

/**
 * Color stops: [r, g, b, a] at visual RMS thresholds (after sqrt remap)
 * 青玉色 Jade Teal: #3ABAB4 -> dark teal to bright teal
 */
export const COLOR_STOPS = [
  { at: 0.0, color: [18, 40, 48, 0.82] },
  { at: 0.25, color: [30, 110, 120, 0.88] },
  { at: 0.6, color: [58, 186, 180, 0.92] },
  { at: 1.0, color: [130, 230, 220, 0.98] },
];

/**
 * Linear interpolation between two RGBA color arrays.
 * @param {number[]} a - Source color [r, g, b, a]
 * @param {number[]} b - Target color [r, g, b, a]
 * @param {number} t - Interpolation factor in [0, 1]
 * @returns {number[]} Interpolated color [r, g, b, a]
 */
export function lerpColor(a, b, t) {
  return [
    a[0] + (b[0] - a[0]) * t,
    a[1] + (b[1] - a[1]) * t,
    a[2] + (b[2] - a[2]) * t,
    a[3] + (b[3] - a[3]) * t,
  ];
}

/**
 * Map raw RMS value through sqrt for better visual responsiveness.
 * @param {number} rms - Raw RMS value (expected 0..1, but not clamped here)
 * @returns {number} Remapped value = rms^0.5
 */
export function remapRms(rms) {
  return Math.pow(rms, 0.5);
}

/**
 * Get interpolated color for a given visual RMS value using COLOR_STOPS.
 * @param {number} visualRms - Visual RMS in [0, 1]
 * @returns {number[]} RGBA color array
 */
export function getColor(visualRms) {
  if (visualRms <= COLOR_STOPS[0].at) return COLOR_STOPS[0].color;
  if (visualRms >= COLOR_STOPS[COLOR_STOPS.length - 1].at)
    return COLOR_STOPS[COLOR_STOPS.length - 1].color;
  for (let i = 0; i < COLOR_STOPS.length - 1; i++) {
    if (
      visualRms >= COLOR_STOPS[i].at &&
      visualRms <= COLOR_STOPS[i + 1].at
    ) {
      const t =
        (visualRms - COLOR_STOPS[i].at) /
        (COLOR_STOPS[i + 1].at - COLOR_STOPS[i].at);
      return lerpColor(COLOR_STOPS[i].color, COLOR_STOPS[i + 1].color, t);
    }
  }
  return COLOR_STOPS[0].color;
}

/**
 * Generate CSS box-shadow string based on visual RMS level.
 * Includes an additional glow layer when visualRms > 0.35.
 * @param {number} visualRms - Visual RMS in [0, 1]
 * @returns {string} CSS box-shadow value
 */
export function getShadow(visualRms) {
  const r = Math.round(30 + 28 * visualRms);
  const g = Math.round(120 + 66 * visualRms);
  const b = Math.round(140 + 40 * visualRms);
  const alpha = (0.2 + visualRms * 0.15).toFixed(2);
  const spread = 18 + visualRms * 8;
  let shadow = `0 4px ${Math.round(spread)}px rgba(${r},${g},${b},${alpha})`;
  if (visualRms > 0.35) {
    const glowAlpha = ((visualRms - 0.35) * 0.25).toFixed(2);
    shadow += `, 0 0 ${Math.round(22 + visualRms * 15)}px rgba(58,186,180,${glowAlpha})`;
  }
  return shadow;
}
