// Shared progress-bar write primitive (settings model-download bar /
// transcribe transcription bar).
//
// Single source for the percent → DOM mechanics triple: clamping +
// aria-valuenow + scaleX transform. Both windows previously hand-rolled
// these writes (settings without a clamp, transcribe with one); the helper
// is now the only place that turns a percent number into the transform
// string and the ARIA value, so the two consumers cannot drift.
//
// Label TEXT is intentionally NOT unified here: settings' #progress-percent
// displays a bare percent ("42%") while transcribe's #progress-label
// displays stage text ("转录中 42%" / "LLM 纠错中…"). Call sites own the
// text format — pass `label` + `labelText` and the helper performs the
// guarded write. A label-only write with no percent mechanics (e.g.
// settings' cancel-failure text) does not belong here: write textContent
// directly at the call site.

/**
 * Write one progress step onto a progress bar.
 *
 * @param {object} parts - bar parts; every element is optional (null-safe,
 *   matching the old per-window `if (el)` guards).
 * @param {HTMLElement|null} parts.fill - fill element (inline transform target).
 * @param {HTMLElement|null} parts.wrap - the role="progressbar" element
 *   (aria-valuenow target).
 * @param {number} parts.percent - 0-100; clamped to [0, 100] before both
 *   writes (the transform string keeps the old `String(clamped / 100)`
 *   semantics, e.g. 42 → 'scaleX(0.42)').
 * @param {HTMLElement|null} [parts.label] - optional label element.
 * @param {string} [parts.labelText] - text to write into `label`; written
 *   only when both `label` and `labelText` are provided.
 */
export function setProgressBar({ fill, wrap, percent, label, labelText }) {
    const clamped = Math.min(100, Math.max(0, percent));
    if (fill) fill.style.transform = `scaleX(${clamped / 100})`;
    if (wrap) wrap.setAttribute('aria-valuenow', String(clamped));
    if (label && labelText !== undefined) label.textContent = labelText;
}
