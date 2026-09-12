// @vitest-environment jsdom
//
// State-machine tests for ui/floating.js (hide scheduling, error
// auto-clear, defensive cleanup, spring scheduling, ripple
// threshold/throttle/cap/removal). Follows the loadFresh pattern from
// floating.record-only.test.js; performance.now is controlled via
// vi.spyOn for the ripple-throttle tests (the only dependency).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

let listeners;
let rafCallbacks;

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
const ripples = () => document.querySelectorAll('.ripple');

/** Drain the rAF queue n rounds: each round invokes all queued callbacks
 *  (each may re-schedule). Returns callbacks scheduled in the last round. */
function stepFrames(rounds) {
    let queued = 0;
    for (let i = 0; i < rounds; i++) {
        const batch = rafCallbacks;
        rafCallbacks = [];
        for (const cb of batch) cb();
        queued = rafCallbacks.length;
    }
    return queued;
}

describe('floating state machine', () => {
    beforeEach(async () => {
        vi.useFakeTimers();
        rafCallbacks = [];
        vi.stubGlobal('requestAnimationFrame', (cb) => {
            rafCallbacks.push(cb);
            return rafCallbacks.length;
        });
        vi.stubGlobal('cancelAnimationFrame', () => {});
        await loadFresh();
    });

    afterEach(() => {
        vi.useRealTimers();
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
    });

    it('error channels show error state and auto-hide after 4.5s (payload truncated to 40 + ellipsis)', () => {
        for (const evt of ['speech-error', 'llm-error', 'injection-error']) {
            listeners[evt]({ payload: 'x'.repeat(60) });
            expect(indicator().classList.contains('error')).toBe(true);
            expect(indicator().classList.contains('visible')).toBe(true);
            expect(text().textContent).toBe(`${'x'.repeat(40)}…`);
        }
        // Last error's delayed hide — locked from both sides: still visible
        // at 4499ms, hidden at exactly 4500ms.
        vi.advanceTimersByTime(4_499);
        expect(indicator().classList.contains('visible')).toBe(true);
        vi.advanceTimersByTime(1);
        expect(indicator().classList.contains('visible')).toBe(false);
    });

    it('error fallback text is used for empty payloads', () => {
        listeners['speech-error']({ payload: '' });
        expect(text().textContent).toBe('语音识别失败');
    });

    it('a later show() cancels a pending delayed hide', () => {
        listeners['speech-error']({ payload: 'boom' }); // schedules hide(4500)
        listeners['recording-start']({ payload: null }); // show() clears it
        vi.advanceTimersByTime(6_000);
        expect(indicator().classList.contains('visible')).toBe(true);
    });

    it('pipeline-cancelled hides immediately', () => {
        listeners['recording-start']({ payload: null });
        listeners['pipeline-cancelled']({ payload: null });
        expect(indicator().classList.contains('visible')).toBe(false);
    });

    it('recording-start defensively stops a running record-only timer', () => {
        listeners['record-only-started']({ payload: { stem: 'x' } });
        expect(text().textContent).toBe('录音中 00:00');
        listeners['recording-start']({ payload: null });
        vi.advanceTimersByTime(2_000);
        // Timer is dead: text stays cleared instead of ticking to 录音中 00:02
        expect(text().textContent).toBe('');
    });

    it('transcription-partial shows text; transcription-complete shows processing placeholder', () => {
        listeners['recording-start']({ payload: null });
        listeners['transcription-partial']({ payload: '你好世界' });
        expect(text().textContent).toBe('你好世界');
        expect(text().classList.contains('visible')).toBe(true);
        listeners['transcription-complete']({ payload: null });
        expect(text().textContent).toBe('转录中… 按 Esc 取消');
    });

    it('audio-rms schedules spring frames and settles to target scale', () => {
        listeners['recording-start']({ payload: null });
        listeners['audio-rms']({ payload: 0.9 });
        expect(rafCallbacks.length).toBeGreaterThan(0);
        // Step until the spring settles (no new frames scheduled).
        let still = 0;
        for (let i = 0; i < 600 && still < 3; i++) {
            still = stepFrames(1) === 0 ? still + 1 : 0;
        }
        expect(still).toBeGreaterThanOrEqual(3);
        // Settled transform is an exact snap to a scale() value.
        expect(indicator().style.transform).toMatch(/^scale\([0-9.]+\)$/);
    });

    describe('ripple', () => {
        beforeEach(() => {
            // Base t=1000: floating.js initializes `lastRippleTime = 0`, so a
            // t=0 first call would hit the 250ms throttle gate immediately
            // (`0 - 0 > 250` is false) and never spawn — any base > 250 works
            // (Iteration 2 review finding: mockReturnValue(0) collided with
            // the module's own initial value).
            vi.spyOn(performance, 'now').mockReturnValue(1_000);
        });

        it('spawns only above threshold with >=3 history samples', () => {
            listeners['recording-start']({ payload: null });
            listeners['audio-rms']({ payload: 0.05 });
            listeners['audio-rms']({ payload: 0.05 });
            listeners['audio-rms']({ payload: 0.05 });
            listeners['audio-rms']({ payload: 0.05 }); // 4th low: avg 0.05, not > 1.5x
            expect(ripples().length).toBe(0);
            listeners['audio-rms']({ payload: 0.5 }); // high vs avg ~0.14; t=1000: 1000-0>250 passes
            expect(ripples().length).toBe(1);
        });

        it('throttles spawns to one per 250ms', () => {
            listeners['recording-start']({ payload: null });
            for (let i = 0; i < 3; i++)
                listeners['audio-rms']({ payload: 0.05 });
            listeners['audio-rms']({ payload: 0.5 }); // t=1000: spawn (1000-0>250)
            expect(ripples().length).toBe(1);
            performance.now.mockReturnValue(1_100);
            listeners['audio-rms']({ payload: 0.05 });
            listeners['audio-rms']({ payload: 0.5 }); // t=1100: 1100-1000=100 < 250: throttled
            expect(ripples().length).toBe(1);
            performance.now.mockReturnValue(1_300);
            listeners['audio-rms']({ payload: 0.5 }); // t=1300: 300>250: allowed
            expect(ripples().length).toBe(2);
        });

        it('caps active ripples at 3 (4th qualifying attempt adds nothing)', () => {
            listeners['recording-start']({ payload: null });
            for (let i = 0; i < 3; i++)
                listeners['audio-rms']({ payload: 0.05 });
            // All 4 attempts pass threshold AND throttle (300ms apart) — the
            // 4th is blocked ONLY by MAX_ACTIVE_RIPPLES, which is what this
            // test locks. animationend never fires under stub, so all spawned
            // ripples stay active.
            for (const t of [1_000, 1_300, 1_600, 1_900]) {
                performance.now.mockReturnValue(t);
                listeners['audio-rms']({ payload: 0.9 });
            }
            expect(ripples().length).toBe(3);
        });

        it('removes the ripple element on animationend', () => {
            listeners['recording-start']({ payload: null });
            for (let i = 0; i < 3; i++)
                listeners['audio-rms']({ payload: 0.05 });
            listeners['audio-rms']({ payload: 0.5 }); // t=1000 spawn
            const el = ripples()[0];
            el.dispatchEvent(new Event('animationend'));
            expect(ripples().length).toBe(0);
        });
    });
});
