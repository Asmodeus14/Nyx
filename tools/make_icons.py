#!/usr/bin/env python3
"""Generate the desktop's app icons as 48x48 RGBA PNGs.

No PIL — just stdlib zlib/struct — so this runs anywhere Python does. Every icon is a rounded
square in the app's accent colour with a flat white glyph on top, which keeps the set visually
coherent and legible at the 24px the taskbar draws them at.

Run from the repo root:  python3 tools/make_icons.py
Output:                  assets/icons/*.png   (bundled to /mnt/nvme/icons by Build.sh)

The PNGs are committed, so a normal build does NOT need Python. Re-run this only to change the art.
"""

import math
import os
import struct
import zlib

S = 48                      # icon edge, in pixels
OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "assets", "icons")

WHITE = (255, 255, 255, 255)


# ─────────────────────────── canvas helpers ───────────────────────────

class Img:
    def __init__(self, size=S):
        self.s = size
        self.px = [[(0, 0, 0, 0)] * size for _ in range(size)]

    def put(self, x, y, c, a=1.0):
        """Blend colour `c` over the pixel at x,y with extra coverage `a` (for antialiasing)."""
        if not (0 <= x < self.s and 0 <= y < self.s):
            return
        sa = (c[3] / 255.0) * a
        if sa <= 0:
            return
        dr, dg, db, da = self.px[y][x]
        da /= 255.0
        oa = sa + da * (1 - sa)
        if oa <= 0:
            return
        r = (c[0] * sa + dr * da * (1 - sa)) / oa
        g = (c[1] * sa + dg * da * (1 - sa)) / oa
        b = (c[2] * sa + db * da * (1 - sa)) / oa
        self.px[y][x] = (int(r + 0.5), int(g + 0.5), int(b + 0.5), int(oa * 255 + 0.5))

    def rect(self, x0, y0, w, h, c):
        for y in range(int(y0), int(y0 + h)):
            for x in range(int(x0), int(x0 + w)):
                self.put(x, y, c)

    def rrect(self, x0, y0, w, h, r, c):
        """Rounded rectangle, antialiased on the corner arcs."""
        x1, y1 = x0 + w, y0 + h
        for y in range(self.s):
            for x in range(self.s):
                # Distance outside the rounded box, in pixels; <0 inside.
                dx = max(x0 + r - (x + 0.5), (x + 0.5) - (x1 - r), 0)
                dy = max(y0 + r - (y + 0.5), (y + 0.5) - (y1 - r), 0)
                if x + 0.5 < x0 or x + 0.5 > x1 or y + 0.5 < y0 or y + 0.5 > y1:
                    continue
                d = math.hypot(dx, dy) - r
                if d <= -0.5:
                    self.put(x, y, c)
                elif d < 0.5:
                    self.put(x, y, c, 0.5 - d)

    def disc(self, cx, cy, r, c):
        for y in range(self.s):
            for x in range(self.s):
                d = math.hypot(x + 0.5 - cx, y + 0.5 - cy) - r
                if d <= -0.5:
                    self.put(x, y, c)
                elif d < 0.5:
                    self.put(x, y, c, 0.5 - d)

    def ring(self, cx, cy, r, thick, c, a0=0.0, a1=math.tau):
        """Arc of a circle, `thick` px wide, between angles a0 and a1 (radians, 0 = east, CW)."""
        for y in range(self.s):
            for x in range(self.s):
                px, py = x + 0.5 - cx, y + 0.5 - cy
                d = abs(math.hypot(px, py) - r) - thick / 2
                if d >= 0.5:
                    continue
                ang = math.atan2(py, px) % math.tau
                if not (a0 <= ang <= a1):
                    continue
                self.put(x, y, c, 1.0 if d <= -0.5 else 0.5 - d)

    def line(self, x0, y0, x1, y1, thick, c):
        """Thick antialiased segment, via distance-to-segment."""
        vx, vy = x1 - x0, y1 - y0
        ll = vx * vx + vy * vy
        for y in range(self.s):
            for x in range(self.s):
                px, py = x + 0.5 - x0, y + 0.5 - y0
                t = 0.0 if ll == 0 else max(0.0, min(1.0, (px * vx + py * vy) / ll))
                d = math.hypot(px - vx * t, py - vy * t) - thick / 2
                if d <= -0.5:
                    self.put(x, y, c)
                elif d < 0.5:
                    self.put(x, y, c, 0.5 - d)

    def tri(self, pts, c):
        def side(a, b, p):
            return (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
        for y in range(self.s):
            for x in range(self.s):
                p = (x + 0.5, y + 0.5)
                s0 = side(pts[0], pts[1], p)
                s1 = side(pts[1], pts[2], p)
                s2 = side(pts[2], pts[0], p)
                if (s0 >= 0 and s1 >= 0 and s2 >= 0) or (s0 <= 0 and s1 <= 0 and s2 <= 0):
                    self.put(x, y, c)

    def write(self, path):
        raw = b"".join(
            b"\x00" + b"".join(struct.pack("4B", *self.px[y][x]) for x in range(self.s))
            for y in range(self.s)
        )
        def chunk(tag, data):
            return (struct.pack(">I", len(data)) + tag + data
                    + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))
        png = (b"\x89PNG\r\n\x1a\n"
               + chunk(b"IHDR", struct.pack(">IIBBBBB", self.s, self.s, 8, 6, 0, 0, 0))
               + chunk(b"IDAT", zlib.compress(raw, 9))
               + chunk(b"IEND", b""))
        with open(path, "wb") as f:
            f.write(png)


