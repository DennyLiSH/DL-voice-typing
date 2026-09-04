// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The re-transcription confirmation goes through the shared in-app dialog.
vi.mock('../ui/lib/confirm-dialog.js', () => ({
    confirmDialog: vi.fn(async () => false),
    isDialogOpen: () => false,
}));
import {
    filterRecordOnly,
    findActiveSegmentIndex,
    formatTimestamp,
    injectTargetLabel,
    llmConfigured,
    mergeSegmentTexts,
    PHASE,
    shouldAutoScroll,
    statusBadge,
    uiFlags,
} from '../ui/lib/transcribe.js';

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

describe('formatTimestamp', () => {
    it('formats zero', () => {
        expect(formatTimestamp(0)).toBe('0:00');
    });

    it('floors sub-second precision (59.9s)', () => {
        expect(formatTimestamp(59_900)).toBe('0:59');
    });

    it('crosses the minute boundary (60.1s)', () => {
        expect(formatTimestamp(60_100)).toBe('1:00');
    });

    it('crosses the hour boundary', () => {
        expect(formatTimestamp(3_661_500)).toBe('1:01:01');
        expect(formatTimestamp(3_600_000)).toBe('1:00:00');
    });

    it('handles invalid input', () => {
        expect(formatTimestamp(-5)).toBe('0:00');
        expect(formatTimestamp(Number.NaN)).toBe('0:00');
    });
});

describe('findActiveSegmentIndex', () => {
    const segments = [
        { start_ms: 0, end_ms: 2000 },
        { start_ms: 2000, end_ms: 4500 },
        { start_ms: 5000, end_ms: 8000 },
    ];

    it('returns -1 when nothing matches', () => {
        expect(findActiveSegmentIndex([], 1000)).toBe(-1);
        expect(findActiveSegmentIndex(segments, 4600)).toBe(-1); // gap
        expect(findActiveSegmentIndex(segments, 8000)).toBe(-1); // at last end
        expect(findActiveSegmentIndex(segments, 99_000)).toBe(-1);
    });

    it('matches the first segment', () => {
        expect(findActiveSegmentIndex(segments, 0)).toBe(0);
        expect(findActiveSegmentIndex(segments, 1000)).toBe(0);
    });

    it('matches a middle segment', () => {
        expect(findActiveSegmentIndex(segments, 3000)).toBe(1);
    });

    it('matches the last segment', () => {
        expect(findActiveSegmentIndex(segments, 7999)).toBe(2);
    });

    it('start boundary is inclusive, end boundary exclusive', () => {
        expect(findActiveSegmentIndex(segments, 2000)).toBe(1);
        expect(findActiveSegmentIndex(segments, 5000)).toBe(2);
    });

    it('handles invalid input', () => {
        expect(findActiveSegmentIndex(null, 1000)).toBe(-1);
        expect(findActiveSegmentIndex(segments, Number.NaN)).toBe(-1);
    });
});

describe('mergeSegmentTexts', () => {
    const segments = [
        { text: '第一段。' },
        { text: '第二段。' },
        { text: '第三段。' },
    ];

    it('returns empty string for no segments', () => {
        expect(mergeSegmentTexts([], new Map())).toBe('');
        expect(mergeSegmentTexts(null, new Map())).toBe('');
    });

    it('merges a single segment', () => {
        expect(mergeSegmentTexts([{ text: '唯一段落' }], new Map())).toBe(
            '唯一段落',
        );
    });

    it('merges multiple segments without separators', () => {
        expect(mergeSegmentTexts(segments, new Map())).toBe(
            '第一段。第二段。第三段。',
        );
    });

    it('applies edits without touching the segments', () => {
        const edits = new Map([[1, '改过的第二段。']]);
        expect(mergeSegmentTexts(segments, edits)).toBe(
            '第一段。改过的第二段。第三段。',
        );
        // Segments untouched (rule 5: timestamps stay aligned).
        expect(segments[1].text).toBe('第二段。');
    });
});

describe('statusBadge', () => {
    it('maps the three statuses', () => {
        expect(statusBadge('pending').text).toBe('待转录');
        expect(statusBadge('done').text).toBe('已转录');
        expect(statusBadge('failed').text).toBe('转录失败');
        expect(statusBadge(undefined).text).toBe('待转录');
    });
});

