/**
 * Undo toast for soft-deleted recordings.
 *
 * Single-instance slot: a second `showPendingToast` call while one is alive
 * replaces the visible toast (the earlier batch's backend 5-second timer
 * keeps counting independently — its slot is taken on completion either
 * way, so a stale click on a vanished toast cannot resurrect it).
 *
 * Lifecycle: hidden → countdown (5..1s) → finalized (归零文案) → hidden.
 * At countdown zero the toast does NOT vanish: the text switches to
 * 「已永久删除 N 条」and the undo button is disabled (the backend entry is
 * taken-once by the finalize timer — a late undo click would only surface
 * an expired error). The finalized state auto-dismisses 2s later. Keeping
 * the button (disabled) instead of removing it preserves focus.
 *
 * All DOM construction goes through `createElement` + `textContent` —
 * never `innerHTML` (project-wide convention; the only innerHTML exception
 * is the icon SVG `ui/data-manager.js` constants).
 */

let toastEl = null;
let countdownTimer = null;
let remaining = 0;
let movedCount = 0;
let finalized = false;
let onUndoRef = null;

const TOTAL_SECONDS = 5;
const TICK_MS = 1000;
const DISMISS_MS = 2000;

/**
 * Show the undo toast. If one is already alive, replace it (the new batch's
 * undo callback wins).
 *
 * @param {Object} opts
 * @param {number} opts.moved - count of successfully moved stems
 * @param {Function} opts.onUndo - called when the user clicks 撤销; the
 *   toast is destroyed before the callback fires.
 */
export function showPendingToast({ moved, onUndo }) {
    destroyPendingToast();
    remaining = TOTAL_SECONDS;
    movedCount = moved;
    finalized = false;
    onUndoRef = onUndo;

    const root = document.createElement('div');
    root.id = 'pending-toast';
    root.className = 'pending-toast';
    root.setAttribute('role', 'status');
    root.setAttribute('aria-live', 'polite');

    const text = document.createElement('span');
    text.id = 'pending-toast-text';
    text.className = 'pending-toast-text';
    setText(text, movedCount, remaining);

    const undoBtn = document.createElement('button');
    undoBtn.type = 'button';
    undoBtn.id = 'pending-toast-undo';
    undoBtn.className = 'pending-toast-undo';
    undoBtn.textContent = '撤销';
    undoBtn.addEventListener('click', () => {
        if (finalized) return;
        const cb = onUndoRef;
        destroyPendingToast();
        if (typeof cb === 'function') cb();
    });

    root.appendChild(text);
    root.appendChild(undoBtn);
    document.body.appendChild(root);
    toastEl = root;

    countdownTimer = setInterval(tick, TICK_MS);
}

/**
 * Tear down the toast (clear timer + remove node). Safe to call when no
 * toast is alive (no-op).
 */
export function destroyPendingToast() {
    if (countdownTimer !== null) {
        // The slot holds either the countdown interval or the finalized
        // dismiss timeout — clear both (ids share a pool per HTML spec).
        clearInterval(countdownTimer);
        clearTimeout(countdownTimer);
        countdownTimer = null;
    }
    if (toastEl?.isConnected) {
        toastEl.remove();
    }
    toastEl = null;
    onUndoRef = null;
    movedCount = 0;
    remaining = 0;
    finalized = false;
}

function tick() {
    remaining -= 1;
    if (!toastEl) return;
    if (remaining <= 0) {
        enterFinalized();
        return;
    }
    const text = toastEl.querySelector('#pending-toast-text');
    if (text) setText(text, movedCount, remaining);
}

function enterFinalized() {
    finalized = true;
    const text = toastEl.querySelector('#pending-toast-text');
    if (text) text.textContent = `已永久删除 ${movedCount} 条`;
    const undo = toastEl.querySelector('#pending-toast-undo');
    if (undo) undo.disabled = true;
    clearInterval(countdownTimer);
    countdownTimer = setTimeout(destroyPendingToast, DISMISS_MS);
}

function setText(el, moved, secsLeft) {
    el.textContent = `已删除 ${moved} 条（${secsLeft}s 内可撤销）`;
}
