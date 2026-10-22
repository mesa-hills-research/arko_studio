#!/usr/bin/env python3
"""Builds the icon files that are made from the SVGs in this folder.

Run it from anywhere with `python3 assets/make-icons.py`. It needs Python 3
and `rsvg-convert` (from librsvg) or `resvg` on the PATH, and uses no network.

The mark has three levels of detail (the arch, stem and arrow, then the nodes,
then the thin links and the gap in the top of the arch). A small icon cannot carry
all of them, so every size below 48 px can use one of two kinds of drawing.

Full mark, drawn from assets/icon.svg by this script
  n  the full mark scaled to the size, with the margin it has in icon.svg
  d  the same drawing scaled to fill the canvas
  e  d with extra weight, so the thin shapes keep their weight after the downscale
  f  e with its size, position and weight nudged so the stem, the arrow shaft and
     the circles fall on whole pixels, drawn at 8 times the size and averaged down
  g  like f, but only the thin shapes get heavier, the arrow shaft is redrawn a whole
     number of pixels thick, and the nodes get a little bigger

Candidates drawn by hand, named icon-<size>-<letter>.svg in assets/icon-candidates
  a  the arch, the stem and the arrow only
  b  adds the nodes and the line through them, and the links at 32 and 40 px
  c  the mark of b in near-black on a rounded tile

CHOICE below says which letter each size uses. Sizes from 48 px up always use the
full mark in assets/icon.svg.

Outputs
  assets/icon-1024.png                                  the tile at 1024 px
  assets/icon.ico                                       Windows, 16 to 256 px
  packaging/linux/icons/hicolor/<size>x<size>/apps/*.png  Linux, 16 to 512 px
  packaging/linux/icons/hicolor/scalable/apps/*.svg      Linux, the full mark
"""

import math
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib

APP_ID = "com.mesahillsresearch.ArkoStudio"

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ASSETS = os.path.join(ROOT, "assets")
CANDIDATES = os.path.join(ASSETS, "icon-candidates")
HICOLOR = os.path.join(ROOT, "packaging", "linux", "icons", "hicolor")

FULL = os.path.join(ASSETS, "icon.svg")
TILE = os.path.join(ASSETS, "icon-tile.svg")

# The drawing used at each size below 48 px. To switch a size, change its letter
# and run the script again. 22 px is only used for Linux, 20 and 40 px only for Windows.
CHOICE = {16: "g", 20: "g", 22: "g", 24: "g", 32: "g", 40: "g"}

# Windows sizes. The 64 px entry and the larger ones are stored as PNG, the
# smaller ones as the classic bitmap that every Windows version reads.
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
ICO_PNG_FROM = 64
LINUX_SIZES = [16, 22, 24, 32, 48, 64, 128, 256, 512]


def source(size):
    """The hand-drawn SVG of a size, or None when the size is drawn from the full mark."""
    letter = CHOICE.get(size, "n")
    if letter in "abc":
        svg = os.path.join(CANDIDATES, f"icon-{size}-{letter}.svg")
        if not os.path.exists(svg):
            sys.exit(f"CHOICE[{size}] is {letter!r}, but {svg} does not exist.")
        return svg
    if letter not in "ndefg":
        sys.exit(f"CHOICE[{size}] is {letter!r}, which is not a letter this script knows.")
    return None


def check_grid(svg, size):
    """A drawing made for one size must be drawn on a grid of that size."""
    with open(svg, encoding="utf-8") as f:
        head = f.read(600)
    match = re.search(r'viewBox="0 0 (\d+) (\d+)"', head)
    if match and match.group(1) == match.group(2) == str(size):
        return
    if os.path.basename(svg) == "icon.svg":
        return
    sys.exit(f"{svg} is not drawn on a {size} px grid.")


def rasterizer():
    for name in ("rsvg-convert", "resvg"):
        path = shutil.which(name)
        if path:
            return name, path
    sys.exit("make-icons.py needs rsvg-convert (librsvg) or resvg on the PATH.")


def run_rasterizer(svg, width, out):
    name, path = rasterizer()
    if name == "rsvg-convert":
        cmd = [path, "-w", str(width), "-h", str(width), svg, "-o", out]
    else:
        cmd = [path, "-w", str(width), "-h", str(width), svg, out]
    subprocess.run(cmd, check=True)


