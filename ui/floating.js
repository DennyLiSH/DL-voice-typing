const { listen } = window.__TAURI__.event;

const indicator = document.getElementById('indicator');

// Scale range — moderate bouncing
const MIN_SCALE = 0.7;
const MAX_SCALE = 1.5;

// Spring physics parameters
const STIFFNESS = 0.28;
const DAMPING = 0.75;

// Color stops: [r, g, b, a] at visual RMS thresholds (after sqrt remap)
const COLOR_STOPS = [
    { at: 0.0, color: [15, 25, 55, 0.82] },
    { at: 0.25, color: [35, 90, 200, 0.88] },
    { at: 0.6, color: [0, 180, 220, 0.92] },
    { at: 1.0, color: [120, 230, 250, 0.98] },
];

const BASE_BG = 'rgba(15, 25, 55, 0.82)';
const BASE_SHADOW = '0 4px 18px rgba(25,60,160,0.2)';

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

function lerpColor(a, b, t) {
    return [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ];
}

function getColor(visualRms) {
    if (visualRms <= COLOR_STOPS[0].at) return COLOR_STOPS[0].color;
    if (visualRms >= COLOR_STOPS[COLOR_STOPS.length - 1].at) return COLOR_STOPS[COLOR_STOPS.length - 1].color;
    for (let i = 0; i < COLOR_STOPS.length - 1; i++) {
        if (visualRms >= COLOR_STOPS[i].at && visualRms <= COLOR_STOPS[i + 1].at) {
            const t = (visualRms - COLOR_STOPS[i].at) / (COLOR_STOPS[i + 1].at - COLOR_STOPS[i].at);
            return lerpColor(COLOR_STOPS[i].color, COLOR_STOPS[i + 1].color, t);
        }
    }
    return COLOR_STOPS[0].color;
}

function getShadow(visualRms) {
    const r = Math.round(25 + 20 * visualRms);
    const g = Math.round(60 + 120 * visualRms);
    const b = Math.round(160 + 50 * visualRms);
    const alpha = (0.2 + visualRms * 0.15).toFixed(2);
    const spread = 18 + visualRms * 8;
    let shadow = `0 4px ${Math.round(spread)}px rgba(${r},${g},${b},${alpha})`;
    if (visualRms > 0.35) {
        const glowAlpha = ((visualRms - 0.35) * 0.25).toFixed(2);
        shadow += `, 0 0 ${Math.round(22 + visualRms * 15)}px rgba(0,180,220,${glowAlpha})`;
    }
    return shadow;
}

function remapRms(rms) {
    return Math.pow(rms, 0.5);
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
    currentScale = Math.max(MIN_SCALE * 0.9, Math.min(MAX_SCALE * 1.1, currentScale));

    indicator.style.transform = `scale(${currentScale})`;

    // Update color based on current scale position in range
    const t = (currentScale - MIN_SCALE) / (MAX_SCALE - MIN_SCALE);
    updateVisuals(Math.max(0, Math.min(1, t)));

    // Continue animation if spring is still moving
    if (Math.abs(velocity) > 0.001 || Math.abs(targetScale - currentScale) > 0.005) {
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
    indicator.classList.add('visible');
}

function hide(delay = 0) {
    if (delay > 0) {
        hideTimeout = setTimeout(() => hide(), delay);
        return;
    }
    // Remove any lingering ripples
    document.querySelectorAll('.ripple').forEach(r => r.remove());
    indicator.classList.remove('visible', 'processing');
    indicator.classList.add('exit');
    isSpringActive = false;
    if (rafId) cancelAnimationFrame(rafId);
}

function showError() {
    indicator.style.background = '';
    indicator.style.boxShadow = '';
    indicator.classList.remove('processing');
    indicator.classList.add('error', 'visible');
    hide(2000);
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
    // Let spring settle naturally before switching to CSS animation
    targetScale = 1.0;
    const settleAndTransition = () => {
        if (Math.abs(velocity) > 0.005 || Math.abs(targetScale - currentScale) > 0.01) {
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
});

listen('audio-rms', (event) => {
    updatePulse(event.payload);
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

listen('injection-error', () => {
    showError();
});

listen('speech-error', () => {
    showError();
});

listen('llm-error', () => {
    showError();
});