describe('filterRecordOnly', () => {
    it('keeps only record_only entries', () => {
        const items = [
            { filename: 'a', source: 'record_only' },
            { filename: 'b', source: 'classic' },
            { filename: 'c' },
        ];
        expect(filterRecordOnly(items).map((i) => i.filename)).toEqual(['a']);
    });

    it('handles non-array input', () => {
        expect(filterRecordOnly(null)).toEqual([]);
    });
});

describe('shouldAutoScroll', () => {
    it('allows scrolling when nothing is focused', () => {
        expect(shouldAutoScroll(null)).toBe(true);
    });

    it('suppresses scrolling while a segment input is focused', () => {
        const row = document.createElement('div');
        const input = document.createElement('input');
        input.className = 'segment-text';
        row.appendChild(input);
        document.body.appendChild(row);
        expect(shouldAutoScroll(input)).toBe(false);
        row.remove();
    });

    it('allows scrolling for elements outside segment inputs', () => {
        const div = document.createElement('div');
        document.body.appendChild(div);
        expect(shouldAutoScroll(div)).toBe(true);
        div.remove();
    });
});

// ---------------------------------------------------------------------------
// uiFlags (C5 phase state machine — pure mapping, all 8 output fields)
// ---------------------------------------------------------------------------

describe('uiFlags', () => {
    const base = {
        phase: PHASE.IDLE,
        selected: '2026-08-18_10-00-00',
        status: 'pending',
        mergedEmpty: false,
    };

    it('idle with selection: everything actionable, nothing visible', () => {
        expect(uiFlags(base)).toEqual({
            transcribeDisabled: false,
            transcribeLabel: '转录',
            cancelVisible: false,
            progressVisible: false,
            segmentsLoading: false,
            detailBusy: false,
            injectDisabled: false,
            injectSpinnerVisible: false,
            listLocked: false,
        });
    });

    it('transcribeLabel flips to 重新转录 only when status is done', () => {
        expect(uiFlags({ ...base, status: 'done' }).transcribeLabel).toBe(
            '重新转录',
        );
        expect(uiFlags({ ...base, status: 'failed' }).transcribeLabel).toBe(
            '转录',
        );
        expect(uiFlags({ ...base, status: null }).transcribeLabel).toBe('转录');
    });

    it('idle without selection disables transcribe', () => {
        const flags = uiFlags({ ...base, selected: null });
        expect(flags.transcribeDisabled).toBe(true);
        // Inject is independently gated by merged emptiness.
        expect(flags.injectDisabled).toBe(false);
    });

    it('idle with empty merged text disables inject', () => {
        expect(uiFlags({ ...base, mergedEmpty: true }).injectDisabled).toBe(
            true,
        );
    });

    it('transcribing: cancel+progress visible, entries blocked, list locked', () => {
        expect(uiFlags({ ...base, phase: PHASE.TRANSCRIBING })).toEqual({
            transcribeDisabled: true,
            transcribeLabel: '转录',
            cancelVisible: true,
            progressVisible: true,
            segmentsLoading: false,
            detailBusy: false,
            injectDisabled: true,
            injectSpinnerVisible: false,
            listLocked: true,
        });
    });

    it('loading: entries blocked and list locked (BC(C5)#1 unified guard)', () => {
        const flags = uiFlags({ ...base, phase: PHASE.LOADING });
        expect(flags.transcribeDisabled).toBe(true);
        expect(flags.injectDisabled).toBe(true);
        expect(flags.listLocked).toBe(true);
        expect(flags.cancelVisible).toBe(false);
        expect(flags.progressVisible).toBe(false);
        expect(flags.segmentsLoading).toBe(true);
        // aria-busy on #detail: skeleton is aria-hidden, this is the SR cue.
        expect(flags.detailBusy).toBe(true);
        expect(flags.injectSpinnerVisible).toBe(false);
    });

    it('injecting: spinner visible, entries blocked, list locked', () => {
        const flags = uiFlags({ ...base, phase: PHASE.INJECTING });
        expect(flags.injectSpinnerVisible).toBe(true);
        expect(flags.transcribeDisabled).toBe(true);
        expect(flags.injectDisabled).toBe(true);
        expect(flags.listLocked).toBe(true);
        expect(flags.cancelVisible).toBe(false);
        expect(flags.segmentsLoading).toBe(false);
        expect(flags.detailBusy).toBe(false);
    });
});

