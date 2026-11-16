// @vitest-environment jsdom
//
// Unit tests for ui/lib/banner.js — the shared banner write primitive
// (Round 6 P5 Task 3). The three former call sites (settings error-banner,
// data page data-error-bar, transcribe list-error bar) delegate here; the
// single-source mechanics are textContent + variant class swap + the
// `hidden` attribute.
//
// The swap contract is the behavioral heart: showBanner owns variant
// correctness (removes the other variant class, adds the requested one);
// hideBanner intentionally does NO class cleanup — the next showBanner
// re-swaps. Visual recipes (colors) stay in CSS; only the class mechanics
// are pinned here.
import { beforeEach, describe, expect, it } from 'vitest';

import { hideBanner, showBanner } from '../ui/lib/banner.js';
import { loadFresh } from './helpers.js';

function buildBanner() {
    const el = document.createElement('div');
    el.className = 'banner banner-error';
    el.hidden = true;
    document.body.appendChild(el);
    return el;
}

beforeEach(() => {
    document.body.innerHTML = '';
});

describe('showBanner', () => {
    it('default variant adds banner-error and removes notice (swap)', () => {
        const el = buildBanner();
        el.classList.add('notice');
        showBanner(el, '加载失败');
        expect(el.classList.contains('banner-error')).toBe(true);
        expect(el.classList.contains('notice')).toBe(false);
    });

    it("variant='notice' adds notice and removes banner-error (swap the other way)", () => {
        const el = buildBanner(); // starts with banner-error in its class chain
        showBanner(el, '已撤销删除', { variant: 'notice' });
        expect(el.classList.contains('notice')).toBe(true);
        expect(el.classList.contains('banner-error')).toBe(false);
    });

    it('writes textContent and clears hidden', () => {
        const el = buildBanner();
        showBanner(el, '加载失败');
        expect(el.textContent).toBe('加载失败');
        expect(el.hidden).toBe(false);
    });

    it('null-safe: a missing element does not throw', () => {
        expect(() => showBanner(null, '加载失败')).not.toThrow();
        expect(() =>
            showBanner(null, 'msg', { variant: 'notice' }),
        ).not.toThrow();
    });

    it('unknown variant throws BEFORE any mutation (element untouched)', () => {
        // classList.add coerces undefined to the valid DOM token "undefined"
        // (it only throws on empty/whitespace tokens), so without the guard
        // a typo'd variant would silently render an unstyled banner.
        const el = buildBanner();
        el.textContent = 'keep me';
        expect(() => showBanner(el, 'boom', { variant: 'eror' })).toThrow(
            'banner: unknown variant "eror"',
        );
        expect(el.textContent).toBe('keep me');
        expect(el.classList.contains('banner-error')).toBe(true);
        expect(el.classList.contains('notice')).toBe(false);
        expect(el.hidden).toBe(true);
    });
});

describe('hideBanner', () => {
    it('sets hidden and does NO class cleanup (the single-source contract)', () => {
        const el = buildBanner();
        showBanner(el, '加载失败', { variant: 'notice' });
        expect(el.classList.contains('notice')).toBe(true);
        hideBanner(el);
        expect(el.hidden).toBe(true);
        // Dead variant class is left in place by design: the next showBanner
        // owns variant correctness. Do not "fix" this into a classList.remove.
        expect(el.classList.contains('notice')).toBe(true);
        expect(el.textContent).toBe('加载失败');
    });

    it('null-safe: a missing element does not throw', () => {
        expect(() => hideBanner(null)).not.toThrow();
    });
});

describe('swap sequence (the data-manager undo flow)', () => {
    it('notice → hide → error re-swaps correctly across the hide', () => {
        const el = buildBanner();
        showBanner(el, '已撤销删除，恢复 2 条。', { variant: 'notice' });
        hideBanner(el);
        showBanner(el, '恢复 1 条恢复失败');
        expect(el.hidden).toBe(false);
        expect(el.classList.contains('banner-error')).toBe(true);
        expect(el.classList.contains('notice')).toBe(false);
        expect(el.textContent).toBe('恢复 1 条恢复失败');
    });
});

describe('ui-utils delegation (API-stable signatures)', () => {
    it('showError/hideError keep their signatures and delegate to banner mechanics', async () => {
        document.body.innerHTML =
            '<div class="error-banner banner banner-error" id="error-banner" role="alert" hidden></div>';
        const { showError, hideError } = await loadFresh(
            '../ui/lib/ui-utils.js',
        );
        const banner = document.getElementById('error-banner');
        showError('加载配置失败');
        expect(banner.hidden).toBe(false);
        expect(banner.textContent).toBe('加载配置失败');
        expect(banner.classList.contains('banner-error')).toBe(true);
        hideError();
        expect(banner.hidden).toBe(true);
    });
});
