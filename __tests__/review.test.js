import { describe, expect, it } from 'vitest';
import { formatInjectionError } from '../ui/review-utils.js';

describe('formatInjectionError', () => {
    it('renders non-empty payload as-is (backend sends a complete user-facing sentence)', () => {
        expect(
            formatInjectionError('文本粘贴失败，原剪贴板内容已尝试恢复'),
        ).toBe('文本粘贴失败，原剪贴板内容已尝试恢复');
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
        expect(result.length).toBe(500);
        expect(result.startsWith('x'.repeat(10))).toBe(true);
    });

    it('does not parse HTML in payload (textContent contract)', () => {
        const html = '<img src=x onerror=alert(1)>';
        const result = formatInjectionError(html);
        expect(result).toBe('<img src=x onerror=alert(1)>');
    });
});
