// ui/lib/recordings.js
//
// Shared recording-list helpers for the transcribe window and the settings
// data-management page: list_saved_recordings fetching with normalized
// errors, and the blob-URL audio element lifecycle (attach / release).
// Callers own their state containers and error display; this module owns
// the command invocation and the element-level resource cleanup.

import { call } from './api.js';

/**
 * Fetch a page of saved recordings. Errors are normalized to an Error with
 * a user-displayable Chinese message (raw string / CommandError / unknown
 * shapes all converge), so callers only need `e.message`. call()'s
 * reportError chain still forwards the ORIGINAL error to the backend log.
 *
 * @param {{offset: number, limit: number, query?: string|null}} args
 * @returns {Promise<{items: Array, total: number, total_bytes: number, offset: number}>}
 */
export async function loadRecordings({ offset, limit, query = null }) {
    try {
        return await call('list_saved_recordings', { offset, limit, query });
    } catch (e) {
        const msg = typeof e === 'string' ? e : e?.message || '加载失败';
        throw new Error(msg);
    }
}

/**
 * Attach WAV bytes to an audio element via a blob object URL. The URL is
 * tracked on `el.dataset.blobUrl` so releaseAudio() can revoke it later.
 *
 * @param {HTMLAudioElement|null} el
 * @param {Uint8Array|ArrayLike<number>} bytes
 */
export function attachAudio(el, bytes) {
    if (!el) return;
    const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    const blob = new Blob([u8], { type: 'audio/wav' });
    const url = URL.createObjectURL(blob);
    el.dataset.blobUrl = url;
    el.src = url;
}

/**
 * Release an audio element's resources: pause, revoke the tracked blob URL,
 * clear dataset + src. Null-safe and idempotent.
 *
 * @param {HTMLAudioElement|null} el
 */
export function releaseAudio(el) {
    if (!el) return;
    try {
        el.pause();
    } catch (_e) {
        /* jsdom / detached element */
    }
    if (el.dataset.blobUrl) {
        URL.revokeObjectURL(el.dataset.blobUrl);
        delete el.dataset.blobUrl;
    }
    el.removeAttribute('src');
}
