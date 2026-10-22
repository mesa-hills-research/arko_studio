#!/usr/bin/env python3
"""Builds the icon files that are made from the SVGs in this folder.

Run it from anywhere with `python3 assets/make-icons.py`. It needs Python 3
and `rsvg-convert` (from librsvg) or `resvg` on the PATH, and uses no network.

The mark has three levels of detail (the arch, stem and arrow, then the nodes,
then the thin links and the gap in the top of the arch). A small icon cannot carry
all of them, so every size below 48 px has its own drawings and none is scaled from
another. Each of those sizes has three candidates, named icon-<size>-<letter>.svg
in assets/icon-candidates.

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

# The candidate used at each size below 48 px. To switch a size, change its letter
# and run the script again. 22 px is only used for Linux, 20 and 40 px only for Windows.
CHOICE = {16: "b", 20: "b", 22: "b", 24: "b", 32: "b", 40: "b"}

# Windows sizes. The 64 px entry and the larger ones are stored as PNG, the
# smaller ones as the classic bitmap that every Windows version reads.
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
ICO_PNG_FROM = 64
LINUX_SIZES = [16, 22, 24, 32, 48, 64, 128, 256, 512]


def source(size):
    """The SVG for a size. A drawing made for one size is rendered at exactly that size."""
    if size not in CHOICE:
        return FULL
    svg = os.path.join(CANDIDATES, f"icon-{size}-{CHOICE[size]}.svg")
    if not os.path.exists(svg):
        sys.exit(f"CHOICE[{size}] is {CHOICE[size]!r}, but {svg} does not exist.")
    return svg


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


def render(svg, size, out):
    """Writes the SVG as a size x size PNG with a transparent background."""
    check_grid(svg, size)
    name, path = rasterizer()
    if name == "rsvg-convert":
        cmd = [path, "-w", str(size), "-h", str(size), svg, "-o", out]
    else:
        cmd = [path, "-w", str(size), "-h", str(size), svg, out]
    subprocess.run(cmd, check=True)


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
        render(source(size), size, png)
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
        render(source(size), size, os.path.join(folder, f"{APP_ID}.png"))
    folder = os.path.join(HICOLOR, "scalable", "apps")
    os.makedirs(folder, exist_ok=True)
    shutil.copyfile(FULL, os.path.join(folder, f"{APP_ID}.svg"))


if __name__ == "__main__":
    main()