def tile(bg):
    """A rounded-square icon plate in `bg`, the shared shape for the whole set."""
    im = Img()
    im.rrect(3, 3, S - 6, S - 6, 10, bg)
    return im


# ─────────────────────────── the icons ───────────────────────────

def terminal():
    im = tile((45, 45, 48, 255))
    im.line(14, 18, 22, 24, 3.2, WHITE)
    im.line(22, 24, 14, 30, 3.2, WHITE)
    im.line(26, 32, 35, 32, 3.2, WHITE)
    return im


def settings():
    im = tile((108, 117, 125, 255))
    # Cog: a ring plus eight teeth.
    im.ring(24, 24, 8.5, 4.0, WHITE)
    for i in range(8):
        a = i * math.tau / 8
        im.line(24 + 10 * math.cos(a), 24 + 10 * math.sin(a),
                24 + 14 * math.cos(a), 24 + 14 * math.sin(a), 3.4, WHITE)
    im.disc(24, 24, 3.2, (108, 117, 125, 255))
    return im


def explorer():
    im = tile((230, 126, 34, 255))
    im.rrect(11, 17, 26, 17, 2.5, WHITE)          # folder body
    im.rrect(11, 13, 12, 6, 2.0, WHITE)           # tab
    return im


def network():
    im = tile((41, 128, 185, 255))
    im.ring(24, 24, 11, 2.6, WHITE)               # globe
    im.line(13, 24, 35, 24, 2.2, WHITE)
    im.ring(24, 24, 11, 2.2, WHITE)
    for r in (4.5,):                              # meridian, as a narrow ellipse pair
        for y in range(13, 36):
            t = (y - 24) / 11.0
            if abs(t) > 1:
                continue
            dx = r * math.sqrt(max(0.0, 1 - t * t)) * 1.0
            im.put(int(24 + dx), y, WHITE)
            im.put(int(24 - dx), y, WHITE)
    return im


def sysmon():
    im = tile((39, 174, 96, 255))
    for i, h in enumerate((8, 14, 20, 11)):       # bar chart
        im.rrect(12 + i * 7, 34 - h, 5, h, 1.5, WHITE)
    return im


def glcube():
    im = tile((142, 68, 173, 255))
    # Isometric cube: three visible faces as an outline.
    top = [(24, 11), (35, 17), (24, 23), (13, 17)]
    for i in range(4):
        im.line(top[i][0], top[i][1], top[(i + 1) % 4][0], top[(i + 1) % 4][1], 2.4, WHITE)
    im.line(13, 17, 13, 30, 2.4, WHITE)
    im.line(35, 17, 35, 30, 2.4, WHITE)
    im.line(24, 23, 24, 37, 2.4, WHITE)
    im.line(13, 30, 24, 37, 2.4, WHITE)
    im.line(35, 30, 24, 37, 2.4, WHITE)
    return im


def imageviewer():
    im = tile((26, 188, 156, 255))
    im.rrect(11, 14, 26, 20, 2.5, WHITE)
    im.disc(18, 20, 2.6, (26, 188, 156, 255))     # sun
    im.tri([(14, 32), (23, 22), (32, 32)], (26, 188, 156, 255))   # hills
    im.tri([(24, 32), (30, 25), (35, 32)], (26, 188, 156, 255))
    return im


