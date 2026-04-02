const { listen } = window.__TAURI__.event;

const indicator = document.getElementById('indicator');

const MIN_SCALE = 1.0;
const MAX_SCALE = 1.3;

let prevScale = MIN_SCALE;
let hideTimeout = null;

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
    indicator.classList.remove('processing');
    indicator.classList.add('error', 'visible');
    hide(2000);
}

function showRecording() {
    indicator.classList.remove('processing', 'error');
    prevScale = MIN_SCALE;
}

function showProcessing() {
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
