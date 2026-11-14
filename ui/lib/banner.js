// Shared banner write primitive (settings error-banner / data page
// data-error-bar / transcribe list-error bar).
//
// Single source for the show/hide mechanics triple: textContent + variant
// class swap + the `hidden` attribute. The three windows previously
// hand-rolled these writes in three idioms (settings toggled a `.visible`
// class, data-manager added/removed `notice` ad hoc, transcribe wrote
// inline); the helper is now the only place that decides which variant class
// a banner carries, so the consumers cannot drift.
//
// Variant SWAP contract: showBanner owns variant correctness — it removes
// the other variant class before adding the requested one (error ↔
// `banner-error`, notice ↔ `notice`). hideBanner intentionally does NO class
// cleanup: a hidden banner's variant class is dead state, and the NEXT
// showBanner call re-swaps it. Do not "fix" hideBanner by adding a
// classList.remove — that reintroduces the scattered class writes this
// module exists to delete (pinned by banner.test.js).
//
// Visual recipe (colors/padding/wrapping) is NOT owned here: elements
// compose `banner banner-error` from common.css plus their own behavioral
// class; the helper only manages text, the variant class, and `hidden`.

const VARIANT_CLASSES = {
    error: 'banner-error',
    notice: 'notice',
};

/**
 * Show a banner element with a message (variant-correct class swap).
 *
 * @param {HTMLElement|null} el - banner element; null-safe (matching the
 *   old per-window `if (el)` guards).
 * @param {string} msg - text written to textContent.
 * @param {object} [opts]
 * @param {'error'|'notice'} [opts.variant='error'] - variant class the
 *   element must carry after the swap; an unknown variant throws (a
 *   programming error, not a runtime condition).
 */
export function showBanner(el, msg, { variant = 'error' } = {}) {
    if (!el) return;
    const target = VARIANT_CLASSES[variant];
    const other =
        variant === 'notice' ? VARIANT_CLASSES.error : VARIANT_CLASSES.notice;
    el.classList.remove(other);
    el.classList.add(target);
    el.textContent = msg;
    el.hidden = false;
}

/**
 * Hide a banner element (sets `hidden`; deliberately no class cleanup —
 * variant correctness is owned by the next showBanner's swap).
 *
 * @param {HTMLElement|null} el - banner element; null-safe.
 */
export function hideBanner(el) {
    if (!el) return;
    el.hidden = true;
}
