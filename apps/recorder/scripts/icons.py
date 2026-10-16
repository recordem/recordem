#!/usr/bin/env python3
"""The app icons, drawn from the menu-bar glyph (icons/tray.png): five bars of a waveform and the record ring at the
bottom right, cut out of the bars. Writes icons/ (the release, a red ring) and icons/dev/ (a dev build, violet), each
with source.png (1024), icon.png (512), 32x32, 128x128, 128x128@2x and icon.icns, plus icons/dev/tray-rec.png (the
menu bar while recording, violet). Needs Pillow: `python3 apps/recorder/scripts/icons.py`."""
from pathlib import Path
from PIL import Image, ImageDraw

ICONS = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"
BG, BARS = (41, 37, 36, 255), (248, 248, 248, 255)
RING = {"": (220, 38, 38, 255), "dev": (124, 58, 237, 255)}
X = 4  # drawn at 4x, then scaled down: smooth edges

# The glyph in tray.png's 44 px grid: bars as (centre x, top, bottom), width 6; the ring's centre, radius, stroke; the
# gap cut around it.
BAR_W = 6
BARS_44 = [(6, 17, 28), (14, 13, 32), (22, 8, 37), (30, 13, 32), (38, 17, 28)]
RING_44 = (35.5, 35.5, 6.5, 3.0)
GAP_44 = 1.6


def draw(ring):
    size = 1024 * X
    im = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    m = 64 * X
    d.rounded_rectangle((m, m, size - m, size - m), radius=200 * X, fill=BG)
    # The glyph spans x 3..41, y 8..42 of the grid: centred in the square, ~60% of its width.
    s = 0.6 * (size - 2 * m) / 38
    ox, oy = size / 2 - 22 * s, size / 2 - 25 * s
    for cx, top, bottom in BARS_44:
        x0, x1 = ox + (cx - BAR_W / 2) * s, ox + (cx + BAR_W / 2) * s
        d.rounded_rectangle((x0, oy + top * s, x1, oy + bottom * s), radius=BAR_W / 2 * s, fill=BARS)
    cx, cy, r, w = (v * s for v in RING_44)
    cx, cy = ox + cx, oy + cy
    g = GAP_44 * s
    d.ellipse((cx - r - g, cy - r - g, cx + r + g, cy + r + g), fill=BG)
    d.ellipse((cx - r, cy - r, cx + r, cy + r), outline=ring, width=round(w))
    return im.resize((1024, 1024), Image.LANCZOS)


def write(dir, ring):
    dir.mkdir(exist_ok=True)
    src = draw(ring)
    src.save(dir / "source.png")
    for name, px in [("icon.png", 512), ("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256)]:
        src.resize((px, px), Image.LANCZOS).save(dir / name)
    src.save(dir / "icon.icns", sizes=[(16, 16), (32, 32), (64, 64), (128, 128), (256, 256), (512, 512), (1024, 1024)])


write(ICONS, RING[""])
write(ICONS / "dev", RING["dev"])

# The menu bar while a dev build records: tray-rec.png's red, violet.
alpha = Image.open(ICONS / "tray-rec.png").convert("RGBA").getchannel("A")
violet = Image.new("RGBA", alpha.size, RING["dev"])
violet.putalpha(alpha)
violet.save(ICONS / "dev" / "tray-rec.png")