// ---------------------------------------------------------------------------
// Window orchestration (listeners + DOM interactions)
// ---------------------------------------------------------------------------

const ITEM = {
    filename: '2026-08-18_10-00-00',
    timestamp: '2026-08-18T10:00:00+08:00',
    source: 'record_only',
    transcription_status: 'done',
    dropped_blocks: 0,
    wav_size: 100,
    json_size: 50,
};

const SEGS = {
    segments: [
        { text: '第一段。', start_ms: 0, end_ms: 2000 },
        { text: '第二段。', start_ms: 2000, end_ms: 4000 },
    ],
    duration_ms: 4000,
    transcription_status: 'done',
    dropped_blocks: 0,
};

const BODY_HTML = `
    <button id="btn-refresh"></button>
    <div id="list-error" hidden></div>
    <div id="rec-list" role="listbox" aria-label="录音列表"></div>
    <div id="rec-empty" hidden></div>
    <div id="detail-empty"></div>
    <div id="detail" hidden>
        <span id="detail-title"></span>
        <span id="detail-badges"></span>
        <audio id="audio" controls></audio>
        <span id="audio-error" hidden></span>
        <button id="btn-transcribe"></button>
        <input type="checkbox" id="chk-llm">
        <button id="btn-cancel" hidden></button>
        <div id="progress-wrap" hidden>
            <div id="progress-fill"></div>
            <span id="progress-label"></span>
        </div>
        <div id="segments"></div>
        <div id="segments-loading" hidden>
            <div class="skel-row"></div>
        </div>
        <div id="segments-empty" hidden></div>
        <textarea id="merged"></textarea>
        <button id="btn-inject"></button>
        <span id="inject-spinner" hidden></span>
    </div>
    <div id="toast" hidden></div>
`;

let listeners;
let invokeMock;
let scrollSpy;

const flush = () => new Promise((r) => setTimeout(r, 0));

async function loadFresh(invokeImpl) {
    listeners = {};
    invokeMock = vi.fn(invokeImpl);

    vi.stubGlobal('__TAURI__', {
        event: {
            listen: vi.fn((evt, cb) => {
                listeners[evt] = cb;
                return () => {};
            }),
        },
        core: { invoke: invokeMock },
    });

    URL.createObjectURL = vi.fn(() => 'blob:mock-url');
    URL.revokeObjectURL = vi.fn();
    scrollSpy = vi.fn();
    Element.prototype.scrollIntoView = scrollSpy;

    document.body.innerHTML = BODY_HTML;
    vi.resetModules();
    await import('../ui/transcribe.js');
    await flush();
}

function defaultInvoke(cmd) {
    switch (cmd) {
        case 'list_saved_recordings':
            return Promise.resolve({
                items: [ITEM],
                total: 1,
                total_bytes: 150,
                offset: 0,
                limit: 200,
                path_configured: true,
            });
        case 'get_recording_segments':
            return Promise.resolve(SEGS);
        case 'read_recording_audio':
            return Promise.resolve([0, 1, 2, 3]);
        default:
            return Promise.resolve(null);
    }
}

async function selectFirstRecording() {
    const row = document.querySelector('.rec-row');
    row.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await flush();
    await flush();
}

beforeEach(() => {
    // stubbed per test in loadFresh
});

afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    delete Element.prototype.scrollIntoView;
});

const get = (id) => document.getElementById(id);

describe('list loading', () => {
    it('shows the error bar when the list fails to load', async () => {
        await loadFresh(() => Promise.reject(new Error('磁盘读取失败')));
        await flush();
        const errBar = get('list-error');
        expect(errBar.hidden).toBe(false);
        expect(errBar.textContent).toBe('磁盘读取失败');
    });

    it('renders record_only rows with status badge', async () => {
        await loadFresh(defaultInvoke);
        const rows = document.querySelectorAll('.rec-row');
        expect(rows.length).toBe(1);
        expect(rows[0].querySelector('.badge-done').textContent).toBe('已转录');
    });
});

