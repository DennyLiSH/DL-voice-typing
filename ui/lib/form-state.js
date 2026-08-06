/**
 * Tiny event bus for form change notifications.
 *
 * Breaks the bidirectional import between settings-form.js and model-manager.js:
 * model-manager needs to notify "form changed" (e.g., on model selection change),
 * settings-form needs to react (recompute dirty state). Without this module,
 * model-manager imports updateDirtyState from settings-form while settings-form
 * imports getSelectedModel from model-manager — a cycle.
 *
 * With this bus: model-manager → notifyFormChange(); settings-form subscribes
 * via onFormChange(updateDirtyState). The settings-form → model-manager edge
 * (getSelectedModel/getModelStatus) remains, but the reverse edge is removed.
 */

const changeListeners = new Set();

/**
 * Subscribe to form change events.
 * @param {() => void} cb - callback invoked on notifyFormChange()
 * @returns {() => void} unsubscribe function
 */
export function onFormChange(cb) {
    changeListeners.add(cb);
    return () => changeListeners.delete(cb);
}

/**
 * Notify all subscribers that the form state changed (e.g., model selection,
 * toggle, input value). Subscribers recompute their derived state.
 */
export function notifyFormChange() {
    changeListeners.forEach((cb) => cb());
}
