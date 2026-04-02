const { listen } = window.__TAURI__.event;

const indicator = document.getElementById('indicator');

const MIN_SCALE = 0.55;
const MAX_SCALE = 1.3;

let prevScale = MIN_SCALE;
let hideTimeout = null;

// Color stops: [r, g, b, a] at RMS thresholds
const COLOR_STOPS = [
    { at: 0.0, color: [10, 10, 12, 0.78] },
    { at: 0.4, color: [30, 70, 160, 0.78] },
    { at: 1.0, color: [0, 190, 210, 0.85] },
];

const BASE_BG = 'rgba(10, 10, 12, 0.78)';
const BASE_SHADOW = '0 4px 16px rgba(0,0,0,0.3)';

function lerpColor(a, b, t) {
    return [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ];
}

function getColor(rms) {
    if (rms <= COLOR_STOPS[0].at) return COLOR_STOPS[0].color;
    if (rms >= COLOR_STOPS[COLOR_STOPS.length - 1].at) return COLOR_STOPS[COLOR_STOPS.length - 1].color;
    for (let i = 0; i < COLOR_STOPS.length - 1; i++) {
        if (rms >= COLOR_STOPS[i].at && rms <= COLOR_STOPS[i + 1].at) {
            const t = (rms - COLOR_STOPS[i].at) / (COLOR_STOPS[i + 1].at - COLOR_STOPS[i].at);
            return lerpColor(COLOR_STOPS[i].color, COLOR_STOPS[i + 1].color, t);
        }
    }
    return COLOR_STOPS[0].color;
}

function getShadow(rms) {
    const r = Math.round(30 * rms);
    const g = Math.round(70 + 120 * rms);
    const b = Math.round(160 + 50 * rms);
    const alpha = 0.3 + rms * 0.2;
    const spread = 16 + rms * 8;
    let shadow = `0 4px ${spread}px rgba(${r},${g},${b},${alpha.toFixed(2)})`;
    if (rms > 0.3) {
        const glowAlpha = ((rms - 0.3) * 0.3).toFixed(2);
        shadow += `, 0 0 ${Math.round(20 + rms * 20)}px rgba(0,190,210,${glowAlpha})`;
    }
    return shadow;
}

function updatePulse(rms) {
    const target = MIN_SCALE + rms * (MAX_SCALE - MIN_SCALE);
    const clamped = Math.min(Math.max(target, MIN_SCALE), MAX_SCALE);

    const attack = 0.4;
    const release = 0.15;

    const smooth = clamped > prevScale
        ? prevScale + (clamped - prevScale) * attack
        : prevScale + (clamped - prevScale) * release;

    prevScale = Math.min(Math.max(smooth, MIN_SCALE), MAX_SCALE);
    indicator.style.transform = `scale(${prevScale})`;

    const c = getColor(prevScale / MAX_SCALE);
    indicator.style.background = `rgba(${Math.round(c[0])},${Math.round(c[1])},${Math.round(c[2])},${c[3].toFixed(2)})`;
    indicator.style.boxShadow = getShadow(prevScale / MAX_SCALE);
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
    indicator.classList.remove('visible', 'processing');
    indicator.classList.add('exit');
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
    indicator.classList.remove('processing', 'error');
    prevScale = MIN_SCALE;
}

function showProcessing() {
    indicator.style.background = '';
    indicator.style.boxShadow = '';
    indicator.classList.remove('error');
    indicator.classList.add('processing');
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