describe('transcription-error event', () => {
    // C5 fix: events are app-global; without a local in-flight stem the
    // window ignores them (e.g. window re-created mid-transcription).
    it('is ignored when no transcription is in flight (stem guard)', async () => {
        await loadFresh(defaultInvoke);
        listeners['transcription-error']({
            payload: { message: '转录失败，请查看日志' },
        });
        expect(get('toast').hidden).toBe(true);
    });

    it('renders the payload message in the toast while in flight', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        get('btn-transcribe').dispatchEvent(
            new MouseEvent('click', { bubbles: true }),
        );
        await flush();
        listeners['transcription-error']({
            payload: { message: '转录失败，请查看日志' },
        });
        expect(get('toast').hidden).toBe(false);
        expect(get('toast').textContent).toBe('转录失败，请查看日志');
        expect(get('toast').classList.contains('error')).toBe(true);
        // Phase reset: buttons actionable again.
        expect(get('btn-transcribe').disabled).toBe(false);
        expect(get('btn-cancel').hidden).toBe(true);
    });
});

describe('stale/foreign transcription events (C5 stem guard)', () => {
    it('ignores all 4 transcription-* events when stem is null', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        const fillBefore = get('progress-fill').style.transform;
        listeners['transcription-progress']({
            payload: { percent: 42, stage: 'whisper' },
        });
        listeners['transcription-done']({
            payload: { filename: ITEM.filename },
        });
        listeners['transcription-cancelled']({
            payload: { filename: ITEM.filename },
        });
        listeners['transcription-error']({ payload: { message: 'x' } });
        expect(get('progress-fill').style.transform).toBe(fillBefore);
        expect(get('toast').hidden).toBe(true);
        expect(get('progress-wrap').hidden).toBe(true);
    });

    it('ignores done/cancelled for a different filename while in flight', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        get('btn-transcribe').dispatchEvent(
            new MouseEvent('click', { bubbles: true }),
        );
        await flush();
        listeners['transcription-done']({ payload: { filename: 'other' } });
        listeners['transcription-cancelled']({
            payload: { filename: 'other' },
        });
        // Still transcribing: no toast, cancel visible, button disabled.
        expect(get('toast').hidden).toBe(true);
        expect(get('btn-cancel').hidden).toBe(false);
        expect(get('btn-transcribe').disabled).toBe(true);
    });

    it('done with the matching stem finishes and reloads the detail', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        invokeMock.mockClear();
        get('btn-transcribe').dispatchEvent(
            new MouseEvent('click', { bubbles: true }),
        );
        await flush();
        listeners['transcription-done']({
            payload: { filename: ITEM.filename },
        });
        await flush();
        expect(get('toast').textContent).toBe('转录完成');
        expect(get('btn-cancel').hidden).toBe(true);
        // Rule 8: stored segments re-read after completion.
        expect(
            invokeMock.mock.calls.some(
                ([cmd]) => cmd === 'get_recording_segments',
            ),
        ).toBe(true);
    });
});

describe('audio error event', () => {
    it('reveals the audio error badge', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        get('audio').dispatchEvent(new Event('error'));
        expect(get('audio-error').hidden).toBe(false);
    });
});

describe('zero-segment transcription', () => {
    it('shows the empty state and disables inject when whisper returns 0 segments', async () => {
        await loadFresh((cmd) => {
            if (cmd === 'get_recording_segments') {
                return Promise.resolve({
                    segments: [],
                    duration_ms: 4000,
                    transcription_status: 'done',
                    dropped_blocks: 0,
                });
            }
            return defaultInvoke(cmd);
        });
        await selectFirstRecording();
        expect(get('segments-empty').hidden).toBe(false);
        expect(document.querySelectorAll('.segment-row').length).toBe(0);
        expect(get('btn-inject').disabled).toBe(true);
        // And injection is never attempted with empty merged text.
        get('btn-inject').dispatchEvent(
            new MouseEvent('click', { bubbles: true }),
        );
        await flush();
        expect(
            invokeMock.mock.calls.some(
                ([cmd]) => cmd === 'inject_transcript_text',
            ),
        ).toBe(false);
    });
});