def render(svg, size, out):
    """Writes the SVG as a size x size PNG with a transparent background."""
    check_grid(svg, size)
    run_rasterizer(svg, size, out)


def read_png(path):
    """Returns (width, height, rows of RGBA bytes) of an 8-bit RGBA PNG."""
    with open(path, "rb") as f:
        data = f.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path} is not a PNG")
    pos, idat, header = 8, [], None
    while pos < len(data):
        length, kind = struct.unpack(">I4s", data[pos : pos + 8])
        body = data[pos + 8 : pos + 8 + length]
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat.append(body)
        pos += 12 + length
    width, height, depth, color, _, _, interlace = header
    if (depth, color, interlace) != (8, 6, 0):
        raise ValueError(f"{path} must be an 8-bit RGBA PNG without interlacing")
    raw = zlib.decompress(b"".join(idat))
    stride, rows, prev = width * 4, [], bytearray(width * 4)
    for y in range(height):
        kind = raw[y * (stride + 1)]
        line = bytearray(raw[y * (stride + 1) + 1 : (y + 1) * (stride + 1)])
        for i in range(stride):
            left = line[i - 4] if i >= 4 else 0
            up = prev[i]
            up_left = prev[i - 4] if i >= 4 else 0
            if kind == 1:
                line[i] = (line[i] + left) & 255
            elif kind == 2:
                line[i] = (line[i] + up) & 255
            elif kind == 3:
                line[i] = (line[i] + ((left + up) >> 1)) & 255
            elif kind == 4:
                p = left + up - up_left
                pa, pb, pc = abs(p - left), abs(p - up), abs(p - up_left)
                pred = left if pa <= pb and pa <= pc else up if pb <= pc else up_left
                line[i] = (line[i] + pred) & 255
        rows.append(bytes(line))
        prev = line
    return width, height, rows


def write_png(path, width, height, rows):
    """Writes 8-bit RGBA rows as a PNG."""
    def chunk(kind, body):
        crc = zlib.crc32(kind + body) & 0xFFFFFFFF
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", crc)

    raw = b"".join(b"\x00" + bytes(row) for row in rows)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(raw, 9)))
        f.write(chunk(b"IEND", b""))


# ---- the full mark at small sizes ---------------------------------------------------

# Where things are in icon.svg, in its own units. Grid fitting aligns these to whole
# pixels, so they have to match the drawing. MARK is the left, top, right and bottom of
# the whole mark, THIN the width of the shapes that are thinner than the arch.
MARK = (123.0, 153.5, 901.25, 869.0)
STEM = (681.3, 761.8)
STEM_BOTTOM = 861.4
ARCH_TOP = 153.5
SHAFT_MID = (505.3 + 544.6) / 2
THIN = {"link": 34.0, "shaft": 39.3, "ring": 32.3}
HINT_NUMBERS = ("681.3", "761.8", "861.4", "505.3", "544.6", "123", "901.1", "597.6")

# What each size gets, in px. margin is the empty border around the mark. grow is the weight
# added around every shape (e and f). least is the thinnest the links, the shaft and the ring
# wall may get, and node how much the nodes grow (g).
LOOK = {
    16: dict(margin=0, grow=0.5, least=1.3, node=0.25),
    20: dict(margin=0, grow=0.5, least=1.3, node=0.25),
    22: dict(margin=0, grow=0.4, least=1.2, node=0.2),
    24: dict(margin=0, grow=0.4, least=1.2, node=0.2),
    32: dict(margin=0, grow=0.25, least=1.1, node=0.1),
    40: dict(margin=0, grow=0.15, least=1.0, node=0.1),
}
SUPERSAMPLE = 8
# Grid fitting costs. SHRINK is for making the mark smaller than the canvas allows and
# CENTRED for unequal margins. Both are in px squared per pixel of size.
SHRINK = 60.0
CENTRED = 0.15


