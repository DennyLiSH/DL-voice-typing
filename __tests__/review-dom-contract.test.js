/**
 * Contract: the DOM ids the backend's embedded review-window JS touches
 * must exist in ui/review.html.
 *
 * The backend pushes the final transcription into the review window via
 * `WindowController::set_review_text` (Round 6 P7 — formerly
 * `eval_review_js` on DeliveryController); the Tauri impl embeds a JS
 * snippet that names the elements by id. If the frontend renames one, the
 * snippet silently does nothing — the window shows stale text and the
 * `eval` still "succeeded". The ids were shared knowledge with no
 * contract test.
 *
 * The `preview` element is a live example: it was removed from
 * review.html in 9832f59 ("review window cleanup") while the embedded JS
 * kept clearing it — harmless (the snippet guards every lookup with
 * `if (el)`) but dead, and invisible without this test.
 */
import { describe, expect, it } from 'vitest';

import { readSource } from './helpers.js';

const WINDOW_CONTROLLER = readSource(
    'src-tauri/src/commands/window_controller.rs',
);
const REVIEW_HTML = readSource('ui/review.html');

describe('review window DOM contract', () => {
    it('the embedded js only touches ids that exist in review.html', () => {
        const used = new Set(
            [...WINDOW_CONTROLLER.matchAll(/getElementById\('([^']+)'\)/g)].map(
                (m) => m[1],
            ),
        );
        // Check-face: the scrape must have found something (an empty set
        // would pass the containment assertion vacuously).
        expect(used.size).toBeGreaterThan(0);

        const available = new Set(
            [...REVIEW_HTML.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]),
        );
        expect(available.size).toBeGreaterThan(0);

        const missing = [...used].filter((id) => !available.has(id)).sort();
        expect(missing).toEqual([]);
    });
});
