"""Pixel-exact stand-in for the GfxRenderer calls the chess screens use.

Draws into a 1-bit canvas with the firmware's own built-in font bitmaps
(lib/EpdFont/builtinFonts) and dither patterns (GfxRenderer::drawPixelDither),
so a mockup PNG shows what the panel can show. Kerning is not applied, so text
can be a pixel or two wider than on the device. Stdlib only.
"""

from __future__ import annotations

import math
import re
import sys
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import gen_chess_pieces as pieces  # noqa: E402

FONT_DIR = ROOT / "lib" / "EpdFont" / "builtinFonts"


class Font:
    """Parsed built-in EpdFont header."""

    def __init__(self, name: str):
        text = (FONT_DIR / f"{name}.h").read_text(encoding="utf-8")
        body = re.search(r"Bitmaps\[\d+\] = \{(.*?)\};", text, re.S).group(1)
        self.bitmap = bytes(int(v, 16) for v in re.findall(r"0x[0-9A-Fa-f]+", body))
        glyphs = re.search(r"EpdGlyph \w+Glyphs\[\] = \{(.*?)\n\};", text, re.S).group(1)
        self.glyphs = [tuple(int(v) for v in g.split(",")) for g in re.findall(r"\{\s*([-\d,\s]+?)\s*\}", glyphs)]
        intervals = re.search(r"EpdUnicodeInterval \w+Intervals\[\] = \{(.*?)\n\};", text, re.S).group(1)
        self.intervals = [tuple(int(v, 16) for v in m) for m in re.findall(r"\{ (0x\w+), (0x\w+), (0x\w+) \}", intervals)]
        data = re.search(rf"EpdFontData {name} = \{{(.*?)\}};", text, re.S).group(1)
        fields = [f.strip() for f in data.split(",")]
        self.advance_y = int(fields[4])
        self.ascender = int(fields[5])
        self.two_bit = fields[7] == "true"
        self.group_of: dict[int, bytes] = {}
        self.aligned_offset: dict[int, int] = {}
        groups = re.search(r"EpdFontGroup \w+Groups\[\] = \{(.*?)\n\};", text, re.S)
        if groups:
            # Compressed fonts: each group inflates to byte-aligned rows in glyph order.
            for g in re.findall(r"\{\s*([\d,\s]+?)\s*\}", groups.group(1)):
                offset, size, _raw, count, first = (int(v) for v in g.split(","))
                block = zlib.decompress(self.bitmap[offset : offset + size], -15)
                acc = 0
                for i in range(first, first + count):
                    self.group_of[i] = block
                    self.aligned_offset[i] = acc
                    acc += (self.glyphs[i][0] + 3) // 4 * self.glyphs[i][1]

    def glyph(self, cp: int):
        for first, last, offset in self.intervals:
            if first <= cp <= last:
                index = offset + cp - first
                return index, self.glyphs[index]
        return self.glyph(ord("?")) if cp != ord("?") else (None, None)

    def width(self, s: str) -> int:
        pen = 0
        last = 0
        for ch in s:
            _, g = self.glyph(ord(ch))
            if g is None:
                continue
            last = (pen + 8) // 16 + g[3] + g[0]
            pen += g[2]
        return max((pen + 8) // 16, last)


FONTS: dict[str, Font] = {}


def font(name: str) -> Font:
    aliases = {
        "small": "ubuntu_8_regular",
        "ui10": "ubuntu_10_regular",
        "ui10b": "ubuntu_10_bold",
        "ui12": "ubuntu_12_regular",
        "ui12b": "ubuntu_12_bold",
        "big": "notosans_18_bold",
        "mid": "notosans_14_bold",
    }
    key = aliases.get(name, name)
    if key not in FONTS:
        FONTS[key] = Font(key)
    return FONTS[key]


class Canvas:
    """1-bit canvas: 1 = black ink, 0 = white paper."""

    def __init__(self, width: int = 528, height: int = 792):
        self.w = width
        self.h = height
        self.px = bytearray(width * height)

    # --- pixels and fills ---------------------------------------------------
    def set(self, x: int, y: int, black: bool = True) -> None:
        if 0 <= x < self.w and 0 <= y < self.h:
            self.px[y * self.w + x] = 1 if black else 0

    def get(self, x: int, y: int) -> int:
        return self.px[y * self.w + x] if 0 <= x < self.w and 0 <= y < self.h else 0

    def fill(self, x: int, y: int, w: int, h: int, color: str = "black") -> None:
        """color: black, white, light (25% dots), dark (50% checker), hatch."""
        for yy in range(max(0, y), min(self.h, y + h)):
            for xx in range(max(0, x), min(self.w, x + w)):
                self.set(xx, yy, ink(color, xx, yy))

    def rect(self, x: int, y: int, w: int, h: int, lw: int = 1, black: bool = True) -> None:
        self.fill(x, y, w, lw, "black" if black else "white")
        self.fill(x, y + h - lw, w, lw, "black" if black else "white")
        self.fill(x, y, lw, h, "black" if black else "white")
        self.fill(x + w - lw, y, lw, h, "black" if black else "white")

    def _inside_round(self, x: int, y: int, rx: int, ry: int, w: int, h: int, r: int) -> bool:
        cx = min(max(x, rx + r), rx + w - 1 - r)
        cy = min(max(y, ry + r), ry + h - 1 - r)
        return (x - cx) ** 2 + (y - cy) ** 2 <= r * r + r * 0.8

    def round_fill(self, x: int, y: int, w: int, h: int, r: int, color: str = "black") -> None:
        for yy in range(y, y + h):
            for xx in range(x, x + w):
                if self._inside_round(xx, yy, x, y, w, h, r):
                    self.set(xx, yy, ink(color, xx, yy))

    def round_rect(self, x: int, y: int, w: int, h: int, r: int, lw: int = 1, black: bool = True) -> None:
        for yy in range(y, y + h):
            for xx in range(x, x + w):
                if self._inside_round(xx, yy, x, y, w, h, r) and not self._inside_round(
                    xx, yy, x + lw, y + lw, w - 2 * lw, h - 2 * lw, max(0, r - lw)
                ):
                    self.set(xx, yy, black)

    def disc(self, cx: float, cy: float, r: float, color: str = "black") -> None:
        for yy in range(int(cy - r - 1), int(cy + r + 2)):
            for xx in range(int(cx - r - 1), int(cx + r + 2)):
                if (xx + 0.5 - cx) ** 2 + (yy + 0.5 - cy) ** 2 <= r * r:
                    self.set(xx, yy, ink(color, xx, yy))

    def line(self, x0: float, y0: float, x1: float, y1: float, lw: float = 1, black: bool = True) -> None:
        steps = int(max(abs(x1 - x0), abs(y1 - y0)) * 2) + 1
        for i in range(steps + 1):
            t = i / steps
            self.disc(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, lw / 2, "black" if black else "white")

    def polygon(self, pts: list[tuple[float, float]], color: str = "black") -> None:
        ys = [p[1] for p in pts]
        for yy in range(int(min(ys)), int(max(ys)) + 1):
            yc = yy + 0.5
            xs = []
            for (xa, ya), (xb, yb) in zip(pts, pts[1:] + pts[:1]):
                if (ya <= yc < yb) or (yb <= yc < ya):
                    xs.append(xa + (yc - ya) * (xb - xa) / (yb - ya))
            xs.sort()
            for a, b in zip(xs[0::2], xs[1::2]):
                for xx in range(math.ceil(a - 0.5), math.ceil(b - 0.5)):
                    self.set(xx, yy, ink(color, xx, yy))

    # --- text ---------------------------------------------------------------
    def text(self, fname: str, x: int, y: int, s: str, black: bool = True) -> int:
        """GfxRenderer::drawText: y is the top of the line box. Returns width."""
        f = font(fname)
        pen = x * 16
        base = y + f.ascender
        for ch in s:
            index, g = f.glyph(ord(ch))
            if g is None:
                continue
            gw, gh, adv, left, top, _length, offset = g
            data = f.group_of.get(index, f.bitmap)
            aligned = index in f.group_of  # compressed groups pad rows to a byte
            stride = (gw + 3) // 4
            if aligned:
                offset = f.aligned_offset[index]
            gx = (pen + 8) // 16 + left
            gy = base - top
            for j in range(gh):
                for i in range(gw):
                    bit = j * gw + i
                    if aligned:
                        v = (data[offset + j * stride + (i >> 2)] >> (6 - (i & 3) * 2)) & 3
                    elif f.two_bit:
                        v = (data[offset + (bit >> 2)] >> (6 - (bit & 3) * 2)) & 3
                    else:
                        v = (data[offset + (bit >> 3)] >> (7 - (bit & 7))) & 1
                    if v:  # BW pass: any ink is black
                        self.set(gx + i, gy + j, black)
            pen += adv
        return f.width(s)

    def text_rotated(self, fname: str, x: int, cy: int, s: str) -> None:
        """Text turned 90 degrees clockwise, centred on cy, left edge at x."""
        f = font(fname)
        w, h = f.width(s) + 2, f.advance_y
        tmp = Canvas(w, h)
        tmp.text(fname, 1, 0, s)
        top = cy - w // 2
        for yy in range(h):
            for xx in range(w):
                if tmp.px[yy * w + xx]:
                    self.set(x + h - 1 - yy, top + xx)

    def text_halo(self, fname: str, x: int, y: int, s: str) -> None:
        """Black text with a 1 px white ring, readable on any texture."""
        for dx in (-1, 0, 1):
            for dy in (-1, 0, 1):
                if dx or dy:
                    self.text(fname, x + dx, y + dy, s, black=False)
        self.text(fname, x, y, s)

    def text_center(self, fname: str, cx: int, y: int, s: str, black: bool = True) -> None:
        self.text(fname, cx - font(fname).width(s) // 2, y, s, black)

    def text_right(self, fname: str, right: int, y: int, s: str, black: bool = True) -> None:
        self.text(fname, right - font(fname).width(s), y, s, black)

    # --- pieces -------------------------------------------------------------
    def piece(self, glyph: pieces.PieceBitmap, x: int, y: int, black_piece: bool, rotate: bool = False) -> None:
        """Halo white, then ink black: what ChessActivity does per piece.

        rotate draws the bitmap turned 90 degrees clockwise (a toppled king).
        """
        ink_plane = glyph.black if black_piece else glyph.white
        for j in range(glyph.height):
            for i in range(glyph.width):
                k = j * glyph.width + i
                if rotate:
                    px, py = x + glyph.size - 1 - (glyph.top + j), y + glyph.left + i
                else:
                    px, py = x + glyph.left + i, y + glyph.top + j
                if glyph.halo[k]:
                    self.set(px, py, False)
                if ink_plane[k]:
                    self.set(px, py, True)

    # --- output -------------------------------------------------------------
    def save(self, path: Path, scale: int = 1) -> None:
        w, h = self.w * scale, self.h * scale
        out = bytearray(w * h)
        for y in range(h):
            row = (y // scale) * self.w
            for x in range(w):
                out[y * w + x] = 0 if self.px[row + x // scale] else 255
        pieces.write_png(path, w, h, out)


def ink(color: str, x: int, y: int) -> bool:
    """Pixel of a fill pattern, matching GfxRenderer::drawPixelDither."""
    if color == "black":
        return True
    if color == "white":
        return False
    if color == "light":
        return x % 2 == 0 and y % 2 == 0
    if color == "dark":
        return (x + y) % 2 == 0
    if color == "hatch":
        return pieces.hatch_dark(x, y)
    raise ValueError(color)
