import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const read = (p) => readFileSync(new URL(p, import.meta.url)).toString('utf-8');

// Extract one closed rule block (selector up to the first '}') so assertions
// cannot be satisfied by unrelated rules later in the file (same helper
// convention as ui-progress-contract.test.js).
const blockOf = (css, selector) => {
    const s = css.indexOf(selector);
    if (s === -1) throw new Error(`selector not found: ${selector}`);
    return css.slice(s, css.indexOf('}', s) + 1);
};

describe('button geometry uniformity', () => {
    it('.btn-danger carries no custom geometry (relies on button base)', () => {
        const css = read('../ui/common.css');
        const block = blockOf(css, '.btn-danger {');
        expect(block).not.toMatch(/padding|border-radius|cursor/);
    });

    it('.btn-danger joins the shared disabled token group', () => {
        const css = read('../ui/common.css');
        const block = blockOf(css, '.btn-primary:disabled,');
        expect(block).toContain('.btn-danger:disabled');
        expect(block).toContain('var(--disabled-bg)');
    });

    it('base button:disabled rule lives in common.css, not settings.css', () => {
        expect(read('../ui/common.css')).toMatch(
            /button:disabled\s*\{\s*cursor: not-allowed;/,
        );
        expect(read('../ui/settings.css')).not.toMatch(
            /^button:disabled\s*\{/m,
        );
    });

    it('download button composes .btn-primary (html + js swap sites)', () => {
        expect(read('../ui/settings.html')).toContain(
            'class="btn-primary btn-download-model"',
        );
        const js = read('../ui/model-manager.js');
        expect(js).toContain(`'btn-primary btn-download-model'`);
        expect(js).toContain(`'btn-primary btn-download-model btn-danger'`);
    });

    it('.btn-download-model is a layout-only modifier (no color/geometry)', () => {
        const block = blockOf(
            read('../ui/settings.css'),
            '.btn-download-model {',
        );
        expect(block).not.toMatch(
            /background|border:|height|padding|font-size|transition|box-shadow/,
        );
        expect(block).toContain('white-space: nowrap');
    });

    it('review window buttons inherit base geometry (no padding, no 0.96 scale)', () => {
        const css = read('../ui/review.css');
        expect(blockOf(css, '.btn-confirm {')).not.toContain('padding');
        expect(blockOf(css, '.btn-cancel {')).not.toContain('padding');
        expect(css).not.toContain('scale(0.96)');
    });

    it('.toggle-password centers via margin-top, no transform declaration (base press-scale owns transform)', () => {
        const block = blockOf(read('../ui/settings.css'), '.toggle-password {');
        expect(block).toContain('top: 50%');
        expect(block).toMatch(/margin-top:\s*-16px/);
        // Declaration-anchored on purpose: blockOf slices raw text INCLUDING
        // comments, and the block's own comment legitimately mentions
        // transform/translateY — only a transform declaration is the defect.
        expect(block).not.toMatch(/^\s*transform\s*:/m);
    });

    it('settings.css declares no transform: translateY(-50%) centering (clobbered by button:active scale)', () => {
        expect(read('../ui/settings.css')).not.toMatch(
            /^\s*transform\s*:\s*translateY\(-50%\)/m,
        );
    });
});

describe('badge uniformity', () => {
    it('.mode-badge has no own geometry (badge base provides it)', () => {
        const css = read('../ui/settings.css');
        // 基础块必须整体删除（^ 锚定不误伤 .mode-badge.gpu 等变体选择器；
        // blockOf 对不存在的 selector 现在会 throw，故用 not.toMatch 显式锁定删除态）
        expect(css).not.toMatch(/^\.mode-badge\s*\{/m);
        expect(blockOf(css, '.mode-badge.gpu')).not.toMatch(
            /padding|border-radius|font-size|font-weight/,
        );
    });

    it('mode-badge variants share the badge-done/warning/failed recipes (0.15 tint)', () => {
        const css = read('../ui/settings.css');
        expect(blockOf(css, '.mode-badge.gpu')).toContain(
            'rgba(var(--success-rgb), 0.15)',
        );
        expect(blockOf(css, '.mode-badge.cpu')).toContain(
            'rgba(var(--warning-rgb), 0.15)',
        );
        expect(blockOf(css, '.mode-badge.unloaded')).toContain(
            'rgba(var(--error-rgb), 0.15)',
        );
    });

    it('audio badges are gone as custom classes; JS emits badge-family classes', () => {
        expect(read('../ui/settings.css')).not.toContain(
            '.audio-missing-badge',
        );
        expect(read('../ui/settings.css')).not.toContain('.audio-error-badge');
        expect(read('../ui/transcribe.css')).not.toContain(
            '.audio-error-badge',
        );
        const dm = read('../ui/lib/data-management.js');
        expect(dm).toContain(`'badge badge-failed'`);
        expect(dm).toContain(`'badge badge-neutral'`);
    });

    it('.badge-neutral is the shared neutral variant', () => {
        const block = blockOf(read('../ui/common.css'), '.badge-neutral,');
        expect(block).toContain('var(--btn-secondary-bg)');
        expect(block).toContain('var(--text-secondary)');
    });

    it('html badges compose the badge base', () => {
        expect(read('../ui/settings.html')).toContain(
            'class="badge mode-badge unloaded"',
        );
        expect(read('../ui/transcribe.html')).toContain(
            'class="badge badge-failed"',
        );
    });

    it('.badge base guards [hidden] against the author display declaration', () => {
        const css = read('../ui/common.css');
        expect(css).toMatch(/^\.badge\[hidden\]\s*\{\s*display:\s*none;/m);
    });
});

describe('error banner uniformity', () => {
    it('common.css owns the .banner/.banner-error recipe', () => {
        const css = read('../ui/common.css');
        const base = blockOf(css, '.banner {');
        expect(base).toContain('border-radius: var(--radius-md)');
        expect(base).toContain('white-space: pre-line');
        expect(base).not.toContain('display');
        const err = blockOf(css, '.banner-error {');
        expect(err).toContain('rgba(var(--error-rgb), 0.08)');
    });

    it('window error bars carry no own color recipe (banner composition)', () => {
        const s = read('../ui/settings.css');
        expect(blockOf(s, '.error-banner {')).not.toContain('background');
        expect(blockOf(s, '.data-error-bar {')).not.toContain('background');
        const t = read('../ui/transcribe.css');
        expect(blockOf(t, '.error-bar {')).not.toContain('background');
    });

    it('html error bars compose banner banner-error', () => {
        expect(read('../ui/settings.html')).toContain(
            'class="error-banner banner banner-error"',
        );
        expect(read('../ui/settings.html')).toContain(
            'class="data-error-bar banner banner-error"',
        );
        expect(read('../ui/transcribe.html')).toContain(
            'class="error-bar banner banner-error"',
        );
    });

    it('notice variant fully overrides banner-error (green state, not green bg with red border)', () => {
        const block = blockOf(
            read('../ui/settings.css'),
            '.data-error-bar.notice',
        );
        expect(block).toContain('rgba(var(--success-rgb), 0.08)');
        expect(block).toContain('border-color');
        expect(block).toContain('color: var(--success-text)');
    });
});

describe('input treatment uniformity', () => {
    it('global input rule covers type=search', () => {
        expect(read('../ui/settings.css')).toMatch(
            /select, input\[type="text"\], input\[type="password"\], input\[type="search"\]/,
        );
    });

    it('.data-search-input is layout-only (no own border recipe)', () => {
        const block = blockOf(
            read('../ui/settings.css'),
            '.data-search-input {',
        );
        expect(block).not.toMatch(/border|background|padding|font-size|color/);
    });

    it('.segment-text uses inset ring and keeps keyboard focus visible', () => {
        const css = read('../ui/transcribe.css');
        const block = blockOf(css, '.segment-text {');
        expect(block).toContain('box-shadow: inset 0 0 0 1px');
        expect(blockOf(css, '.segment-text:focus')).not.toContain(
            'outline: none',
        );
        expect(blockOf(css, '.segment-text:focus')).toContain('var(--accent)');
    });

    it('#merged gets the same focus treatment', () => {
        const css = read('../ui/transcribe.css');
        expect(blockOf(css, '#merged {')).toContain(
            'box-shadow: inset 0 0 0 1px',
        );
        expect(blockOf(css, '#merged:focus')).toContain('var(--accent)');
    });
});

describe('list row state uniformity', () => {
    it('selected rows use accent tint, not a side-stripe', () => {
        const css = read('../ui/transcribe.css');
        const block = blockOf(css, '.rec-row.selected {');
        expect(block).not.toContain('inset 2px');
        expect(block).toContain('rgba(var(--accent-rgb), 0.18)');
    });

    it('row hover is neutral surface, accent reserved for selection', () => {
        expect(
            blockOf(read('../ui/settings.css'), '.data-row:hover {'),
        ).toContain('var(--surface-hover)');
        expect(
            blockOf(read('../ui/transcribe.css'), '.rec-row:hover {'),
        ).toContain('var(--surface-hover)');
    });
});

describe('spinner/toast uniformity', () => {
    it('.spinner is owned by common.css, not transcribe.css', () => {
        expect(read('../ui/common.css')).toMatch(/\.spinner \{/);
        expect(read('../ui/transcribe.css')).not.toMatch(/\.spinner \{/);
    });

    it('toasts share the duration-short entrance', () => {
        expect(
            blockOf(read('../ui/settings.css'), '.pending-toast {'),
        ).toContain(
            'animation: status-pop var(--duration-short) var(--ease-spring)',
        );
        expect(blockOf(read('../ui/transcribe.css'), '.toast {')).toContain(
            'animation: status-pop var(--duration-short) var(--ease-spring)',
        );
    });
});
