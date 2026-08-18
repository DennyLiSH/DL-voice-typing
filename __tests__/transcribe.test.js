// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
    filterRecordOnly,
    findActiveSegmentIndex,
    formatTimestamp,
    mergeSegmentTexts,
    shouldAutoScroll,
    statusBadge,
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
    <div id="rec-list"></div>
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
    it('renders the payload message in the toast', async () => {
        await loadFresh(defaultInvoke);
        listeners['transcription-error']({
            payload: { message: '转录失败，请查看日志' },
        });
        expect(get('toast').hidden).toBe(false);
        expect(get('toast').textContent).toBe('转录失败，请查看日志');
        expect(get('toast').classList.contains('error')).toBe(true);
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
        expect(get('progress-fill').style.width).toBe('42%');
        expect(get('progress-label').textContent).toBe('转录中 42%');
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
});
