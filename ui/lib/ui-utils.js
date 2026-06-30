/**
 * Shared UI helpers used by settings page modules.
 *
 * Kept in a separate tiny module to avoid circular imports between
 * app-shell.js, settings-form.js, model-manager.js, and data-manager.js.
 */

const errorBanner = document.getElementById('error-banner');

export function showError(msg) {
    if (!errorBanner) return;
    errorBanner.textContent = msg;
    errorBanner.classList.add('visible');
}

export function hideError() {
    if (!errorBanner) return;
    errorBanner.classList.remove('visible');
}
