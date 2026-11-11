"""A quiet ambient bed for demo videos, made from scratch (no licensing): slow pad chords,
soft and low-passed, with a faint high shimmer and a long echo.

    python music.py <out.wav> <seconds> [seed]
"""

import sys
import wave

import numpy as np

RATE = 44100


def note(midi):
    return 440.0 * 2 ** ((midi - 69) / 12)


# Cmaj9 → Am9 → Fmaj7(#11) → G6/9: warm, unresolved, never asks for attention.
CHORDS = [
    [48, 55, 59, 62, 64],
    [45, 52, 55, 59, 64],
    [41, 48, 52, 57, 59],
    [43, 50, 52, 57, 62],
]


def lowpass(x, cutoff):
    # In the frequency domain: a gentle second-order rolloff above `cutoff`.
    spectrum = np.fft.rfft(x)
    f = np.fft.rfftfreq(len(x), 1 / RATE)
    spectrum /= 1 + (f / cutoff) ** 4
    return np.fft.irfft(spectrum, len(x))


def pad(freq, length, rng):
    t = np.arange(length) / RATE
    voice = np.zeros(length)
    # A few detuned partials with slow vibrato: a soft, breathing tone.
    for detune in (-0.12, 0.0, 0.11):
        f = freq * 2 ** (detune / 12)
        wobble = 1 + 0.0015 * np.sin(2 * np.pi * (0.13 + rng.random() * 0.1) * t + rng.random() * 6)
        phase = 2 * np.pi * np.cumsum(f * wobble) / RATE
        voice += np.sin(phase) + 0.18 * np.sin(2 * phase) + 0.06 * np.sin(3 * phase)
    return voice / 3


def main():
    out = sys.argv[1]
    seconds = float(sys.argv[2])
    rng = np.random.default_rng(int(sys.argv[3]) if len(sys.argv) > 3 else 7)
    total = int(seconds * RATE) + RATE * 2
    chord_len = 8.0  # seconds per chord
    fade = 3.5  # crossfade between chords
    mix = np.zeros(total)
    start = 0.0
    i = 0
    while start * RATE < total:
        chord = CHORDS[i % len(CHORDS)]
        length = int((chord_len + fade) * RATE)
        seg = np.zeros(length)
        for m in chord:
            seg += pad(note(m), length, rng) * (0.9 if m < 50 else 0.55)
        env = np.ones(length)
        ramp = int(fade * RATE)
        env[:ramp] = np.sin(np.linspace(0, np.pi / 2, ramp)) ** 2
        env[-ramp:] = np.cos(np.linspace(0, np.pi / 2, ramp)) ** 2
        seg *= env
        a = int(start * RATE)
        b = min(total, a + length)
        mix[a:b] += seg[: b - a]
        start += chord_len
        i += 1

    # A faint shimmer an octave or two up, drifting in and out.
    t = np.arange(total) / RATE
    shimmer = np.zeros(total)
    for m, rate in ((76, 0.05), (79, 0.037), (83, 0.029)):
        shimmer += np.sin(2 * np.pi * note(m) * t) * (0.5 + 0.5 * np.sin(2 * np.pi * rate * t + rng.random() * 6)) ** 3
    mix += shimmer * 0.08

    # Soft, then an echo for space.
    mix = lowpass(mix, 1400.0)
    echo = np.zeros_like(mix)
    for delay, gain in ((0.43, 0.32), (0.86, 0.18), (1.29, 0.09)):
        d = int(delay * RATE)
        echo[d:] += mix[:-d] * gain
    mix = mix + echo

    # Stereo: the echo a little wider.
    left = mix + 0.15 * np.roll(echo, 300)
    right = mix + 0.15 * np.roll(echo, -300)
    stereo = np.stack([left, right], axis=1)[: int(seconds * RATE)]
    n = len(stereo)
    env = np.ones(n)
    fi, fo = int(2.5 * RATE), int(4.0 * RATE)
    env[:fi] = np.linspace(0, 1, fi) ** 2
    env[-fo:] = np.linspace(1, 0, fo) ** 2
    stereo *= env[:, None]
    stereo /= np.max(np.abs(stereo)) + 1e-9
    stereo *= 0.8
    data = (stereo * 32767).astype("<i2")
    with wave.open(out, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(data.tobytes())


if __name__ == "__main__":
    main()
