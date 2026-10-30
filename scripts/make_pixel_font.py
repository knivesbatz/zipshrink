#!/usr/bin/env python3
"""Builds the "ZipShrink Pixel" fonts in assets/fonts/.

The retro interface needs text drawn the way 1990s desktops drew it:
monochrome, hinted, no anti-aliasing. egui always anti-aliases outlines, so
instead we rasterise Liberation Sans (SIL Open Font License 1.1) with
FreeType's monochrome hinted renderer at one pixel size and turn every lit
pixel into a square in a new TrueType font. Drawn at its design size, egui
then reproduces the bitmap exactly.

Requires: pip install fonttools pillow numpy
Usage:    python3 scripts/make_pixel_font.py /path/to/LiberationSans-Regular.ttf

The bold face is synthesised from the regular bitmap the way GDI did it:
the glyph drawn twice, one pixel apart.
"""

import sys
from pathlib import Path

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from PIL import Image, ImageDraw, ImageFont

PX = 11          # rasterisation size
UNIT = 100       # font units per pixel
# Hand-drawn digits, as bitmap UI fonts of the era had: FreeType's hinting
# at 11 px leaves them uneven. Each is 5 px wide on a 6 px advance, sitting
# on the baseline.
DIGITS = {
    "0": [".###.", "#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."],
    "1": ["..#..", ".##..", "..#..", "..#..", "..#..", "..#..", "..#..", ".###."],
    "2": [".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#....", "#####"],
    "3": [".###.", "#...#", "....#", "..##.", "....#", "....#", "#...#", ".###."],
    "4": ["...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#.", "...#."],
    "5": ["#####", "#....", "####.", "....#", "....#", "....#", "#...#", ".###."],
    "6": [".###.", "#....", "#....", "####.", "#...#", "#...#", "#...#", ".###."],
    "7": ["#####", "....#", "...#.", "...#.", "..#..", "..#..", ".#...", ".#..."],
    "8": [".###.", "#...#", "#...#", ".###.", "#...#", "#...#", "#...#", ".###."],
    "9": [".###.", "#...#", "#...#", "#...#", ".####", "....#", "...#.", ".##.."],
}

RANGES = [(0x20, 0x7E), (0xA0, 0x17F), (0x2010, 0x2027), (0x2030, 0x203A), (0x2190, 0x2193), (0x2248, 0x2248), (0x2264, 0x2265)]


def build(src: str, out: Path, style: str, embolden: bool = False) -> None:
    font = ImageFont.truetype(src, PX)
    asc, desc = font.getmetrics()
    height = asc + desc
    order, cmap, glyphs, metrics = [".notdef"], {}, {}, {}

    pen = TTGlyphPen(None)
    glyphs[".notdef"] = pen.glyph()
    metrics[".notdef"] = (4 * UNIT, 0)

    for lo, hi in RANGES:
        for cp in range(lo, hi + 1):
            ch = chr(cp)
            try:
                if font.getmask(ch).size == (0, 0) and ch != " ":
                    continue
            except Exception:
                continue
            adv = round(font.getlength(ch)) + (1 if embolden and ch != " " else 0)
            w = max(adv, 1) + 4
            img = Image.new("1", (w + 4, height), 0)
            d = ImageDraw.Draw(img)
            d.fontmode = "1"  # FreeType monochrome, hinted: no grey pixels
            if ch in DIGITS:
                rows = DIGITS[ch]
                for r, row in enumerate(rows):
                    for c, v in enumerate(row):
                        if v == "#":
                            img.putpixel((2 + c, asc - len(rows) + r), 1)
                adv = 6 + (1 if embolden else 0)
            else:
                d.text((2, 0), ch, font=font, fill=1)
            if embolden:
                # GDI-style synthetic bold: the glyph OR'd with itself one pixel to the right.
                shifted = Image.new("1", img.size, 0)
                shifted.paste(img, (1, 0))
                img = Image.fromarray(__import__("numpy").logical_or(__import__("numpy").array(img), __import__("numpy").array(shifted)))
            px = img.load()
            pen = TTGlyphPen(None)
            xmin = None
            for y in range(height):
                x = 0
                while x < img.width:
                    if px[x, y]:
                        start = x
                        while x < img.width and px[x, y]:
                            x += 1
                        # pixel (start..x, y) -> square in font units; y up from baseline
                        x0, x1 = (start - 2) * UNIT, (x - 2) * UNIT
                        y1, y0 = (asc - y) * UNIT, (asc - y - 1) * UNIT
                        pen.moveTo((x0, y0))
                        pen.lineTo((x0, y1))
                        pen.lineTo((x1, y1))
                        pen.lineTo((x1, y0))
                        pen.closePath()
                        xmin = x0 if xmin is None else min(xmin, x0)
                    else:
                        x += 1
            name = f"uni{cp:04X}"
            order.append(name)
            cmap[cp] = name
            glyphs[name] = pen.glyph()
            metrics[name] = (adv * UNIT, xmin or 0)

    family = "ZipShrink Pixel"
    fb = FontBuilder(height * UNIT, isTTF=True)
    fb.setupGlyphOrder(order)
    fb.setupCharacterMap(cmap)
    fb.setupGlyf(glyphs)
    fb.setupHorizontalMetrics(metrics)
    fb.setupHorizontalHeader(ascent=asc * UNIT, descent=-desc * UNIT)
    fb.setupNameTable({
        "familyName": family,
        "styleName": style,
        "uniqueFontIdentifier": f"{family} {style}",
        "fullName": f"{family} {style}",
        "psName": f"ZipShrinkPixel-{style}",
        "copyright": "Bitmap derived from Liberation Sans (c) 2012 Red Hat, Inc., SIL Open Font License 1.1",
        "licenseDescription": "SIL Open Font License, Version 1.1",
    })
    fb.setupOS2(sTypoAscender=asc * UNIT, sTypoDescender=-desc * UNIT, usWinAscent=asc * UNIT, usWinDescent=desc * UNIT, usWeightClass=700 if style == "Bold" else 400)
    fb.setupPost()
    fb.save(str(out))
    print(f"{out}: {len(order)} glyphs, {height}px line height (draw at size {height})")


if __name__ == "__main__":
    dest = Path(__file__).resolve().parent.parent / "assets" / "fonts"
    dest.mkdir(parents=True, exist_ok=True)
    build(sys.argv[1], dest / "ZipShrinkPixel-Regular.ttf", "Regular")
    build(sys.argv[1], dest / "ZipShrinkPixel-Bold.ttf", "Bold", embolden=True)
