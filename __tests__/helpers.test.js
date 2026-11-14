// @vitest-environment jsdom
//
// Unit tests for __tests__/helpers.js — the shared frontend test
// infrastructure (Round 6 P4 Task 1). Everything is verified against
// REAL repo artifacts: readSource reads ui/settings.html itself (not a
// fixture copy), loadSettingsDom parses that same file into jsdom, and
// loadFresh proves module state resets between re-imports (form-state's
// module-level dirty flag).

import { afterEach, describe, expect, it, vi } from 'vitest';
import {
    blockOf,
    loadFresh,
    loadSettingsDom,
    readSource,
    stubTauri,
} from './helpers.js';

afterEach(() => {
    vi.unstubAllGlobals();
});

describe('readSource', () => {
    it('reads the real repo file, repo-root-relative', () => {
        const html = readSource('ui/settings.html');
        expect(html).toContain('<title>语文兔语音输入法 - 设置</title>');
        expect(html).toContain('id="save-status"');
    });
});

describe('blockOf', () => {
    const css = '.btn-danger { color: red; }\n.later { other: 1; }';

    it('returns the block closed at the first end marker (default })', () => {
        expect(blockOf(css, '.btn-danger {')).toBe(
            '.btn-danger { color: red; }',
        );
        // Not satisfied by the later block's content or closing brace.
        expect(blockOf(css, '.btn-danger {')).not.toContain('other');
    });

    it('returns the slice including a multi-char end marker', () => {
        const src = 'start here /* end */ trailing';
        expect(blockOf(src, 'start', 'end */')).toBe('start here /* end */');
    });

    it('throws when the start marker is missing', () => {
        expect(() => blockOf(css, '.missing {')).toThrow(
            'start marker not found',
        );
    });

    it('throws when the end marker is missing', () => {
        expect(() => blockOf('.a { never closed', '.a {')).toThrow(
            'end marker not found',
        );
    });
});

describe('stubTauri', () => {
    it('stubs __TAURI__ with defaults and returns the handle', () => {
        const handle = stubTauri();
        expect(window.__TAURI__).toBe(handle);
        expect(typeof handle.core.invoke).toBe('function');
        expect(typeof handle.event.listen).toBe('function');
    });

    it('default listen resolves to an unlisten function', async () => {
        const handle = stubTauri();
        const unlisten = await handle.event.listen('some-event', () => {});
        expect(typeof unlisten).toBe('function');
    });

    it('shallow-merges overrides over the defaults', () => {
        const invoke = vi.fn();
        const handle = stubTauri({ core: { invoke } });
        expect(handle.core.invoke).toBe(invoke);
        // Non-overridden default survives.
        expect(typeof handle.event.listen).toBe('function');
        handle.event.listen('evt', () => {});
        expect(handle.event.listen).toHaveBeenCalledTimes(1);
    });

    it('is safe to call repeatedly: latest stub wins, overrides land', () => {
        stubTauri();
        const dialogOpen = vi.fn();
        const second = stubTauri({ dialog: { open: dialogOpen } });
        expect(window.__TAURI__).toBe(second);
        second.dialog.open('pick');
        expect(dialogOpen).toHaveBeenCalledTimes(1);
        second.core.invoke('get_config');
        expect(second.core.invoke).toHaveBeenCalledWith('get_config');
    });
});

describe('loadSettingsDom', () => {
    it('loads real settings.html markup into document.body', () => {
        loadSettingsDom();
        expect(document.getElementById('save-status')).not.toBeNull();
        expect(document.getElementById('hotkey-warning')).not.toBeNull();
    });

    it('strips <script> and <link> tags (jsdom must not re-run entry scripts)', () => {
        loadSettingsDom();
        expect(document.querySelector('script')).toBeNull();
        expect(document.querySelector('link')).toBeNull();
    });
});

describe('loadFresh', () => {
    it('returns a usable module namespace', async () => {
        const mod = await loadFresh('../ui/lib/form-state.js');
        expect(typeof mod.onFormChange).toBe('function');
        expect(typeof mod.isFormDirty).toBe('function');
    });

    it('re-imports fresh module state (resetModules before import)', async () => {
        const first = await loadFresh('../ui/lib/form-state.js');
        first.setFormDirty(true);
        expect(first.isFormDirty()).toBe(true);

        const second = await loadFresh('../ui/lib/form-state.js');
        expect(second).not.toBe(first);
        expect(second.isFormDirty()).toBe(false);
    });
});