describe('segment interactions', () => {
    it('renders stored segments for a done recording (rule 8)', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        const inputs = document.querySelectorAll('.segment-text');
        expect(inputs.length).toBe(2);
        expect(inputs[0].value).toBe('第一段。');
        expect(get('merged').value).toBe('第一段。第二段。');
    });

    it('clicking the timestamp badge seeks the audio (rule 3)', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        const badges = document.querySelectorAll('.segment-ts');
        badges[1].dispatchEvent(new MouseEvent('click', { bubbles: true }));
        expect(get('audio').currentTime).toBeCloseTo(2.0, 5);
    });

    it('Enter inside a segment input never injects (rule 6)', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        const input = document.querySelector('.segment-text');
        input.dispatchEvent(
            new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }),
        );
        await flush();
        expect(
            invokeMock.mock.calls.some(
                ([cmd]) => cmd === 'inject_transcript_text',
            ),
        ).toBe(false);
    });

    it('segment edit updates merged text only (rule 5)', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        const inputs = document.querySelectorAll('.segment-text');
        inputs[0].value = '编辑后的第一段。';
        inputs[0].dispatchEvent(new Event('input', { bubbles: true }));
        expect(get('merged').value).toBe('编辑后的第一段。第二段。');
    });
});

describe('playback highlight auto-scroll (rule 4)', () => {
    it('suppresses scrollIntoView while a segment input is focused', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        const input = document.querySelector('.segment-text');
        input.focus();
        expect(document.activeElement).toBe(input);

        const audio = get('audio');
        audio.currentTime = 3.0; // segment index 1
        audio.dispatchEvent(new Event('timeupdate'));
        expect(scrollSpy).not.toHaveBeenCalled();

        const rows = document.querySelectorAll('.segment-row');
        expect(rows[1].classList.contains('active')).toBe(true);
    });

    it('scrolls when focus is outside the segment inputs', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        const audio = get('audio');
        audio.currentTime = 3.0;
        audio.dispatchEvent(new Event('timeupdate'));
        expect(scrollSpy).toHaveBeenCalledTimes(1);
    });
});

describe('transcription lifecycle UI', () => {
    it('locks list switching while transcribing (rule 2)', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        get('btn-transcribe').dispatchEvent(
            new MouseEvent('click', { bubbles: true }),
        );
        await flush();
        // In-flight: transcribe button disabled, cancel visible, rows disabled.
        expect(get('btn-transcribe').disabled).toBe(true);
        expect(get('btn-cancel').hidden).toBe(false);
        expect(
            document.querySelector('.rec-row').classList.contains('disabled'),
        ).toBe(true);
        expect(
            invokeMock.mock.calls.some(
                ([cmd, args]) =>
                    cmd === 'transcribe_recording' &&
                    args.filename === ITEM.filename,
            ),
        ).toBe(true);
    });

    it('progress event updates the bar and stage label', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        get('btn-transcribe').dispatchEvent(
            new MouseEvent('click', { bubbles: true }),
        );
        await flush();
        listeners['transcription-progress']({
            payload: { percent: 42, stage: 'whisper' },
        });
        expect(get('progress-fill').style.transform).toBe('scaleX(0.42)');
        expect(get('progress-label').textContent).toBe('转录中 42%');
        expect(get('progress-wrap').getAttribute('aria-valuenow')).toBe('42');
        listeners['transcription-progress']({
            payload: { percent: 100, stage: 'llm' },
        });
        expect(get('progress-label').textContent).toBe('LLM 纠错中…');
    });

    it('cancelled event restores buttons and keeps status pending', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        get('btn-transcribe').dispatchEvent(
            new MouseEvent('click', { bubbles: true }),
        );
        await flush();
        listeners['transcription-cancelled']({
            payload: { filename: ITEM.filename },
        });
        expect(get('btn-transcribe').disabled).toBe(false);
        expect(get('btn-cancel').hidden).toBe(true);
        expect(get('progress-wrap').hidden).toBe(true);
    });

    it('startup failure resets phase and clears the stem', async () => {
        await loadFresh((cmd) => {
            if (cmd === 'transcribe_recording') {
                return Promise.reject(new Error('启动失败'));
            }
            return defaultInvoke(cmd);
        });
        await selectFirstRecording();
        get('btn-transcribe').dispatchEvent(
            new MouseEvent('click', { bubbles: true }),
        );
        await flush();
        await flush();
        expect(get('toast').textContent).toBe('启动失败');
        expect(get('btn-transcribe').disabled).toBe(false);
        // Stem cleared: a late done event for the same file is ignored.
        listeners['transcription-done']({
            payload: { filename: ITEM.filename },
        });
        expect(get('toast').textContent).toBe('启动失败');
    });
});

