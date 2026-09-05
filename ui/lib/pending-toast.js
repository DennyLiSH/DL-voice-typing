/**
 * Undo toast for soft-deleted recordings.
 *
 * Single-instance slot: a second `showPendingToast` call while one is alive
 * replaces the visible toast (the earlier batch's backend 5-second timer
 * keeps counting independently — its slot is taken on completion either
 * way, so a stale click on a vanished toast cannot resurrect it).
 *
 * All DOM construction goes through `createElement` + `textContent` —
 * never `innerHTML` (project-wide convention; the only innerHTML exception
 * is the icon SVG `ui/data-manager.js` constants).
 */

let toastEl = null;
let countdownTimer = null;
let remaining = 0;
let onUndoRef = null;

const TOTAL_SECONDS = 5;
const TICK_MS = 1000;

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
    onUndoRef = onUndo;

    const root = document.createElement('div');
    root.id = 'pending-toast';
    root.className = 'pending-toast';
    root.dataset.moved = String(moved);
    root.setAttribute('role', 'status');
    root.setAttribute('aria-live', 'polite');

    const text = document.createElement('span');
    text.id = 'pending-toast-text';
    text.className = 'pending-toast-text';
    setText(text, moved, remaining);

    const undoBtn = document.createElement('button');
    undoBtn.type = 'button';
    undoBtn.id = 'pending-toast-undo';
    undoBtn.className = 'pending-toast-undo';
    undoBtn.textContent = '撤销';
    undoBtn.addEventListener('click', () => {
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
        clearInterval(countdownTimer);
        countdownTimer = null;
    }
    if (toastEl?.isConnected) {
        toastEl.remove();
    }
    toastEl = null;
    onUndoRef = null;
    remaining = 0;
}

function tick() {
    remaining -= 1;
    if (!toastEl) return;
    const text = toastEl.querySelector('#pending-toast-text');
    if (remaining <= 0) {
        destroyPendingToast();
        return;
    }
    if (text) setText(text, currentMoved(), remaining);
}

function currentMoved() {
    // The moved count is captured at show-time and never changes during the
    // countdown (subsequent batches replace the toast entirely). The
    // backward channel through the toast element is not needed — read from
    // the live text by parsing is unnecessary; we just keep `moved` on the
    // element via dataset.
    return toastEl ? Number(toastEl.dataset.moved) || 0 : 0;
}

function setText(el, moved, secsLeft) {
    el.textContent = `已删除 ${moved} 条（${secsLeft}s 内可撤销）`;
}
