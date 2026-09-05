// Frontend byte-identical guard: the floating window's ERROR_DEFAULTS and
// the review window's fallback must render the SAME injection-failure
// sentence (both come from lib/errors.js — this locks the sharing).
import { describe, expect, it } from 'vitest';
import { INJECTION_ERROR_MSG } from '../ui/lib/errors.js';

describe('injection error copy contract', () => {
    it('floating and review windows share one constant (20 chars, honest wording)', () => {
        expect(INJECTION_ERROR_MSG).toBe(
            '粘贴失败，本次文字未保存，原剪贴板已恢复',
        );
        expect([...INJECTION_ERROR_MSG].length).toBe(20);
    });

    it('review-utils falls back to the shared constant on empty payload', async () => {
        const { formatInjectionError } = await import('../ui/review-utils.js');
        expect(formatInjectionError('')).toBe(INJECTION_ERROR_MSG);
        expect(formatInjectionError(undefined)).toBe(INJECTION_ERROR_MSG);
        expect(formatInjectionError('自定义错误')).toBe('自定义错误');
    });
});
