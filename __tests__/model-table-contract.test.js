/**
 * Contract: frontend MODELS (ui/lib/transcribe-models.js) ↔ backend
 * BUILT_IN_MODELS serde_keys (src-tauri/src/config/schema.rs). The
 * frontend table is the display/source authority; the Rust table is the
 * runtime authority. The two must hold the same id set or the settings
 * model dropdown advertises models the backend cannot resolve.
 */
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { MODELS } from '../ui/lib/transcribe-models.js';

const SCHEMA_RS = readFileSync(
    new URL('../src-tauri/src/config/schema.rs', import.meta.url),
    'utf8',
);

function backendKeys() {
    const block = SCHEMA_RS.match(
        /const BUILT_IN_MODELS:\s*&\[ModelMeta\]\s*=\s*&\[([\s\S]*?)\];/,
    );
    if (!block) throw new Error('BUILT_IN_MODELS not found in schema.rs');
    const keys = [...block[1].matchAll(/serde_key:\s*"([^"]+)"/g)].map(
        (m) => m[1],
    );
    if (keys.length === 0)
        throw new Error('scrape failed: BUILT_IN_MODELS empty');
    return keys.slice().sort();
}

describe('model table contract', () => {
    it('frontend MODELS ids ≡ BUILT_IN_MODELS serde_keys (both directions)', () => {
        const frontendIds = MODELS.map((m) => m.id)
            .slice()
            .sort();
        expect(frontendIds).toEqual(backendKeys());
    });

    it('MODELS ids are unique', () => {
        const ids = MODELS.map((m) => m.id);
        expect(new Set(ids).size).toBe(ids.length);
    });
});
