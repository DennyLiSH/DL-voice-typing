import {
    errorDisplayText,
    getColor,
    getShadow,
    remapRms,
} from './floating-utils.js';
import { INJECTION_ERROR_MSG } from './lib/errors.js';

const { listen } = window.__TAURI__.event;

const indicator = document.getElementById('indicator');
const transcriptText = document.getElementById('transcript-text');

// Scale range — moderate bouncing
const MIN_SCALE = 0.7;
const MAX_SCALE = 1.5;

// Spring physics parameters
const STIFFNESS = 0.28;
const DAMPING = 0.75;

const BASE_BG = 'rgba(18, 40, 48, 0.82)';
const BASE_SHADOW = '0 4px 20px rgba(58,186,180,0.15)';

// Spring state
let currentScale = MIN_SCALE;
let velocity = 0;
let targetScale = MIN_SCALE;

// Ripple state
const RMS_HISTORY_LEN = 10;
let rmsHistory = [];
let lastRippleTime = 0;
const RIPPLE_MIN_INTERVAL = 250;
const RIPPLE_THRESHOLD = 1.5;
const MAX_ACTIVE_RIPPLES = 3;

let hideTimeout = null;
let rafId = null;
let isSpringActive = false;

// Record-only in-flight timer state (long sessions: 30min+).
let recordOnlyTimer = null;
let recordOnlySeconds = 0;

