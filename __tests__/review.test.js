import { describe, expect, it } from 'vitest';
import { formatInjectionError } from '../ui/review-utils.js';

describe('formatInjectionError', () => {
    it('renders payload with prefix when non-empty string', () => {
        expect(formatInjectionError('剪贴板被占用')).toBe(
            '粘贴失败：剪贴板被占用',
        );
    });

    it('renders default fallback when payload is empty string', () => {
        expect(formatInjectionError('')).toBe('粘贴失败，剪贴板被占用');
    });

    it('renders default fallback when payload is non-string', () => {
        expect(formatInjectionError(null)).toBe('粘贴失败，剪贴板被占用');
        expect(formatInjectionError(undefined)).toBe('粘贴失败，剪贴板被占用');
        expect(formatInjectionError({ msg: 'x' })).toBe(
            '粘贴失败，剪贴板被占用',
        );
        expect(formatInjectionError(42)).toBe('粘贴失败，剪贴板被占用');
    });

    it('truncates payload longer than 500 chars', () => {
        const long = 'x'.repeat(600);
        const result = formatInjectionError(long);
        expect(result.length).toBe('粘贴失败：'.length + 500);
        expect(result.startsWith('粘贴失败：')).toBe(true);
    });

    it('does not parse HTML in payload (textContent contract)', () => {
        const html = '<img src=x onerror=alert(1)>';
        const result = formatInjectionError(html);
        expect(result).toBe('粘贴失败：<img src=x onerror=alert(1)>');
    });
});
