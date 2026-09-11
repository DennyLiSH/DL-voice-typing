/**
 * Contract test: the frontend mirror constant `UNDO_WINDOW_SECS` must equal
 * the backend `pub(crate) const UNDO_WINDOW_SECS: u64 = N;` value in
 * `src-tauri/src/commands/data_management_cmd.rs`. If either side changes,
 * this test fails — the assertion is symmetric, so both sides must move
 * together. This guards against the three-write problem (backend sleep +
 * frontend countdown + confirm copy) where the literal was duplicated
 * without a single source.
 */
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { UNDO_WINDOW_SECS } from '../ui/lib/data-management.js';

const RS = readFileSync(
    new URL(
        '../src-tauri/src/commands/data_management_cmd.rs',
        import.meta.url,
    ),
    'utf8',
);

describe('undo-window FFI contract', () => {
    it('frontend mirror constant equals backend UNDO_WINDOW_SECS', () => {
        const m = RS.match(/pub\(crate\) const UNDO_WINDOW_SECS: u64 = (\d+);/);
        expect(
            m,
            'UNDO_WINDOW_SECS not found in data_management_cmd.rs',
        ).not.toBeNull();
        expect(UNDO_WINDOW_SECS).toBe(Number(m[1]));
    });
});
