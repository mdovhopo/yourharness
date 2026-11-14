"""Builds the voice bench's test set: 16 kHz mono WAVs with reference text, in <bench>/audio/<set>/.

  coding     developer prompts as you'd dictate them to Claude, five English accents (system voices)
  noisy      the same with fan-like noise mixed in at 12 dB SNR, as a laptop mic in a room hears it
  libri      real human read speech (LibriSpeech's 73-clip sample, from <bench>/audio/librispeech/dummy.parquet)
  uk, ru     Ukrainian and Russian prompts (system voices Lesya, Milena)
  mine       your own recordings, if <bench>/audio/mine/ has <name>.wav (or .m4a) with <name>.txt beside it

Usage: python -I make_audio.py <bench dir>
"""

import io
import json
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
import pyarrow.parquet as pq
import soundfile as sf

CODING = [
    "Refactor the sidebar so pinned threads stay on top, and add a unit test for the sort order.",
    "Run cargo test for the core crate and tell me which tests fail.",
    "The WebSocket reconnect loop in the phone app retries too fast, add exponential backoff with jitter.",
    "Open a pull request against main with the GitHub CLI and stack it on top of the accounts branch.",
    "Why does the GPUI list render every row again when only the last message changes?",
    "Move the SQLite migration into a transaction and keep the schema compatible with the old version.",
    "Use serde to parse the JSON event and ignore unknown fields instead of panicking.",
    "Check the Kubernetes deployment YAML, the readiness probe points at the wrong port.",
    "Write a TypeScript function that debounces the search input by three hundred milliseconds.",
    "Rename the async function fetch user profile to load profile and update every caller.",
    "Revert the last commit, it broke the macOS bundle script and the DMG check.",
    "The Postgres query is slow, add an index on created at and run explain analyze.",
    "Let Claude use the browser to take a screenshot of the settings page in dark mode.",
    "Make the Tailscale toggle persist across restarts and show a toast when the server stops.",
    "Can you look at the stack trace, it panics on unwrap inside the tokio runtime.",
]
TERMS = ["sidebar", "cargo", "websocket", "backoff", "jitter", "github", "gpui", "sqlite", "migration", "serde",
         "json", "kubernetes", "yaml", "typescript", "debounces", "async", "macos", "dmg", "postgres", "claude",
         "screenshot", "tailscale", "toast", "unwrap", "tokio"]
VOICES = ["Samantha", "Daniel", "Karen", "Rishi", "Moira"]

UK = [
    "Привіт, будь ласка, перевір чому тести падають після останніх змін.",
    "Додай кнопку налаштувань у бічну панель і збережи вибір користувача.",
    "Сервер на телефоні не підключається, подивись логи і знайди помилку.",
    "Зроби інтерфейс швидшим, список повідомлень не повинен перемальовуватись повністю.",
    "Напиши коротке пояснення, як працює синхронізація між пристроями.",
]
RU = [
    "Посмотри, почему приложение падает при запуске на новой версии системы.",
    "Добавь проверку ошибок и напиши тест для этого случая.",
    "Сделай так, чтобы окно настроек открывалось быстрее.",
]

SR = 16000


def say(text: str, voice: str, out: Path) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        aiff = Path(tmp) / "x.wav"
        subprocess.run(["say", "-v", voice, "-o", str(aiff), "--file-format=WAVE", "--data-format=LEI16@16000", text], check=True)
        audio, sr = sf.read(aiff, dtype="float32")
    assert sr == SR
    sf.write(out, audio, SR)


def write_set(root: Path, name: str, items: list[tuple[str, str]]) -> None:
    d = root / name
    d.mkdir(parents=True, exist_ok=True)
    (d / "refs.json").write_text(json.dumps(dict(items), ensure_ascii=False, indent=1))


def noise_like(n: int, rng: np.random.Generator) -> np.ndarray:
    # Brownish noise (a fan, a room) plus a little hiss.
    brown = np.cumsum(rng.standard_normal(n)).astype(np.float32)
    brown -= np.convolve(brown, np.ones(400) / 400, mode="same")
    return brown / (np.std(brown) + 1e-9) + 0.3 * rng.standard_normal(n).astype(np.float32)


def main() -> None:
    bench = Path(sys.argv[1])
    root = bench / "audio"
    rng = np.random.default_rng(7)

    coding, noisy = [], []
    (root / "coding").mkdir(parents=True, exist_ok=True)
    (root / "noisy").mkdir(parents=True, exist_ok=True)
    for vi, voice in enumerate(VOICES):
        for i, text in enumerate(CODING):
            name = f"{voice.lower()}-{i:02}.wav"
            clean = root / "coding" / name
            if not clean.exists():
                say(text, voice, clean)
            coding.append((name, text))
            audio, _ = sf.read(clean, dtype="float32")
            n = noise_like(len(audio), rng)
            snr = 10 ** (12 / 20)
            n *= np.sqrt(np.mean(audio**2)) / snr / (np.sqrt(np.mean(n**2)) + 1e-9)
            sf.write(root / "noisy" / name, np.clip(audio + n, -1, 1), SR)
            noisy.append((name, text))
    write_set(root, "coding", coding)
    write_set(root, "noisy", noisy)

    for lang, voice, texts in [("uk", "Lesya", UK), ("ru", "Milena", RU)]:
        (root / lang).mkdir(parents=True, exist_ok=True)
        items = []
        for i, text in enumerate(texts):
            name = f"{i:02}.wav"
            say(text, voice, root / lang / name)
            items.append((name, text))
        write_set(root, lang, items)

    table = pq.read_table(bench / "audio" / "librispeech" / "dummy.parquet").to_pylist()
    (root / "libri").mkdir(parents=True, exist_ok=True)
    items = []
    for row in table:
        audio, sr = sf.read(io.BytesIO(row["audio"]["bytes"]), dtype="float32")
        assert sr == SR
        name = f"{row['id']}.wav"
        sf.write(root / "libri" / name, audio, SR)
        items.append((name, row["text"]))
    write_set(root, "libri", items)

    mine = root / "mine"
    if mine.is_dir():
        items = []
        for txt in sorted(mine.glob("*.txt")):
            src = next((p for p in (txt.with_suffix(".wav"), txt.with_suffix(".m4a")) if p.exists()), None)
            if not src:
                continue
            out = mine / "16k" / f"{txt.stem}.wav"
            out.parent.mkdir(exist_ok=True)
            subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(src), "-ac", "1", "-ar", str(SR), str(out)], check=True)
            items.append((f"16k/{txt.stem}.wav", txt.read_text().strip()))
        if items:
            write_set(root, "mine", items)

    (root / "terms.json").write_text(json.dumps(TERMS))
    for d in sorted(p for p in root.iterdir() if (p / "refs.json").exists()):
        refs = json.loads((d / "refs.json").read_text())
        secs = sum(sf.info(d / n).duration for n in refs)
        print(f"{d.name:8} {len(refs):3} clips  {secs/60:5.1f} min")


if __name__ == "__main__":
    main()
