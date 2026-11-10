/**
 * Behaviour tests for the API-key reset channel.
 *
 * The channel owns the wipe intent (pending flag + snapshot channel) and
 * the sentinel discrimination. The DOM-side behavior is covered by
 * `settings.form-behavior.test.js` (through the real form); these tests
 * pin the state machine itself.
 */
import { describe, expect, it } from 'vitest';

import { createApiKeyChannel } from '../ui/lib/api-key-channel.js';
import { CLEAR_MARKER, MASKED_MARKER } from '../ui/lib/api-key-mask.js';

describe('api key channel', () => {
    it('keeps the stored key when the input is empty and nothing is pending', () => {
        const channel = createApiKeyChannel();
        expect(channel.getForSave('', { llm_api_key: MASKED_MARKER })).toBe(
            MASKED_MARKER,
        );
        expect(channel.getForSave('', {})).toBe('');
    });

    it('sends what the user typed', () => {
        const channel = createApiKeyChannel();
        expect(
            channel.getForSave('sk-typed', { llm_api_key: MASKED_MARKER }),
        ).toBe('sk-typed');
    });

    it('neutralizes a hand-typed sentinel', () => {
        // A sentinel typed by hand is indistinguishable from the masked
        // roundtrip echo at the validate layer — it must degrade to the
        // empty-input semantics (keep the stored key), never reach the
        // backend as a sentinel.
        const channel = createApiKeyChannel();
        expect(channel.getForSave(CLEAR_MARKER, {})).toBe('');
        expect(channel.getForSave(MASKED_MARKER, {})).toBe('');
    });

    it('sends the CLEAR sentinel once the wipe intent is armed', () => {
        const channel = createApiKeyChannel();
        channel.armReset(true);
        expect(channel.isResetPending()).toBe(true);
        expect(channel.getForSave('', { llm_api_key: MASKED_MARKER })).toBe(
            CLEAR_MARKER,
        );
    });

    it('does not wipe a key that never existed', () => {
        const channel = createApiKeyChannel();
        channel.armReset(false);
        expect(channel.getForSave('', { llm_api_key: '' })).toBe('');
    });

    it('disarm clears the intent and the snapshot', () => {
        const channel = createApiKeyChannel();
        channel.armReset(true);
        channel.disarm();
        expect(channel.isResetPending()).toBe(false);
        expect(channel.getForSave('', { llm_api_key: '' })).toBe('');
    });

    it('cancelling the pending intent keeps the snapshot as defense-in-depth', () => {
        const channel = createApiKeyChannel();
        channel.armReset(true);
        channel.cancelPending();
        expect(channel.isResetPending()).toBe(false);
        // The snapshot channel survives: with a masked baseline the
        // intent would still be visible through the snapshot.
        expect(channel.getForSave('', { llm_api_key: MASKED_MARKER })).toBe(
            MASKED_MARKER,
        );
    });
});
