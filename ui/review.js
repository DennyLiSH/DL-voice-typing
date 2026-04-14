const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;

const textarea = document.getElementById('review-text');
const btnConfirm = document.getElementById('btn-confirm');
const btnCancel = document.getElementById('btn-cancel');
const errorMsg = document.getElementById('error-msg');
const container = document.getElementById('container');

let isClosing = false;

// Populate textarea when the backend shows this pre-created window.
listen('review-show', async () => {
    textarea.value = '';
    errorMsg.textContent = '';
    isClosing = false;

    try {
        const text = await invoke('get_review_text');
        if (text) {
            textarea.value = text;
        }
    } catch (e) {
        console.error('failed to get review text:', e);
    }
    textarea.focus();
    textarea.select();
    container.classList.add('visible');
});

// First-load fallback: WebView2 may defer JS init for hidden windows.
// If review-show was emitted before this script loaded, the event was lost.
// This IIFE catches that case by pulling pending text directly.
(async () => {
    try {
        const text = await invoke('get_review_text');
        if (text) {
            textarea.value = text;
            errorMsg.textContent = '';
            isClosing = false;
            textarea.focus();
            textarea.select();
            container.classList.add('visible');
        }
    } catch (_) {}
})();

// Confirm button click.
btnConfirm.addEventListener('click', () => {
    doConfirm();
});

// Cancel button click.
btnCancel.addEventListener('click', () => {
    doCancel();
});

// Keyboard shortcuts.
textarea.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        doConfirm();
    } else if (e.key === 'Escape') {
        e.preventDefault();
        doCancel();
    }
});

document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
        e.preventDefault();
        doCancel();
    }
});

async function doConfirm() {
    if (isClosing) return;
    isClosing = true;
    const text = textarea.value;

    try {
        errorMsg.textContent = '';
        await invoke('confirm_inject', { text });
    } catch (err) {
        errorMsg.textContent = typeof err === 'string' ? err : '粘贴失败，请重试';
        isClosing = false;
    }
}

async function doCancel() {
    if (isClosing) return;
    isClosing = true;
    try {
        await invoke('cancel_review');
    } catch (_) {
        // Ignore cancel errors.
    }
}
