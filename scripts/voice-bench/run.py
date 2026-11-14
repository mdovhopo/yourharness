"""Runs every model over the voice bench's sets and writes <bench>/out/<model>.json, then a table.

Each model runs in a process of its own (`run.py <bench> --one <model>`), so its load time and peak
memory are its own. Usage: python -I run.py <bench> [model …]; `--table` prints the table only.
"""

import json
import re
import resource
import subprocess
import sys
import time
import unicodedata
from pathlib import Path

import jiwer
import numpy as np
import soundfile as sf

SETS = ["coding", "noisy", "libri", "uk", "ru", "mine"]
GLOSSARY = "Claude, Rust, cargo, GitHub, macOS, JSON, YAML, SQLite, TypeScript, WebSocket, Tailscale."

# name: (engine, model, languages it can do, options)
MODELS = {
    "whisper-base": ("whisper", "ggml-base.bin", "*", {}),
    "whisper-small": ("whisper", "ggml-small.bin", "*", {}),
    "whisper-medium-q5": ("whisper", "ggml-medium-q5_0.bin", "*", {}),
    "whisper-turbo-q5": ("whisper", "ggml-large-v3-turbo-q5_0.bin", "*", {}),
    "whisper-turbo-q5+glossary": ("whisper", "ggml-large-v3-turbo-q5_0.bin", "*", {"initial_prompt": GLOSSARY}),
    "whisper-turbo": ("whisper", "ggml-large-v3-turbo.bin", "*", {}),
    "whisper-large-v3-q5": ("whisper", "ggml-large-v3-q5_0.bin", "*", {}),
    "distil-large-v3.5": ("whisper", "ggml-distil-large-v3.5.bin", "en", {"language": "en"}),
    "parakeet-v2": ("transducer", "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8", "en", {}),
    "parakeet-v3": ("transducer", "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8", "*", {}),
    "moonshine-base": ("moonshine", "sherpa-onnx-moonshine-base-en-int8", "en", {}),
    "canary-180m": ("canary", "sherpa-onnx-nemo-canary-180m-flash-en-es-de-fr-int8", "en", {}),
}
SET_LANG = {"coding": "en", "noisy": "en", "libri": "en", "uk": "uk", "ru": "ru", "mine": "en"}

# Spellings that mean the same thing to Claude.
SAME = [(r"\bthree hundred\b", "300"), (r"\bmac os\b", "macos"), (r"\bweb socket\b", "websocket"),
        (r"\btype script\b", "typescript"), (r"\bgit hub\b", "github"), (r"\bpost gres\b", "postgres"),
        (r"\bok\b", "okay")]


def norm(text: str) -> str:
    t = unicodedata.normalize("NFKC", text).lower().replace("ё", "е").replace("’", "'").replace("ʼ", "'")
    t = re.sub(r"[-_/]", " ", t)
    t = re.sub(r"[^\w\s']", "", t)
    t = re.sub(r"\s+", " ", t).strip()
    for a, b in SAME:
        t = re.sub(a, b, t)
    return t


def loader(engine: str, model: str, models: Path, opts: dict):
    if engine == "whisper":
        from pywhispercpp.model import Model

        m = Model(str(models / "whisper" / model), n_threads=8, print_progress=False, print_realtime=False,
                  redirect_whispercpp_logs_to=None)

        def run(audio, lang):
            o = {"language": opts.get("language", "auto")}
            if "initial_prompt" in opts:
                o["initial_prompt"] = opts["initial_prompt"]
            return " ".join(s.text for s in m.transcribe(audio, **o)).strip()

        return run

    import sherpa_onnx

    d = models / "sherpa" / model
    if engine == "transducer":
        r = sherpa_onnx.OfflineRecognizer.from_transducer(
            encoder=str(d / "encoder.int8.onnx"), decoder=str(d / "decoder.int8.onnx"), joiner=str(d / "joiner.int8.onnx"),
            tokens=str(d / "tokens.txt"), model_type="nemo_transducer", num_threads=8)
    elif engine == "moonshine":
        r = sherpa_onnx.OfflineRecognizer.from_moonshine(
            preprocessor=str(d / "preprocess.onnx"), encoder=str(d / "encode.int8.onnx"),
            uncached_decoder=str(d / "uncached_decode.int8.onnx"), cached_decoder=str(d / "cached_decode.int8.onnx"),
            tokens=str(d / "tokens.txt"), num_threads=8)
    else:
        r = sherpa_onnx.OfflineRecognizer.from_nemo_canary(
            encoder=str(d / "encoder.int8.onnx"), decoder=str(d / "decoder.int8.onnx"), tokens=str(d / "tokens.txt"),
            src_lang="en", tgt_lang="en", num_threads=8)

    def run(audio, lang):
        s = r.create_stream()
        s.accept_waveform(16000, audio)
        r.decode_stream(s)
        return s.result.text.strip()

    return run


