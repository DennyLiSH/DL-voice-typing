// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import {
    buildModelOptions,
    populateTranscribeModelSelect,
} from '../ui/lib/transcribe-models.js';

describe('buildModelOptions', () => {
    it('lists only downloaded built-ins under the built-in group', () => {
        const opts = buildModelOptions({
            built_in: { tiny: true, base: false, 'base-q8_0': true },
            custom: [],
        });
        expect(opts[0]).toEqual({ value: '', label: '默认模型', group: null });
        expect(opts.filter((o) => o.value === 'tiny')).toHaveLength(1);
        expect(opts.filter((o) => o.value === 'base')).toHaveLength(0);
        expect(opts.find((o) => o.value === 'base-q8_0').group).toBe(
            '内置模型',
        );
    });

    it('lists custom models under the custom group', () => {
        const opts = buildModelOptions({ built_in: {}, custom: ['m.bin'] });
        expect(opts.find((o) => o.value === 'custom:m.bin').group).toBe(
            '自定义模型',
        );
    });

    it('degrades to default-only when nothing is available', () => {
        expect(buildModelOptions({ built_in: {}, custom: [] })).toEqual([
            { value: '', label: '默认模型', group: null },
        ]);
    });
});

describe('populateTranscribeModelSelect', () => {
    it('builds optgroups and unhides when options exist', () => {
        const select = document.createElement('select');
        populateTranscribeModelSelect(
            select,
            buildModelOptions({ built_in: { tiny: true }, custom: ['m.bin'] }),
        );
        const groups = select.querySelectorAll('optgroup');
        expect(groups).toHaveLength(2);
        expect(groups[0].label).toBe('内置模型');
        expect(select.querySelectorAll('option')).toHaveLength(3);
        expect(select.hidden).toBe(false);
    });

    it('hides the select when only the default option exists', () => {
        const select = document.createElement('select');
        populateTranscribeModelSelect(
            select,
            buildModelOptions({ built_in: {}, custom: [] }),
        );
        expect(select.querySelectorAll('option')).toHaveLength(1);
        expect(select.hidden).toBe(true);
    });
});
