// @vitest-environment node
//
// M5-a: settings window must save on Ctrl+S (with IME guard + in-flight guard).
//
// 降级（plan Step 1 预警：完整 saveSettings 调用链需全 DOM fixture + modelStatus
// 初始化 + dirty 标记 + validation pass——成本过高，已登记降级）。
// 走静态契约断言：源码刮取 + 行为子集（keydown 事件 preventDefault）单测。
// saveSettings 全链路留给 settings.form-behavior.test.js + 实机验证。

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const SRC = readFileSync(
    new URL('../ui/settings-form.js', import.meta.url),
    'utf8',
);

describe('settings Ctrl+S binding (M5-a static contract)', () => {
    it('exports saveSettings as a top-level named export', () => {
        expect(SRC).toMatch(/^export\s+async\s+function\s+saveSettings\s*\(/m);
    });

    it('saveSettings has shared in-flight guard via saveInFlight flag', () => {
        // The guard must use a dedicated saveInFlight flag — saveBtn.disabled
        // is also true when the form is clean (no edits), so checking that
        // would break the "save a non-dirty form" path that settings.form-behavior
        // tests rely on. The dedicated flag is the only safe re-entry guard.
        expect(SRC).toMatch(/let\s+saveInFlight\s*=\s*false/);
        const m = SRC.match(
            /export\s+async\s+function\s+saveSettings\s*\(\s*\)\s*\{[\s\S]*?\n(\s{4})if\s*\(\s*saveInFlight\s*\)\s*return;/,
        );
        expect(
            m,
            'saveInFlight guard not at top of saveSettings',
        ).not.toBeNull();
        // saveInFlight = true must be set before the await call(...) so a
        // double-click during in-flight save is rejected by the guard.
        expect(SRC).toMatch(/saveInFlight\s*=\s*true/);
        // ...and reset to false in finally so the next save can run.
        expect(SRC).toMatch(/saveInFlight\s*=\s*false/);
    });

    it('document-level keydown listener handles Ctrl+S with IME guard', () => {
        // Pattern: document.addEventListener('keydown', ... (e.ctrlKey || e.metaKey)
        // && e.key.toLowerCase() === 's' && !e.isComposing) { e.preventDefault(); saveSettings(); }
        const m = SRC.match(
            /document\.addEventListener\(\s*['"]keydown['"]\s*,\s*\(e\)\s*=>\s*\{[\s\S]*?\(e\.ctrlKey\s*\|\|\s*e\.metaKey\)[\s\S]*?e\.key\.toLowerCase\(\)\s*===\s*['"]s['"][\s\S]*?!e\.isComposing[\s\S]*?e\.preventDefault\(\)[\s\S]*?saveSettings\(\)/,
        );
        expect(
            m,
            'Ctrl+S/Meta+S keydown handler with IME guard + preventDefault + saveSettings not found',
        ).not.toBeNull();
    });

    it('saveBtn click is bound to saveSettings (no inline arrow anymore)', () => {
        // Old pattern: saveBtn.addEventListener('click', async () => { ... });
        // New pattern:  saveBtn.addEventListener('click', saveSettings);
        expect(SRC).toMatch(
            /saveBtn\.addEventListener\(\s*['"]click['"]\s*,\s*saveSettings\s*\)/,
        );
        // Make sure the inline-arrow click listener was removed.
        expect(SRC).not.toMatch(
            /saveBtn\.addEventListener\(\s*['"]click['"]\s*,\s*async\s*\(\s*\)\s*=>\s*\{/,
        );
    });
});