// ---------------------------------------------------------------------------
// Phase guards (BC(C5)#1: unified phase!==IDLE entry guard + visual lock)
// ---------------------------------------------------------------------------

const ITEM2 = {
    ...ITEM,
    filename: '2026-08-18_11-00-00',
    transcription_status: 'pending',
};

function twoItemInvoke(cmd) {
    if (cmd === 'list_saved_recordings') {
        return Promise.resolve({
            items: [ITEM, ITEM2],
            total: 2,
            total_bytes: 300,
            offset: 0,
            limit: 200,
        });
    }
    return defaultInvoke(cmd);
}

function clickRow(filename) {
    document
        .querySelector(`.rec-row[data-filename="${filename}"]`)
        .dispatchEvent(new MouseEvent('click', { bubbles: true }));
}

describe('phase entry guards', () => {
    it('loading blocks list switching and startTranscription, rows grayed', async () => {
        let resolveSegs;
        await loadFresh((cmd) => {
            if (cmd === 'get_recording_segments') {
                return new Promise((r) => {
                    resolveSegs = r;
                });
            }
            if (cmd === 'read_recording_audio') return Promise.resolve([0]);
            return twoItemInvoke(cmd);
        });
        await flush();

        clickRow(ITEM.filename); // starts LOADING (segments deferred)
        await flush();
        expect(document.querySelectorAll('.rec-row.disabled').length).toBe(2);

        clickRow(ITEM2.filename); // blocked by the phase guard
        get('btn-transcribe').dispatchEvent(
            new MouseEvent('click', { bubbles: true }),
        ); // also blocked
        await flush();
        const segCalls = () =>
            invokeMock.mock.calls.filter(
                ([cmd]) => cmd === 'get_recording_segments',
            );
        expect(segCalls().length).toBe(1);
        expect(
            invokeMock.mock.calls.some(
                ([cmd]) => cmd === 'transcribe_recording',
            ),
        ).toBe(false);

        resolveSegs(SEGS); // finish loading → phase back to IDLE
        await flush();
        await flush();
        expect(document.querySelectorAll('.rec-row.disabled').length).toBe(0);

        clickRow(ITEM2.filename); // now allowed
        await flush();
        await flush();
        expect(segCalls().length).toBe(2);
    });

    it('injecting blocks list switching and grays rows', async () => {
        let resolveInject;
        await loadFresh((cmd) => {
            if (cmd === 'inject_transcript_text') {
                return new Promise((r) => {
                    resolveInject = r;
                });
            }
            return twoItemInvoke(cmd);
        });
        await flush();
        clickRow(ITEM.filename);
        await flush();
        await flush();

        get('btn-inject').dispatchEvent(
            new MouseEvent('click', { bubbles: true }),
        );
        await flush();
        expect(get('inject-spinner').hidden).toBe(false);
        expect(document.querySelectorAll('.rec-row.disabled').length).toBe(2);

        clickRow(ITEM2.filename); // blocked while injecting
        await flush();
        expect(
            invokeMock.mock.calls.filter(
                ([cmd, args]) =>
                    cmd === 'get_recording_segments' &&
                    args.filename === ITEM2.filename,
            ).length,
        ).toBe(0);

        resolveInject(null);
        await flush();
        await flush();
        expect(get('inject-spinner').hidden).toBe(true);
        expect(document.querySelectorAll('.rec-row.disabled').length).toBe(0);
    });

    it('catch exit resets the phase (failed load does not lock the list)', async () => {
        await loadFresh((cmd) => {
            if (cmd === 'read_recording_audio') {
                return Promise.reject(new Error('读取失败'));
            }
            return twoItemInvoke(cmd);
        });
        await flush();

        clickRow(ITEM.filename); // load fails → catch → toast
        await flush();
        await flush();
        expect(get('toast').textContent).toBe('读取失败');

        clickRow(ITEM2.filename); // phase must be IDLE again
        await flush();
        await flush();
        expect(
            invokeMock.mock.calls.some(
                ([cmd, args]) =>
                    cmd === 'get_recording_segments' &&
                    args.filename === ITEM2.filename,
            ),
        ).toBe(true);
    });

    it('catch-stale exit resets the phase (selection cleared mid-load)', async () => {
        let listItems = [ITEM];
        let resolveSegs;
        await loadFresh((cmd) => {
            if (cmd === 'list_saved_recordings') {
                return Promise.resolve({
                    items: listItems,
                    total: listItems.length,
                    total_bytes: 0,
                    offset: 0,
                    limit: 200,
                });
            }
            if (cmd === 'get_recording_segments') {
                return new Promise((r) => {
                    resolveSegs = r;
                });
            }
            if (cmd === 'read_recording_audio') return Promise.resolve([0]);
            return Promise.resolve(null);
        });
        await flush();

        clickRow(ITEM.filename); // LOADING, segments deferred
        await flush();

        // Recording deleted elsewhere: focus refresh clears the selection.
        listItems = [];
        window.dispatchEvent(new Event('focus'));
        await flush();
        await flush();

        resolveSegs(SEGS); // stale path in try → finally must still reset
        await flush();
        await flush();

        // Re-list the recording and select it again — only possible from IDLE.
        listItems = [ITEM];
        window.dispatchEvent(new Event('focus'));
        await flush();
        await flush();
        expect(
            document.querySelector('.rec-row').classList.contains('disabled'),
        ).toBe(false);
        clickRow(ITEM.filename);
        await flush();
        await flush();
        expect(
            invokeMock.mock.calls.filter(
                ([cmd]) => cmd === 'get_recording_segments',
            ).length,
        ).toBe(2);
    });
});

