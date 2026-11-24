#!/usr/bin/env python3
"""Rasterise an outline font into the orpheus-ui bitmap font format (YFN1).

Development tool (host only). The generated `.yfnt` files are committed under
`crates/orpheus-ui/fonts/`; run `fontgen/regen.sh` to rebuild them all. Glyphs
are rasterised by FreeType through Pillow with the font's own hinting, so the
result depends on the FreeType and font versions recorded in
`fonts/README.md`.

Format (all integers little-endian):

    0   b"YFN1"
    4   u8  bits per pixel (1, 2 or 4)
    5   u8  ascent   (cap height: baseline to top of 'H')
    6   u8  descent  (baseline to bottom of 'g')
    7   u8  line height suggested by the font (ascent + descent + gap)
    8   u16 number of ranges
    10  u16 number of glyphs
    12  ranges: { u32 first code point, u16 count, u16 first glyph index }
        glyphs: { u24 bitmap offset, u8 advance, u8 width, u8 height,
                  i8 x bearing, i8 top (rows above the baseline) }
        bitmap: per glyph, width*height pixels of `bpp` bits, row-major,
                most significant bits first, each glyph starting on a byte

Code points inside a range that the font lacks are emitted as empty glyphs
with advance 0; the renderer treats advance 0 + width 0 as "missing" and asks
the fallback font.
"""

import argparse
import struct
import sys

from fontTools.ttLib import TTFont
from PIL import Image, ImageDraw, ImageFont


def parse_ranges(spec):
    out = []
    for part in spec.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            a, b = part.split("-")
            out.append((int(a, 16), int(b, 16)))
        else:
            v = int(part, 16)
            out.append((v, v))
    return out


def cmap_of(path):
    tt = TTFont(path, fontNumber=0)
    cps = set()
    for table in tt["cmap"].tables:
        if table.isUnicode():
            cps.update(table.cmap.keys())
    return cps


def size_for_digit_height(path, target):
    s = target
    while True:
        f = ImageFont.truetype(path, s)
        b = f.getbbox("0", anchor="ls")
        if -b[1] >= target:
            return s
        s += 1


def render(font, ch, mode, bpp):
    """Return (advance, x, top, w, h, levels[]) for one glyph."""
    size = font.size
    canvas = Image.new("L", (size * 4, size * 4), 0)
    d = ImageDraw.Draw(canvas)
    d.fontmode = mode
    ox, oy = size, size * 3
    d.text((ox, oy), ch, font=font, fill=255, anchor="ls")
    adv = int(round(font.getlength(ch, mode=mode)))
    bbox = canvas.getbbox()
    if bbox is None:
        return adv, 0, 0, 0, 0, []
    x0, y0, x1, y1 = bbox
    maxv = (1 << bpp) - 1
    levels = []
    for y in range(y0, y1):
        for x in range(x0, x1):
            v = canvas.getpixel((x, y))
            levels.append(min(maxv, (v * maxv + 127) // 255))
    # Trim rows/columns that quantised to zero.
    w, h = x1 - x0, y1 - y0

    def col_empty(c):
        return all(levels[r * w + c] == 0 for r in range(h))

    def row_empty(r):
        return all(levels[r * w + c] == 0 for c in range(w))

    top_r, bot_r, left_c, right_c = 0, h, 0, w
    while top_r < bot_r and row_empty(top_r):
        top_r += 1
    while bot_r > top_r and row_empty(bot_r - 1):
        bot_r -= 1
    while left_c < right_c and col_empty(left_c):
        left_c += 1
    while right_c > left_c and col_empty(right_c - 1):
        right_c -= 1
    if top_r == bot_r:
        return adv, 0, 0, 0, 0, []
    nl = [levels[r * w + c] for r in range(top_r, bot_r) for c in range(left_c, right_c)]
    return adv, x0 + left_c - ox, oy - (y0 + top_r), right_c - left_c, bot_r - top_r, nl


def pack(levels, bpp):
    out = bytearray()
    acc = 0
    nbits = 0
    for v in levels:
        acc = (acc << bpp) | v
        nbits += bpp
        if nbits == 8:
            out.append(acc)
            acc = 0
            nbits = 0
    if nbits:
        out.append(acc << (8 - nbits))
    return bytes(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--font", required=True)
    ap.add_argument("--size", type=int, help="pixel size passed to FreeType")
    ap.add_argument("--digit-height", type=int, help="choose the size so '0' is this tall")
    ap.add_argument("--bpp", type=int, choices=(1, 2, 4), default=2)
    ap.add_argument("--ranges", required=True)
    ap.add_argument("--tabular", action="store_true", help="give digits one common advance")
    ap.add_argument("--space", type=int, help="override the advance of U+0020")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()

    size = a.size or size_for_digit_height(a.font, a.digit_height)
    font = ImageFont.truetype(a.font, size)
    mode = "1" if a.bpp == 1 else "L"
    have = cmap_of(a.font)

    ranges = []
    for lo, hi in parse_ranges(a.ranges):
        cps = [c for c in range(lo, hi + 1) if c in have or c == 0x20]
        # Split into runs of consecutive code points that the font has.
        run = []
        for c in cps:
            if run and c != run[-1] + 1:
                ranges.append(run)
                run = []
            run.append(c)
        if run:
            ranges.append(run)

    glyphs = {}
    for run in ranges:
        for c in run:
            glyphs[c] = render(font, chr(c), mode, a.bpp)

    if a.tabular:
        digits = [c for c in range(0x30, 0x3A) if c in glyphs]
        tab = max(glyphs[c][0] for c in digits)
        for c in digits:
            adv, x, top, w, h, lv = glyphs[c]
            glyphs[c] = (tab, x + (tab - adv) // 2, top, w, h, lv)
    if a.space is not None and 0x20 in glyphs:
        g = glyphs[0x20]
        glyphs[0x20] = (a.space,) + g[1:]

    ascent = glyphs.get(0x48, glyphs.get(0x30))[2]
    if 0x67 in glyphs:
        g = glyphs[0x67]
        descent = g[4] - g[2]
    else:
        descent = 0
    line = ascent + descent + max(1, size // 6)

    head = bytearray(b"YFN1")
    head += struct.pack("<BBBBHH", a.bpp, ascent, descent, line, len(ranges), len(glyphs))
    rtab = bytearray()
    gtab = bytearray()
    bitmap = bytearray()
    gi = 0
    for run in ranges:
        rtab += struct.pack("<IHH", run[0], len(run), gi)
        for c in run:
            adv, x, top, w, h, lv = glyphs[c]
            off = len(bitmap)
            bitmap += pack(lv, a.bpp)
            if not (0 <= adv < 256 and -128 <= x < 128 and -128 <= top < 128 and w < 256 and h < 256):
                sys.exit(f"glyph U+{c:04X} out of range: {(adv, x, top, w, h)}")
            gtab += struct.pack("<I", off)[:3] + struct.pack("<BBBbb", adv, w, h, x, top)
            gi += 1
    data = bytes(head + rtab + gtab + bitmap)
    with open(a.out, "wb") as f:
        f.write(data)
    print(f"{a.out}: size {size}px bpp {a.bpp} ascent {ascent} descent {descent} "
          f"line {line} ranges {len(ranges)} glyphs {len(glyphs)} bytes {len(data)}")


if __name__ == "__main__":
    main()
