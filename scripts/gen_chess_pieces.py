#!/usr/bin/env python3
"""Generate the CrossChess piece bitmaps from design/pieces/*.svg.

Writes src/activities/chess/ChessPieceBitmaps.h: 1-bit bitmaps in the packed
glyph format GfxRenderer::drawGlyphBitmap() reads (MSB first, rows packed with
no padding). Every piece has three planes of the same size:

  halo  - painted white first: the silhouette plus a ring around it, so the
          piece stays readable on hatched or dithered squares;
  white - painted black over the halo for a white piece (outline + details);
  black - painted black over the halo for a black piece (body minus details).

Only the Python standard library is needed. The SVG reader supports the small
subset the piece sources use: <rect> (with rx), <circle>, <ellipse>, <line>,
<polyline>, <polygon> and <path> with M L H V C S Q T Z (absolute or
relative). Arcs and transforms are rejected so a source edit cannot silently
render wrong. The layer groups are described in design/pieces/pawn.svg.

Usage:
  python scripts/gen_chess_pieces.py            # write the header + previews
  python scripts/gen_chess_pieces.py --gray     # also write 2-bit planes
  python scripts/gen_chess_pieces.py --check    # fail if the header is stale
"""

from __future__ import annotations

import argparse
import math
import re
import shutil
import struct
import subprocess
import sys
import xml.etree.ElementTree as ET
import zlib
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PIECE_DIR = ROOT / "design" / "pieces"
PREVIEW_DIR = PIECE_DIR / "preview"
HEADER = ROOT / "src" / "activities" / "chess" / "ChessPieceBitmaps.h"
GRAY_HEADER = ROOT / "src" / "activities" / "chess" / "ChessPieceBitmapsGray.h"

# Order matches the CC_ piece kinds in chess/include/crosschess.h (kind - 1).
PIECES = ["pawn", "knight", "bishop", "rook", "queen", "king"]
SOURCE_UNITS = 64.0  # viewBox size of every source SVG


@dataclass(frozen=True)
class SizeSpec:
    """How one bitmap size is rendered."""

    size: int  # square size in pixels
    outline: float  # white piece outline radius (px)
    rim: float  # black rim kept around white details on black pieces (px)
    halo: float  # white ring around the silhouette (px)
    detail_min: float  # thinnest detail stroke after scaling (px)
    supersample: int


SIZES = [
    SizeSpec(size=64, outline=2.0, rim=2.0, halo=2.0, detail_min=2.0, supersample=4),  # X3 board
    SizeSpec(size=56, outline=2.0, rim=2.0, halo=2.0, detail_min=2.0, supersample=4),  # X4 board
    SizeSpec(size=24, outline=1.0, rim=1.0, halo=1.0, detail_min=1.0, supersample=8),  # figurines, mini board
]
SMALL_SIZE = 40  # below this, only details marked data-small="keep" are drawn


# --------------------------------------------------------------------------
# SVG subset -> polygons (in source units)
# --------------------------------------------------------------------------

Point = tuple[float, float]
CURVE_STEPS = 16
CIRCLE_STEPS = 48


@dataclass
class Shape:
    """Closed subpaths filled together with the nonzero rule."""

    subpaths: list[list[Point]]


@dataclass
class Layers:
    body: list[Shape]
    cut: list[Shape]
    detail: list[Shape]  # both colors
    detail_white: list[Shape]  # white piece only


def _ellipse(cx: float, cy: float, rx: float, ry: float, steps: int = CIRCLE_STEPS) -> list[Point]:
    return [(cx + rx * math.cos(2 * math.pi * i / steps), cy + ry * math.sin(2 * math.pi * i / steps)) for i in range(steps)]


