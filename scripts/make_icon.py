#!/usr/bin/env python3
"""Renders the 16x16 pixel-art app icon (from the GUI source) to PNG/ICO
with nearest-neighbour scaling. Requires Pillow."""
import re
from pathlib import Path
from PIL import Image

root = Path(__file__).resolve().parent.parent
src = (root / "crates/zipshrink-gui/src/icons.rs").read_text()
art = re.search(r"pub const APP: Art = \[(.*?)\];", src, re.S).group(1)
rows = re.findall(r'"([^"]+)"', art)
pal = {"K": (0, 0, 0), "W": (255, 255, 255), "G": (192, 192, 192), "D": (128, 128, 128), "Y": (255, 255, 0), "R": (255, 0, 0)}
img = Image.new("RGBA", (16, 16), (0, 0, 0, 0))
for y, row in enumerate(rows):
    for x, c in enumerate(row):
        if c in pal:
            img.putpixel((x, y), pal[c] + (255,))
assets = root / "assets"
big = img.resize((256, 256), Image.NEAREST)
big.save(assets / "icon-256.png")
img.resize((48, 48), Image.NEAREST).save(assets / "icon-48.png")
sizes = [16, 24, 32, 48, 64, 128, 256]
big.save(assets / "zipshrink.ico", sizes=[(s, s) for s in sizes], append_images=[img.resize((s, s), Image.NEAREST) for s in sizes])
print("wrote", assets / "zipshrink.ico")
