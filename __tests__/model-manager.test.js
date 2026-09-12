// @vitest-environment jsdom
//
// Behaviour tests for ui/model-manager.js (previously zero coverage):
// select population, action-state matrix, download lifecycle incl. the
// F6 'download cancelled' dual-shape guard, custom-model deletion,
// progress events, compute-mode badge.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../ui/lib/confirm-dialog.js', () => ({
    confirmDialog: vi.fn(),
}));

let listeners;
let invokeMock;
let formChangeEventCount;

const DOM = `
  <select id="whisper-model"></select>
  <span id="model-status-text"></span>
  <button id="btn-download-model"></button>
  <div id="download-progress"><div id="progress-fill"></div><span id="progress-percent">0%</span></div>
  <button id="btn-cancel-download"></button>
  <span id="compute-mode-badge"></span>
  <div id="error-banner"></div>
`;

async function loadFresh() {
    listeners = {};
    // Default impl returns a resolved Promise — the real Tauri invoke always
    // returns a Promise, and reportError() chains .catch() on the result
    // (a bare vi.fn() returning undefined throws synchronously and would
    // shadow showError in the failure paths).
    invokeMock = vi.fn(async () => undefined);
    formChangeEventCount = 0;
    vi.stubGlobal('__TAURI__', {
        event: {
            listen: vi.fn((evt, cb) => {
                listeners[evt] = cb;
                return () => {};
            }),
        },
        core: { invoke: invokeMock },
    });
    document.body.innerHTML = DOM;
    vi.resetModules();
    const formState = await import('../ui/lib/form-state.js');
    formState.onFormChange(() => {
        formChangeEventCount += 1;
    });
    return await import('../ui/model-manager.js');
}

const select = () => document.getElementById('whisper-model');
const statusText = () => document.getElementById('model-status-text');
const btnDownload = () => document.getElementById('btn-download-model');
const progress = () => document.getElementById('download-progress');
const progressFill = () => document.getElementById('progress-fill');
const progressPercent = () => document.getElementById('progress-percent');
const badge = () => document.getElementById('compute-mode-badge');
const errorBanner = () => document.getElementById('error-banner');

