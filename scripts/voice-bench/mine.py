"""Your own recordings through the shortlisted models, transcripts side by side.

Put any audio (m4a, wav, mp3…) in <bench>/audio/mine/; a <name>.txt beside a clip, if there, is what
you said, and that clip gets a WER. Usage: python -I mine.py <bench> [model …]
"""

import json
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import jiwer  # noqa: E402
import soundfile as sf  # noqa: E402
from run import MODELS, loader, norm  # noqa: E402

SHORTLIST = ["whisper-turbo-q5", "whisper-turbo-q5+glossary", "parakeet-v2", "parakeet-v3", "whisper-base"]
AUDIO = {".m4a", ".wav", ".mp3", ".aac", ".caf", ".qta", ".mov"}


def main() -> None:
    bench = Path(sys.argv[1])
    names = sys.argv[2:] or SHORTLIST
    mine = bench / "audio" / "mine"
    conv = mine / "16k"
    conv.mkdir(exist_ok=True)
    clips = []
    for src in sorted(p for p in mine.iterdir() if p.suffix.lower() in AUDIO):
        out = conv / f"{src.stem}.wav"
        if not out.exists():
            subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(src), "-ac", "1", "-ar", "16000", str(out)], check=True)
        ref = src.with_suffix(".txt")
        clips.append((src.stem, sf.read(out, dtype="float32")[0], ref.read_text().strip() if ref.exists() else None))
    if not clips:
        sys.exit(f"No audio in {mine}")

    results = {}
    for name in names:
        engine, model, _, opts = MODELS[name]
        t = time.perf_counter()
        run = loader(engine, model, bench / "models", opts)
        load = time.perf_counter() - t
        run(clips[0][1][:16000 * 3], "auto")
        for stem, audio, ref in clips:
            t = time.perf_counter()
            hyp = run(audio, "auto")
            results.setdefault(stem, []).append((name, hyp, time.perf_counter() - t, load))

    (bench / "out").mkdir(exist_ok=True)
    (bench / "out" / "mine.json").write_text(json.dumps(results, ensure_ascii=False, indent=1))
    for stem, audio, ref in clips:
        print(f"\n## {stem} ({len(audio) / 16000:.0f} s)")
        if ref:
            print(f"   said: {ref}")
        for name, hyp, took, load in results[stem]:
            wer = f" · WER {jiwer.wer(norm(ref), norm(hyp) or '_') * 100:.1f}" if ref else ""
            print(f"\n   {name} ({took:.2f} s{wer}):\n   {hyp}")


if __name__ == "__main__":
    main()
