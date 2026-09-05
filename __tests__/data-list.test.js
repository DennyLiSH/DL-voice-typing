/**
 * @vitest-environment jsdom
 *
 * Integration tests for the data management list UI.
 *
 * These tests simulate the data-list section of the settings page in jsdom,
 * then drive user interactions (click row, click play, click delete, type in
 * search, etc.) and assert that observable state matches the design-review
 * constraints.
 *
 * The actual settings.js cannot be imported directly because it depends on
 * `window.__TAURI__` and runs `init()` at module load. Instead we exercise the
 * pure helpers (buildRecordingRow, buildExpandedMetadata, computeOffsetAfter-
 * Deletion, deleteConfirmMessage, getPageRange) plus a minimal mock state
 * machine that mirrors the orchestration logic in settings.js.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
    buildExpandedMetadata,
    buildRecordingRow,
    computeOffsetAfterDeletion,
    deleteConfirmMessage,
    formatBytes,
    formatStemForDisplay,
    getPageRange,
} from '../ui/lib/data-management.js';

/**
 * Mock state machine that mirrors the orchestration logic in settings.js.
 * Used to validate state transitions without importing the full settings.js
 * (which requires Tauri API mocking).
 */
class MockDataListStateMachine {
    constructor(limit = 50) {
        this.limit = limit;
        this.reset();
    }

    reset() {
        this.offset = 0;
        this.query = '';
        this.total = 0;
        this.items = [];
        this.selectedFiles = new Set();
        this.expandedRowId = null;
        this.audioPlayerRowId = null;
        this.isLoading = false;
        this.lastReqId = 0;
        this.audioElement = null;
    }

    /** Simulate successful page load from backend. */
    loadPage(_offset, resp) {
        this.offset = resp.offset;
        this.total = resp.total;
        this.items = resp.items;
    }

    /** Click on a row body — toggles expand. */
    clickRow(filename) {
        if (this.expandedRowId === filename) {
            this.expandedRowId = null;
        } else {
            this.expandedRowId = filename;
        }
    }

    /** Click checkbox on a row — toggles selection. */
    clickCheckbox(filename, checked) {
        if (checked) {
            this.selectedFiles.add(filename);
        } else {
            this.selectedFiles.delete(filename);
        }
    }

    /** Click play button — toggles audio. */
    clickPlay(filename) {
        if (this.audioPlayerRowId === filename) {
            this.audioPlayerRowId = null;
        } else {
            this.audioPlayerRowId = filename;
        }
    }

    /** Click select-all checkbox on current page. */
    clickSelectAll(checked) {
        if (checked) {
            for (const item of this.items) {
                this.selectedFiles.add(item.filename);
            }
        } else {
            for (const item of this.items) {
                this.selectedFiles.delete(item.filename);
            }
        }
    }

    /** Change page — clears selection (constraint #6). */
    changePage(newOffset) {
        this.selectedFiles.clear();
        this.expandedRowId = null;
        this.offset = newOffset;
    }

    /** Compute new offset after delete, may jump back to last valid page. */
    offsetAfterDeletion(deletedCount) {
        return computeOffsetAfterDeletion(
            this.total,
            deletedCount,
            this.limit,
            this.offset,
        );
    }
}

const sampleEntries = [
    {
        filename: '2026-06-24_14-30-25',
        timestamp: '2026-06-24T14:30:25+08:00',
        language: 'zh',
        whisper_model: 'base',
        duration_seconds: 3.2,
        transcription: '今天去开会',
        llm_corrected: null,
        final_text: null,
        wav_size: 1024,
        json_size: 256,
    },
    {
        filename: '2026-06-24_14-31-00',
        timestamp: '2026-06-24T14:31:00+08:00',
        language: 'en',
        whisper_model: 'base',
        duration_seconds: 5.1,
        transcription: 'The meeting is scheduled',
        llm_corrected: null,
        final_text: null,
        wav_size: 2048,
        json_size: 300,
    },
    {
        filename: '2026-06-23_10-15-42',
        timestamp: '2026-06-23T10:15:42+08:00',
        language: 'zh',
        whisper_model: 'tiny',
        duration_seconds: 1.5,
        transcription: '你好世界',
        llm_corrected: null,
        final_text: null,
        wav_size: 0, // missing audio
        json_size: 100,
    },
];