def load_mark():
    """The colour of icon.svg and its shapes as (kind, element). The kind comes from the
    comment above each shape, so the shapes can be told apart without reading the paths."""
    with open(FULL, encoding="utf-8") as f:
        text = f.read()
    if not all(n in text for n in HINT_NUMBERS):
        print("icon.svg has changed, so update MARK and the numbers after it in make-icons.py.",
              file=sys.stderr)
    colour = re.search(r'<g fill="(#[0-9A-Fa-f]{6})">', text).group(1)
    shapes = []
    for m in re.finditer(r"<!--(.*?)-->\s*(<(?:path|circle)\b[^>]*/>)", text, re.S):
        c = m.group(1).lower()
        kind = ("link" if "link" in c else "shaft" if "shaft" in c else "head" if "head" in c
                else "ring" if "hollow" in c else "node" if "node" in c else "arc")
        shapes.append((kind, m.group(2)))
    return colour, shapes


def ring_of(shapes):
    """Centre, outer radius and hole radius of the hollow node."""
    for kind, el in shapes:
        if kind == "ring":
            d = re.search(r'd="([^"]*)"', el).group(1)
            g = re.match(r"M([\d.]+) ([\d.]+)a([\d.]+) [\d.]+ 0 1 1 [\d.]+ 0a[^Z]*ZM[\d.]+ [\d.]+a([\d.]+) ", d)
            x, y, ro, ri = (float(g.group(i)) for i in (1, 2, 3, 4))
            return x + ro, y, ro, ri
    sys.exit("icon.svg has no hollow node.")


def circle_of(el):
    m = re.match(r'<circle cx="([\d.]+)" cy="([\d.]+)" r="([\d.]+)"', el)
    return tuple(float(v) for v in m.groups())


def weights(letter, s, look):
    """The width in px added around each kind of shape. For the ring it is how far
    the outside grows, because the hole stays as it is. shaft_t is the width the arrow shaft
    ends up with. In g the shaft is redrawn with a whole number of pixels for its width."""
    kinds = ("arc", "head", "link", "shaft", "ring", "node")
    natural = s * THIN["shaft"]
    if letter == "d":
        w = {k: 0.0 for k in kinds}
        w["shaft_t"] = natural
        return w
    if letter in "ef":
        w = {k: look["grow"] for k in kinds}
        w["shaft_t"] = natural + look["grow"]
        return w
    least = look["least"]
    return {"arc": 0.0, "head": 0.0, "node": look["node"],
            "link": max(0.0, least - s * THIN["link"]),
            "shaft": 0.0, "shaft_t": float(max(1, round(natural))), "shaft_rect": True,
            "ring": max(0.0, least - s * THIN["ring"])}


def fit(size, margin, letter, look):
    """Scale and offset that make the mark, with its added weight, fill the canvas inside the margin."""
    left, top, right, bottom = MARK
    s = (size - 2 * margin) / (right - left)
    for _ in range(4):
        w = weights(letter, s, look)
        s = (size - 2 * margin - w["ring"] - w["head"] / 2) / (right - left)
    w = weights(letter, s, look)
    tx = margin + w["ring"] - s * left
    ty = (size - (s * (bottom - top) + w["arc"])) / 2 + w["arc"] / 2 - s * top
    return s, s, tx, ty


def dist_edge(p):
    return abs(p - round(p))


def dist_centre(p):
    return abs(p - (math.floor(p) + 0.5))