describe('segments loading skeleton (LOADING phase)', () => {
    it('clears stale segments and shows skeleton while loading', async () => {
        let resolveSecond;
        let segCalls = 0;
        await loadFresh((cmd) => {
            if (cmd === 'get_recording_segments') {
                segCalls += 1;
                // First recording resolves immediately (establishes stale
                // content); second stays deferred (holds LOADING in flight).
                if (segCalls === 1) return Promise.resolve(SEGS);
                return new Promise((r) => {
                    resolveSecond = r;
                });
            }
            if (cmd === 'read_recording_audio') return Promise.resolve([0]);
            return twoItemInvoke(cmd);
        });
        await flush();

        // First recording loads fully — stale content in #segments.
        clickRow(ITEM.filename);
        await flush();
        await flush();
        expect(get('segments').children.length).toBe(2);

        // Switch to the second recording — LOADING in flight.
        clickRow(ITEM2.filename);
        await flush();
        expect(get('segments-loading').hidden).toBe(false);
        // Screen-reader cue: the skeleton itself is aria-hidden.
        expect(get('detail').getAttribute('aria-busy')).toBe('true');
        // BehaviorChange lock: clearDetail wiped the stale segment rows and
        // hid the empty-state badge.
        expect(get('segments').children.length).toBe(0);
        expect(get('segments-empty').hidden).toBe(true);

        resolveSecond(SEGS);
        await flush();
        await flush();
        // Loaded: skeleton hidden again, real segments rendered.
        expect(get('segments-loading').hidden).toBe(true);
        expect(get('detail').getAttribute('aria-busy')).toBe('false');
        expect(get('segments').children.length).toBe(2);
    });
});

describe('live inject-button refresh (BC(C5)#2)', () => {
    it('clearing all segment text disables inject; restoring re-enables', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        const btn = get('btn-inject');
        expect(btn.disabled).toBe(false);

        const inputs = document.querySelectorAll('.segment-text');
        for (const input of inputs) {
            input.value = '';
            input.dispatchEvent(new Event('input', { bubbles: true }));
        }
        expect(btn.disabled).toBe(true);

        inputs[0].value = '恢复文本';
        inputs[0].dispatchEvent(new Event('input', { bubbles: true }));
        expect(btn.disabled).toBe(false);
    });
});

describe('injectTargetLabel', () => {
    it('shows the target window title when present', () => {
        expect(injectTargetLabel('WeChat')).toEqual({
            text: '将粘贴到：WeChat',
            muted: false,
        });
    });
    it('falls back to the reopen hint for null/empty titles', () => {
        expect(injectTargetLabel(null)).toEqual({
            text: '重新打开窗口以选择粘贴目标',
            muted: true,
        });
        expect(injectTargetLabel('   ')).toEqual({
            text: '重新打开窗口以选择粘贴目标',
            muted: true,
        });
    });
});

