/**
 * In-app confirmation dialog replacing native confirm()/alert() at the five
 * decision sites (design review 2026-09-04 #4: the OS-styled
 * dialogs broke the jade visual language and duplicated the in-app
 * banner/toast/error-bar channels).
 *
 * Dynamic single-instance injection: the overlay is created on first call and
 * removed on close, so window HTML never carries dialog markup (test rigs
 * with MINIMAL_DOM keep working untouched). All content goes through
 * createElement + textContent — no innerHTML.
 */

let openCount = 0;

/** Whether a dialog is currently open (modal short-circuit for guards). */
export function isDialogOpen() {
    return openCount > 0;
}

/**
 * Show a modal confirmation. Resolves true on confirm, false on cancel
 * (cancel button, Escape, or overlay click).
 *
 * @param {Object} opts
 * @param {string} opts.title
 * @param {string} opts.message - plain text; "\n" renders as line breaks
 * @param {string} [opts.confirmText='确认']
 * @param {string} [opts.cancelText='取消']
 * @param {boolean} [opts.danger=false] - destructive action: red confirm
 *   button, initial focus lands on CANCEL so a stray Enter cannot confirm.
 * @returns {Promise<boolean>}
 */
export function confirmDialog({
    title,
    message,
    confirmText = '确认',
    cancelText = '取消',
    danger = false,
}) {
    if (openCount > 0) {
        // Defensive: all call sites are user-triggered modals, so this is
        // unreachable in practice — fail loudly rather than queue silently.
        return Promise.reject(new Error('confirmDialog is already open'));
    }
    openCount = 1;

    return new Promise((resolve) => {
        const previouslyFocused = document.activeElement;

        const overlay = document.createElement('div');
        overlay.className = 'dialog-overlay';

        const dialog = document.createElement('div');
        dialog.setAttribute('role', 'alertdialog');
        dialog.setAttribute('aria-modal', 'true');
        dialog.setAttribute('aria-labelledby', 'dialog-title');
        dialog.setAttribute('aria-describedby', 'dialog-message');
        dialog.className = 'dialog';

        const titleEl = document.createElement('h2');
        titleEl.id = 'dialog-title';
        titleEl.className = 'dialog-title';
        titleEl.textContent = title;

        const msgEl = document.createElement('p');
        msgEl.id = 'dialog-message';
        msgEl.className = 'dialog-message';
        msgEl.textContent = message;

        const actions = document.createElement('div');
        actions.className = 'dialog-actions';

        const close = (result) => {
            document.removeEventListener('focusin', trapFocus);
            overlay.remove();
            openCount = 0;
            // Focus restore (element may have been removed meanwhile).
            if (previouslyFocused && previouslyFocused.isConnected) {
                previouslyFocused.focus();
            }
            resolve(result);
        };

        const cancelBtn = document.createElement('button');
        cancelBtn.type = 'button';
        cancelBtn.className = 'btn-secondary';
        cancelBtn.textContent = cancelText;
        cancelBtn.addEventListener('click', () => close(false));

        const confirmBtn = document.createElement('button');
        confirmBtn.type = 'button';
        // btn-danger: destructive confirm; btn-primary otherwise.
        confirmBtn.className = danger ? 'btn-danger' : 'btn-primary';
        confirmBtn.textContent = confirmText;
        confirmBtn.addEventListener('click', () => close(true));

        actions.appendChild(cancelBtn);
        actions.appendChild(confirmBtn);

        dialog.appendChild(titleEl);
        dialog.appendChild(msgEl);
        dialog.appendChild(actions);
        overlay.appendChild(dialog);

        // Focus trap: pull focus back whenever it escapes the dialog.
        const trapFocus = (e) => {
            if (!dialog.contains(e.target)) {
                (danger ? cancelBtn : confirmBtn).focus();
            }
        };

        dialog.addEventListener('keydown', (e) => {
            if (e.key === 'Escape' && !e.isComposing) {
                e.preventDefault();
                close(false);
            } else if (e.key === 'Enter' && !e.isComposing) {
                // Only when focus is not on a button — button Enter would
                // double-fire (click + this handler).
                if (e.target !== cancelBtn && e.target !== confirmBtn) {
                    e.preventDefault();
                    close(true);
                }
            }
        });
        overlay.addEventListener('click', (e) => {
            if (e.target === overlay) close(false);
        });
        document.addEventListener('focusin', trapFocus);

        document.body.appendChild(overlay);
        // Danger focuses CANCEL (stray Enter must not confirm destruction);
        // otherwise focus CONFIRM (the common affirmative path).
        (danger ? cancelBtn : confirmBtn).focus();
    });
}
