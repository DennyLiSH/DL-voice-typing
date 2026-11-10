/**
 * Behaviour tests for the transcribe window's decision layer.
 *
 * `ui/transcribe.js` has no behaviour tests of its own — the only test
 * file imported the pure `ui/lib/transcribe.js`. These tests cover the
 * three accident areas the controller now owns (the file's own comment
 * carries the bug history: a phase-reset leak that locked list
 * switching forever, and a guard that fired twice):
 *
 * - the phase state machine (mutually exclusive operations),
 * - the transcription ownership token (stale events must be ignored),
 * - the merged-edit lock (segment edits must not clobber a full-text
 *   edit).
 */
import { describe, expect, it } from 'vitest';
import { PHASE } from '../ui/lib/transcribe.js';
import { createTranscribeController } from '../ui/lib/transcribe-controller.js';

function makeController() {
    const renders = [];
    const controller = createTranscribeController({
        onRender: (state) => renders.push(state),
    });
    return { controller, renders };
}

describe('transcribe controller — phase state machine', () => {
    it('starts idle with nothing selected', () => {
        const { controller } = makeController();
        expect(controller.phase()).toBe(PHASE.IDLE);
        expect(controller.selectedStem()).toBeNull();
        expect(controller.transcribingStem()).toBeNull();
    });

    it('accepts idle -> loading -> idle', () => {
        const { controller } = makeController();
        expect(controller.setPhase(PHASE.LOADING)).toBe(true);
        expect(controller.phase()).toBe(PHASE.LOADING);
        expect(controller.setPhase(PHASE.IDLE)).toBe(true);
        expect(controller.phase()).toBe(PHASE.IDLE);
    });

    it('rejects a transition that would start a second operation', () => {
        const { controller } = makeController();
        controller.setPhase(PHASE.LOADING);
        // Loading -> transcribing is not a legal transition: the phases
        // are mutually exclusive.
        expect(controller.setPhase(PHASE.TRANSCRIBING)).toBe(false);
        expect(controller.phase()).toBe(PHASE.LOADING);
        expect(controller.setPhase(PHASE.INJECTING)).toBe(false);
        expect(controller.phase()).toBe(PHASE.LOADING);
    });

    it('rejects a transition out of a finished operation', () => {
        const { controller } = makeController();
        controller.setPhase(PHASE.INJECTING);
        expect(controller.setPhase(PHASE.TRANSCRIBING)).toBe(false);
        expect(controller.phase()).toBe(PHASE.INJECTING);
    });

    it('renders on every accepted transition', () => {
        const { controller, renders } = makeController();
        controller.setPhase(PHASE.LOADING);
        controller.setPhase(PHASE.IDLE);
        expect(renders.length).toBe(2);
        expect(renders[0].phase).toBe(PHASE.LOADING);
        expect(renders[1].phase).toBe(PHASE.IDLE);
    });
});

describe('transcribe controller — selection guard', () => {
    it('refuses to switch while an operation is in flight', () => {
        const { controller } = makeController();
        controller.select('a');
        controller.setPhase(PHASE.LOADING);
        expect(controller.canSelect()).toBe(false);
        expect(controller.select('b')).toBe(false);
        expect(controller.selectedStem()).toBe('a');
    });

    it('ignores a re-selection of the same recording', () => {
        const { controller } = makeController();
        expect(controller.select('a')).toBe(true);
        expect(controller.select('a')).toBe(false);
    });

    it('clears a selection when the recording is deleted elsewhere', () => {
        const { controller } = makeController();
        controller.select('a');
        expect(controller.clearSelection()).toBe(true);
        expect(controller.selectedStem()).toBeNull();
        expect(controller.clearSelection()).toBe(false);
    });

    it('detects a stale selection', () => {
        const { controller } = makeController();
        controller.select('a');
        expect(controller.isSelected('a')).toBe(true);
        expect(controller.isSelected('b')).toBe(false);
    });
});

describe('transcribe controller — ownership token', () => {
    it('ignores events when no transcription is in flight', () => {
        const { controller } = makeController();
        expect(controller.ownsEvent()).toBe(false);
        expect(controller.ownsEvent('a')).toBe(false);
    });

    it('owns filename-less events while the token is held', () => {
        const { controller } = makeController();
        controller.select('a');
        controller.beginTranscription('a');
        expect(controller.ownsEvent()).toBe(true);
    });

    it('rejects an event for a different recording', () => {
        const { controller } = makeController();
        controller.select('a');
        controller.beginTranscription('a');
        expect(controller.ownsEvent('b')).toBe(false);
        expect(controller.ownsEvent('a')).toBe(true);
    });

    it('releases the token when the transcription ends', () => {
        const { controller } = makeController();
        controller.select('a');
        controller.beginTranscription('a');
        controller.endTranscription();
        expect(controller.transcribingStem()).toBeNull();
        expect(controller.ownsEvent()).toBe(false);
        expect(controller.phase()).toBe(PHASE.IDLE);
    });

    it('refuses to start a transcription while an operation is in flight', () => {
        const { controller } = makeController();
        controller.select('a');
        controller.setPhase(PHASE.LOADING);
        expect(controller.beginTranscription('a')).toBe(false);
        expect(controller.transcribingStem()).toBeNull();
    });
});

describe('transcribe controller — merged-edit lock', () => {
    it('locks segment edits while the merged text is edited', () => {
        const { controller } = makeController();
        expect(controller.canEditSegment()).toBe(true);
        controller.enterMergedEdit();
        expect(controller.isMergedEditing()).toBe(true);
        expect(controller.canEditSegment()).toBe(false);
        controller.exitMergedEdit();
        expect(controller.canEditSegment()).toBe(true);
    });

    it('takes the textarea as the injection source while editing', () => {
        const { controller } = makeController();
        controller.enterMergedEdit();
        expect(
            controller.textForInjection('typed by hand', 'from segments'),
        ).toBe('typed by hand');
    });

    it('rebuilds the injection source from the segments otherwise', () => {
        const { controller } = makeController();
        expect(
            controller.textForInjection('typed by hand', 'from segments'),
        ).toBe('from segments');
    });
});

describe('transcribe controller — unsaved edits', () => {
    it('tracks whether segment edits are pending', () => {
        const { controller } = makeController();
        expect(controller.hasEdits()).toBe(false);
        controller.markEditsDirty();
        expect(controller.hasEdits()).toBe(true);
        controller.clearEdits();
        expect(controller.hasEdits()).toBe(false);
    });
});

describe('transcribe controller — injection guard', () => {
    it('needs an idle phase and a selection', () => {
        const { controller } = makeController();
        expect(controller.canInject()).toBe(false);
        controller.select('a');
        expect(controller.canInject()).toBe(true);
        controller.setPhase(PHASE.INJECTING);
        expect(controller.canInject()).toBe(false);
    });
});
