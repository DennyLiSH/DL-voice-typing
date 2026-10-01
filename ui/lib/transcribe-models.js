// Single source of truth for built-in whisper model metadata on the
// frontend. Mirrored with src-tauri/src/config/schema.rs BUILT_IN_MODELS
// — drift is pinned by __tests__/model-table-contract.test.js (id set
// must equal Rust serde_key set). Adding a model = one entry here; the
// Rust side mirror must follow.
export const MODELS = [
    { id: 'tiny', name: 'Tiny', size: '75MB' },
    { id: 'tiny-q8_0', name: 'Tiny Q8_0', size: '~40MB', tag: '量化' },
    { id: 'base', name: 'Base', size: '142MB' },
    { id: 'base-q8_0', name: 'Base Q8_0', size: '~75MB', tag: '量化' },
    { id: 'small', name: 'Small', size: '466MB' },
    { id: 'small-q8_0', name: 'Small Q8_0', size: '~250MB', tag: '量化' },
    { id: 'medium', name: 'Medium', size: '1.5GB' },
    { id: 'medium-q8_0', name: 'Medium Q8_0', size: '~800MB', tag: '量化' },
];

const MODEL_LABELS = MODELS.map(({ id, name, size }) => [
    id,
    `${name} (${size})`,
]);

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
