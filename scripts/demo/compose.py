"""Demo recordings (demo.rs) cut into a finished video: the window framed on a backdrop with
its traffic lights, rounded corners and shadow, the camera easing in and out where the script
zoomed, captions as fading pills, title cards, and a quiet music bed.

    python compose.py <cut.json>

cut.json:
    {"out": "overview.mp4", "theme": "dark", "music": true, "music_lufs": -30,
     "clips": [
        {"card": {"title": "YourHarness", "subtitle": "…"}, "dur": 3.5},
        {"video": "scene.mp4", "from": 0, "to": 12.5}
     ]}

A clip's captions and zooms come from `<video>.captions.json`, next to it. Writes `<out>`,
`<out>.srt` (captions with their final times) and, with "poster": <seconds>, a PNG still.
"""

import json
import os
import subprocess
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
FFMPEG = "/opt/homebrew/bin/ffmpeg"
OUT_W, OUT_H = 1920, 1080
SS = 2  # the canvas is drawn at twice the output size, as the window is
CW, CH = OUT_W * SS, OUT_H * SS
FPS = 30
FONT = "/System/Library/Fonts/SFNS.ttf"
FADE = 0.35

THEMES = {
    "dark": {"top": (28, 30, 46), "bottom": (12, 13, 20), "glow": (84, 70, 160), "text": (245, 245, 247), "sub": (170, 170, 182), "pill": (20, 20, 24, 215), "edge": (255, 255, 255, 40)},
    "light": {"top": (236, 238, 245), "bottom": (214, 219, 232), "glow": (170, 190, 255), "text": (29, 29, 31), "sub": (99, 99, 106), "pill": (28, 28, 30, 220), "edge": (0, 0, 0, 30)},
}


def font(size, weight="Regular"):
    f = ImageFont.truetype(FONT, size)
    try:
        f.set_variation_by_name(weight)
    except Exception:
        pass
    return f


def ease(t):
    t = min(max(t, 0.0), 1.0)
    return 4 * t * t * t if t < 0.5 else 1 - (-2 * t + 2) ** 3 / 2


class Stage:
    """The backdrop and where the window sits on it."""

    def __init__(self, theme, win_w, win_h, centred=False):
        self.t = THEMES[theme]
        self.win_w, self.win_h = win_w, win_h
        # The window at its own pixels (2x), centred a little high: room for captions below
        # (stills: centred).
        self.wx = (CW - win_w) // 2
        self.wy = (CH - win_h) // 2 if centred else max(60, (CH - win_h) // 2 - 70)
        self.backdrop = self._backdrop()
        self.base = self._with_shadow()
        self.mask = self._corner_mask(28)

    def _backdrop(self):
        y = np.linspace(0, 1, CH)[:, None, None]
        top, bottom = np.array(self.t["top"], float), np.array(self.t["bottom"], float)
        img = top * (1 - y) + bottom * y
        img = np.broadcast_to(img, (CH, CW, 3)).copy()
        # A soft coloured glow behind the window.
        yy, xx = np.mgrid[0:CH:4, 0:CW:4]
        d = ((xx - CW * 0.5) / (CW * 0.55)) ** 2 + ((yy - CH * 0.42) / (CH * 0.6)) ** 2
        glow = np.exp(-d * 2.2)[:, :, None]
        glow = np.array(Image.fromarray((glow[:, :, 0] * 255).astype(np.uint8)).resize((CW, CH), Image.BILINEAR), float)[:, :, None] / 255
        img = img * (1 - glow * 0.55) + np.array(self.t["glow"], float) * glow * 0.55
        return img.clip(0, 255).astype(np.uint8)

    def _with_shadow(self):
        shadow = Image.new("L", (CW, CH), 0)
        d = ImageDraw.Draw(shadow)
        d.rounded_rectangle([self.wx, self.wy + 30, self.wx + self.win_w, self.wy + self.win_h + 30], 40, fill=150)
        shadow = shadow.filter(ImageFilter.GaussianBlur(70))
        a = np.array(shadow, float)[:, :, None] / 255 * (0.85 if self.t is THEMES["dark"] else 0.45)
        return (self.backdrop * (1 - a)).astype(np.uint8)

    def _corner_mask(self, r):
        m = Image.new("L", (r * 4, r * 4), 0)
        ImageDraw.Draw(m).ellipse([0, 0, r * 8 - 1, r * 8 - 1], fill=255)
        m = m.resize((r, r), Image.LANCZOS)  # a quarter circle, antialiased
        return np.array(m, float)[:, :, None] / 255

    def place(self, window):
        """The canvas with this window frame on it."""
        canvas = self.base.copy()
        h, w = window.shape[:2]
        x, y = self.wx, self.wy
        canvas[y : y + h, x : x + w] = window
        r = self.mask.shape[0]
        # Rounded corners: blend each corner patch with the backdrop beneath.
        for flip_y in (False, True):
            for flip_x in (False, True):
                m = self.mask
                if flip_y:
                    m = m[::-1]
                if flip_x:
                    m = m[:, ::-1]
                ys = slice(y + h - r, y + h) if flip_y else slice(y, y + r)
                xs = slice(x + w - r, x + w) if flip_x else slice(x, x + r)
                canvas[ys, xs] = (canvas[ys, xs] * m + self.base[ys, xs] * (1 - m)).astype(np.uint8)
        # A hairline edge, as macOS draws around windows.
        edge = np.array(self.t["edge"][:3], float)
        alpha = self.t["edge"][3] / 255
        for ys, xs in ((slice(y, y + 2), slice(x + r, x + w - r)), (slice(y + h - 2, y + h), slice(x + r, x + w - r)), (slice(y + r, y + h - r), slice(x, x + 2)), (slice(y + r, y + h - r), slice(x + w - 2, x + w))):
            canvas[ys, xs] = (canvas[ys, xs] * (1 - alpha) + edge * alpha).astype(np.uint8)
        return canvas