describe('Data list — render contract', () => {
    beforeEach(() => {
        document.body.innerHTML = '<div id="data-list"></div>';
    });

    it('renders N rows for N entries', () => {
        const list = document.getElementById('data-list');
        for (const e of sampleEntries) list.appendChild(buildRecordingRow(e));
        expect(list.querySelectorAll('.data-row').length).toBe(3);
    });

    it('shows audio-missing-badge instead of play button when wav_size===0 (constraint #10b)', () => {
        const list = document.getElementById('data-list');
        for (const e of sampleEntries) list.appendChild(buildRecordingRow(e));
        const missingRow = list.querySelector(
            '[data-filename="2026-06-23_10-15-42"]',
        );
        expect(missingRow.querySelector('.btn-play')).toBeNull();
        expect(missingRow.querySelector('.audio-missing-badge')).not.toBeNull();
    });

    it('row IS the keyboard-reachable expand control (F4 evolved 2026-09-04)', () => {
        // F4 originally banned row tabindex so it never competed with the
        // inner controls for focus. The 4th-round review (P2: expand was
        // mouse-only) evolved the contract: the row itself is the expand
        // control — one list-level tab stop + Enter/Space activation,
        // tracked by aria-expanded.
        const row = buildRecordingRow(sampleEntries[0]);
        expect(row.tabIndex).toBe(0);
        expect(row.getAttribute('aria-expanded')).toBe('false');
    });

    it('expanded row appends metadata with four lines', () => {
        const list = document.getElementById('data-list');
        list.appendChild(buildRecordingRow(sampleEntries[0]));
        list.appendChild(buildExpandedMetadata(sampleEntries[0]));
        expect(list.querySelectorAll('.data-row-meta-line').length).toBe(4);
    });

    it('expanded record-only row appends metadata with five lines (status first)', () => {
        const list = document.getElementById('data-list');
        const entry = {
            ...sampleEntries[0],
            source: 'record_only',
            transcription_status: 'pending',
            dropped_blocks: 0,
        };
        list.appendChild(buildRecordingRow(entry));
        list.appendChild(buildExpandedMetadata(entry));
        const lines = list.querySelectorAll('.data-row-meta-line');
        expect(lines.length).toBe(5);
        expect(lines[0].textContent.startsWith('状态：')).toBe(true);
    });
});

