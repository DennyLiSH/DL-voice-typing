// Pure view-state derivation for the settings model page action area.
// Mirrors the transcribe uiFlags precedent (ui/lib/transcribe.js): the
// branch matrix lives in a testable pure function; the DOM application
// layer in model-manager.js stays a thin property loop.
//
// Branch priority matches the pre-refactor else-if chain:
//   downloading-this > downloading-other > custom > downloaded > default.

/**
 * @param {string} selectedModel - e.g. 'base' or 'custom:my-model.bin'
 * @param {string|null} activeDownload - model id currently downloading, or null
 * @param {Record<string, boolean>} modelStatus - built-in download state
 * @returns {{action: 'delete'|'download'|'none', actionDisabled: boolean,
 *            actionDanger: boolean, progressVisible: boolean,
 *            selectDisabled: boolean, statusVisible: boolean}}
 */
export function modelActionView(selectedModel, activeDownload, modelStatus) {
    const isDownloading = activeDownload !== null;
    const downloadingThis = activeDownload === selectedModel;
    const isCustom = selectedModel.startsWith('custom:');

    if (downloadingThis) {
        return {
            action: 'none',
            actionDisabled: true,
            actionDanger: false,
            progressVisible: true,
            selectDisabled: true,
            statusVisible: false,
        };
    }
    if (isDownloading) {
        // Another model is downloading: this model's action shows, disabled.
        return {
            action: isCustom ? 'delete' : 'download',
            actionDisabled: true,
            actionDanger: isCustom,
            progressVisible: false,
            selectDisabled: true,
            statusVisible: false,
        };
    }
    if (isCustom) {
        return {
            action: 'delete',
            actionDisabled: false,
            actionDanger: true,
            progressVisible: false,
            selectDisabled: false,
            statusVisible: false,
        };
    }
    if (modelStatus[selectedModel]) {
        return {
            action: 'none',
            actionDisabled: true,
            actionDanger: false,
            progressVisible: false,
            selectDisabled: false,
            statusVisible: true,
        };
    }
    return {
        action: 'download',
        actionDisabled: false,
        actionDanger: false,
        progressVisible: false,
        selectDisabled: false,
        statusVisible: false,
    };
}
