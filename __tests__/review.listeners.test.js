// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

let listeners;
let invokeMock;

async function loadFresh() {
    listeners = {};
    // cmd-based default mock — avoids the review.js top-level IIFE consuming
    // a per-test mockResolvedValueOnce('hello') before review-show's listener
    // gets a chance to call invoke, which would otherwise make tests flaky.
    invokeMock = vi.fn(async (cmd) => (cmd === 'get_review_text' ? '' : null));

    // vi.stubGlobal is vitest 4's recommended API. Direct `global.window.__TAURI__`
    // assignment is fragile under jsdom 25/29 because `global.window` may not be
    // initialized by the time review.js evaluates its top-level destructures.
    vi.stubGlobal('__TAURI__', {
        event: {
            listen: vi.fn((evt, cb) => {
                listeners[evt] = cb;
                return () => {};
            }),
        },
        core: { invoke: invokeMock },
    });

    document.body.innerHTML = `
    <div id="container"></div>
    <textarea id="review-text"></textarea>
    <div id="preview"></div>
    <button id="btn-confirm"></button>
    <button id="btn-cancel"></button>
    <div id="error-msg"></div>
  `;

    vi.resetModules();
    await import('../ui/review.js');

    // Let the top-level IIFE's invoke('get_review_text') promise settle so it
    // does not leak across tests as an unhandled rejection.
    await new Promise((r) => setTimeout(r, 0));
}

beforeEach(async () => {
    await loadFresh();
});

afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
});

const get = (id) => document.getElementById(id);

describe('review-show listener', () => {
    it('resets state and populates textarea on successful invoke', async () => {
        invokeMock.mockImplementation(async (cmd) =>
            cmd === 'get_review_text' ? 'hello world' : null,
        );

        const textarea = get('review-text');
        textarea.value = 'old';
        textarea.dispatchEvent(new Event('input')); // sets userEdited = true

        await listeners['review-show']();

        expect(textarea.value).toBe('hello world');
        expect(get('container').classList.contains('visible')).toBe(true);
        expect(get('error-msg').textContent).toBe('');
    });

    it('shows error when invoke fails', async () => {
        invokeMock.mockImplementation(async () => {
            throw new Error('x');
        });

        await listeners['review-show']();

        expect(get('error-msg').textContent).toBe('加载转写结果失败');
    });
});

describe('transcription-partial listener', () => {
    it('updates textarea when payload arrives and user has not edited', () => {
        const textarea = get('review-text');
        const container = get('container');

        listeners['transcription-partial']({ payload: '新文本' });

        expect(textarea.value).toBe('新文本');
        expect(container.classList.contains('visible')).toBe(true);
    });

    it('does not update textarea when payload is empty', () => {
        const textarea = get('review-text');
        textarea.value = 'existing';

        listeners['transcription-partial']({ payload: '' });

        expect(textarea.value).toBe('existing');
    });

    it('does not overwrite textarea when user has edited', () => {
        const textarea = get('review-text');
        textarea.value = '用户输入';
        textarea.dispatchEvent(new Event('input')); // userEdited = true

        listeners['transcription-partial']({ payload: '自动文本' });

        expect(textarea.value).toBe('用户输入');
    });
});

describe('speech-error listener', () => {
    it('hides container when textarea is empty', () => {
        const container = get('container');
        container.classList.add('visible');

        listeners['speech-error']();

        expect(container.classList.contains('visible')).toBe(false);
    });

    it('does not hide container when textarea has content', () => {
        const container = get('container');
        const textarea = get('review-text');
        container.classList.add('visible');
        textarea.value = 'some content';

        listeners['speech-error']();

        expect(container.classList.contains('visible')).toBe(true);
    });
});

describe('injection-error listener', () => {
    it('renders payload with formatted prefix', () => {
        listeners['injection-error']({ payload: '剪贴板被占用' });

        expect(get('error-msg').textContent).toBe('粘贴失败：剪贴板被占用');
    });

    it('renders default fallback when payload is empty', () => {
        listeners['injection-error']({ payload: '' });

        expect(get('error-msg').textContent).toBe('粘贴失败，剪贴板被占用');
    });

    // Regression guard for review.js:82 contract:
    //   "textContent (not innerHTML) — payload is backend string, no HTML parsing"
    // This is the XSS defense contract — do not delete this test.
    it('guards against XSS: payload is not parsed as HTML (textContent contract)', () => {
        listeners['injection-error']({
            payload: '<img src=x onerror=alert(1)>',
        });

        expect(get('error-msg').textContent).toBe(
            '粘贴失败：<img src=x onerror=alert(1)>',
        );
    });
});

describe('doCancel error recovery', () => {
    it('re-enables buttons and shows error when cancel_review fails', async () => {
        invokeMock.mockImplementation(async (cmd) => {
            if (cmd === 'cancel_review') {
                throw new Error('state machine race');
            }
            return cmd === 'get_review_text' ? '' : null;
        });

        const btnCancel = get('btn-cancel');
        const btnConfirm = get('btn-confirm');
        // Seed textarea so btnConfirm would be enabled were it not for isClosing.
        get('review-text').value = 'hello';

        expect(btnCancel.disabled).toBe(false);

        btnCancel.click(); // fires doCancel() async
        // After sync portion: isClosing=true, buttons disabled.
        expect(btnCancel.disabled).toBe(true);
        expect(btnConfirm.disabled).toBe(true);

        // Let the rejected invoke settle so catch block runs.
        await new Promise((r) => setTimeout(r, 0));

        // Failure recovery: isClosing reset, error shown.
        expect(btnCancel.disabled).toBe(false);
        expect(btnConfirm.disabled).toBe(false);
        expect(get('error-msg').textContent).toBe('取消失败，请重试');
    });
});

// Note: the `isClosing === true` branch inside injection-error is not exercised
// here. isClosing is a closure-local mutable flag in review.js with no external
// setter; reaching it requires driving doConfirm (which sets isClosing = true
// before awaiting confirm_inject). That path is out of scope for this listener
// integration test and belongs to a future doConfirm integration test.