def hint(size, letter, look, shapes):
    """Searches a small range of weights, scales and offsets for the one that puts the stem,
    the arrow shaft and the top and bottom of the mark on pixel boundaries and the centres of the
    circles on pixel centres. Returns the scales and offsets and the look it settled on."""
    ring = ring_of(shapes)
    centres = [(ring[0], ring[1])] + [circle_of(el)[:2] for kind, el in shapes if kind == "node"]
    margin = look["margin"]
    left, top, right, bottom = MARK
    key = "least" if letter == "g" else "grow"

    def axis(s_list, lo, hi, lo_extra, hi_extra, edges, centre_values):
        best = None
        for s in s_list:
            tmin = margin + lo_extra(s) - s * lo
            tmax = size - margin - hi_extra(s) - s * hi
            if tmax < tmin:
                continue
            steps = max(1, int(round((tmax - tmin) / 0.02)))
            for i in range(steps + 1):
                t = tmin + (tmax - tmin) * i / steps
                cost = sum(w * dist_edge(t + s * v + off(s)) ** 2 for v, w, off in edges)
                cost += sum(1.5 * dist_centre(t + s * v) ** 2 for v in centre_values)
                cost += CENTRED * ((t + s * lo - lo_extra(s)) - (size - (t + s * hi + hi_extra(s)))) ** 2
                if best is None or (cost, -s) < (best[0], -best[1]):
                    best = (cost, s, t)
        return best

    best = None
    for step in range(11):
        trial = dict(look)
        trial[key] = max(0.0, look[key] - 0.25 + 0.05 * step)
        wt = lambda s, name: weights(letter, s, trial)[name]
        s0 = fit(size, margin, letter, trial)[0]
        edges_x = [(123.0, 2, lambda s: -wt(s, "ring")), (STEM[0], 3, lambda s: -wt(s, "arc") / 2),
                   (STEM[1], 3, lambda s: wt(s, "arc") / 2), (901.25, 2, lambda s: wt(s, "head") / 2)]
        bx = axis([s0 * (1 - 0.004 * k) for k in range(16)], left, right,
                  lambda s: wt(s, "ring"), lambda s: wt(s, "head") / 2, edges_x, [c[0] for c in centres])
        edges_y = [(ARCH_TOP, 2, lambda s: -wt(s, "arc") / 2), (SHAFT_MID, 3, lambda s: -wt(s, "shaft_t") / 2),
                   (SHAFT_MID, 3, lambda s: wt(s, "shaft_t") / 2), (STEM_BOTTOM, 2, lambda s: wt(s, "arc") / 2)]
        by = axis([bx[1] * (1 + 0.004 * k) for k in range(-8, 9)], top, bottom,
                  lambda s: wt(s, "arc") / 2, lambda s: wt(s, "arc") / 2, edges_y, [c[1] for c in centres])
        total = bx[0] + by[0] + SHRINK * size * (1 - bx[1] / s0) ** 2
        if best is None or total < best[0]:
            best = (total, bx[1], by[1], bx[2], by[2], trial)
    return best[1:]


def fnum(v):
    return f"{v:.4f}".rstrip("0").rstrip(".")


def mark_svg(size, k, sx, sy, tx, ty, w, colour, shapes):
    """The full mark as an SVG on a size x size px grid, drawn k times as large. w is the width in px
    added around each kind of shape. The added weight is a stroke in the colour of the shape."""
    unit = (sx + sy) / 2
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{size * k}" height="{size * k}" viewBox="0 0 {size} {size}">',
           f'<g transform="translate({fnum(tx)} {fnum(ty)}) scale({fnum(sx)} {fnum(sy)})" fill="{colour}">']
    for kind, el in shapes:
        if kind == "ring":
            cx, cy, ro, ri = ring_of(shapes)
            ro += w["ring"] / unit
            out.append(f'<path fill-rule="evenodd" d="M{fnum(cx - ro)} {fnum(cy)}a{fnum(ro)} {fnum(ro)} 0 1 1 {fnum(2 * ro)} 0'
                       f'a{fnum(ro)} {fnum(ro)} 0 1 1 {fnum(-2 * ro)} 0ZM{fnum(cx - ri)} {fnum(cy)}'
                       f'a{fnum(ri)} {fnum(ri)} 0 1 0 {fnum(2 * ri)} 0a{fnum(ri)} {fnum(ri)} 0 1 0 {fnum(-2 * ri)} 0Z"/>')
        elif kind == "shaft" and w.get("shaft_rect"):
            m = re.match(r'<path d="M([\d.]+) ([\d.]+)L([\d.]+) ', el)
            x0, x1 = float(m.group(1)), float(m.group(3))
            half = w["shaft_t"] / unit / 2
            out.append(f'<rect x="{fnum(x0)}" y="{fnum(SHAFT_MID - half)}" width="{fnum(x1 - x0)}" height="{fnum(2 * half)}"/>')
        elif w[kind] > 0:
            stroke = f' stroke="{colour}" stroke-width="{fnum(w[kind] / unit)}" stroke-linejoin="round"/>'
            out.append(el[:-2] + stroke)
        else:
            out.append(el)
    out.append("</g></svg>")
    return "\n".join(out)


