/**
 * @vitest-environment jsdom
 */
import { beforeEach, describe, expect, it } from 'vitest';
import {
    buildExpandedMetadata,
    buildRecordingRow,
    computeOffsetAfterDeletion,
    deleteConfirmMessage,
    formatBytes,
    formatDuration,
    formatStemForDisplay,
    getPageRange,
    previewText,
    truncateText,
} from '../ui/lib/data-management.js';

describe('formatBytes', () => {
    it('returns 0 B for zero', () => {
        expect(formatBytes(0)).toBe('0 B');
    });

    it('returns 0 B for negative', () => {
        expect(formatBytes(-100)).toBe('0 B');
    });

    it('formats bytes below 1024', () => {
        expect(formatBytes(1)).toBe('1 B');
        expect(formatBytes(500)).toBe('500 B');
        expect(formatBytes(1023)).toBe('1023 B');
    });

    it('formats KB at boundary', () => {
        expect(formatBytes(1024)).toBe('1.0 KB');
        expect(formatBytes(1536)).toBe('1.5 KB');
    });

    it('formats MB', () => {
        expect(formatBytes(1048576)).toBe('1.0 MB');
        expect(formatBytes(1572864)).toBe('1.5 MB');
        expect(formatBytes(1048576 * 234)).toBe('234.0 MB');
    });

    it('formats GB', () => {
        expect(formatBytes(1073741824)).toBe('1.0 GB');
        expect(formatBytes(1073741824 * 1.5)).toBe('1.5 GB');
    });

    it('formats TB for very large', () => {
        expect(formatBytes(1099511627776)).toBe('1.0 TB');
    });

    it('handles u64-ish large numbers without panic', () => {
        expect(formatBytes(Number.MAX_SAFE_INTEGER)).toMatch(/TB$/);
    });
});

describe('formatDuration', () => {
    it('returns 0s for zero', () => {
        expect(formatDuration(0)).toBe('0s');
    });

    it('returns 0s for negative', () => {
        expect(formatDuration(-1)).toBe('0s');
    });

    it('formats sub-minute', () => {
        expect(formatDuration(1.5)).toBe('1.5s');
        expect(formatDuration(3)).toBe('3s');
        expect(formatDuration(59)).toBe('59s');
    });

    it('formats minutes', () => {
        expect(formatDuration(60)).toBe('1m 0s');
        expect(formatDuration(125)).toBe('2m 5s');
    });

    it('formats hours', () => {
        expect(formatDuration(3600)).toBe('1h 0m');
        expect(formatDuration(3600 * 86400 + 60)).toBe('86400h 1m');
    });
});

describe('truncateText', () => {
    it('returns empty for non-string', () => {
        expect(truncateText(null)).toBe('');
        expect(truncateText(undefined)).toBe('');
    });

    it('returns unchanged when short enough', () => {
        expect(truncateText('hello', 30)).toBe('hello');
        expect(truncateText('exactly30chars_exactly30chars', 30)).toBe(
            'exactly30chars_exactly30chars',
        );
    });

    it('truncates with ellipsis when too long', () => {
        expect(truncateText('abcdefghijklmnopqrstuvwxyz', 10)).toBe(
            'abcdefghij…',
        );
    });

    it('uses default maxLen of 30', () => {
        expect(truncateText('a'.repeat(40))).toBe(`${'a'.repeat(30)}…`);
    });
});

describe('formatStemForDisplay', () => {
    it('formats a valid stem', () => {
        expect(formatStemForDisplay('2026-06-24_14-30-25')).toBe(
            '2026-06-24 14:30:25',
        );
    });

    it('returns input for malformed stem', () => {
        expect(formatStemForDisplay('')).toBe('');
        expect(formatStemForDisplay('short')).toBe('short');
        expect(formatStemForDisplay(null)).toBe('');
    });
});