class PairStage:
    """The Mac window (smaller) and an iPhone beside it, on the same backdrop."""

    PHONE_H = 1720  # the phone's screen height on the canvas
    BEZEL = 26

    def __init__(self, theme, win_w, win_h, phone_w, phone_h):
        self.t = THEMES[theme]
        self.backdrop = Stage(theme, 2880, 1800).backdrop
        self.ph = self.PHONE_H
        self.pw = round(phone_w * self.ph / phone_h)
        gap = 150
        self.mw = CW - 2 * 170 - gap - self.pw - 2 * self.BEZEL
        self.mh = round(win_h * self.mw / win_w)
        self.mx, self.my = 170, max(60, (CH - self.mh) // 2 - 70)
        self.px = self.mx + self.mw + gap + self.BEZEL
        self.py = max(60, (CH - self.ph) // 2 - 70)
        img = Image.fromarray(self.backdrop)
        shadow = Image.new("L", (CW, CH), 0)
        d = ImageDraw.Draw(shadow)
        d.rounded_rectangle([self.mx, self.my + 26, self.mx + self.mw, self.my + self.mh + 26], 36, fill=150)
        b = self.BEZEL
        d.rounded_rectangle([self.px - b, self.py - b + 26, self.px + self.pw + b, self.py + self.ph + b + 26], 150, fill=150)
        shadow = shadow.filter(ImageFilter.GaussianBlur(60))
        a = np.array(shadow, float)[:, :, None] / 255 * (0.85 if self.t is THEMES["dark"] else 0.45)
        base = (self.backdrop * (1 - a)).astype(np.uint8)
        # The phone's body: a dark titanium edge around the screen.
        body = Image.fromarray(base)
        bd = ImageDraw.Draw(body)
        bd.rounded_rectangle([self.px - b, self.py - b, self.px + self.pw + b, self.py + self.ph + b], 150, fill=(38, 38, 42))
        bd.rounded_rectangle([self.px - b + 4, self.py - b + 4, self.px + self.pw + b - 4, self.py + self.ph + b - 4], 146, outline=(90, 90, 96), width=2)
        self.base = np.array(body)
        self.win_mask = Stage._corner_mask(self, 24)
        m = Image.new("L", (self.pw * 2, self.ph * 2), 0)
        ImageDraw.Draw(m).rounded_rectangle([0, 0, self.pw * 2 - 1, self.ph * 2 - 1], 124 * 2, fill=255)
        self.phone_mask = np.array(m.resize((self.pw, self.ph), Image.LANCZOS), float)[:, :, None] / 255

    def place(self, window, phone):
        canvas = self.base.copy()
        win = np.array(Image.fromarray(window).resize((self.mw, self.mh), Image.BICUBIC))
        x, y, w, h = self.mx, self.my, self.mw, self.mh
        canvas[y : y + h, x : x + w] = win
        r = self.win_mask.shape[0]
        for fy in (False, True):
            for fx in (False, True):
                m = self.win_mask[::-1] if fy else self.win_mask
                m = m[:, ::-1] if fx else m
                ys = slice(y + h - r, y + h) if fy else slice(y, y + r)
                xs = slice(x + w - r, x + w) if fx else slice(x, x + r)
                canvas[ys, xs] = (canvas[ys, xs] * m + self.base[ys, xs] * (1 - m)).astype(np.uint8)
        ph = np.array(Image.fromarray(phone).resize((self.pw, self.ph), Image.BICUBIC), float)
        ys, xs = slice(self.py, self.py + self.ph), slice(self.px, self.px + self.pw)
        canvas[ys, xs] = (ph * self.phone_mask + canvas[ys, xs] * (1 - self.phone_mask)).astype(np.uint8)
        return canvas


def phone_frames(video, start, w, h):
    """The Simulator's film from `start` seconds (before its first frame: the first frame), at
    the edit's rate: it only writes a frame when the screen changes."""
    first = None
    lead = int(max(0.0, -start) * FPS)
    for frame in frames_of_vfr(video, max(0.0, start), w, h):
        if first is None:
            first = frame
            for _ in range(lead):
                yield first
        yield frame
    while True:
        yield first if first is not None else np.zeros((h, w, 3), np.uint8)


def frames_of_vfr(video, start, w, h):
    args = [FFMPEG, "-v", "error", "-ss", str(start), "-i", video, "-vf", f"fps={FPS}", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]
    p = subprocess.Popen(args, stdout=subprocess.PIPE, bufsize=w * h * 3 * 2)
    size = w * h * 3
    last = None
    while True:
        buf = p.stdout.read(size)
        if len(buf) < size:
            break
        last = np.frombuffer(buf, np.uint8).reshape(h, w, 3)
        yield last
    p.wait()
    # The last frame held: the film ends at its last change.
    while last is not None:
        yield last


def blur_regions(frame, regions, t):
    """Rects (in the frame's pixels) blurred while `from` <= t < `to`: addresses kept private."""
    img = None
    for r in regions:
        if r.get("from", -1e9) <= t < r.get("to", 1e9):
            if img is None:
                img = Image.fromarray(frame)
            x, y, w, h = r["rect"]
            box = (int(x), int(y), int(x + w), int(y + h))
            img.paste(img.crop(box).filter(ImageFilter.GaussianBlur(r.get("radius", 28))), box)
    return frame if img is None else np.array(img)


def traffic_lights(frame):
    """The window's close, minimise and zoom buttons, which AppKit draws (not in the frame)."""
    img = Image.fromarray(frame)
    d = ImageDraw.Draw(img)
    for i, (fill, rim) in enumerate((((255, 95, 87), (226, 70, 63)), ((254, 188, 46), (223, 158, 35)), ((40, 200, 64), (30, 170, 50)))):
        cx, cy, r = (14 + 6 + i * 20) * 2, (14 + 6) * 2, 12
        d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=fill, outline=rim, width=1)
    return np.array(img)


class Camera:
    """Where the output frame looks on the canvas: all of it, or eased in on a zoom."""

    def __init__(self, stage, zooms, offset):
        self.stage = stage
        self.keys = []  # (start, dur, from rect, to rect)
        full = (0.0, 0.0, float(CW), float(CH))
        current = full
        for z in zooms:
            target = full if z["rect"] is None else self.fit(z["rect"], z.get("pad"))
            self.keys.append((z["t"] - offset, z.get("dur", 1.0), current, target))
            current = target
        self.full = full

    def fit(self, rect, pad):
        x, y, w, h = rect
        pad = pad if isinstance(pad, (int, float)) else 40
        # Window points → canvas pixels, padded, grown to 16:9, kept on the canvas.
        x, y = self.stage.wx + (x - pad) * 2, self.stage.wy + (y - pad) * 2
        w, h = (w + 2 * pad) * 2, (h + 2 * pad) * 2
        cx, cy = x + w / 2, y + h / 2
        if w / h > CW / CH:
            h = w * CH / CW
        else:
            w = h * CW / CH
        w, h = min(max(w, CW / 2.6), CW), min(max(h, CH / 2.6), CH)
        # Kept on the window when it fits there, so the backdrop doesn't creep in.
        st = self.stage
        lo_x, hi_x = (st.wx, st.wx + st.win_w) if w <= st.win_w else (0, CW)
        lo_y, hi_y = (st.wy, st.wy + st.win_h) if h <= st.win_h else (0, CH)
        x = min(max(cx - w / 2, lo_x), hi_x - w)
        y = min(max(cy - h / 2, lo_y), hi_y - h)
        return (x, y, w, h)

    def at(self, t):
        rect = self.full
        for start, dur, a, b in self.keys:
            if t < start:
                break
            k = ease((t - start) / dur) if dur > 0 else 1
            rect = tuple(a[i] + (b[i] - a[i]) * k for i in range(4))
        return rect


def shoot(canvas, rect):
    x, y, w, h = rect
    if w >= CW - 1:
        return np.array(Image.fromarray(canvas).reduce(SS))
    crop = Image.fromarray(canvas[int(y) : int(y + h), int(x) : int(x + w)])
    return np.array(crop.resize((OUT_W, OUT_H), Image.BICUBIC))


class Captions:
    def __init__(self, theme):
        self.t = THEMES[theme]
        self.font = font(34, "Medium")
        self.cache = {}

    def pill(self, text):
        if text in self.cache:
            return self.cache[text]
        pad_x, pad_y = 34, 18
        lines = text.split("\n")
        boxes = [self.font.getbbox(l) for l in lines]
        tw = max(b[2] - b[0] for b in boxes)
        lh = 46
        w, h = tw + 2 * pad_x, lh * len(lines) + 2 * pad_y - 6
        img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
        d = ImageDraw.Draw(img)
        d.rounded_rectangle([0, 0, w - 1, h - 1], h // 2 if len(lines) == 1 else 22, fill=self.t["pill"])
        for i, l in enumerate(lines):
            lw = self.font.getbbox(l)[2]
            d.text(((w - lw) / 2, pad_y + i * lh - 4), l, font=self.font, fill=(255, 255, 255))
        self.cache[text] = img
        return img

    def draw(self, frame, text, alpha):
        if not text or alpha <= 0:
            return frame
        pill = self.pill(text)
        img = Image.fromarray(frame).convert("RGBA")
        layer = Image.new("RGBA", img.size, (0, 0, 0, 0))
        x = (OUT_W - pill.width) // 2
        y = OUT_H - pill.height - 34
        p = pill.copy()
        if alpha < 1:
            a = np.array(p)
            a[:, :, 3] = (a[:, :, 3] * alpha).astype(np.uint8)
            p = Image.fromarray(a)
        layer.paste(p, (x, y), p)
        return np.array(Image.alpha_composite(img, layer).convert("RGB"))


def card_frame(stage, card, theme, icon):
    t = THEMES[theme]
    img = Image.fromarray(np.array(Image.fromarray(stage.backdrop).reduce(SS)))
    d = ImageDraw.Draw(img)
    y = OUT_H // 2 - 150
    if icon is not None and card.get("icon", True):
        ic = icon.resize((168, 168), Image.LANCZOS)
        img.paste(ic, ((OUT_W - 168) // 2, y - 40), ic)
        y += 160
    title = card.get("title", "")
    f = font(card.get("size", 76), "Bold")
    w = d.textbbox((0, 0), title, font=f)[2]
    d.text(((OUT_W - w) / 2, y), title, font=f, fill=t["text"])
    y += int(card.get("size", 76) * 1.35)
    for line in card.get("subtitle", "").split("\n"):
        if not line:
            continue
        f2 = font(36, "Regular")
        w = d.textbbox((0, 0), line, font=f2)[2]
        d.text(((OUT_W - w) / 2, y), line, font=f2, fill=t["sub"])
        y += 52
    return np.array(img)


def frames_of(video, start, end, w, h, count=None):
    args = [FFMPEG, "-v", "error", "-ss", str(start)]
    if end is not None:
        args += ["-to", str(end)]
    args += ["-i", video]
    if count:
        args += ["-frames:v", str(count)]
    args += ["-f", "rawvideo", "-pix_fmt", "rgb24", "-"]
    p = subprocess.Popen(args, stdout=subprocess.PIPE, bufsize=w * h * 3 * 2)
    size = w * h * 3
    while True:
        buf = p.stdout.read(size)
        if len(buf) < size:
            break
        yield np.frombuffer(buf, np.uint8).reshape(h, w, 3)
    p.wait()


def probe_size(video):
    out = subprocess.check_output(["/opt/homebrew/bin/ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0", video]).decode().strip()
    w, h = out.split(",")
    return int(w), int(h)


def srt_time(t):
    ms = int(round(t * 1000))
    return f"{ms // 3600000:02}:{ms // 60000 % 60:02}:{ms // 1000 % 60:02},{ms % 1000:03}"


def still(video, at, out, theme, width=2560, crop=None):
    """One framed still from a recording (`--still <video> <seconds> <out.png> [theme] [width]`),
    as the videos frame it, at `width` pixels across (16:9)."""
    w, h = probe_size(video)
    stage = Stage(theme, w, h, centred=True)
    frame = next(frames_of(video, at, None, w, h, count=1))
    canvas = stage.place(traffic_lights(frame))
    img = Image.fromarray(canvas)
    if crop:
        img = img.crop(crop)
    img.resize((width, round(width * img.height / img.width)), Image.LANCZOS).save(out, optimize=True)
    print(out)


def main():
    if sys.argv[1] == "--still":
        video, at, out = sys.argv[2], float(sys.argv[3]), sys.argv[4]
        theme = sys.argv[5] if len(sys.argv) > 5 else "light"
        width = int(sys.argv[6]) if len(sys.argv) > 6 else 2560
        return still(video, at, out, theme, width)
    cut_path = sys.argv[1]
    cut = json.load(open(cut_path))
    base = os.path.dirname(os.path.abspath(cut_path))
    path = lambda p: p if os.path.isabs(p) else os.path.join(base, p)
    theme = cut.get("theme", "dark")
    out = path(cut["out"])
    silent = out + ".silent.mp4"
    icon_file = os.path.join(ROOT, "assets", "icons", cut.get("icon", "stable.png"))
    icon = Image.open(icon_file).convert("RGBA") if os.path.exists(icon_file) else None

    enc = subprocess.Popen(
        [FFMPEG, "-y", "-v", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{OUT_W}x{OUT_H}", "-r", str(FPS), "-i", "-",
         "-c:v", "libx264", "-preset", "slow", "-crf", str(cut.get("crf", 18)), "-pix_fmt", "yuv420p", "-movflags", "+faststart", silent],
        stdin=subprocess.PIPE,
    )
    caps = Captions(theme)
    srt = []
    clock = 0.0  # where the finished video stands
    poster_at = cut.get("poster")
    poster = None
    stage = None
    n = 0

    def emit(frame):
        nonlocal n, poster
        enc.stdin.write(frame.tobytes())
        if poster_at is not None and poster is None and n / FPS >= poster_at:
            poster = frame
        n += 1

    for clip in cut["clips"]:
        if "card" in clip:
            card_theme = clip.get("theme", theme)
            if stage is None or stage.t is not THEMES[card_theme]:
                stage = Stage(card_theme, 2880, 1800)
            frame = card_frame(stage, clip["card"], card_theme, icon)
            back = np.array(Image.fromarray(stage.backdrop).reduce(SS))
            count = int(clip.get("dur", 3) * FPS)
            for i in range(count):
                t = i / FPS
                a = min(1.0, t / FADE, (count / FPS - t) / FADE)
                emit((back * (1 - ease(a)) + frame * ease(a)).astype(np.uint8))
            clock += count / FPS
            continue

        clip_theme = clip.get("theme", theme)
        if caps.t is not THEMES[clip_theme]:
            caps = Captions(clip_theme)
        video = path(clip["video"])
        meta = json.load(open(video + ".captions.json"))
        start = clip.get("from", 0.0)
        end = clip.get("to")
        length = (end if end is not None else meta["duration"]) - start
        w, h = probe_size(video)
        if stage is None or (stage.win_w, stage.win_h) != (w, h) or stage.t is not THEMES[clip_theme]:
            stage = Stage(clip_theme, w, h)
        # `phone`: the Simulator's film beside the Mac, lined up by `sync` (record-phone.sh).
        pair = phones = None
        if "phone" in clip:
            phone = path(clip["phone"])
            sync = json.load(open(path(clip["sync"])))
            pw, ph = probe_size(phone)
            pair = PairStage(clip_theme, w, h, pw, ph)
            offset = sync["phone_start"] - sync["mac_start"]
            phone_t0 = clip.get("from", 0.0) - offset + clip.get("phone_shift", 0.0)
            phones = phone_frames(phone, phone_t0, pw, ph)
            phone_blur = clip.get("phone_blur", [])
        speed = max(1, int(clip.get("speed", 1)))
        cam = Camera(stage, meta.get("zooms", []), start)
        cues = [c for c in meta.get("captions", []) if c["end"] > start and (end is None or c["start"] < end)]
        for c in cues:
            s, e = max(c["start"], start) - start, min(c["end"], start + length) - start
            srt.append((clock + s / speed, clock + e / speed, c["text"]))
        back = np.array(Image.fromarray(stage.backdrop).reduce(SS))
        fade_in, fade_out = clip.get("fade_in", True), clip.get("fade_out", True)
        for i, win in enumerate(frames_of(video, start, end, w, h)):
            if i % speed:
                continue
            t = i / FPS
            if pair is not None:
                ph_frame = next(phones)
                for _ in range(speed - 1):
                    next(phones)
                ph_frame = blur_regions(ph_frame, phone_blur, phone_t0 + t)
                frame = np.array(Image.fromarray(pair.place(traffic_lights(win), ph_frame)).reduce(SS))
            else:
                canvas = stage.place(traffic_lights(win))
                frame = shoot(canvas, cam.at(t))
            text, alpha = None, 0.0
            for c in cues:
                s, e = c["start"] - start, c["end"] - start
                if s <= t < e:
                    text = c["text"]
                    alpha = min(1.0, (t - s) / 0.3, (e - t) / 0.3)
            frame = caps.draw(frame, text, alpha)
            a = 1.0
            if fade_in:
                a = min(a, t / speed / FADE)
            if fade_out:
                a = min(a, (length - t) / speed / FADE)
            if a < 1:
                frame = (back * (1 - ease(a)) + frame * ease(max(a, 0))).astype(np.uint8)
            emit(frame)
            if i % 60 == 0:
                print(f"  {os.path.basename(video)} {t:5.1f}/{length:.1f} s", flush=True)
        clock += length / speed

    enc.stdin.close()
    enc.wait()
    duration = n / FPS

    with open(os.path.splitext(out)[0] + ".srt", "w") as f:
        for i, (s, e, text) in enumerate(srt, 1):
            f.write(f"{i}\n{srt_time(s)} --> {srt_time(e)}\n{text}\n\n")
    if poster is not None:
        Image.fromarray(poster).save(os.path.splitext(out)[0] + ".png")

    if cut.get("music", True):
        wav = out + ".music.wav"
        subprocess.check_call([sys.executable, "-I", os.path.join(HERE, "music.py"), wav, str(duration + 0.5), str(cut.get("music_seed", 7))])
        lufs = cut.get("music_lufs", -30)
        subprocess.check_call([FFMPEG, "-y", "-v", "error", "-i", silent, "-i", wav, "-filter_complex",
                               f"[1:a]loudnorm=I={lufs}:TP=-6:LRA=7,afade=t=out:st={max(0, duration - 3)}:d=3[a]",
                               "-map", "0:v", "-map", "[a]", "-c:v", "copy", "-c:a", "aac", "-b:a", "160k", "-shortest", out])
        os.remove(wav)
        os.remove(silent)
    else:
        os.replace(silent, out)
    print(f"{out}: {duration:.1f} s, {n} frames")


if __name__ == "__main__":
    main()
