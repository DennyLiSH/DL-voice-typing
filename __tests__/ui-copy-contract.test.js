// Static text-scrape contract for the 2026-09-13 copy batch: pins the
// audited strings so they cannot silently regress (same pattern as
// undo-window-contract.test.js).
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const read = (rel) =>
    readFileSync(fileURLToPath(new URL(`../${rel}`, import.meta.url)), 'utf8');

describe('review.html copy contract', () => {
    const html = read('ui/review.html');
    it('title unified to 粘贴前确认', () => {
        expect(html).toContain('<title>粘贴前确认</title>');
    });
    it('Enter convention hint present', () => {
        expect(html).toContain('Enter 确认粘贴 · Shift+Enter 换行');
    });
    it('no redundant ARIA roles on native elements', () => {
        expect(html).not.toContain('role="textbox"');
        expect(html).not.toContain('role="button"');
    });
});

describe('settings.html copy contract', () => {
    const html = read('ui/settings.html');
    it('first paint: SVG eye, no emoji', () => {
        expect(html).not.toContain('👁');
        expect(html).toContain('toggle-key');
    });
    it('inline eye SVG is byte-identical to EYE_SVG in settings-form.js', () => {
        const js = read('ui/settings-form.js');
        const m = js.match(/const EYE_SVG =\s+'([^']+)';/);
        expect(m).not.toBeNull();
        expect(html).toContain(m[1]);
    });
    it('naming drift: 录音模式 everywhere, no 启用录音模式', () => {
        expect(html).not.toContain('启用录音模式');
        expect(html).toContain('开启「录音模式」');
    });
    it('settings-utils.js error copy matches the toggle name', () => {
        const js = read('ui/lib/settings-utils.js');
        expect(js).not.toContain('启用录音模式');
        expect(js).toContain('开启「录音模式」');
    });
    it('typography: no fullwidth-paren spaces, ASCII ellipses removed', () => {
        expect(html).not.toContain('（ Whisper ）');
        expect(html).not.toContain('检测中...');
        expect(html).not.toContain('录音转录...');
        expect(html).toContain('检测中…');
    });
    it('Q8_0 hint is lay wording with a default recommendation', () => {
        expect(html).toContain('不确定时选不带后缀的标准版即可');
    });
    it('F1/F12 warning slots + feedback link + errors section id', () => {
        expect(html).toContain('id="hotkey-warning"');
        expect(html).toContain('id="record-only-hotkey-warning"');
        expect(html).toContain('id="feedback-link"');
        expect(html).toContain('id="help-errors-section"');
    });
});

describe('feedback link security contract', () => {
    it('URL single-sourced across HTML href and JS constant (+ rel)', () => {
        const url = 'https://github.com/DennyLiSH/DL-voice-typing/issues';
        expect(read('ui/settings.html')).toContain(`href="${url}"`);
        expect(read('ui/app-shell.js')).toContain(
            `const FEEDBACK_URL = '${url}'`,
        );
        expect(read('ui/settings.html')).toContain('rel="noopener noreferrer"');
    });
    it('opener permission is the SCOPED object form locked to the GitHub repo', () => {
        const caps = JSON.parse(read('src-tauri/capabilities/settings.json'));
        const entries = caps.permissions.filter(
            (p) =>
                p === 'opener:allow-open-url' ||
                p?.identifier === 'opener:allow-open-url',
        );
        // Exactly one entry, and it must be the scoped object form — a bare
        // string entry (or a dead scope object alongside one) would disable
        // the URL restriction.
        expect(entries).toHaveLength(1);
        const allowUrl = entries[0]?.allow?.[0]?.url;
        expect(allowUrl).toBe('https://github.com/DennyLiSH/*');
        // The link URL must actually fall inside the scope glob prefix.
        const js = read('ui/app-shell.js');
        const m = js.match(/const FEEDBACK_URL = '([^']+)';/);
        expect(m?.[1].startsWith(allowUrl.replace(/\*$/, ''))).toBe(true);
    });
});

describe('settings-form.js copy contract', () => {
    const js = read('ui/settings-form.js');
    it('ASCII ellipses replaced, initial-paint EYE_SVG assignment removed', () => {
        expect(js).not.toContain('测试中...');
        expect(js).not.toContain('保存中...');
        expect(js).toContain('测试中…');
        // The SVG is now inlined in settings.html so first paint matches
        // without a JS replacement pass. The initial-paint assignment
        // (a bare top-level statement) was dead code; only the click
        // handler's LOCK_SVG/EYE_SVG swap remains. Lock by *occurrence
        // count* — exactly 1 EYE_SVG assignment (the click handler), not
        // 2 (which would mean a top-level initial-paint assignment was
        // reintroduced).
        const eyeAssignments =
            js.match(/toggleKeyBtn\.innerHTML\s*=\s*EYE_SVG\s*;/g) ?? [];
        expect(eyeAssignments).toHaveLength(1);
    });
});

describe('floating.html copy contract', () => {
    const html = read('ui/floating.html');
    it('transcript defaults to polite live region without timer role', () => {
        expect(html).toContain('aria-live="polite"');
        expect(html).not.toContain('role="timer"');
    });
});