def render_mark(size, letter, out, tmp):
    """Draws the full mark for one of the letters d, e, f and g at size px."""
    colour, shapes = load_mark()
    look = LOOK[size]
    if letter in "de":
        sx, sy, tx, ty = fit(size, look["margin"], letter, look)
        w = weights(letter, sx, look)
        svg = os.path.join(tmp, f"mark-{size}.svg")
        with open(svg, "w", encoding="utf-8") as f:
            f.write(mark_svg(size, 1, sx, sy, tx, ty, w, colour, shapes))
        run_rasterizer(svg, size, out)
        return
    sx, sy, tx, ty, look = hint(size, letter, look, shapes)
    w = weights(letter, (sx + sy) / 2, look)
    k = SUPERSAMPLE
    svg = os.path.join(tmp, f"mark-{size}.svg")
    with open(svg, "w", encoding="utf-8") as f:
        f.write(mark_svg(size, k, sx, sy, tx, ty, w, colour, shapes))
    big = os.path.join(tmp, f"mark-{size}-big.png")
    run_rasterizer(svg, size * k, big)
    width, height, rows = read_png(big)
    rgb = bytes(int(colour[i : i + 2], 16) for i in (1, 3, 5))
    result = []
    for y in range(size):
        total = [0] * width
        for r in rows[y * k : (y + 1) * k]:
            total = [a + b for a, b in zip(total, r[3::4])]
        line = bytearray()
        for x in range(size):
            cover = sum(total[x * k : (x + 1) * k])
            line += rgb + bytes((min(255, (cover + k * k // 2) // (k * k)),))
        result.append(line)
    write_png(out, size, size, result)


def render_size(size, out, tmp):
    """Writes the PNG for a size, from the drawing that CHOICE names for it."""
    svg = source(size)
    if svg:
        render(svg, size, out)
    elif CHOICE.get(size, "n") == "n":
        render(FULL, size, out)
    else:
        render_mark(size, CHOICE[size], out, tmp)


def bitmap_entry(path):
    """An ICO image as a 32 bit bitmap (BGRA, bottom row first) with an empty mask."""
    width, height, rows = read_png(path)
    pixels = bytearray()
    for row in reversed(rows):
        for i in range(0, len(row), 4):
            r, g, b, a = row[i : i + 4]
            pixels += bytes((b, g, r, a))
    mask = bytes(((width + 31) // 32 * 4) * height)
    header = struct.pack(
        "<IiiHHIIiiII", 40, width, height * 2, 1, 32, 0, len(pixels) + len(mask), 0, 0, 0, 0
    )
    return header + bytes(pixels) + mask


def write_ico(path, tmp):
    entries = []
    for size in ICO_SIZES:
        png = os.path.join(tmp, f"ico-{size}.png")
        render_size(size, png, tmp)
        if size >= ICO_PNG_FROM:
            with open(png, "rb") as f:
                entries.append((size, f.read()))
        else:
            entries.append((size, bitmap_entry(png)))
    out = bytearray(struct.pack("<HHH", 0, 1, len(entries)))
    offset = 6 + 16 * len(entries)
    for size, data in entries:
        side = 0 if size >= 256 else size
        out += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    for _, data in entries:
        out += data
    with open(path, "wb") as f:
        f.write(out)


def main():
    with tempfile.TemporaryDirectory() as tmp:
        render(TILE, 1024, os.path.join(ASSETS, "icon-1024.png"))
        write_ico(os.path.join(ASSETS, "icon.ico"), tmp)
        for size in LINUX_SIZES:
            folder = os.path.join(HICOLOR, f"{size}x{size}", "apps")
            os.makedirs(folder, exist_ok=True)
            render_size(size, os.path.join(folder, f"{APP_ID}.png"), tmp)
    folder = os.path.join(HICOLOR, "scalable", "apps")
    os.makedirs(folder, exist_ok=True)
    shutil.copyfile(FULL, os.path.join(folder, f"{APP_ID}.svg"))


if __name__ == "__main__":
    main()