describe('model-manager', () => {
    beforeEach(async () => {
        await loadFresh();
    });

    afterEach(() => {
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
    });

    it('populates 8 built-in options, no custom group when none exist', async () => {
        const mm = await loadFresh();
        mm.setCustomModels([]);
        mm.populateModelSelect();
        const groups = select().querySelectorAll('optgroup');
        expect(groups).toHaveLength(1);
        expect(groups[0].label).toBe('内置模型');
        expect(groups[0].querySelectorAll('option')).toHaveLength(8);
    });

    it('adds a custom optgroup with custom: prefixed values', async () => {
        const mm = await loadFresh();
        mm.setCustomModels(['my-model.bin']);
        mm.populateModelSelect();
        const groups = select().querySelectorAll('optgroup');
        expect(groups).toHaveLength(2);
        expect(groups[1].label).toBe('自定义模型');
        expect(groups[1].querySelector('option').value).toBe(
            'custom:my-model.bin',
        );
    });

    it('not-downloaded model shows an enabled download button', async () => {
        const mm = await loadFresh();
        mm.setModelStatus({ tiny: false });
        mm.setSelectedModel('tiny');
        mm.updateModelAction();
        expect(btnDownload().style.display).toBe('inline-block');
        expect(btnDownload().disabled).toBe(false);
        expect(select().disabled).toBe(false);
    });

    it('downloaded model shows status text instead of button', async () => {
        const mm = await loadFresh();
        mm.setModelStatus({ tiny: true });
        mm.setSelectedModel('tiny');
        mm.updateModelAction();
        expect(statusText().style.display).toBe('inline');
        expect(btnDownload().style.display).toBe('none');
    });

    it('custom model without download shows the danger delete button', async () => {
        const mm = await loadFresh();
        mm.setCustomModels(['m.bin']);
        mm.setSelectedModel('custom:m.bin');
        mm.updateModelAction();
        expect(btnDownload().textContent).toBe('删除');
        expect(btnDownload().className).toContain('btn-danger');
        expect(btnDownload().disabled).toBe(false);
    });

    it('download lifecycle: in-flight disables everything; success updates status + notifies form', async () => {
        const mm = await loadFresh();
        let resolveDownload;
        invokeMock.mockImplementation(
            (cmd) =>
                new Promise((res) => {
                    if (cmd === 'download_whisper_model') resolveDownload = res;
                    else res(undefined);
                }),
        );
        mm.setModelStatus({});
        mm.setSelectedModel('tiny');

        btnDownload().click();
        await vi.waitFor(() => {
            expect(invokeMock).toHaveBeenCalledWith('download_whisper_model', {
                size: 'tiny',
            });
        });
        // In-flight: progress bar visible, select + button disabled.
        expect(progress().style.display).toBe('block');
        expect(select().disabled).toBe(true);
        expect(btnDownload().disabled).toBe(true);

        const before = formChangeEventCount;
        resolveDownload?.();
        await vi.waitFor(() => {
            expect(mm.getModelStatus().tiny).toBe(true);
        });
        expect(formChangeEventCount).toBeGreaterThan(before);
        expect(progress().style.display).toBe('none');
    });

    it('F6 guard: raw-string "download cancelled" stays silent', async () => {
        const mm = await loadFresh();
        invokeMock.mockRejectedValueOnce('download cancelled');
        mm.setModelStatus({});
        mm.setSelectedModel('tiny');
        btnDownload().click();
        // Positive assertions distinguish the CANCEL branch from a silent
        // success: cancel must NOT mark the model downloaded, and the action
        // state must reset to the not-downloaded affordance (button enabled).
        await vi.waitFor(() => {
            expect(mm.getModelStatus().tiny).toBeUndefined();
        });
        await vi.waitFor(() => {
            expect(btnDownload().disabled).toBe(false);
        });
        expect(errorBanner().classList.contains('visible')).toBe(false);
        expect(
            invokeMock.mock.calls.some((c) => c[0] === 'log_frontend_error'),
        ).toBe(false);
    });

    it('F6 guard: CommandError-object {message: "download cancelled"} stays silent', async () => {
        const mm = await loadFresh();
        invokeMock.mockRejectedValueOnce({
            message: 'download cancelled',
        });
        mm.setModelStatus({});
        mm.setSelectedModel('tiny');
        btnDownload().click();
        await vi.waitFor(() => {
            expect(mm.getModelStatus().tiny).toBeUndefined();
        });
        await vi.waitFor(() => {
            expect(btnDownload().disabled).toBe(false);
        });
        expect(errorBanner().classList.contains('visible')).toBe(false);
        expect(
            invokeMock.mock.calls.some((c) => c[0] === 'log_frontend_error'),
        ).toBe(false);
    });

    it('download failure surfaces showError and reports to backend', async () => {
        const mm = await loadFresh();
        invokeMock.mockRejectedValueOnce(new Error('boom'));
        mm.setModelStatus({});
        mm.setSelectedModel('tiny');
        btnDownload().click();
        await vi.waitFor(() => {
            expect(errorBanner().classList.contains('visible')).toBe(true);
        });
        expect(
            invokeMock.mock.calls.some((c) => c[0] === 'log_frontend_error'),
        ).toBe(true);
    });

    it('cancel-download failure shows the failure text', async () => {
        await loadFresh();
        invokeMock.mockRejectedValueOnce(new Error('x'));
        document.getElementById('btn-cancel-download').click();
        await vi.waitFor(() => {
            expect(progressPercent().textContent).toBe('取消下载失败');
        });
    });

    it('download-progress: mismatched size ignored; match updates fill/percent/aria', async () => {
        const mm = await loadFresh();
        listeners['download-progress']({
            payload: { size: 'small', percent: 50 },
        });
        expect(progressPercent().textContent).toBe('0%');
        // Start a download of tiny to make it the active model.
        let resolveDownload;
        invokeMock.mockImplementation(
            (cmd) =>
                new Promise((res) => {
                    if (cmd === 'download_whisper_model') resolveDownload = res;
                    else res(undefined);
                }),
        );
        mm.setModelStatus({});
        mm.setSelectedModel('tiny');
        btnDownload().click();
        await vi.waitFor(() => {
            expect(
                invokeMock.mock.calls.some(
                    (c) => c[0] === 'download_whisper_model',
                ),
            ).toBe(true);
        });
        listeners['download-progress']({
            payload: { size: 'tiny', percent: 42 },
        });
        expect(progressFill().style.transform).toBe('scaleX(0.42)');
        expect(progressPercent().textContent).toBe('42%');
        expect(progress().getAttribute('aria-valuenow')).toBe('42');
        resolveDownload?.();
        await vi.waitFor(() => {
            expect(mm.getModelStatus().tiny).toBe(true);
        });
    });

    it('loadComputeMode badge matrix: gpu/cpu/other/failure', async () => {
        const cases = [
            ['gpu', 'GPU 加速', 'mode-badge gpu'],
            ['cpu', 'CPU 模式（未检测到 GPU）', 'mode-badge cpu'],
            ['unloaded', '模型未加载', 'mode-badge unloaded'],
        ];
        for (const [mode, label, cls] of cases) {
            const mm = await loadFresh();
            invokeMock.mockResolvedValueOnce(mode);
            await mm.loadComputeMode();
            expect(badge().textContent).toBe(label);
            expect(badge().className).toBe(cls);
        }
        const mm = await loadFresh();
        invokeMock.mockRejectedValueOnce(new Error('x'));
        await mm.loadComputeMode();
        expect(badge().textContent).toBe('检测失败');
    });

    it('model-loaded event refreshes the compute-mode badge', async () => {
        await loadFresh();
        invokeMock.mockResolvedValueOnce('gpu');
        listeners['model-loaded']({ payload: null });
        await vi.waitFor(() => {
            expect(badge().textContent).toBe('GPU 加速');
        });
    });
});