def _rounded_rect(x: float, y: float, w: float, h: float, r: float) -> list[Point]:
    r = max(0.0, min(r, w / 2, h / 2))
    if r == 0:
        return [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
    pts: list[Point] = []
    corners = [(x + w - r, y + r, -90), (x + w - r, y + h - r, 0), (x + r, y + h - r, 90), (x + r, y + r, 180)]
    for cx, cy, start in corners:
        for i in range(9):
            a = math.radians(start + 90 * i / 8)
            pts.append((cx + r * math.cos(a), cy + r * math.sin(a)))
    return pts


def _cubic(p0: Point, p1: Point, p2: Point, p3: Point) -> list[Point]:
    out = []
    for i in range(1, CURVE_STEPS + 1):
        t = i / CURVE_STEPS
        u = 1 - t
        out.append(
            (
                u * u * u * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t * t * t * p3[0],
                u * u * u * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t * t * t * p3[1],
            )
        )
    return out


def _quad(p0: Point, p1: Point, p2: Point) -> list[Point]:
    out = []
    for i in range(1, CURVE_STEPS + 1):
        t = i / CURVE_STEPS
        u = 1 - t
        out.append((u * u * p0[0] + 2 * u * t * p1[0] + t * t * p2[0], u * u * p0[1] + 2 * u * t * p1[1] + t * t * p2[1]))
    return out


_TOKEN = re.compile(r"[A-Za-z]|[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?")


def parse_path(d: str) -> list[tuple[list[Point], bool]]:
    """Flatten path data into (points, closed) subpaths."""
    tokens = _TOKEN.findall(d)
    subpaths: list[tuple[list[Point], bool]] = []
    current: list[Point] = []
    pos: Point = (0.0, 0.0)
    start: Point = (0.0, 0.0)
    last_ctrl: Point | None = None
    last_cmd = ""
    i = 0
    cmd = ""

    def num() -> float:
        nonlocal i
        value = float(tokens[i])
        i += 1
        return value

    def finish(closed: bool) -> None:
        nonlocal current
        if len(current) > 1:
            subpaths.append((current, closed))
        current = []

    while i < len(tokens):
        if tokens[i].isalpha():
            cmd = tokens[i]
            i += 1
        elif cmd in ("", "Z", "z"):
            raise ValueError(f"number without command in path: {d!r}")
        rel = cmd.islower()
        c = cmd.upper()
        ox, oy = pos if rel else (0.0, 0.0)
        if c == "M":
            finish(False)
            pos = (ox + num(), oy + num())
            start = pos
            current = [pos]
            cmd = "l" if rel else "L"  # extra pairs are implicit lineto
        elif c == "L":
            pos = (ox + num(), oy + num())
            current.append(pos)
        elif c == "H":
            pos = (ox + num(), pos[1])
            current.append(pos)
        elif c == "V":
            pos = (pos[0], oy + num())
            current.append(pos)
        elif c == "C":
            p1 = (ox + num(), oy + num())
            p2 = (ox + num(), oy + num())
            p3 = (ox + num(), oy + num())
            current.extend(_cubic(pos, p1, p2, p3))
            last_ctrl, pos = p2, p3
        elif c == "S":
            p1 = (2 * pos[0] - last_ctrl[0], 2 * pos[1] - last_ctrl[1]) if last_cmd in "CS" and last_ctrl else pos
            p2 = (ox + num(), oy + num())
            p3 = (ox + num(), oy + num())
            current.extend(_cubic(pos, p1, p2, p3))
            last_ctrl, pos = p2, p3
        elif c == "Q":
            p1 = (ox + num(), oy + num())
            p2 = (ox + num(), oy + num())
            current.extend(_quad(pos, p1, p2))
            last_ctrl, pos = p1, p2
        elif c == "T":
            p1 = (2 * pos[0] - last_ctrl[0], 2 * pos[1] - last_ctrl[1]) if last_cmd in "QT" and last_ctrl else pos
            p2 = (ox + num(), oy + num())
            current.extend(_quad(pos, p1, p2))
            last_ctrl, pos = p1, p2
        elif c == "Z":
            finish(True)
            pos = start
            current = [pos]
        else:
            raise ValueError(f"unsupported path command {cmd!r} (arcs are not supported)")
        last_cmd = c
    finish(False)
    return subpaths


def _attr(el: ET.Element, inherited: dict[str, str], name: str, default: str = "") -> str:
    return el.get(name, inherited.get(name, default))


def _stroke_shapes(points: list[Point], closed: bool, width: float) -> list[Shape]:
    """A round-capped, round-joined stroke as a union of quads and discs."""
    half = width / 2
    pts = points + [points[0]] if closed else points
    shapes = [Shape([_ellipse(x, y, half, half, 24)]) for x, y in pts]
    for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
        length = math.hypot(x1 - x0, y1 - y0)
        if length == 0:
            continue
        nx, ny = -(y1 - y0) / length * half, (x1 - x0) / length * half
        shapes.append(Shape([[(x0 + nx, y0 + ny), (x1 + nx, y1 + ny), (x1 - nx, y1 - ny), (x0 - nx, y0 - ny)]]))
    return shapes


def _element_shapes(el: ET.Element, inherited: dict[str, str], min_stroke: float) -> list[Shape]:
    tag = el.tag.split("}")[-1]
    if el.get("transform"):
        raise ValueError("transform attributes are not supported")
    f = lambda name: float(el.get(name, "0"))  # noqa: E731
    if tag == "rect":
        outlines = [(_rounded_rect(f("x"), f("y"), f("width"), f("height"), f("rx") or f("ry")), True)]
    elif tag == "circle":
        outlines = [(_ellipse(f("cx"), f("cy"), f("r"), f("r")), True)]
    elif tag == "ellipse":
        outlines = [(_ellipse(f("cx"), f("cy"), f("rx"), f("ry")), True)]
    elif tag == "line":
        outlines = [([(f("x1"), f("y1")), (f("x2"), f("y2"))], False)]
    elif tag in ("polyline", "polygon"):
        nums = [float(v) for v in _TOKEN.findall(el.get("points", ""))]
        outlines = [(list(zip(nums[0::2], nums[1::2])), tag == "polygon")]
    elif tag == "path":
        outlines = parse_path(el.get("d", ""))
    else:
        raise ValueError(f"unsupported SVG element <{tag}>")

    shapes: list[Shape] = []
    fill = _attr(el, inherited, "fill", "#000")
    if fill != "none":
        filled = [pts for pts, _ in outlines if len(pts) > 2]
        if filled:
            shapes.append(Shape(filled))
    stroke = _attr(el, inherited, "stroke", "none")
    if stroke != "none":
        width = max(float(_attr(el, inherited, "stroke-width", "1")), min_stroke)
        for pts, closed in outlines:
            shapes.extend(_stroke_shapes(pts, closed, width))
    return shapes


def load_layers(path: Path, min_stroke: float = 0.0, small: bool = False) -> Layers:
    """Read the layer groups of one piece SVG.

    min_stroke is in source units: detail strokes are widened to it so they
    survive downscaling. small drops details not marked data-small="keep".
    """
    root = ET.parse(path).getroot()
    if root.get("viewBox", "").split() != ["0", "0", "64", "64"]:
        raise ValueError(f"{path.name}: viewBox must be '0 0 64 64'")
    layers = Layers([], [], [], [])
    for group in root:
        gid = group.get("id")
        if gid not in ("body", "cut", "detail", "detail-white"):
            continue
        inherited = {k: v for k, v in group.attrib.items() if k != "id"}
        for el in group:
            if small and gid.startswith("detail") and el.get("data-small") != "keep":
                continue
            getattr(layers, gid.replace("-", "_")).extend(_element_shapes(el, inherited, min_stroke))
    if not layers.body:
        raise ValueError(f"{path.name}: missing <g id='body'>")
    return layers


# --------------------------------------------------------------------------
# Rasterizer and morphology
# --------------------------------------------------------------------------


class Mask:
    """Square bit mask stored as a bytearray (1 = set)."""

    def __init__(self, n: int, data: bytearray | None = None):
        self.n = n
        self.data = data if data is not None else bytearray(n * n)

    def get(self, x: int, y: int) -> int:
        if 0 <= x < self.n and 0 <= y < self.n:
            return self.data[y * self.n + x]
        return 0

    def op(self, other: "Mask", fn) -> "Mask":
        return Mask(self.n, bytearray(fn(a, b) for a, b in zip(self.data, other.data)))

    def __or__(self, other: "Mask") -> "Mask":
        return self.op(other, lambda a, b: a | b)

    def __and__(self, other: "Mask") -> "Mask":
        return self.op(other, lambda a, b: a & b)

    def __sub__(self, other: "Mask") -> "Mask":
        return self.op(other, lambda a, b: a & (1 - b))

    def count(self) -> int:
        return sum(self.data)


def rasterize(shapes: list[Shape], n: int, scale: float) -> Mask:
    """Union of shapes, sampled at pixel centers of an n x n grid."""
    mask = Mask(n)
    for shape in shapes:
        edges = []
        for pts in shape.subpaths:
            for (x0, y0), (x1, y1) in zip(pts, pts[1:] + pts[:1]):
                if y0 != y1:
                    edges.append((x0 * scale, y0 * scale, x1 * scale, y1 * scale))
        if not edges:
            continue
        ymin = max(0, int(min(min(e[1], e[3]) for e in edges)))
        ymax = min(n - 1, int(max(max(e[1], e[3]) for e in edges)) + 1)
        for row in range(ymin, ymax + 1):
            yc = row + 0.5
            hits = []
            for x0, y0, x1, y1 in edges:
                if (y0 <= yc < y1) or (y1 <= yc < y0):
                    hits.append((x0 + (yc - y0) * (x1 - x0) / (y1 - y0), 1 if y1 > y0 else -1))
            hits.sort()
            winding = 0
            for k, (x, d) in enumerate(hits):
                before = winding
                winding += d
                if before == 0 and winding != 0:
                    span_start = x
                elif before != 0 and winding == 0:
                    first = max(0, math.ceil(span_start - 0.5))
                    last = min(n, math.ceil(x - 0.5))
                    base = row * n
                    for col in range(first, last):
                        mask.data[base + col] = 1
    return mask


def downsample(hi: Mask, factor: int) -> tuple[Mask, list[float]]:
    """1-bit mask by majority coverage, plus the coverage of each pixel."""
    n = hi.n // factor
    out = Mask(n)
    coverage = [0.0] * (n * n)
    area = factor * factor
    for y in range(n):
        for x in range(n):
            total = 0
            for sy in range(y * factor, (y + 1) * factor):
                row = sy * hi.n
                total += sum(hi.data[row + x * factor : row + (x + 1) * factor])
            coverage[y * n + x] = total / area
            out.data[y * n + x] = 1 if total * 2 >= area else 0
    return out, coverage


def _disk(radius: float) -> list[tuple[int, int]]:
    r = math.ceil(radius)
    return [
        (dx, dy)
        for dy in range(-r, r + 1)
        for dx in range(-r, r + 1)
        if (dx or dy) and dx * dx + dy * dy <= radius * radius + 1e-6
    ]


def near_outside(mask: Mask, radius: float) -> Mask:
    """Set pixels with a clear pixel (or the canvas edge) within radius."""
    offsets = _disk(radius)
    out = Mask(mask.n)
    for y in range(mask.n):
        for x in range(mask.n):
            if mask.data[y * mask.n + x] and any(not mask.get(x + dx, y + dy) for dx, dy in offsets):
                out.data[y * mask.n + x] = 1
    return out


def dilate(mask: Mask, radius: float) -> Mask:
    offsets = _disk(radius)
    out = Mask(mask.n, bytearray(mask.data))
    for y in range(mask.n):
        for x in range(mask.n):
            if not mask.data[y * mask.n + x] and any(mask.get(x + dx, y + dy) for dx, dy in offsets):
                out.data[y * mask.n + x] = 1
    return out


@dataclass
class PieceBitmap:
    name: str
    size: int
    left: int
    top: int
    width: int
    height: int
    halo: list[int]  # cropped rows, 1 = paint
    white: list[int]
    black: list[int]
    white_gray: list[int] | None = None  # 2-bit levels 0..3, same crop
    black_gray: list[int] | None = None


def _crop(mask: Mask, left: int, top: int, width: int, height: int) -> list[int]:
    return [mask.get(left + x, top + y) for y in range(height) for x in range(width)]


def _gray_levels(coverage: list[float], n: int) -> Mask:
    """Quantize coverage to 2-bit levels stored in a Mask (values 0..3)."""
    out = Mask(n)
    for i, c in enumerate(coverage):
        out.data[i] = min(3, int(c * 3 + 0.5))
    return out


def render_piece(name: str, spec: SizeSpec, gray: bool = False) -> PieceBitmap:
    scale = spec.size / SOURCE_UNITS
    layers = load_layers(PIECE_DIR / f"{name}.svg", spec.detail_min / scale, spec.size < SMALL_SIZE)
    hi_n = spec.size * spec.supersample
    hi_scale = scale * spec.supersample

    body_hi = rasterize(layers.body, hi_n, hi_scale) - rasterize(layers.cut, hi_n, hi_scale)
    detail_hi = rasterize(layers.detail, hi_n, hi_scale)
    finial_hi = rasterize(layers.detail_white, hi_n, hi_scale)
    body, body_cov = downsample(body_hi, spec.supersample)
    detail, _ = downsample(detail_hi, spec.supersample)
    finial, _ = downsample(finial_hi, spec.supersample)

    edge = near_outside(body, spec.outline)
    inner = body - near_outside(body, spec.rim)
    white = edge | ((detail | finial) & body)
    black = body - (detail & inner)
    halo = dilate(body, spec.halo)

    xs = [i % halo.n for i, v in enumerate(halo.data) if v]
    ys = [i // halo.n for i, v in enumerate(halo.data) if v]
    left, top = min(xs), min(ys)
    width, height = max(xs) - left + 1, max(ys) - top + 1
    bitmap = PieceBitmap(
        name,
        spec.size,
        left,
        top,
        width,
        height,
        _crop(halo, left, top, width, height),
        _crop(white, left, top, width, height),
        _crop(black, left, top, width, height),
    )
    if gray:
        # Anti-aliased ink: edge band and details keep their 1-bit core and
        # get gray fringes from the sub-pixel coverage of the body outline.
        edge_cov = [
            1.0 if e else (c if not b else 0.0) for e, b, c in zip(white.data, body.data, body_cov)
        ]
        white_gray = _gray_levels(edge_cov, white.n)
        black_cov = [c if not d else 0.0 for c, d in zip(body_cov, (detail & inner).data)]
        black_gray = _gray_levels(black_cov, black.n)
        bitmap.white_gray = _crop(white_gray, left, top, width, height)
        bitmap.black_gray = _crop(black_gray, left, top, width, height)
    return bitmap


def render_all(gray: bool = False) -> dict[int, list[PieceBitmap]]:
    return {spec.size: [render_piece(name, spec, gray) for name in PIECES] for spec in SIZES}


# --------------------------------------------------------------------------
# Output: C++ header, PNG previews
# --------------------------------------------------------------------------


def pack_bits(values: list[int], bits: int) -> bytes:
    """Pack values MSB first with no row padding (glyphBitmap layout)."""
    out = bytearray()
    acc = 0
    nbits = 0
    for v in values:
        acc = (acc << bits) | v
        nbits += bits
        if nbits == 8:
            out.append(acc)
            acc = 0
            nbits = 0
    if nbits:
        out.append(acc << (8 - nbits))
    return bytes(out)


def _c_array(name: str, data: bytes) -> str:
    lines = [f"static const uint8_t {name}[{len(data)}] = {{"]
    for i in range(0, len(data), 16):
        lines.append("    " + ", ".join(f"0x{b:02X}" for b in data[i : i + 16]) + ",")
    lines.append("};")
    return "\n".join(lines)


def clang_format(text: str, path: Path) -> str:
    """Format like CI's bin/clang-format-fix, so the generated file stays clean.

    Uses clang-format from PATH or the repo .venv (CI pins version 21); returns
    the text unchanged, with a warning, when none is installed.
    """
    candidates = [shutil.which("clang-format-21"), shutil.which("clang-format")]
    for sub in ("Scripts", "bin"):
        for name in ("clang-format.exe", "clang-format"):
            candidates.append(str(ROOT / ".venv" / sub / name))
    for exe in candidates:
        if exe and Path(exe).exists():
            result = subprocess.run(
                [exe, "--style=file", f"--assume-filename={path}"],
                input=text.encode("utf-8"),
                capture_output=True,
                cwd=ROOT,
                check=True,
            )
            return result.stdout.decode("utf-8").replace("\r\n", "\n")
    print("warning: clang-format not found; header left unformatted", file=sys.stderr)
    return text


def build_header(sets: dict[int, list[PieceBitmap]]) -> tuple[str, int]:
    parts = [
        "// Generated by scripts/gen_chess_pieces.py from design/pieces/*.svg. Do not edit.",
        "#pragma once",
        "",
        "#include <cstdint>",
        "",
        "namespace chesspieces {",
        "",
        "// One piece shape at one size. Bitmaps are 1 bit per pixel, MSB first, rows",
        "// packed without padding: the format GfxRenderer::drawGlyphBitmap() reads.",
        "// Draw halo with state=false (white), then white or black with state=true.",
        "struct Glyph {",
        "  uint8_t left;  // bitmap offset inside the square",
        "  uint8_t top;",
        "  uint8_t width;",
        "  uint8_t height;",
        "  const uint8_t* halo;   // silhouette plus a white ring",
        "  const uint8_t* white;  // ink of the white piece",
        "  const uint8_t* black;  // ink of the black piece",
        "};",
        "",
        "struct Set {",
        "  uint8_t squareSize;",
        "  Glyph glyphs[6];  // indexed by CC_ piece kind - 1",
        "};",
        "",
    ]
    total = 0
    for size, bitmaps in sets.items():
        for bm in bitmaps:
            stem = f"k{bm.name.capitalize()}{size}"
            for plane in ("halo", "white", "black"):
                data = pack_bits(getattr(bm, plane), 1)
                total += len(data)
                parts.append(_c_array(f"{stem}{plane.capitalize()}", data))
            parts.append("")
        parts.append(f"static const Set kSet{size} = {{")
        parts.append(f"    {size},")
        parts.append("    {")
        for bm in bitmaps:
            stem = f"k{bm.name.capitalize()}{size}"
            parts.append(
                f"        {{{bm.left}, {bm.top}, {bm.width}, {bm.height}, {stem}Halo, {stem}White, {stem}Black}},"
            )
        parts.append("    },")
        parts.append("};")
        parts.append("")
    parts.insert(2, f"// Bitmap data: {total} bytes of flash.")
    parts.append("}  // namespace chesspieces")
    parts.append("")
    return "\n".join(parts), total


def build_gray_header(bitmaps: list[PieceBitmap]) -> tuple[str, int]:
    parts = [
        "// Generated by scripts/gen_chess_pieces.py --gray. Do not edit.",
        "// Experimental 2-bit ink planes (0 white .. 3 black) for a grayscale pass.",
        "// Same crop as the matching 1-bit glyph in ChessPieceBitmaps.h.",
        "#pragma once",
        "",
        "#include <cstdint>",
        "",
        "namespace chesspieces {",
        "",
    ]
    total = 0
    for bm in bitmaps:
        stem = f"k{bm.name.capitalize()}{bm.size}"
        for plane in ("white_gray", "black_gray"):
            data = pack_bits(getattr(bm, plane), 2)
            total += len(data)
            parts.append(_c_array(f"{stem}{'White' if plane == 'white_gray' else 'Black'}Gray", data))
        parts.append("")
    parts.append("}  // namespace chesspieces")
    parts.append("")
    return "\n".join(parts), total


def write_png(path: Path, width: int, height: int, gray: bytes | bytearray) -> None:
    """8-bit grayscale PNG, stdlib only. gray holds width*height levels."""
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        raw.extend(gray[y * width : (y + 1) * width])

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)


def hatch_dark(x: int, y: int) -> bool:
    """Dark square texture: 1-pixel diagonal lines, see design/README.md."""
    return (x + y) % 4 == 0


def write_previews(sets: dict[int, list[PieceBitmap]]) -> None:
    """Contact sheet per size: white/black pieces on light and dark squares."""
    for size, bitmaps in sets.items():
        zoom = 4 if size >= 48 else 8
        cols, rows = 6, 4
        w, h = cols * size, rows * size
        px = bytearray(w * h)  # 0 white, 1 black
        for row in range(rows):
            dark = row % 2 == 1
            is_black = row >= 2
            for col, bm in enumerate(bitmaps):
                sx, sy = col * size, row * size
                if dark:
                    for y in range(size):
                        for x in range(size):
                            px[(sy + y) * w + sx + x] = 1 if hatch_dark(sx + x, sy + y) else 0
                ink = bm.black if is_black else bm.white
                for y in range(bm.height):
                    for x in range(bm.width):
                        i = y * bm.width + x
                        o = (sy + bm.top + y) * w + sx + bm.left + x
                        if bm.halo[i]:
                            px[o] = 0
                        if ink[i]:
                            px[o] = 1
        big = bytearray(w * zoom * h * zoom)
        for y in range(h * zoom):
            for x in range(w * zoom):
                big[y * w * zoom + x] = 0 if px[(y // zoom) * w + x // zoom] else 255
        write_png(PREVIEW_DIR / f"pieces_{size}px_x{zoom}.png", w * zoom, h * zoom, big)
        write_png(PREVIEW_DIR / f"pieces_{size}px.png", w, h, bytearray(0 if v else 255 for v in px))

        if bitmaps[0].white_gray is not None:
            levels = [255, 170, 85, 0]
            g = bytearray(255 for _ in range(w * 2 * size))
            for col, bm in enumerate(bitmaps):
                for k, plane in enumerate((bm.white_gray, bm.black_gray)):
                    sy = k * size
                    for y in range(bm.height):
                        for x in range(bm.width):
                            v = plane[y * bm.width + x]
                            if v:
                                g[(sy + bm.top + y) * w + col * size + bm.left + x] = levels[v]
            big = bytearray(w * zoom * 2 * size * zoom)
            for y in range(2 * size * zoom):
                for x in range(w * zoom):
                    big[y * w * zoom + x] = g[(y // zoom) * w + x // zoom]
            write_png(PREVIEW_DIR / f"pieces_{size}px_gray_x{zoom}.png", w * zoom, 2 * size * zoom, big)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--gray", action="store_true", help="also write the experimental 2-bit header")
    parser.add_argument("--check", action="store_true", help="exit 1 if the header is out of date")
    parser.add_argument("--no-preview", action="store_true", help="skip PNG previews")
    args = parser.parse_args()

    sets = render_all(gray=True)  # 2-bit planes feed the previews either way
    header, total = build_header(sets)
    header = clang_format(header, HEADER)
    if args.check:
        current = HEADER.read_text(encoding="utf-8") if HEADER.exists() else ""
        if current != header:
            print(f"{HEADER.relative_to(ROOT)} is stale; run scripts/gen_chess_pieces.py", file=sys.stderr)
            return 1
        print("piece bitmaps up to date")
        return 0

    HEADER.write_text(header, encoding="utf-8", newline="\n")
    print(f"wrote {HEADER.relative_to(ROOT)}: {total} bytes of bitmap data")
    for size, bitmaps in sets.items():
        sizes = ", ".join(f"{bm.name} {bm.width}x{bm.height}" for bm in bitmaps)
        print(f"  {size}px: {sizes}")
    if args.gray:
        gray_header, gray_total = build_gray_header(sets[SIZES[0].size])
        GRAY_HEADER.write_text(clang_format(gray_header, GRAY_HEADER), encoding="utf-8", newline="\n")
        print(f"wrote {GRAY_HEADER.relative_to(ROOT)}: {gray_total} bytes")
    if not args.no_preview:
        write_previews(sets)
        print(f"previews in {PREVIEW_DIR.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
