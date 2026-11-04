#!/usr/bin/env python3
"""Renders the ZipShrink app icon in the macOS Big Sur style: a rounded
square (superellipse) with depth, a zipped document in the centre, and a
soft drop shadow. Writes assets/icon-1024.png (the macOS AppIcon.icns is
made from it by packaging/build-macos.sh), icon-256.png, icon-48.png and
zipshrink.ico. Requires Pillow and NumPy."""
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

S = 1024  # canvas
root = Path(__file__).resolve().parent.parent
assets = root / "assets"


def superellipse_mask(size, inset, n=5.0):
    """Big Sur's continuous-corner rounded square."""
    a = (size - 2 * inset) / 2
    c = size / 2
    y, x = np.mgrid[0:size, 0:size].astype(np.float64)
    v = np.abs((x + 0.5 - c) / a) ** n + np.abs((y + 0.5 - c) / a) ** n
    # Soft edge for anti-aliasing.
    alpha = np.clip((1.0 - v) * a / 2.0 + 0.5, 0, 1)
    return Image.fromarray((alpha * 255).astype(np.uint8), "L")


def vertical_gradient(size, top, bottom):
    t = np.linspace(0, 1, size)[:, None]
    rgb = np.array(top)[None, None, :] * (1 - t[..., None]) + np.array(bottom)[None, None, :] * t[..., None]
    rgb = np.repeat(rgb, size, axis=1)
    return Image.fromarray(rgb.astype(np.uint8), "RGB")


def render():
    inset = 100  # Big Sur grid: 824 px icon on a 1024 px canvas
    canvas = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    mask = superellipse_mask(S, inset)

    # Drop shadow under the tile.
    shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    sm = mask.point(lambda v: int(v * 0.30))
    shadow.putalpha(sm)
    shadow = shadow.transform((S, S), Image.AFFINE, (1, 0, 0, 0, 1, -12)).filter(ImageFilter.GaussianBlur(14))
    canvas = Image.alpha_composite(canvas, shadow)

    # Tile: blue gradient with a light top sheen.
    tile = vertical_gradient(S, (88, 182, 255), (0, 86, 230)).convert("RGBA")
    sheen = vertical_gradient(S, (255, 255, 255), (255, 255, 255)).convert("RGBA")
    sheen_alpha = np.clip(np.linspace(0.22, 0.0, S), 0, 1)[:, None].repeat(S, axis=1)
    sheen.putalpha(Image.fromarray((sheen_alpha * 255).astype(np.uint8), "L"))
    tile = Image.alpha_composite(tile, sheen)
    tile.putalpha(mask)
    canvas = Image.alpha_composite(canvas, tile)

    # The document: white sheet with a folded corner and a zipper.
    doc = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(doc)
    l, t, r, b, fold = 312, 222, 712, 802, 120
    body = [(l + 28, t), (r - fold, t), (r, t + fold), (r, b - 28), (r - 28, b), (l + 28, b), (l, b - 28), (l, t + 28)]
    # Shadow of the sheet.
    sh = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(sh).polygon([(x, y + 22) for x, y in body], fill=(0, 30, 90, 120))
    sh = sh.filter(ImageFilter.GaussianBlur(22))
    canvas = Image.alpha_composite(canvas, sh)
    d.polygon(body, fill=(255, 255, 255, 255))
    # Rounded top-left corner.
    d.pieslice([l, t, l + 56, t + 56], 180, 270, fill=(255, 255, 255, 255))
    # Folded corner.
    d.polygon([(r - fold, t), (r - fold, t + fold - 18), (r - fold + 18, t + fold), (r, t + fold)], fill=(214, 228, 245, 255))
    # Zipper teeth down the middle.
    cx = (l + r - fold / 2) / 2 - 10
    y = t + 70
    k = 0
    while y < b - 150:
        off = -26 if k % 2 == 0 else 2
        d.rounded_rectangle([cx + off, y, cx + off + 24, y + 22], radius=6, fill=(120, 132, 150, 255))
        y += 30
        k += 1
    # The pull, part-way down, with the opened gap below it in blue.
    py = b - 150
    d.rounded_rectangle([cx - 30, py, cx + 30, py + 34], radius=10, fill=(70, 82, 100, 255))
    d.rounded_rectangle([cx - 18, py + 30, cx + 18, py + 112], radius=16, fill=(70, 82, 100, 255))
    d.ellipse([cx - 8, py + 74, cx + 8, py + 90], fill=(255, 255, 255, 255))
    canvas = Image.alpha_composite(canvas, doc)

    # Inner edge highlight of the tile.
    edge = Image.new("RGBA", (S, S), (255, 255, 255, 0))
    inner = superellipse_mask(S, inset + 3)
    ring = Image.fromarray(np.clip(np.array(mask, dtype=np.int16) - np.array(inner, dtype=np.int16), 0, 255).astype(np.uint8), "L")
    edge.putalpha(ring.point(lambda v: int(v * 0.25)))
    canvas = Image.alpha_composite(canvas, edge)
    return canvas


def main():
    big = render()
    big.save(assets / "icon-1024.png", optimize=True)
    big.resize((256, 256), Image.LANCZOS).save(assets / "icon-256.png")
    big.resize((48, 48), Image.LANCZOS).save(assets / "icon-48.png")
    big.resize((256, 256), Image.LANCZOS).save(
        assets / "zipshrink.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
    )


if __name__ == "__main__":
    main()