function formatDuration(totalSeconds) {
    const m = Math.floor(totalSeconds / 60);
    const s = totalSeconds % 60;
    return `${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
}

function stopRecordOnlyTimer() {
    if (recordOnlyTimer) {
        clearInterval(recordOnlyTimer);
        recordOnlyTimer = null;
    }
}

function updateRecordOnlyText() {
    transcriptText.textContent = `录音中 ${formatDuration(recordOnlySeconds)}`;
    transcriptText.classList.remove('error');
    transcriptText.classList.add('visible');
}

function showRecordOnly() {
    // The record-only look is CSS-driven (breathe keyframes + tint), so any
    // inline background/shadow/transform left by the spring-driven modes
    // must be cleared — inline styles beat class rules.
    indicator.style.background = '';
    indicator.style.boxShadow = '';
    indicator.style.transform = '';
    indicator.classList.remove('processing', 'error', 'exit');
    indicator.classList.add('record-only', 'visible');
    stopRecordOnlyTimer();
    recordOnlySeconds = 0;
    updateRecordOnlyText();
    recordOnlyTimer = setInterval(() => {
        recordOnlySeconds += 1;
        updateRecordOnlyText();
    }, 1000);
}

function updateVisuals(visualRms) {
    const c = getColor(visualRms);
    indicator.style.background = `rgba(${Math.round(c[0])},${Math.round(c[1])},${Math.round(c[2])},${c[3].toFixed(2)})`;
    indicator.style.boxShadow = getShadow(visualRms);
}

function springFrame() {
    // Spring physics update
    const force = (targetScale - currentScale) * STIFFNESS;
    velocity += force;
    velocity *= DAMPING;
    currentScale += velocity;

    // Clamp to reasonable range
    currentScale = Math.max(
        MIN_SCALE * 0.9,
        Math.min(MAX_SCALE * 1.1, currentScale),
    );

    indicator.style.transform = `scale(${currentScale})`;

    // Update color based on current scale position in range
    const t = (currentScale - MIN_SCALE) / (MAX_SCALE - MIN_SCALE);
    updateVisuals(Math.max(0, Math.min(1, t)));

    // Continue animation if spring is still moving
    if (
        Math.abs(velocity) > 0.001 ||
        Math.abs(targetScale - currentScale) > 0.005
    ) {
        rafId = requestAnimationFrame(springFrame);
    } else {
        currentScale = targetScale;
        indicator.style.transform = `scale(${currentScale})`;
        isSpringActive = false;
    }
}

function startSpring() {
    if (!isSpringActive) {
        isSpringActive = true;
        rafId = requestAnimationFrame(springFrame);
    }
}

function updatePulse(rms) {
    const visualRms = remapRms(rms);
    targetScale = MIN_SCALE + visualRms * (MAX_SCALE - MIN_SCALE);
    targetScale = Math.max(MIN_SCALE, Math.min(MAX_SCALE, targetScale));
    startSpring();

    // Ripple logic
    rmsHistory.push(rms);
    if (rmsHistory.length > RMS_HISTORY_LEN) rmsHistory.shift();

    const now = performance.now();
    if (rmsHistory.length >= 3 && now - lastRippleTime > RIPPLE_MIN_INTERVAL) {
        const avg = rmsHistory.reduce((a, b) => a + b, 0) / rmsHistory.length;
        if (rms > avg * RIPPLE_THRESHOLD) {
            spawnRipple();
            lastRippleTime = now;
        }
    }
}

function spawnRipple() {
    const ripples = document.querySelectorAll('.ripple');
    if (ripples.length >= MAX_ACTIVE_RIPPLES) return;

    const el = document.createElement('div');
    el.className = 'ripple';
    document.body.appendChild(el);
    el.addEventListener('animationend', () => el.remove());
}

function show() {
    if (hideTimeout) {
        clearTimeout(hideTimeout);
        hideTimeout = null;
    }
    indicator.classList.remove('exit', 'error', 'processing');
    transcriptText.classList.remove('error');
    indicator.classList.add('visible');
}

function hide(delay = 0) {
    if (hideTimeout) {
        clearTimeout(hideTimeout);
        hideTimeout = null;
    }
    if (delay > 0) {
        hideTimeout = setTimeout(() => hide(), delay);
        return;
    }
    // Remove any lingering ripples
    document.querySelectorAll('.ripple').forEach((r) => r.remove());
    stopRecordOnlyTimer();
    indicator.classList.remove('visible', 'processing', 'record-only');
    indicator.classList.add('exit');
    transcriptText.textContent = '';
    transcriptText.classList.remove('visible', 'error');
    isSpringActive = false;
    if (rafId) cancelAnimationFrame(rafId);
}

const ERROR_DEFAULTS = {
    'speech-error': '语音识别失败',
    'llm-error': 'LLM 纠错失败，已保留原始转录',
    'injection-error': INJECTION_ERROR_MSG,
};

function showError(eventName, payload) {
    indicator.style.background = '';
    indicator.style.boxShadow = '';
    indicator.classList.remove('processing');
    indicator.classList.add('error', 'visible');
    transcriptText.textContent = errorDisplayText(
        payload,
        ERROR_DEFAULTS[eventName] || '出错了',
    );
    transcriptText.classList.add('visible', 'error');
    hide(4500);
}

function showRecording() {
    // Defensive double cleanup: backend escape hatches (watchdog/tray reset)
    // only OS-hide the window — the webview interval would otherwise keep
    // overwriting transcription-partial text with the record-only timer.
    stopRecordOnlyTimer();
    indicator.style.background = BASE_BG;
    indicator.style.boxShadow = BASE_SHADOW;
    indicator.style.transform = `scale(${MIN_SCALE})`;
    indicator.classList.remove('processing', 'error', 'exit', 'record-only');
    currentScale = MIN_SCALE;
    velocity = 0;
    targetScale = MIN_SCALE;
    rmsHistory = [];
}

function showProcessing() {
    // Placeholder text keeps the transcript area from going blank while the
    // final transcription runs (same textContent path as the error states).
    // Esc hint advertises the cancel affordance: Transcribing/LLMRefining
    // are the two phases during which Esc is swallowed by the hook (see
    // PipelineState::cancel_active_pipeline).
    transcriptText.textContent = '转录中… 按 Esc 取消';
    transcriptText.classList.remove('error');
    transcriptText.classList.add('visible');
    // Let spring settle naturally before switching to CSS animation
    targetScale = 1.0;
    const settleAndTransition = () => {
        if (
            Math.abs(velocity) > 0.005 ||
            Math.abs(targetScale - currentScale) > 0.01
        ) {
            requestAnimationFrame(settleAndTransition);
        } else {
            indicator.style.background = '';
            indicator.style.boxShadow = '';
            indicator.classList.remove('error', 'visible', 'exit');
            indicator.classList.add('processing', 'visible');
            currentScale = 1.0;
            velocity = 0;
            isSpringActive = false;
        }
    };
    startSpring();
    requestAnimationFrame(settleAndTransition);
}

// Listen for events from Rust backend
listen('recording-start', () => {
    show();
    showRecording();
    transcriptText.textContent = '';
    transcriptText.classList.remove('visible');
});

listen('audio-rms', (event) => {
    updatePulse(event.payload);
});

listen('transcription-partial', (event) => {
    transcriptText.textContent = event.payload;
    transcriptText.classList.add('visible');
});

listen('transcription-complete', () => {
    showProcessing();
});

listen('llm-refining', () => {
    showProcessing();
});

listen('injection-complete', () => {
    hide();
});

listen('pipeline-cancelled', () => {
    // Esc-cancel swallowed the keypress in the hook; the backend has already
    // reset the state machine and hidden the floating window via the
    // WindowController, but we hide() defensively in case the user pressed
    // Esc during the LLMRefining phase (the cancel path goes through
    // DeliveryController only when text is ready to deliver).
    hide();
});

listen('injection-error', (event) => {
    showError('injection-error', event.payload);
});

listen('speech-error', (event) => {
    showError('speech-error', event.payload);
});

listen('llm-error', (event) => {
    showError('llm-error', event.payload);
});

// Record-only mode: long-session indicator (corner-pinned breathing circle
// + mm:ss timer). Payloads are OBJECTS ({stem}/{stem,status}/{message}),
// unlike the bare-string error payloads above.
listen('record-only-started', () => {
    show();
    showRecordOnly();
});

listen('record-only-finished', (event) => {
    stopRecordOnlyTimer();
    indicator.classList.remove('record-only');
    if (event.payload?.status === 'failed') {
        // Backpressure controlled stop: the WAV was finalized but lost
        // >3.2s of audio — surface it as an error, not "saved".
        indicator.style.background = '';
        indicator.style.boxShadow = '';
        indicator.classList.add('error');
        transcriptText.textContent =
            '录音不完整已提前停止，已保存部分可在转录窗口查看';
        transcriptText.classList.add('visible', 'error');
        hide(4500);
    } else {
        transcriptText.textContent = '录音已保存';
        transcriptText.classList.remove('error');
        transcriptText.classList.add('visible');
        hide(1500);
    }
});

listen('record-only-error', (event) => {
    // Not routed through showError: it resolves defaults by event name and
    // passes object payloads straight to errorDisplayText, which falls back
    // to the table key and would lose the backend message. Unpack here.
    stopRecordOnlyTimer();
    indicator.classList.remove('record-only', 'processing');
    indicator.style.background = '';
    indicator.style.boxShadow = '';
    indicator.classList.add('error', 'visible');
    transcriptText.textContent = errorDisplayText(
        event.payload?.message,
        '录音失败',
    );
    transcriptText.classList.add('visible', 'error');
    hide(4500);
});