describe('llmConfigured', () => {
    it('is true only when all LLM fields are set and enabled', () => {
        expect(
            llmConfigured({
                llm_enabled: true,
                llm_api_url: 'http://x',
                llm_api_key: '__MASKED__',
                llm_model: 'm',
            }),
        ).toBe(true);
    });
    it('is false when disabled', () => {
        expect(
            llmConfigured({
                llm_enabled: false,
                llm_api_url: 'http://x',
                llm_api_key: 'k',
                llm_model: 'm',
            }),
        ).toBe(false);
    });
    it('is false when any field is empty', () => {
        expect(
            llmConfigured({
                llm_enabled: true,
                llm_api_url: '',
                llm_api_key: 'k',
                llm_model: 'm',
            }),
        ).toBe(false);
        expect(llmConfigured(null)).toBe(false);
    });
});

describe('recording list keyboard navigation', () => {
    it('ArrowDown/ArrowUp move focus cyclically across rows', async () => {
        await loadFresh(twoItemInvoke);
        await flush();
        const rows = document.querySelectorAll('.rec-row');
        rows[0].focus();

        rows[0].dispatchEvent(
            new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }),
        );
        expect(document.activeElement).toBe(rows[1]);

        rows[1].dispatchEvent(
            new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }),
        );
        expect(document.activeElement).toBe(rows[0]); // cyclic wrap

        rows[0].dispatchEvent(
            new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }),
        );
        expect(document.activeElement).toBe(rows[1]);
    });

    it('Enter on a focused row selects it (segments fetched, roving tab stop moves)', async () => {
        await loadFresh(twoItemInvoke);
        await flush();
        const rows = document.querySelectorAll('.rec-row');
        rows[1].focus();

        rows[1].dispatchEvent(
            new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }),
        );
        await flush();
        await flush();

        expect(
            invokeMock.mock.calls.map((c) => c[0]),
        ).toContain('get_recording_segments');
        const freshRows = document.querySelectorAll('.rec-row');
        expect(freshRows[1].getAttribute('aria-selected')).toBe('true');
        expect(freshRows[1].tabIndex).toBe(0);
        expect(freshRows[0].tabIndex).toBe(-1);
    });

    it('rows expose listbox semantics with exactly one roving tab stop', async () => {
        await loadFresh(twoItemInvoke);
        await flush();
        const list = document.getElementById('rec-list');
        expect(list.getAttribute('role')).toBe('listbox');
        expect(list.getAttribute('aria-label')).toBe('录音列表');

        const rows = document.querySelectorAll('.rec-row');
        expect(rows[0].getAttribute('role')).toBe('option');
        const stops = [...rows].filter((r) => r.tabIndex === 0);
        expect(stops).toHaveLength(1);
        expect(stops[0]).toBe(rows[0]); // no selection → first row is the stop
    });
});

describe('re-transcription edit-wipe confirmation', () => {
    it('confirm=false with pending edits aborts without invoking transcribe_recording', async () => {
        await loadFresh(defaultInvoke);
        await selectFirstRecording();
        // Simulate a user edit on segment 0 (edits are module-private;
        // drive them through the segment input).
        const input = document.querySelector('.segment-text');
        if (input) {
            input.value = 'edited text';
            input.dispatchEvent(new Event('input', { bubbles: true }));
        }
        const { confirmDialog } = await import('../ui/lib/confirm-dialog.js');
        confirmDialog.mockResolvedValueOnce(false);

        document.getElementById('btn-transcribe').click();
        await flush();

        expect(confirmDialog).toHaveBeenCalledWith({
            title: '重新转录',
            message: '重新转录将清除当前所有编辑，确定继续？',
            danger: true,
        });
        expect(
            invokeMock.mock.calls.map((c) => c[0]),
        ).not.toContain('transcribe_recording');
    });

    it('confirm=true proceeds to transcribe_recording', async () => {
        await loadFresh((cmd) => {
            if (cmd === 'transcribe_recording') return Promise.resolve(null);
            return defaultInvoke(cmd);
        });
        await selectFirstRecording();
        const input = document.querySelector('.segment-text');
        if (input) {
            input.value = 'edited text';
            input.dispatchEvent(new Event('input', { bubbles: true }));
        }
        const { confirmDialog } = await import('../ui/lib/confirm-dialog.js');
        confirmDialog.mockResolvedValueOnce(true);

        document.getElementById('btn-transcribe').click();
        await flush();

        expect(
            invokeMock.mock.calls.map((c) => c[0]),
        ).toContain('transcribe_recording');
    });
});
