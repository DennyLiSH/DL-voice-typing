// Display sizes only. Single source of truth: BUILT_IN_MODELS in
// src-tauri/src/config/schema.rs (same mirror convention as
// ui/model-manager.js MODEL_SIZES). Keep in sync manually.
const MODEL_LABELS = [
    ['tiny', 'Tiny (75MB)'],
    ['tiny-q8_0', 'Tiny Q8_0 (~40MB)'],
    ['base', 'Base (142MB)'],
    ['base-q8_0', 'Base Q8_0 (~75MB)'],
    ['small', 'Small (466MB)'],
    ['small-q8_0', 'Small Q8_0 (~250MB)'],
    ['medium', 'Medium (1.5GB)'],
    ['medium-q8_0', 'Medium Q8_0 (~800MB)'],
];

/**
 * Build select options from a get_whisper_models payload
 * ({ built_in: {id: downloaded}, custom: [name] }). Only downloaded
 * built-ins are listed; the default option means "current global model".
 */
export function buildModelOptions(models) {
    const options = [{ value: '', label: '默认模型', group: null }];
    for (const [id, label] of MODEL_LABELS) {
        if (models.built_in[id])
            options.push({ value: id, label, group: '内置模型' });
    }
    for (const name of models.custom) {
        options.push({
            value: `custom:${name}`,
            label: name,
            group: '自定义模型',
        });
    }
    return options;
}

/** Populate a <select> from buildModelOptions output; hide it when only
 *  the default option exists (feature degrades to current behavior). */
export function populateTranscribeModelSelect(select, options) {
    select.innerHTML = '';
    let currentGroup = null;
    let groupEl = null;
    for (const opt of options) {
        if (opt.group !== currentGroup) {
            currentGroup = opt.group;
            groupEl = null;
            if (opt.group) {
                groupEl = document.createElement('optgroup');
                groupEl.label = opt.group;
                select.appendChild(groupEl);
            }
        }
        const el = document.createElement('option');
        el.value = opt.value;
        el.textContent = opt.label;
        (groupEl ?? select).appendChild(el);
    }
    select.hidden = options.length <= 1;
}
