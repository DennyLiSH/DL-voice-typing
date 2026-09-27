/**
 * Contract test: the userinfo fixture table is embedded in both
 * `src-tauri/src/llm/mod.rs` (test_url_contains_userinfo_fixtures) and
 * the frontend `ui/lib/settings-utils.js::hasUserinfoInUrl`. Each URL
 * literal must appear in the backend source so the backend test cannot
 * silently drift from the frontend detector.
 *
 * Follows the text-scrape style of `credential-words-contract.test.js`
 * (which guards the CREDENTIAL_QUERY_WORDS list): reading the Rust
 * source as a string + `includes` lets the test fail on a renamed or
 * deleted fixture URL even when both Rust and JS tests stay green
 * individually.
 */
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { hasUserinfoInUrl } from '../ui/lib/settings-utils.js';

const RS = readFileSync(
    new URL('../src-tauri/src/llm/mod.rs', import.meta.url),
    'utf8',
);

// Shared fixture table — must equal the table embedded in
// `src-tauri/src/llm/mod.rs::test_url_contains_userinfo_fixtures` and
// in `__tests__/settings.test.js::hasUserinfoInUrl`.
const POSITIVE = [
    'http://user:pass@host/v1',
    'https://user@h.com/v1',
    'http://u:p@[2001:db8::1]:8080/v1',
    'http://user:pass@host',
    'https://u:p@h.com/v1?key=abc#frag',
];
const NEGATIVE = [
    'http://host/v1',
    'http://host/v1?next=@x',
    'https://h.com/v1#frag@ment',
    'mailto:user@host',
    'http://[::1]:8080/v1',
];

describe('userinfo-detection FFI contract', () => {
    it('every fixture URL appears as a string literal in llm/mod.rs', () => {
        for (const url of [...POSITIVE, ...NEGATIVE]) {
            expect(
                RS.includes(url),
                `fixture not found in llm/mod.rs: ${url}`,
            ).toBe(true);
        }
    });

    it('frontend hasUserinfoInUrl matches the shared fixture expectations', () => {
        for (const url of POSITIVE) {
            expect(hasUserinfoInUrl(url), `positive: ${url}`).toBe(true);
        }
        for (const url of NEGATIVE) {
            expect(hasUserinfoInUrl(url), `negative: ${url}`).toBe(false);
        }
    });
});
