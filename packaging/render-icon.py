#!/usr/bin/env python3
"""Generate the Yazi2D app icon as a 1024×1024 PNG, using only the stdlib.

The icon is drawn rather than hand-painted so it stays reproducible: a dark
rounded square, two file-list panels, a cyan accent and a small folder glyph.
Run it with `python3 packaging/render-icon.py`.
"""
import struct
import zlib

SIZE = 1024
OUT = "packaging/icon-source.png"

# Palette
BG_TOP = (32, 36, 46)
BG_BOTTOM = (20, 22, 28)
PANEL = (44, 49, 62)
PANEL_EDGE = (72, 80, 98)
ACCENT = (77, 166, 255)
TEXT = (214, 222, 235)
DIM = (120, 130, 148)
FOLDER = (255, 190, 92)


def lerp(a, b, t):
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))


class Canvas:
    def __init__(self, size):
        self.size = size
        self.pixels = [[BG_BOTTOM for _ in range(size)] for _ in range(size)]

    def put(self, x, y, color):
        if 0 <= x < self.size and 0 <= y < self.size:
            self.pixels[y][x] = color

    def rect(self, x0, y0, x1, y1, color, radius=0):
        """Filled rectangle, optionally with rounded corners."""
        for y in range(max(0, y0), min(self.size, y1)):
            for x in range(max(0, x0), min(self.size, x1)):
                if radius:
                    # Distance to the nearest corner circle centre.
                    cx = min(max(x, x0 + radius), x1 - radius - 1)
                    cy = min(max(y, y0 + radius), y1 - radius - 1)
                    dx, dy = x - cx, y - cy
                    if dx * dx + dy * dy > radius * radius:
                        continue
                self.pixels[y][x] = color

    def vgradient(self, x0, y0, x1, y1, top, bottom, radius=0):
        for y in range(max(0, y0), min(self.size, y1)):
            t = (y - y0) / max(1, (y1 - y0 - 1))
            color = lerp(top, bottom, t)
            for x in range(max(0, x0), min(self.size, x1)):
                if radius:
                    cx = min(max(x, x0 + radius), x1 - radius - 1)
                    cy = min(max(y, y0 + radius), y1 - radius - 1)
                    dx, dy = x - cx, y - cy
                    if dx * dx + dy * dy > radius * radius:
                        continue
                self.pixels[y][x] = color

    def png(self):
        raw = bytearray()
        for row in self.pixels:
            raw.append(0)  # filter type: none
            for r, g, b in row:
                raw.extend((r, g, b))
        compressed = zlib.compress(bytes(raw), 9)

        def chunk(tag, data):
            out = struct.pack(">I", len(data)) + tag + data
            return out + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

        header = struct.pack(">IIBBBBB", self.size, self.size, 8, 2, 0, 0, 0)
        return (b"\x89PNG\r\n\x1a\n"
                + chunk(b"IHDR", header)
                + chunk(b"IDAT", compressed)
                + chunk(b"IEND", b""))


def draw():
    c = Canvas(SIZE)

    # Background: rounded square with a subtle vertical gradient.
    c.vgradient(64, 64, SIZE - 64, SIZE - 64, BG_TOP, BG_BOTTOM, radius=176)

    # --- Two panels ---------------------------------------------------------
    margin = 152
    gap = 40
    panel_w = (SIZE - 2 * margin - gap) // 2
    top, bottom = 264, 760

    for index in range(2):
        x0 = margin + index * (panel_w + gap)
        x1 = x0 + panel_w
        # Panel body with rounded corners.
        c.rect(x0, top, x1, bottom, PANEL, radius=28)
        # Header strip, the panel title bar.
        c.rect(x0, top, x1, top + 56, ACCENT if index == 0 else PANEL_EDGE, radius=28)
        c.rect(x0, top + 28, x1, top + 56, ACCENT if index == 0 else PANEL_EDGE)

        # File rows: a folder glyph then thin lines of decreasing length.
        row_y = top + 104
        for row in range(6):
            y = row_y + row * 62
            if row == 0:
                # Folder glyph in the first row of each list.
                fx, fy = x0 + 34, y - 4
                c.rect(fx, fy + 8, fx + 62, fy + 44, FOLDER, radius=6)
                c.rect(fx, fy, fx + 30, fy + 14, FOLDER, radius=5)
            else:
                c.rect(x0 + 34, y, x0 + 34 + 20, y + 14, DIM, radius=4)

            # Name line, shrinking a little each row.
            length = int((panel_w - 140) * (1.0 - row * 0.07))
            c.rect(x0 + 104, y + 2, x0 + 104 + length, y + 14, TEXT, radius=6)

    # --- Accent bar under the panels ---------------------------------------
    bar_w = 220
    c.rect((SIZE - bar_w) // 2, 812, (SIZE + bar_w) // 2, 828, ACCENT, radius=8)

    return c


def main():
    canvas = draw()
    with open(OUT, "wb") as handle:
        handle.write(canvas.png())
    print(f"wrote {OUT} ({SIZE}×{SIZE})")


if __name__ == "__main__":
    main()
