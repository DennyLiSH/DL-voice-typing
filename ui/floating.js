const { listen } = window.__TAURI__.event;

const bar = document.getElementById('bar');
const bars = document.querySelectorAll('.bar');
const textEl = document.getElementById('text');
const dotsEl = document.getElementById('dots');
const waveformEl = document.getElementById('waveform');

const MIN_BAR_HEIGHT = 4;
const MAX_BAR_HEIGHT = 32;
const BAR_WEIGHTS = [0.5, 0.8, 1.0, 0.75, 0.55];

let prevHeights = [MIN_BAR_HEIGHT, MIN_BAR_HEIGHT, MIN_BAR_HEIGHT, MIN_BAR_HEIGHT, MIN_BAR_HEIGHT];
let hideTimeout = null;

function updateBars(rms) {
    for (let i = 0; i < 5; i++) {
        const jitter = 1 + (Math.random() * 0.08 - 0.04);
        const raw = Math.min(Math.max(rms * BAR_WEIGHTS[i] * jitter * 96, MIN_BAR_HEIGHT), MAX_BAR_HEIGHT);

        const prev = prevHeights[i];
        const attack = 0.4;
        const release = 0.15;

        let h;
        if (raw > prev) {
            h = prev + (raw - prev) * attack;
        } else {
            h = prev + (raw - prev) * release;
        }

        h = Math.min(Math.max(h, MIN_BAR_HEIGHT), MAX_BAR_HEIGHT);
        prevHeights[i] = h;
        bars[i].style.height = h + 'px';
    }
}

function show() {
    if (hideTimeout) {
        clearTimeout(hideTimeout);
        hideTimeout = null;
    }
    bar.classList.remove('exit', 'error');
    bar.classList.add('visible');
}

function hide(delay = 0) {
    if (delay > 0) {
        hideTimeout = setTimeout(() => hide(), delay);
        return;
    }
    bar.classList.remove('visible');
    bar.classList.add('exit');
}

function showError(message) {
    bar.classList.add('error', 'visible');
    textEl.textContent = message;
    textEl.classList.add('error-text');
    waveformEl.style.display = 'none';
    dotsEl.style.display = 'none';
    hide(3000);
}

function showRefining() {
    waveformEl.style.display = 'none';
    textEl.textContent = '';
    dotsEl.style.display = 'flex';
}

function showRecording() {
    waveformEl.style.display = 'flex';
    dotsEl.style.display = 'none';
    textEl.classList.remove('error-text');
}

// Listen for events from Rust backend
listen('recording-start', () => {
    show();
    showRecording();
    textEl.textContent = '';
});

listen('audio-rms', (event) => {
    updateBars(event.payload);
});

listen('transcription-partial', (event) => {
    textEl.textContent = event.payload;
});

listen('transcription-complete', () => {
    // Stop waveform
    prevHeights = [MIN_BAR_HEIGHT, MIN_BAR_HEIGHT, MIN_BAR_HEIGHT, MIN_BAR_HEIGHT, MIN_BAR_HEIGHT];
    for (let i = 0; i < 5; i++) {
        bars[i].style.height = MIN_BAR_HEIGHT + 'px';
    }
});

listen('llm-refining', () => {
    showRefining();
});

listen('llm-complete', (event) => {
    dotsEl.style.display = 'none';
    waveformEl.style.display = 'none';
    textEl.textContent = event.payload;
});

listen('injection-complete', () => {
    hide();
});

listen('injection-error', (event) => {
    showError(event.payload || '⚠ 注入失败');
});

listen('speech-error', (event) => {
    showError(event.payload || '⚠ 转录失败');
});

listen('llm-error', (event) => {
    showError(event.payload || '⚠ LLM 服务不可用');
});