def stdhello():
    im = tile((52, 152, 219, 255))
    im.rrect(11, 13, 26, 22, 3.0, WHITE)          # speech bubble
    im.tri([(17, 34), (17, 41), (25, 34)], WHITE)
    im.disc(18, 24, 1.9, (52, 152, 219, 255))
    im.disc(24, 24, 1.9, (52, 152, 219, 255))
    im.disc(30, 24, 1.9, (52, 152, 219, 255))
    return im


def qcstudio():
    im = tile((155, 89, 182, 255))
    im.disc(24, 24, 3.0, WHITE)                   # nucleus + two orbits
    for rot in (0.6, -0.6):
        for i in range(140):
            a = i * math.tau / 140
            x = 13.5 * math.cos(a)
            y = 6.0 * math.sin(a)
            im.put(int(24 + x * math.cos(rot) - y * math.sin(rot) + 0.5),
                   int(24 + x * math.sin(rot) + y * math.cos(rot) + 0.5), WHITE)
    return im


def stdgui():
    im = tile((243, 156, 18, 255))
    im.rrect(11, 13, 26, 22, 2.5, WHITE)
    im.rect(11, 13, 26, 6, (243, 156, 18, 255))
    im.rect(13, 15, 26 - 4, 2, WHITE)
    return im


def notepad():
    im = tile((236, 240, 241, 255))
    im.rrect(13, 11, 22, 26, 2.5, (52, 73, 94, 255))
    for i in range(4):
        im.rect(17, 17 + i * 5, 14 if i < 3 else 8, 2, (236, 240, 241, 255))
    return im


def wifi():
    im = tile((22, 160, 133, 255))
    # Three arcs plus a dot, drawn upward (angles are CW from east, so pi..tau is the top half).
    for r in (6.5, 11.0, 15.5):
        im.ring(24, 31, r, 2.8, WHITE, math.pi + 0.55, math.tau - 0.55)
    im.disc(24, 31, 2.6, WHITE)
    return im


def browser():
    """A globe with a latitude band and two meridians — the universal 'web' mark."""
    im = tile((52, 120, 246, 255))
    im.ring(24, 24, 13, 2.8, WHITE)                 # globe outline
    im.line(11, 24, 37, 24, 2.4, WHITE)             # equator
    # Meridians, as nested rings squeezed horizontally by drawing narrower arcs.
    im.ring(24, 24, 13, 2.2, WHITE, math.pi * 0.5 - 0.9, math.pi * 0.5 + 0.9)
    im.ring(24, 24, 13, 2.2, WHITE, math.pi * 1.5 - 0.9, math.pi * 1.5 + 0.9)
    im.ring(24, 24, 6.5, 2.2, WHITE)                # inner meridian ellipse, approximated
    return im


def posixprobe():
    """A checklist: three rows, the first two ticked, the third crossed — pass/pass/fail."""
    im = tile((99, 110, 114, 255))
    for i, ok in enumerate((True, True, False)):
        y = 14 + i * 10
        im.rect(23, y + 3, 13, 2, WHITE)            # the item's line of text
        if ok:
            im.line(12, y + 4, 15, y + 7, 2.2, (46, 204, 113, 255))
            im.line(15, y + 7, 20, y + 1, 2.2, (46, 204, 113, 255))
        else:
            im.line(13, y + 1, 19, y + 7, 2.2, (231, 76, 60, 255))
            im.line(19, y + 1, 13, y + 7, 2.2, (231, 76, 60, 255))
    return im


def nyx():
    """The shell's own mark: an orange plate with an angular N."""
    im = tile((255, 87, 34, 255))
    im.line(16, 34, 16, 14, 3.4, WHITE)
    im.line(16, 14, 32, 34, 3.4, WHITE)
    im.line(32, 34, 32, 14, 3.4, WHITE)
    return im


ICONS = {
    "terminal": terminal,
    "settings": settings,
    "explorer": explorer,
    "network": network,
    "sysmon": sysmon,
    "glcube": glcube,
    "imageviewer": imageviewer,
    "stdhello": stdhello,
    "qcstudio": qcstudio,
    "stdgui": stdgui,
    "notepad": notepad,
    "wifi": wifi,
    "browser": browser,
    "posixprobe": posixprobe,
    "nyx": nyx,
}


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, fn in ICONS.items():
        path = os.path.join(OUT, name + ".png")
        fn().write(path)
        print("wrote", path, os.path.getsize(path), "bytes")


if __name__ == "__main__":
    main()
