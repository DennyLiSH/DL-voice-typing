import { afterEach, describe, expect, it } from 'vitest';
import { isFormDirty, setFormDirty } from '../../ui/lib/form-state.js';

describe('form-state dirty flag', () => {
    afterEach(() => {
        setFormDirty(false);
    });

    it('defaults to not dirty', () => {
        expect(isFormDirty()).toBe(false);
    });

    it('setFormDirty round-trips', () => {
        setFormDirty(true);
        expect(isFormDirty()).toBe(true);
        setFormDirty(false);
        expect(isFormDirty()).toBe(false);
    });
});