describe('getPageRange', () => {
    it('returns page 1/1 with 0 items', () => {
        const r = getPageRange(0, 0, 50);
        expect(r.currentPage).toBe(1);
        expect(r.totalPages).toBe(1);
        expect(r.hasNext).toBe(false);
        expect(r.hasPrev).toBe(false);
    });

    it('handles single page', () => {
        const r = getPageRange(25, 0, 50);
        expect(r.currentPage).toBe(1);
        expect(r.totalPages).toBe(1);
        expect(r.hasNext).toBe(false);
    });

    it('handles exact multiple of limit', () => {
        const r = getPageRange(100, 0, 50);
        expect(r.totalPages).toBe(2);
        expect(getPageRange(100, 50, 50).currentPage).toBe(2);
    });

    it('handles partial last page', () => {
        const r = getPageRange(101, 100, 50);
        expect(r.currentPage).toBe(3);
        expect(r.totalPages).toBe(3);
        expect(r.hasNext).toBe(false);
        expect(getPageRange(101, 0, 50).hasNext).toBe(true);
    });
});

describe('computeOffsetAfterDeletion', () => {
    it('stays when items remain on current page', () => {
        // 100 items, page 2 (offset 50), delete 1 → 99 items, last valid offset still 50
        expect(computeOffsetAfterDeletion(100, 1, 50, 50)).toBe(50);
    });

    it('jumps back when current page becomes empty', () => {
        // 100 items, last page (offset 50) has 50 items, delete all 50 → offset stays at 0 (only 1 page now)
        expect(computeOffsetAfterDeletion(100, 50, 50, 50)).toBe(0);
    });

    it('handles deleting to empty', () => {
        // 5 items on page 1, delete all → offset stays 0
        expect(computeOffsetAfterDeletion(5, 5, 50, 0)).toBe(0);
    });

    it('jumps back to previous page when last page becomes empty', () => {
        // 51 items, offset 50 (last page, 1 item), delete 1 → 50 items, last valid offset 0
        expect(computeOffsetAfterDeletion(51, 1, 50, 50)).toBe(0);
    });

    it('keeps current offset when middle of pagination', () => {
        expect(computeOffsetAfterDeletion(200, 5, 50, 50)).toBe(50);
    });
});

describe('deleteConfirmMessage', () => {
    it('returns singular message for count=1', () => {
        const msg = deleteConfirmMessage(1);
        expect(msg).toContain('永久删除');
        expect(msg).toContain('不可恢复');
        expect(msg).toContain('不会进入回收站');
        expect(msg).not.toContain('1');
    });

    it('returns plural message with count for count>1', () => {
        const msg = deleteConfirmMessage(5);
        expect(msg).toContain('永久删除');
        expect(msg).toContain('5');
        expect(msg).toContain('不可恢复');
    });
});

