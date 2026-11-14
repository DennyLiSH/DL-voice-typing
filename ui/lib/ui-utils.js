/**
 * Shared UI helpers used by settings page modules.
 *
 * Kept in a separate tiny module to avoid circular imports between
 * app-shell.js, settings-form.js, model-manager.js, and data-manager.js.
 *
 * showError/hideError are thin delegates over lib/banner.js: the banner
 * mechanics (text + variant class swap + hidden attribute) live there; this
 * module only owns the settings-window element lookup. Signatures are
 * API-stable — settings-form.js / model-manager.js / app-shell.js import
 * these names.
 */

import { hideBanner, showBanner } from './banner.js';

const errorBanner = document.getElementById('error-banner');

export function showError(msg) {
    showBanner(errorBanner, msg);
}

export function hideError() {
    hideBanner(errorBanner);
}
