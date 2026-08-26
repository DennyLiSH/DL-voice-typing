import {
    errorDisplayText,
    getColor,
    getShadow,
    remapRms,
} from './floating-utils.js';

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
    if (delay > 0) {
        hideTimeout = setTimeout(() => hide(), delay);
        return;
    }
    // Remove any lingering ripples
    document.querySelectorAll('.ripple').forEach((r) => r.remove());
    indicator.classList.remove('visible', 'processing');
    indicator.classList.add('exit');
    transcriptText.textContent = '';
    transcriptText.classList.remove('visible', 'error');
    isSpringActive = false;
    if (rafId) cancelAnimationFrame(rafId);
}

const ERROR_DEFAULTS = {
    'speech-error': '语音识别失败',
    'llm-error': 'LLM 纠错失败，已使用原文本',
    'injection-error': '粘贴失败，文本可能已保留在剪贴板',
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
    indicator.style.background = BASE_BG;
    indicator.style.boxShadow = BASE_SHADOW;
    indicator.style.transform = `scale(${MIN_SCALE})`;
    indicator.classList.remove('processing', 'error', 'exit');
    currentScale = MIN_SCALE;
    velocity = 0;
    targetScale = MIN_SCALE;
    rmsHistory = [];
}

function showProcessing() {
    // Clear partial transcript — final transcription is in progress.
    transcriptText.textContent = '';
    transcriptText.classList.remove('visible');
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

listen('injection-error', (event) => {
    showError('injection-error', event.payload);
});

listen('speech-error', (event) => {
    showError('speech-error', event.payload);
});

listen('llm-error', (event) => {
    showError('llm-error', event.payload);
});