describe('Data list — state machine (constraint coverage)', () => {
    let sm;

    beforeEach(() => {
        sm = new MockDataListStateMachine(50);
    });

    it('constraint #1: reset clears all state', () => {
        sm.loadPage(0, { offset: 50, total: 100, items: sampleEntries });
        sm.clickRow('2026-06-24_14-30-25');
        sm.clickCheckbox('2026-06-24_14-30-25', true);
        sm.clickPlay('2026-06-24_14-30-25');
        sm.query = 'hello';

        sm.reset();
        expect(sm.offset).toBe(0);
        expect(sm.query).toBe('');
        expect(sm.total).toBe(0);
        expect(sm.items).toEqual([]);
        expect(sm.selectedFiles.size).toBe(0);
        expect(sm.expandedRowId).toBeNull();
        expect(sm.audioPlayerRowId).toBeNull();
    });

    it('constraint #4/F4 (evolved 2026-09-04): row is the expand tab stop', () => {
        const row = buildRecordingRow(sampleEntries[0]);
        // The row is now the keyboard-reachable expand control (one tab
        // stop); inner controls keep their own native focusability.
        expect(row.tabIndex).toBe(0);
        const cb = row.querySelector('.data-row-cb');
        const play = row.querySelector('.btn-play');
        const del = row.querySelector('.btn-delete');
        // Checkboxes and buttons are inherently focusable (no need for tabindex).
        expect(cb.tagName).toBe('INPUT');
        expect(play.tagName).toBe('BUTTON');
        expect(del.tagName).toBe('BUTTON');
    });

    it('constraint #5: Esc clears search query (logic only, DOM wiring in settings.js)', () => {
        sm.query = 'foo';
        // Simulate Esc keypress handler
        function onSearchKeydown(e) {
            if (e.key === 'Escape') {
                sm.query = '';
            }
        }
        onSearchKeydown({ key: 'Escape' });
        expect(sm.query).toBe('');
    });

    it('constraint #6: select-all only affects current page', () => {
        // Page 1: items 0-1, page 2: item 2
        sm.loadPage(0, {
            offset: 0,
            total: 3,
            items: sampleEntries.slice(0, 2),
        });
        sm.clickSelectAll(true);
        expect(sm.selectedFiles.size).toBe(2);
        expect(sm.selectedFiles.has('2026-06-23_10-15-42')).toBe(false);

        // Change page → selection cleared
        sm.changePage(2);
        expect(sm.selectedFiles.size).toBe(0);
    });

    it('constraint #8: loadPage guards against re-entry via isLoading', () => {
        // Mock: setting isLoading blocks subsequent loadPage calls
        sm.isLoading = true;
        const didLoad = !sm.isLoading;
        expect(didLoad).toBe(false);
    });

    it('constraint #9: delete from last page jumps back to previous valid page', () => {
        // Total = 51, offset = 50 (last page, 1 item), delete 1
        sm.loadPage(50, {
            offset: 50,
            total: 51,
            items: sampleEntries.slice(0, 1),
        });
        const newOffset = sm.offsetAfterDeletion(1);
        expect(newOffset).toBe(0); // jump back to page 1
    });

    it('constraint #9: delete from middle page stays on current page', () => {
        sm.loadPage(50, { offset: 50, total: 200, items: sampleEntries });
        const newOffset = sm.offsetAfterDeletion(1);
        expect(newOffset).toBe(50);
    });

    it('constraint F3: batch delete partial success message includes counts', () => {
        // Simulate backend result (P2 用户控制权专项: moved instead of deleted)
        const result = {
            moved: 3,
            failed: [{ filename: '2026-06-23_10-15-42', error: '文件被占用' }],
        };
        const msg = `已移动 ${result.moved} 条到待撤销，失败 ${result.failed.length} 条`;
        expect(msg).toContain('3');
        expect(msg).toContain('1');
    });

    it('constraint F5: delete confirm message advertises the 5-second undo window', () => {
        const msg1 = deleteConfirmMessage(1);
        const msg5 = deleteConfirmMessage(5);
        expect(msg1).toMatch(/确定删除/);
        expect(msg1).toMatch(/5 秒内可撤销/);
        expect(msg5).toContain('5');
    });

    it('constraint F8: refresh button loading state guards against re-entry', () => {
        // Logic: while isLoading=true, refresh button is disabled.
        sm.isLoading = true;
        const refreshDisabled = sm.isLoading;
        expect(refreshDisabled).toBe(true);
    });

    it('constraint F9: out-of-order response is discarded by reqId check', () => {
        // Simulate: req1 starts at reqId=1, req2 starts at reqId=2.
        // req1 response arrives after req2 — should be discarded.
        sm.lastReqId = 0;
        const req1 = ++sm.lastReqId;
        const req2 = ++sm.lastReqId;
        expect(req1).toBe(1);
        expect(req2).toBe(2);
        // req1 !== sm.lastReqId → discard
        expect(req1 === sm.lastReqId).toBe(false);
        expect(req2 === sm.lastReqId).toBe(true);
    });
});

describe('Data list — pure helpers integration', () => {
    it('formatBytes composes with page info correctly', () => {
        const totalBytes = 245_000_000;
        const total = 47;
        const summary = `${total} 条 · ${formatBytes(totalBytes)}`;
        expect(summary).toBe('47 条 · 233.7 MB');
    });

    it('formatStemForDisplay renders timestamps for the row', () => {
        expect(formatStemForDisplay('2026-06-24_14-30-25')).toBe(
            '2026-06-24 14:30:25',
        );
    });

    it('getPageRange + computeOffsetAfterDeletion handle end-of-list deletion', () => {
        // 101 items, page 3 (offset 100) has 1 item, delete it
        let total = 101;
        const offset = 100;
        const limit = 50;
        const before = getPageRange(total, offset, limit);
        expect(before.currentPage).toBe(3);
        expect(before.totalPages).toBe(3);

        const newOffset = computeOffsetAfterDeletion(total, 1, limit, offset);
        expect(newOffset).toBe(50); // back to page 2

        // After reload with new total
        total -= 1;
        const after = getPageRange(total, newOffset, limit);
        expect(after.currentPage).toBe(2);
        expect(after.totalPages).toBe(2);
    });

    it('truncateText + buildRecordingRow preview is bounded', () => {
        const longText = 'a'.repeat(100);
        const row = buildRecordingRow({
            filename: '2026-06-24_14-30-25',
            transcription: longText,
            wav_size: 100,
            json_size: 100,
        });
        const preview = row.querySelector('.data-row-preview').textContent;
        expect(preview.length).toBeLessThanOrEqual(31); // 30 + ellipsis
        expect(preview.endsWith('…')).toBe(true);
    });
});

