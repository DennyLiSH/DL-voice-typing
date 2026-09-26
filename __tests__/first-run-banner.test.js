// @vitest-environment jsdom

import { readFileSync } from 'node:fs';
import { beforeEach, describe, expect, it } from 'vitest';
import {
    bindFirstRunBanner,
    hideIfVisible,
    showIfRequested,
} from '../ui/lib/first-run-banner.js';

// 环境声明沿用 settings.error_forwarding.test.js 先例（vitest.config.js
// 无全局 environment，默认 node——缺此行则 document 未定义，4 例 DOM 用例全挂）

describe('first-run-banner', () => {
    beforeEach(() => {
        document.body.innerHTML = `
            <div class="first-run-banner" id="first-run-banner" role="status" hidden>
                <span id="first-run-banner-text"></span>
                <button id="first-run-banner-dismiss">知道了</button>
            </div>`;
        window.history.replaceState({}, '', '/');
    });

    it('shows banner when model_missing=1 present', () => {
        window.history.replaceState({}, '', '/?model_missing=1');
        showIfRequested();
        expect(document.getElementById('first-run-banner').hidden).toBe(false);
    });
    it('stays hidden without param', () => {
        showIfRequested();
        expect(document.getElementById('first-run-banner').hidden).toBe(true);
    });
    it('dismiss button hides banner', () => {
        window.history.replaceState({}, '', '/?model_missing=1');
        showIfRequested();
        bindFirstRunBanner();
        document.getElementById('first-run-banner-dismiss').click();
        expect(document.getElementById('first-run-banner').hidden).toBe(true);
    });
    it('hideIfVisible is idempotent', () => {
        window.history.replaceState({}, '', '/?model_missing=1');
        showIfRequested();
        hideIfVisible();
        hideIfVisible();
        expect(document.getElementById('first-run-banner').hidden).toBe(true);
    });
    it('banner copy contains model download guidance (scrapes settings.html)', () => {
        const html = readFileSync('ui/settings.html', 'utf8');
        const m = html.match(/id="first-run-banner-text"[^>]*>([^<]+)</);
        expect(m).not.toBeNull();
        expect(m[1]).toContain('模型');
        expect(m[1]).toContain('下载');
    });
    it('backend param name contract: lib.rs uses model_missing=1', () => {
        // text-scrape 跨层契约（credential-words-contract / undo-window-contract 同族）；
        // jsdom env 下 import.meta.url 是 http:// 形态（不是 file://），new URL(..., ...) 报
        // "URL must be of scheme file"——改走 process.cwd() 解析（vitest 默认 cwd = 仓库根）
        const lib = readFileSync('src-tauri/src/lib.rs', 'utf8');
        expect(lib).toContain('settings.html?model_missing=1');
    });
});