def one(bench: Path, name: str) -> None:
    engine, model, langs, opts = MODELS[name]
    t0 = time.perf_counter()
    run = loader(engine, model, bench / "models", opts)
    load = time.perf_counter() - t0
    terms = json.loads((bench / "audio" / "terms.json").read_text())
    out = {"model": name, "load_s": load, "sets": {}}
    warm = False
    for set_name in SETS:
        d = bench / "audio" / set_name
        if not (d / "refs.json").exists() or (langs != "*" and SET_LANG[set_name] != langs):
            continue
        refs = json.loads((d / "refs.json").read_text())
        clips, audio_s, busy, worst = [], 0.0, 0.0, 0.0
        for clip, ref in refs.items():
            audio, _ = sf.read(d / clip, dtype="float32")
            if not warm:
                run(audio, SET_LANG[set_name])
                warm = True
            t = time.perf_counter()
            hyp = run(audio, SET_LANG[set_name])
            took = time.perf_counter() - t
            busy += took
            worst = max(worst, took)
            audio_s += len(audio) / 16000
            clips.append({"clip": clip, "ref": ref, "hyp": hyp, "s": took})
        r = [norm(c["ref"]) for c in clips]
        h = [norm(c["hyp"]) or "_" for c in clips]
        res = {"wer": jiwer.wer(r, h), "audio_s": audio_s, "busy_s": busy, "worst_clip_s": worst, "clips": clips}
        if SET_LANG[set_name] == "en" and set_name != "libri":
            flat = lambda s: re.sub(r"[^a-z0-9]", "", s)  # noqa: E731
            want = [(t, c) for c in clips for t in terms if flat(t) in flat(norm(c["ref"]))]
            res["terms"] = sum(flat(t) in flat(norm(c["hyp"])) for t, c in want) / max(len(want), 1)
        out["sets"][set_name] = res
    out["peak_rss_mb"] = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1e6
    (bench / "out").mkdir(exist_ok=True)
    (bench / "out" / f"{name}.json").write_text(json.dumps(out, ensure_ascii=False, indent=1))


def table(bench: Path) -> None:
    rows = []
    for name in MODELS:
        p = bench / "out" / f"{name}.json"
        if not p.exists():
            continue
        o = json.loads(p.read_text())
        s = o["sets"]
        cell = lambda k, f: f(s[k]) if k in s else "—"  # noqa: E731
        en = [k for k in ("coding", "noisy", "libri") if k in s]
        busy = sum(s[k]["busy_s"] for k in en)
        audio = sum(s[k]["audio_s"] for k in en)
        rows.append([name,
                     cell("coding", lambda r: f"{r['wer']*100:.1f}"), cell("noisy", lambda r: f"{r['wer']*100:.1f}"),
                     cell("coding", lambda r: f"{r['terms']*100:.0f}"), cell("noisy", lambda r: f"{r['terms']*100:.0f}"),
                     cell("libri", lambda r: f"{r['wer']*100:.1f}"), cell("uk", lambda r: f"{r['wer']*100:.1f}"),
                     cell("ru", lambda r: f"{r['wer']*100:.1f}"), cell("mine", lambda r: f"{r['wer']*100:.1f}"),
                     f"{audio/busy:.0f}x", f"{max(s[k]['worst_clip_s'] for k in en):.2f}", f"{o['load_s']:.1f}",
                     f"{o['peak_rss_mb']:.0f}"])
    head = ["model", "code WER", "noisy WER", "code terms", "noisy terms", "libri WER", "uk WER", "ru WER", "mine WER",
            "speed", "worst clip s", "load s", "peak MB"]
    print("| " + " | ".join(head) + " |\n|" + "---|" * len(head))
    for r in rows:
        print("| " + " | ".join(r) + " |")


def main() -> None:
    bench = Path(sys.argv[1])
    if "--one" in sys.argv:
        one(bench, sys.argv[sys.argv.index("--one") + 1])
        return
    if "--table" not in sys.argv:
        for name in [a for a in sys.argv[2:] if not a.startswith("--")] or MODELS:
            print(f"… {name}", file=sys.stderr, flush=True)
            subprocess.run([sys.executable, "-I", __file__, str(bench), "--one", name], check=False,
                           stderr=subprocess.DEVNULL if "--quiet" in sys.argv else None)
    table(bench)


if __name__ == "__main__":
    main()
