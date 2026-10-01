import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { CLEAR_MARKER, MASKED_MARKER } from '../ui/lib/api-key-mask.js';

describe('api-key-mask contract', () => {
    it('exports the marker expected by the backend', () => {
        expect(MASKED_MARKER).toBe('__MASKED__');
    });

    it('exports the CLEAR sentinel expected by the restore-defaults flow', () => {
        expect(CLEAR_MARKER).toBe('__CLEAR__');
    });

    it('markers stay in sync with the backend presentation constants', () => {
        const src = readFileSync(
            resolve(process.cwd(), 'src-tauri/src/config/presentation.rs'),
            'utf8',
        );
        expect(src).toContain('pub const MASKED_MARKER: &str = "__MASKED__";');
        expect(src).toContain('pub const CLEAR_MARKER: &str = "__CLEAR__";');
    });
});
