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

const VARIANT_CLASSES = new Map([
    ['error', 'banner-error'],
    ['notice', 'notice'],
]);

/**
 * Show a banner element with a message (variant-correct class swap).
 *
 * @param {HTMLElement|null} el - banner element; null-safe (matching the
 *   old per-window `if (el)` guards).
 * @param {string} msg - text written to textContent.
 * @param {object} [opts]
 * @param {'error'|'notice'} [opts.variant='error'] - variant class the
 *   element must carry after the swap; an unknown variant throws before any
 *   mutation (a programming error, not a runtime condition).
 */
export function showBanner(el, msg, { variant = 'error' } = {}) {
    if (!el) return;
    const target = VARIANT_CLASSES.get(variant);
    // Fail loud BEFORE any mutation. classList.add cannot be trusted to
    // throw here: it coerces undefined to the valid DOM token "undefined"
    // (DOMTokenList only rejects empty/whitespace tokens), so a typo'd
    // variant would silently render an unstyled banner. Map.get keeps
    // prototype keys ('constructor', 'toString') out of the lookup too.
    if (!target) throw new Error(`banner: unknown variant "${variant}"`);
    const other =
        variant === 'notice'
            ? VARIANT_CLASSES.get('error')
            : VARIANT_CLASSES.get('notice');
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
