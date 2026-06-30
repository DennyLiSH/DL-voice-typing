import { describe, expect, it } from 'vitest';
import { MASKED_MARKER } from '../ui/lib/api-key-mask.js';

describe('api-key-mask contract', () => {
    it('exports the marker expected by the backend', () => {
        expect(MASKED_MARKER).toBe('__MASKED__');
    });
});