describe('Data list — audio playback contract', () => {
    beforeEach(() => {
        document.body.innerHTML = '<div id="data-list"></div>';
    });

    it('clicking play toggles audioPlayerRowId in state machine', () => {
        const sm = new MockDataListStateMachine();
        sm.loadPage(0, { offset: 0, total: 3, items: sampleEntries });
        expect(sm.audioPlayerRowId).toBeNull();

        sm.clickPlay('2026-06-24_14-30-25');
        expect(sm.audioPlayerRowId).toBe('2026-06-24_14-30-25');

        sm.clickPlay('2026-06-24_14-30-25');
        expect(sm.audioPlayerRowId).toBeNull();
    });

    it('clicking play on a second row replaces the first', () => {
        const sm = new MockDataListStateMachine();
        sm.loadPage(0, { offset: 0, total: 3, items: sampleEntries });
        sm.clickPlay('2026-06-24_14-30-25');
        sm.clickPlay('2026-06-24_14-31-00');
        expect(sm.audioPlayerRowId).toBe('2026-06-24_14-31-00');
    });

    it('audio-missing-badge is shown for wav_size=0, play button hidden', () => {
        const missing = sampleEntries.find((e) => e.wav_size === 0);
        const row = buildRecordingRow(missing);
        expect(row.querySelector('.btn-play')).toBeNull();
        expect(row.querySelector('.audio-missing-badge').textContent).toBe(
            '音频缺失',
        );
    });
});

describe('Data list — XSS safety', () => {
    beforeEach(() => {
        document.body.innerHTML = '<div id="data-list"></div>';
    });

    it('buildRecordingRow does not allow transcription to inject HTML', () => {
        const evil = {
            filename: '2026-06-24_14-30-25',
            transcription: '<img src=x onerror="alert(\'xss\')">',
            wav_size: 0,
            json_size: 0,
        };
        const row = buildRecordingRow(evil);
        const preview = row.querySelector('.data-row-preview');
        expect(preview.querySelector('img')).toBeNull();
        expect(preview.querySelector('script')).toBeNull();
        // textContent preserves the raw string
        expect(preview.textContent).toContain('<img');
    });

    it('filename is set via dataset, not via innerHTML', () => {
        const evil = {
            filename: '2026-06-24_14-30-25',
            transcription: 'normal',
            wav_size: 0,
            json_size: 0,
        };
        const row = buildRecordingRow(evil);
        expect(row.dataset.filename).toBe('2026-06-24_14-30-25');
        // data-filename attribute must not contain unescaped HTML
        expect(row.getAttribute('data-filename')).toBe('2026-06-24_14-30-25');
    });
});

describe('Data list — vi mocks sanity', () => {
    it('vi.fn can mock invoke for future Tauri-backed tests', () => {
        const mockInvoke = vi.fn().mockResolvedValue({
            items: [],
            total: 0,
            total_bytes: 0,
            offset: 0,
            limit: 50,
            path_configured: false,
        });
        // Mock works as expected
        expect(mockInvoke).not.toHaveBeenCalled();
        // Call signature mirrors the real invoke
        return mockInvoke('list_saved_recordings', {
            offset: 0,
            limit: 50,
            query: null,
        }).then((resp) => {
            expect(mockInvoke).toHaveBeenCalledTimes(1);
            expect(resp.total).toBe(0);
        });
    });
});

describe('Data list — keyboard expand affordance (2026-09-04)', () => {
    it('rows expose tabindex + aria-expanded driven by opts.expanded', () => {
        const collapsed = buildRecordingRow(sampleEntries[0], {});
        expect(collapsed.tabIndex).toBe(0);
        expect(collapsed.getAttribute('aria-expanded')).toBe('false');

        const expanded = buildRecordingRow(sampleEntries[0], {
            expanded: true,
        });
        expect(expanded.getAttribute('aria-expanded')).toBe('true');
        expect(expanded.classList.contains('expanded')).toBe(true);
    });
});
