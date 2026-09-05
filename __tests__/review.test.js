import { describe, expect, it } from 'vitest';
import { formatInjectionError } from '../ui/review-utils.js';

describe('formatInjectionError', () => {
    it('renders non-empty payload as-is (backend sends a complete user-facing sentence)', () => {
        expect(
            formatInjectionError('粘贴失败，本次文字未保存，原剪贴板已恢复'),
        ).toBe('粘贴失败，本次文字未保存，原剪贴板已恢复');
    });

    it('renders default fallback when payload is empty string', () => {
        expect(formatInjectionError('')).toBe('粘贴失败，本次文字未保存，原剪贴板已恢复');
    });

    it('renders default fallback when payload is non-string', () => {
        expect(formatInjectionError(null)).toBe('粘贴失败，本次文字未保存，原剪贴板已恢复');
        expect(formatInjectionError(undefined)).toBe('粘贴失败，本次文字未保存，原剪贴板已恢复');
        expect(formatInjectionError({ msg: 'x' })).toBe(
            '粘贴失败，本次文字未保存，原剪贴板已恢复',
        );
        expect(formatInjectionError(42)).toBe('粘贴失败，本次文字未保存，原剪贴板已恢复');
    });

    it('truncates payload longer than 500 chars', () => {
        const long = 'x'.repeat(600);
        const result = formatInjectionError(long);
        expect(result.length).toBe(500);
        expect(result.startsWith('x'.repeat(10))).toBe(true);
    });

    it('does not parse HTML in payload (textContent contract)', () => {
        const html = '<img src=x onerror=alert(1)>';
        const result = formatInjectionError(html);
        expect(result).toBe('<img src=x onerror=alert(1)>');
    });
});
