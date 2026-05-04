const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;

const textarea = document.getElementById('review-text');
const preview = document.getElementById('preview');
const btnConfirm = document.getElementById('btn-confirm');
const btnCancel = document.getElementById('btn-cancel');
const errorMsg = document.getElementById('error-msg');
const container = document.getElementById('container');

let isClosing = false;
let userEdited = false;

function updateButtons() {
    btnConfirm.disabled = !textarea.value || isClosing;
    btnCancel.disabled = isClosing;
}

function moveCursorToEnd() {
    textarea.selectionStart = textarea.selectionEnd = textarea.value.length;
    textarea.scrollTop = textarea.scrollHeight;
}

// Stop auto-updating textarea when user edits.
textarea.addEventListener('input', () => {
    userEdited = true;
});

// Populate textarea when the backend shows this pre-created window.
listen('review-show', async () => {
    userEdited = false;
    textarea.value = '';
    preview.textContent = '';
    preview.classList.remove('visible');
    errorMsg.textContent = '';
    isClosing = false;
    updateButtons();

    try {
        const text = await invoke('get_review_text');
        if (text) {
            textarea.value = text;
            updateButtons();
        }
    } catch (e) {
        console.error('[review] failed to get review text:', e);
    }
    textarea.focus();
    if (textarea.value) {
        moveCursorToEnd();
    }
    container.classList.add('visible');
});

// Real-time accumulated transcription — write directly into textarea.
// The backend already deduplicates via suffix-matching, so this IS the final text.
listen('transcription-partial', (event) => {
    if (!isClosing) {
        const newText = event.payload || '';
        if (newText && !userEdited) {
            textarea.value = newText;
            moveCursorToEnd();
            updateButtons();
        }
        container.classList.add('visible');
    }
});

// Pipeline error: close review window if no text.
listen('speech-error', () => {
    if (!textarea.value && !isClosing) {
        container.classList.remove('visible');
    }
});

// First-load fallback: WebView2 may defer JS init for hidden windows.
(async () => {
    try {
        const text = await invoke('get_review_text');
        if (text) {
            textarea.value = text;
            errorMsg.textContent = '';
            isClosing = false;
            updateButtons();
            textarea.focus();
            moveCursorToEnd();
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
    const text = textarea.value;
    if (!text) return;
    isClosing = true;
    updateButtons();

    try {
        errorMsg.textContent = '';
        await invoke('confirm_inject', { text });
    } catch (err) {
        errorMsg.textContent = typeof err === 'string' ? err : '粘贴失败，请重试';
        isClosing = false;
        updateButtons();
    }
}

async function doCancel() {
    if (isClosing) return;
    isClosing = true;
    updateButtons();
    try {
        await invoke('cancel_review');
    } catch (_) {}
}
