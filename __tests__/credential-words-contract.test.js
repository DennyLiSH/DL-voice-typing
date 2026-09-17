/**
 * Contract test: backend `CREDENTIAL_QUERY_WORDS` (token-matching redaction
 * list in src-tauri/src/llm/mod.rs) and frontend `CREDENTIAL_WORDS` (warn
 * list in ui/lib/settings-utils.js) must contain the same words. The two
 * lists exist because the backend scrubs log output while the frontend
 * warns at save time — drift on either side silently narrows one of the
 * two defenses.
 */
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { CREDENTIAL_WORDS } from '../ui/lib/settings-utils.js';

const RS = readFileSync(
    new URL('../src-tauri/src/llm/mod.rs', import.meta.url),
    'utf8',
);

function backendWords() {
    const m = RS.match(
        /const CREDENTIAL_QUERY_WORDS: \[&str; (\d+)\] = \[([\s\S]*?)\];/,
    );
    expect(m, 'CREDENTIAL_QUERY_WORDS not found in llm/mod.rs').not.toBeNull();
    const words = [...m[2].matchAll(/"([a-z_]+)"/g)].map((x) => x[1]);
    expect(words.length, 'declared array length must match scraped words')
        .toBe(Number(m[1]));
    return new Set(words);
}

describe('credential-words FFI contract', () => {
    it('frontend CREDENTIAL_WORDS equals backend CREDENTIAL_QUERY_WORDS', () => {
        const backend = backendWords();
        expect(CREDENTIAL_WORDS.size).toBe(backend.size);
        for (const word of CREDENTIAL_WORDS) {
            expect(backend.has(word), `missing in backend: ${word}`).toBe(true);
        }
        for (const word of backend) {
            expect(CREDENTIAL_WORDS.has(word), `missing in frontend: ${word}`)
                .toBe(true);
        }
    });
});