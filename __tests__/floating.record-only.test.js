// @vitest-environment jsdom
//
// Orchestration tests for the floating window's record-only in-flight
// indicator (breathing circle + mm:ss timer) and the showProcessing
// placeholder text. Follows the loadFresh pattern from
// transcribe.test.js: stub window.__TAURI__.event.listen into a capturable
// listeners map, inject the floating DOM, dynamically import floating.js.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

let listeners;

const DOM = `
  <div id="indicator"></div>
  <div id="transcript-text"></div>
`;

async function loadFresh() {
    listeners = {};
    vi.stubGlobal('__TAURI__', {
        event: {
            listen: vi.fn((evt, cb) => {
                listeners[evt] = cb;
                return () => {};
            }),
        },
    });
    document.body.innerHTML = DOM;
    vi.resetModules();
    await import('../ui/floating.js');
}

const indicator = () => document.getElementById('indicator');
const text = () => document.getElementById('transcript-text');

describe('floating record-only in-flight indicator', () => {
    beforeEach(async () => {
        vi.useFakeTimers();
        // The spring/settle transitions run on rAF; they are irrelevant to
        // the assertions here, so stub them as no-ops.
        vi.stubGlobal('requestAnimationFrame', () => 0);
        vi.stubGlobal('cancelAnimationFrame', () => {});
        await loadFresh();
    });

    afterEach(() => {
        vi.useRealTimers();
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
    });

    it('record-only-started shows breathing class and ticking timer text', () => {
        listeners['record-only-started']({ payload: { stem: 'x' } });

        expect(indicator().classList.contains('record-only')).toBe(true);
        expect(indicator().classList.contains('visible')).toBe(true);
        expect(text().textContent).toBe('录音中 00:00');

        vi.advanceTimersByTime(61_000);
        expect(text().textContent).toBe('录音中 01:01');
    });

    it('record-only-finished (pending) shows 已保存 and hides after 1.5s', () => {
        listeners['record-only-started']({ payload: { stem: 'x' } });
        listeners['record-only-finished']({
            payload: { stem: 'x', status: 'pending', dropped_blocks: 0 },
        });

        expect(indicator().classList.contains('record-only')).toBe(false);
        expect(text().textContent).toBe('录音已保存');
        expect(indicator().classList.contains('visible')).toBe(true);

        vi.advanceTimersByTime(1_500);
        expect(indicator().classList.contains('visible')).toBe(false);
    });

    it('record-only-finished (failed) shows the backpressure error, not 已保存', () => {
        listeners['record-only-started']({ payload: { stem: 'x' } });
        listeners['record-only-finished']({
            payload: { stem: 'x', status: 'failed', dropped_blocks: 80 },
        });

        expect(text().textContent).toBe(
            '录音不完整已提前停止，已保存部分可在转录窗口查看',
        );
        expect(indicator().classList.contains('error')).toBe(true);

        vi.advanceTimersByTime(4_500);
        expect(indicator().classList.contains('visible')).toBe(false);
    });

    it('record-only-error unpacks the object payload message', () => {
        listeners['record-only-started']({ payload: { stem: 'x' } });
        listeners['record-only-error']({
            payload: { message: '录音启动失败，请检查数据保存路径' },
        });

        expect(text().textContent).toBe('录音启动失败，请检查数据保存路径');
        expect(indicator().classList.contains('error')).toBe(true);
        expect(indicator().classList.contains('record-only')).toBe(false);
    });

    it('a classic recording-start mid-timer clears the record-only state (cross-mode guard)', () => {
        listeners['record-only-started']({ payload: { stem: 'x' } });
        expect(text().textContent).toBe('录音中 00:00');

        // Backend escape hatches only OS-hide the window; a stale interval
        // would keep overwriting partial-transcript text every second.
        listeners['recording-start']({ payload: null });

        expect(indicator().classList.contains('record-only')).toBe(false);
        const before = text().textContent;
        vi.advanceTimersByTime(10_000);
        expect(text().textContent).toBe(before);
    });

    it('transcription-complete leaves a 转录中… placeholder in the text area', () => {
        listeners['transcription-complete']({ payload: null });
        expect(text().textContent).toBe('转录中… 按 Esc 取消');
    });
});

describe('pipeline-cancelled (Esc cancel)', () => {
    beforeEach(async () => {
        vi.useFakeTimers();
        vi.stubGlobal('requestAnimationFrame', () => 0);
        vi.stubGlobal('cancelAnimationFrame', () => {});
        await loadFresh();
    });

    afterEach(() => {
        vi.useRealTimers();
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
    });

    it('hides the window immediately on pipeline-cancelled', () => {
        // Backend emits pipeline-cancelled after Esc is swallowed; the UI
        // must hide synchronously so the user sees instant feedback.
        indicator().classList.add('visible', 'processing');
        listeners['pipeline-cancelled']({ payload: null });
        expect(indicator().classList.contains('visible')).toBe(false);
    });

    it('transcription-complete copy mentions Esc', () => {
        listeners['transcription-complete']({ payload: null });
        expect(text().textContent).toContain('按 Esc 取消');
    });
});
