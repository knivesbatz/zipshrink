#!/usr/bin/env python3
"""Writes the 16x16 pixel-art program icon as assets/zipshrink.ico (plus PNG
previews), scaled with nearest-neighbour.

The .ico holds classic uncompressed bitmaps only: 16-colour entries (every
colour of the art is in the standard VGA palette) that Windows 95 and NT 4
display, and 32-bit entries for Windows XP and later. PNG-compressed entries,
which Windows before Vista cannot read, are left out. Needs Pillow only for
the previews."""
import struct
from pathlib import Path

ART = [
    "................",
    ".KKKKKKKKKKKKKK.",
    ".KWWWWWWWWWWWDK.",
    ".KWGGGGKKGGGGDK.",
    ".KWGGGGKYGGGGDK.",
    ".KWGGGGYKGGGGDK.",
    ".KWGGGGKYGGGGDK.",
    ".KWGGGGYKGGGGDK.",
    ".KWGGGKKKKGGGDK.",
    ".KWGGGKRRKGGGDK.",
    ".KWGGGKKKKGGGDK.",
    ".KWGGGGGGGGGGDK.",
    ".KWGGGGGGGGGGDK.",
    ".KDDDDDDDDDDDDK.",
    ".KKKKKKKKKKKKKK.",
    "................",
]

# The 16-colour VGA palette, in Windows order.
VGA = [(0, 0, 0), (128, 0, 0), (0, 128, 0), (128, 128, 0), (0, 0, 128), (128, 0, 128), (0, 128, 128), (192, 192, 192),
       (128, 128, 128), (255, 0, 0), (0, 255, 0), (255, 255, 0), (0, 0, 255), (255, 0, 255), (0, 255, 255), (255, 255, 255)]
COLOURS = {"K": (0, 0, 0), "W": (255, 255, 255), "G": (192, 192, 192), "D": (128, 128, 128), "Y": (255, 255, 0), "R": (255, 0, 0)}


def pixel(x, y, size):
    c = ART[y * 16 // size][x * 16 // size]
    return COLOURS.get(c)  # None: transparent


def and_mask(size):
    stride = (size + 31) // 32 * 4
    out = bytearray()
    for y in reversed(range(size)):
        row = bytearray(stride)
        for x in range(size):
            if pixel(x, y, size) is None:
                row[x // 8] |= 0x80 >> (x % 8)
        out += row
    return bytes(out)


def entry(size, bpp):
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, bpp, 0, 0, 0, 0, 16 if bpp == 4 else 0, 0)
    body = bytearray()
    if bpp == 4:
        palette = b"".join(struct.pack("BBBB", b, g, r, 0) for r, g, b in VGA)
        stride = (size * 4 + 31) // 32 * 4
        for y in reversed(range(size)):
            row = bytearray(stride)
            for x in range(size):
                c = pixel(x, y, size)
                i = VGA.index(c) if c else 0
                row[x // 2] |= i << (4 if x % 2 == 0 else 0)
            body += row
        data = header + palette + bytes(body)
    else:
        for y in reversed(range(size)):
            for x in range(size):
                c = pixel(x, y, size)
                body += struct.pack("BBBB", c[2], c[1], c[0], 255) if c else b"\0\0\0\0"
        data = header + bytes(body)
    return data + and_mask(size)


def main():
    root = Path(__file__).resolve().parent.parent
    images = [(s, b) for b in (4, 32) for s in (16, 32, 48)]
    blobs = [entry(s, b) for s, b in images]
    out = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    for (s, b), blob in zip(images, blobs):
        out += struct.pack("<BBBBHHII", s, s, 16 if b == 4 else 0, 0, 1, b, len(blob), offset)
        offset += len(blob)
    out += b"".join(blobs)
    (root / "assets" / "zipshrink.ico").write_bytes(out)
    try:
        from PIL import Image
        img = Image.new("RGBA", (16, 16))
        for y in range(16):
            for x in range(16):
                c = pixel(x, y, 16)
                img.putpixel((x, y), c + (255,) if c else (0, 0, 0, 0))
        img.resize((256, 256), Image.NEAREST).save(root / "assets" / "icon-256.png")
        img.resize((48, 48), Image.NEAREST).save(root / "assets" / "icon-48.png")
    except ImportError:
        pass
    print("wrote", root / "assets" / "zipshrink.ico")


if __name__ == "__main__":
    main()
