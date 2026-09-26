// One-time banner shown when the app auto-opens settings at the model
// page because the whisper model file is missing (R5 Minor #12).
// Visibility is per-launch: tied to the backend `model_missing=1` query
// param on the auto-opened window; no config persistence (D2).

const PARAM = 'model_missing';

function el(id) {
    return document.getElementById(id);
}

export function showIfRequested() {
    const banner = el('first-run-banner');
    if (!banner) return;
    const params = new URLSearchParams(window.location.search);
    if (params.get(PARAM) === '1' && banner.hidden) {
        banner.hidden = false;
    }
}

export function hideIfVisible() {
    const banner = el('first-run-banner');
    if (banner && !banner.hidden) {
        banner.hidden = true;
    }
}

export function bindFirstRunBanner() {
    const dismiss = el('first-run-banner-dismiss');
    if (!dismiss) return;
    dismiss.addEventListener('click', () => hideIfVisible());
}
