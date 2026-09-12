// Shared recordings-list controller: the ONE place that knows how a
// recording list handles keyboard focus (roving tabindex), stale audio
// fetches (identity guard), and audio-failure state. The transcribe
// window and the settings data page both drive it; row DOM stays with
// the callers (each window shapes rows differently).
//
// Design-review invariants baked in (2026-09-11):
//   - Enter/Space activate only the row itself; interactive children
//     keep their native key handling (DR-2.2).
//   - The tab stop is the LAST-FOCUSED row (focusin), falling back to
//     the first row after a re-render (DR-2.3).
//   - Stale audio responses never attach: element identity + DOM
//     connection + caller staleness predicate (DR-1.2).

import { attachAudio, releaseAudio } from './recordings.js';

/**
 * Roving-tabindex keyboard model.
 *
 * @param {Object} opts
 * @param {HTMLElement} opts.container - the list element (rows live inside)
 * @param {string} opts.rowSelector - e.g. '.rec-row' / '.data-row'
 * @param {Function} opts.onActivate - called with the row's filename on Enter/Space
 * @returns {{sync: Function, destroy: Function}}
 *   sync(): re-apply tabindex after a re-render.
 *   destroy(): remove listeners (page teardown).
 */
export function createRovingList({ container, rowSelector, onActivate }) {
    let lastFocusedFilename = null;

    function rows() {
        return Array.from(container.querySelectorAll(rowSelector));
    }

    function onFocusIn(e) {
        const row = e.target.closest?.(rowSelector);
        if (row?.dataset?.filename) lastFocusedFilename = row.dataset.filename;
    }

    function onKeydown(e) {
        const row = e.target.closest?.(rowSelector);
        if (!row) return;
        if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
            e.preventDefault();
            const all = rows();
            if (all.length === 0) return;
            const idx = all.indexOf(row);
            const next =
                e.key === 'ArrowDown'
                    ? (idx + 1) % all.length
                    : (idx - 1 + all.length) % all.length;
            all[next].focus();
        } else if (e.key === 'Enter' || e.key === ' ') {
            if (row !== e.target) return; // interactive child keeps native keys
            e.preventDefault();
            const filename = row.dataset.filename;
            if (filename) onActivate(filename);
            // Activation usually re-renders the list (expand toggle,
            // selection), destroying `row` and dropping focus to <body>.
            // Restore it to the replacement row so the keyboard flow
            // survives the rebuild — but only when focus was actually lost:
            // a caller that deliberately moved focus (dialog, input) keeps
            // it (P3 E2E 2026-09-12).
            if (document.activeElement === document.body) {
                const fresh = rows().find(
                    (r) => r.dataset.filename === filename,
                );
                if (fresh) fresh.focus();
            }
        }
    }

    container.addEventListener('focusin', onFocusIn);
    container.addEventListener('keydown', onKeydown);

    return {
        sync() {
            const all = rows();
            const stop =
                all.find((r) => r.dataset.filename === lastFocusedFilename) ??
                all[0];
            for (const r of all) r.tabIndex = r === stop ? 0 : -1;
        },
        destroy() {
            container.removeEventListener('focusin', onFocusIn);
            container.removeEventListener('keydown', onKeydown);
            lastFocusedFilename = null;
        },
    };
}

/**
 * At-most-one audio attachment with stale-fetch guard + failure tracking.
 *
 * @param {Object} opts
 * @param {Function} opts.fetchBytes - async (filename) => Uint8Array
 * @param {Function} [opts.onFail] - called with filename on load failure
 *   AND the failure is swallowed. When omitted the error rethrows (the
 *   transcribe window folds audio failure into its combined catch/toast).
 * @returns {{bind: Function, release: Function, failedFiles: Set<string>}}
 */
export function createAudioSlot({ fetchBytes, onFail }) {
    const failedFiles = new Set();
    let element = null;

    /**
     * @param {HTMLAudioElement} el
     * @param {string} filename
     * @param {Object} [guards]
     * @param {Function} [guards.isStale] - caller-owned staleness predicate
     *   (e.g. selection changed while loading); evaluated BEFORE the fetch
     *   (a stale-at-call bind skips the invoke entirely) and AGAIN after
     *   the fetch resolves. The pre-check is what makes the data page's
     *   failure loop terminate at one fetch per user action: onFail →
     *   onAudioError → releaseAudio clears audioPlayerRowId → the next
     *   render's bind is stale at call time (审查 S5-F1/S6-F1).
     */
    async function bind(el, filename, { isStale = () => false } = {}) {
        element = el;
        if (isStale()) return; // stale at call: no fetch, no failure mark
        try {
            const bytes = await fetchBytes(filename);
            if (element !== el || !el.isConnected || isStale()) return;
            failedFiles.delete(filename);
            attachAudio(el, bytes);
        } catch (e) {
            failedFiles.add(filename);
            if (onFail) {
                onFail(filename);
                return;
            }
            throw e;
        }
    }

    function release() {
        releaseAudio(element);
        element = null;
    }

    return { bind, release, failedFiles };
}
