#!/usr/bin/env python3
"""Generate the site's raster images from the logo geometry in favicon.svg.

    python3 site/tools/gen-images.py

Writes site/favicon-32.png, site/apple-touch-icon.png (180x180) and
site/og-image.png (1200x630). Standard library only; the outputs are checked
in, so this only needs to run when the mark changes.
"""

import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
SITE = os.path.dirname(HERE)

BLUE_A = (0x2F, 0x7B, 0xF0)
BLUE_B = (0x6A, 0x3D, 0xF0)
WHITE = (0xF5, 0xF7, 0xFC)
AMBER = (0xFF, 0xB5, 0x47)
DARK_BG = (0x10, 0x13, 0x18)
CHUNK_FILL = (0x1E, 0x23, 0x2C)
CHUNK_EDGE = (0x3C, 0x45, 0x55)

# Logo in a 32x32 design space (same numbers as favicon.svg).
DIAMONDS = [((16, 10), WHITE), ((10, 16), WHITE), ((16, 22), WHITE), ((22, 16), AMBER)]
HALF = 4.4
RADIUS = 7


def png(path, width, height, pixels):
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        raw.extend(pixels[y * width * 4:(y + 1) * width * 4])

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    data = b"\x89PNG\r\n\x1a\n"
    data += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    data += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(data)


def logo_sample(u, v, radius=RADIUS):
    """Colour of the logo at design coordinates (u, v) in [0, 32), or None."""
    dx = max(radius - u, u - (32 - radius), 0)
    dy = max(radius - v, v - (32 - radius), 0)
    if dx * dx + dy * dy > radius * radius:
        return None
    for (cx, cy), colour in DIAMONDS:
        if abs(u - cx) + abs(v - cy) <= HALF:
            return colour
    t = (u + v) / 64.0
    return tuple(round(a + (b - a) * t) for a, b in zip(BLUE_A, BLUE_B))


def blend(dst, src, alpha):
    return tuple(round(d + (s - d) * alpha) for d, s in zip(dst, src))


def draw_logo(pixels, width, x0, y0, size, background=None, ss=4, radius=RADIUS):
    scale = 32.0 / size
    for y in range(size):
        for x in range(size):
            acc = [0.0, 0.0, 0.0]
            hits = 0
            for sy in range(ss):
                for sx in range(ss):
                    c = logo_sample((x + (sx + 0.5) / ss) * scale, (y + (sy + 0.5) / ss) * scale, radius)
                    if c is not None:
                        hits += 1
                        for i in range(3):
                            acc[i] += c[i]
            if not hits:
                continue
            colour = tuple(round(a / hits) for a in acc)
            alpha = hits / (ss * ss)
            o = ((y0 + y) * width + (x0 + x)) * 4
            if background is None:
                pixels[o:o + 4] = bytes(colour + (round(alpha * 255),))
            else:
                pixels[o:o + 4] = bytes(blend(background, colour, alpha) + (255,))


def fill_rect(pixels, width, x0, y0, w, h, colour, edge=None):
    for y in range(y0, y0 + h):
        for x in range(x0, x0 + w):
            on_edge = edge is not None and (x in (x0, x0 + w - 1) or y in (y0, y0 + h - 1))
            o = (y * width + x) * 4
            pixels[o:o + 4] = bytes((edge if on_edge else colour) + (255,))


def icon(path, size, radius=RADIUS):
    pixels = bytearray(size * size * 4)
    draw_logo(pixels, size, 0, 0, size, radius=radius)
    png(path, size, size, pixels)


def og_image(path):
    w, h = 1200, 630
    pixels = bytearray(bytes(DARK_BG + (255,)) * (w * h))
    draw_logo(pixels, w, 140, 175, 280, background=DARK_BG, ss=3)
    # Two rows of chunks: an update that only changes two of them.
    rows = [
        (215, [(520, 92), (620, 58), (686, 128), (822, 76), (906, 106), (1020, 84)], set()),
        (345, [(520, 92), (620, 58), (686, 72), (766, 78), (852, 76), (936, 106), (1050, 84)], {2, 3}),
    ]
    for y, cells, new in rows:
        for i, (x, cw) in enumerate(cells):
            if i in new:
                fill_rect(pixels, w, x, y, cw, 70, AMBER)
            else:
                fill_rect(pixels, w, x, y, cw, 70, CHUNK_FILL, CHUNK_EDGE)
    png(path, w, h, pixels)


if __name__ == "__main__":
    icon(os.path.join(SITE, "favicon-32.png"), 32)
    # iOS rounds the corners itself and shows transparency as black.
    icon(os.path.join(SITE, "apple-touch-icon.png"), 180, radius=0)
    og_image(os.path.join(SITE, "og-image.png"))
    print("wrote favicon-32.png, apple-touch-icon.png, og-image.png")
