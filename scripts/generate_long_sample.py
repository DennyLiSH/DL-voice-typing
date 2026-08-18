#!/usr/bin/env python3
"""Generate src-tauri/tests/assets/long_sample.wav deterministically.

A >2-minute 16kHz mono 16-bit PCM WAV used for local validation that
whisper segment timestamps stay absolute across whisper.cpp's internal
~30s processing windows (see the record-only plan, task 10).

Structure: alternating 8s tone / 2s silence blocks so any ASR produces
multiple segments spread across the whole file. Deterministic — pure
sine math, no RNG — so regenerating always yields identical bytes.
Silent on success (Unix convention).
"""

import math
import struct
import wave
from pathlib import Path

SAMPLE_RATE = 16_000
TONE_SECS = 8
SILENCE_SECS = 2
BLOCKS = 13  # 13 * 10s = 130s total (> 2 minutes)
TONE_FREQ_HZ = 440.0
AMPLITUDE = 0.5

OUT = Path(__file__).resolve().parent.parent / "src-tauri" / "tests" / "assets" / "long_sample.wav"


def main() -> None:
    OUT.parent.mkdir(parents=True, exist_ok=True)
    frames = bytearray()
    # Continuous phase across blocks avoids clicks at boundaries.
    total_tone_samples = 0
    for _ in range(BLOCKS):
        for _ in range(TONE_SECS * SAMPLE_RATE):
            t = total_tone_samples / SAMPLE_RATE
            sample = int(AMPLITUDE * 32767 * math.sin(2 * math.pi * TONE_FREQ_HZ * t))
            frames += struct.pack("<h", sample)
            total_tone_samples += 1
        frames += b"\x00\x00" * (SILENCE_SECS * SAMPLE_RATE)

    with wave.open(str(OUT), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(SAMPLE_RATE)
        wav.writeframes(bytes(frames))


if __name__ == "__main__":
    main()