describe('buildRecordingRow', () => {
    beforeEach(() => {
        document.body.innerHTML = '';
    });

    const baseEntry = {
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
    };

    it('builds row with data-filename attribute', () => {
        const row = buildRecordingRow(baseEntry);
        expect(row.dataset.filename).toBe('2026-06-24_14-30-25');
        expect(row.classList.contains('data-row')).toBe(true);
    });

    it('includes checkbox, timestamp, duration, preview, play, delete', () => {
        const row = buildRecordingRow(baseEntry);
        expect(row.querySelector('.data-row-cb')).not.toBeNull();
        expect(row.querySelector('.data-row-ts').textContent).toBe(
            '2026-06-24 14:30:25',
        );
        expect(row.querySelector('.data-row-dur').textContent).toBe('3.2s');
        expect(row.querySelector('.data-row-preview').textContent).toBe(
            '今天去开会',
        );
        expect(row.querySelector('.btn-play')).not.toBeNull();
        expect(row.querySelector('.btn-delete')).not.toBeNull();
    });

    it('hides play button and shows audio-missing-badge when wav_size===0', () => {
        const entry = { ...baseEntry, wav_size: 0 };
        const row = buildRecordingRow(entry);
        expect(row.querySelector('.btn-play')).toBeNull();
        expect(row.querySelector('.audio-missing-badge').textContent).toBe(
            '音频缺失',
        );
    });

    it('does NOT include tabindex on .data-row (constraint F4: rows not focusable)', () => {
        const row = buildRecordingRow(baseEntry);
        expect(row.getAttribute('tabindex')).toBeNull();
    });

    it('reflects selected state in class and checkbox', () => {
        const row = buildRecordingRow(baseEntry, { selected: true });
        expect(row.classList.contains('selected')).toBe(true);
        expect(row.querySelector('.data-row-cb').checked).toBe(true);
    });

    it('reflects expanded state in class', () => {
        const row = buildRecordingRow(baseEntry, { expanded: true });
        expect(row.classList.contains('expanded')).toBe(true);
    });

    it('escapes HTML in transcription (no innerHTML injection)', () => {
        const evil = {
            ...baseEntry,
            transcription: '<script>alert(1)</script>',
        };
        const row = buildRecordingRow(evil);
        // textContent gives the raw string back, NOT executed.
        expect(row.querySelector('.data-row-preview').textContent).toBe(
            '<script>alert(1)</script>',
        );
        // And the DOM must not contain an actual <script> element.
        expect(row.querySelector('script')).toBeNull();
    });

    it('truncates long transcription in preview', () => {
        const long = 'a'.repeat(50);
        const entry = { ...baseEntry, transcription: long };
        const row = buildRecordingRow(entry);
        const preview = row.querySelector('.data-row-preview').textContent;
        expect(preview.length).toBeLessThan(long.length);
        expect(preview.endsWith('…')).toBe(true);
    });

    it('shows classic source badge and no status badge for classic entries', () => {
        const row = buildRecordingRow({ ...baseEntry, source: 'classic' });
        expect(row.querySelector('.badge-source-classic').textContent).toBe(
            '语音输入',
        );
        expect(row.querySelector('.badge-done')).toBeNull();
        expect(row.querySelector('.badge-warning')).toBeNull();
    });

    it('keeps source badge on collapsed record_only rows but no status/warning badges', () => {
        const entry = {
            ...baseEntry,
            source: 'record_only',
            transcription_status: 'done',
            dropped_blocks: 0,
        };
        const row = buildRecordingRow(entry);
        expect(row.querySelector('.badge-source-record').textContent).toBe(
            '录音',
        );
        expect(row.querySelector('.badge-done')).toBeNull();
        expect(row.querySelector('.badge-pending')).toBeNull();
        expect(row.querySelector('.badge-failed')).toBeNull();
    });

    it('collapsed record_only rows with dropped_blocks have no warning badge', () => {
        const entry = {
            ...baseEntry,
            source: 'record_only',
            transcription_status: 'failed',
            dropped_blocks: 12,
        };
        const row = buildRecordingRow(entry);
        expect(row.querySelector('.badge-warning')).toBeNull();
    });
});

describe('previewText', () => {
    const emptyEntry = (source, transcription_status) => ({
        source,
        transcription_status,
        transcription: null,
        final_text: null,
        llm_corrected: null,
    });

    it('returns placeholder 转录失败 for failed record-only rows', () => {
        expect(previewText(emptyEntry('record_only', 'failed'))).toEqual({
            text: '转录失败',
            placeholder: true,
        });
    });

    it('returns placeholder 未转录 for pending record-only rows', () => {
        expect(previewText(emptyEntry('record_only', 'pending'))).toEqual({
            text: '未转录',
            placeholder: true,
        });
    });

    it('truncates transcription text without placeholder', () => {
        const result = previewText({
            transcription: 'a'.repeat(50),
            final_text: null,
            llm_corrected: null,
        });
        expect(result.text.length).toBeLessThan(50);
        expect(result.text.endsWith('…')).toBe(true);
        expect(result.placeholder).toBe(false);
    });

    it('returns empty text for classic rows with no transcription', () => {
        expect(previewText(emptyEntry('classic', undefined))).toEqual({
            text: '',
            placeholder: false,
        });
    });

    it('returns empty text for done record-only rows with all fields empty (no misleading hint)', () => {
        expect(previewText(emptyEntry('record_only', 'done'))).toEqual({
            text: '',
            placeholder: false,
        });
    });

    it('buildRecordingRow adds placeholder class for pending record-only rows', () => {
        const row = buildRecordingRow({
            filename: '2026-06-24_14-30-25',
            source: 'record_only',
            transcription_status: 'pending',
            transcription: null,
            final_text: null,
            llm_corrected: null,
            wav_size: 1024,
        });
        const preview = row.querySelector('.data-row-preview');
        expect(preview.classList.contains('placeholder')).toBe(true);
        expect(preview.textContent).toBe('未转录');
    });

    it('buildRecordingRow does not add placeholder class when transcription exists', () => {
        const row = buildRecordingRow({
            filename: '2026-06-24_14-30-25',
            transcription: '今天去开会',
            wav_size: 1024,
        });
        const preview = row.querySelector('.data-row-preview');
        expect(preview.classList.contains('placeholder')).toBe(false);
        expect(preview.textContent).toBe('今天去开会');
    });
});

describe('buildExpandedMetadata', () => {
    beforeEach(() => {
        document.body.innerHTML = '';
    });

    it('renders four labelled lines for classic entries (language first)', () => {
        const entry = {
            language: 'zh',
            transcription: '原文',
            llm_corrected: '改写',
            final_text: '最终',
        };
        const el = buildExpandedMetadata(entry);
        const lines = el.querySelectorAll('.data-row-meta-line');
        expect(lines.length).toBe(4);
        expect(lines[0].querySelector('.data-row-meta-label').textContent).toBe(
            '语言：',
        );
        expect(lines[0].querySelector('.data-row-meta-value').textContent).toBe(
            'zh',
        );
        expect(lines[1].querySelector('.data-row-meta-label').textContent).toBe(
            '转录：',
        );
        expect(lines[1].querySelector('.data-row-meta-value').textContent).toBe(
            '原文',
        );
        expect(lines[2].querySelector('.data-row-meta-label').textContent).toBe(
            'LLM：',
        );
        expect(lines[3].querySelector('.data-row-meta-label').textContent).toBe(
            '最终：',
        );
    });

    it('renders status line first (five lines) for record_only entries', () => {
        const entry = {
            source: 'record_only',
            transcription_status: 'pending',
            dropped_blocks: 0,
            language: 'zh',
            transcription: '原文',
            llm_corrected: null,
            final_text: null,
        };
        const el = buildExpandedMetadata(entry);
        const lines = el.querySelectorAll('.data-row-meta-line');
        expect(lines.length).toBe(5);
        expect(lines[0].querySelector('.data-row-meta-label').textContent).toBe(
            '状态：',
        );
        expect(lines[0].querySelector('.badge-pending').textContent).toBe(
            '待转录',
        );
        expect(lines[1].querySelector('.data-row-meta-label').textContent).toBe(
            '语言：',
        );
    });

    it('includes dropped-blocks warning badge in status line when dropped_blocks > 0', () => {
        const entry = {
            source: 'record_only',
            transcription_status: 'failed',
            dropped_blocks: 12,
            language: 'zh',
            transcription: null,
            llm_corrected: null,
            final_text: null,
        };
        const el = buildExpandedMetadata(entry);
        expect(el.querySelector('.badge-failed').textContent).toBe('转录失败');
        expect(el.querySelector('.badge-warning').textContent).toBe(
            '音频不完整',
        );
    });

    it('shows （无） for missing fields', () => {
        const entry = {
            language: null,
            transcription: 'x',
            llm_corrected: null,
            final_text: null,
        };
        const el = buildExpandedMetadata(entry);
        const values = el.querySelectorAll('.data-row-meta-value');
        expect(values[0].textContent).toBe('（无）'); // 语言
        expect(values[1].textContent).toBe('x'); // 转录
        expect(values[2].textContent).toBe('（无）'); // LLM
        expect(values[3].textContent).toBe('（无）'); // 最终
    });

    it('escapes HTML safely', () => {
        const entry = {
            language: 'zh',
            transcription: '<b>not bold</b>',
            llm_corrected: null,
            final_text: null,
        };
        const el = buildExpandedMetadata(entry);
        expect(el.querySelector('b')).toBeNull();
        expect(el.querySelectorAll('.data-row-meta-value')[1].textContent).toBe(
            '<b>not bold</b>',
        );
    });

    it('moves language into expanded metadata instead of the row', () => {
        const row = buildRecordingRow({
            filename: '2026-06-24_14-30-25.wav',
            language: 'zh',
        });
        expect(row.querySelector('.data-row-lang')).toBeNull();
        const expanded = buildExpandedMetadata({
            language: 'zh',
            transcription: 't',
        });
        const lines = [...expanded.querySelectorAll('.data-row-meta-line')];
        expect(lines.some((l) => l.textContent.startsWith('语言：'))).toBe(
            true,
        );
    });
});
