#!/usr/bin/env python3
"""Generate the orpheus-img test pictures and their reference decodes.

Usage: gen.py OUTDIR

Writes OUTDIR/<name>.<ext> and OUTDIR/<name>.ref (u32 LE width, u32 LE
height, then RGBA8 rows) plus OUTDIR/manifest.txt with one line per picture:
"<file> <ref> <exact|lossy>". References come from the known source pixels
(PNG files written here) or from Pillow (JPEG via libjpeg-turbo, BMP, GIF,
QOI). Tools used: Python 3 with numpy and Pillow, ImageMagick (`magick`),
libjpeg-turbo's `cjpeg`, ffmpeg. Everything is deterministic.
"""

import io
import os
import struct
import subprocess
import sys
import zlib

import numpy as np
from PIL import Image

OUT = sys.argv[1]
os.makedirs(OUT, exist_ok=True)
manifest = []
RNG = np.random.default_rng(20261123)


def photo(w, h, seed=1):
    """A photo-like RGB picture: gradients, soft blobs, edges and noise."""
    r = np.random.default_rng(seed)
    y, x = np.mgrid[0:h, 0:w].astype(np.float64)
    img = np.zeros((h, w, 3))
    img[..., 0] = 120 + 90 * np.sin(x / (w / 3.1) + 0.5) * np.cos(y / (h / 2.3))
    img[..., 1] = 110 + 80 * np.cos(x / (w / 1.7) - y / (h / 4.0))
    img[..., 2] = 100 + 100 * (x + y) / (w + h)
    for _ in range(12):
        cx, cy, rad = r.uniform(0, w), r.uniform(0, h), r.uniform(min(w, h) / 12, min(w, h) / 4)
        col = r.uniform(0, 255, 3)
        a = np.exp(-(((x - cx) ** 2 + (y - cy) ** 2) / (2 * rad * rad)))[..., None]
        img = img * (1 - 0.7 * a) + col * 0.7 * a
    # A few hard-edged rectangles and thin lines.
    for _ in range(6):
        x0, y0 = int(r.uniform(0, w * 0.8)), int(r.uniform(0, h * 0.8))
        img[y0 : y0 + max(2, h // 10), x0 : x0 + max(2, w // 7)] = r.uniform(0, 255, 3)
    img[:, ::17] = img[:, ::17] * 0.5
    img += r.normal(0, 4, img.shape)
    return np.clip(img, 0, 255).astype(np.uint8)


def write_ref(name, rgba):
    h, w = rgba.shape[:2]
    with open(os.path.join(OUT, name + ".ref"), "wb") as f:
        f.write(struct.pack("<II", w, h))
        f.write(np.ascontiguousarray(rgba, dtype=np.uint8).tobytes())


def ref_from_pil(path, name):
    im = Image.open(path)
    im.seek(0)
    write_ref(name, np.asarray(im.convert("RGBA")))


def add(fname, name, kind):
    manifest.append(f"{fname} {name}.ref {kind}")


# ---------------------------------------------------------------- JPEG

def jpeg_pil(name, rgb, **kw):
    path = os.path.join(OUT, name + ".jpg")
    Image.fromarray(rgb).save(path, "JPEG", **kw)
    ref_from_pil(path, name)
    add(name + ".jpg", name, "lossy")


def jpeg_cjpeg(name, rgb, args):
    path = os.path.join(OUT, name + ".jpg")
    ppm = os.path.join(OUT, name + ".ppm")
    Image.fromarray(rgb).save(ppm)
    subprocess.run(["cjpeg", *args, "-outfile", path, ppm], check=True)
    os.remove(ppm)
    ref_from_pil(path, name)
    add(name + ".jpg", name, "lossy")


P = photo(640, 480)
jpeg_pil("j_444_q90", P, quality=90, subsampling=0)
jpeg_pil("j_420_q85", P, quality=85, subsampling=2)
jpeg_pil("j_422_q75", P, quality=75, subsampling=1)
jpeg_pil("j_gray", np.asarray(Image.fromarray(P).convert("L")), quality=88)
jpeg_pil("j_opt_420", P, quality=80, subsampling=2, optimize=True)
jpeg_pil("j_prog_420", P, quality=85, subsampling=2, progressive=True)
jpeg_pil("j_prog_444", P, quality=92, subsampling=0, progressive=True)
jpeg_pil("j_prog_gray", np.asarray(Image.fromarray(P).convert("L")), quality=85, progressive=True)
jpeg_pil("j_odd_420", photo(333, 221, 5), quality=85, subsampling=2)
jpeg_pil("j_odd_prog", photo(333, 221, 5), quality=85, subsampling=2, progressive=True)
jpeg_pil("j_tiny", photo(13, 7, 6), quality=90)
jpeg_cjpeg("j_restart_420", P, ["-quality", "85", "-sample", "2x2", "-restart", "1"])
jpeg_cjpeg("j_restart_prog", P, ["-quality", "85", "-progressive", "-restart", "3B"])
jpeg_cjpeg("j_440", photo(200, 150, 7), ["-quality", "85", "-sample", "1x2"])
jpeg_cjpeg("j_411", photo(200, 150, 8), ["-quality", "85", "-sample", "4x1"])
jpeg_cjpeg("j_rgb", photo(160, 120, 9), ["-quality", "90", "-rgb"])
# Sequential with one scan per component (needs coefficient storage).
scans = os.path.join(OUT, "scans.txt")
with open(scans, "w") as f:
    f.write("0;\n1;\n2;\n")
jpeg_cjpeg("j_seq_noninterleaved", photo(160, 120, 10), ["-quality", "85", "-scans", scans])
os.remove(scans)
jpeg_pil("j_big_420", photo(2400, 1800, 11), quality=80, subsampling=2)
jpeg_pil("j_big_prog", photo(2400, 1800, 11), quality=80, subsampling=2, progressive=True)

# EXIF orientation 6 (rotate 90° CW to display).
exif = Image.Exif()
exif[0x0112] = 6
jpeg_pil("j_exif6", photo(160, 90, 12), quality=90, exif=exif.tobytes())

# CMYK (Adobe, inverted).
cmyk = Image.fromarray(photo(120, 80, 13)).convert("CMYK")
path = os.path.join(OUT, "j_cmyk.jpg")
cmyk.save(path, "JPEG", quality=92)
ref_from_pil(path, "j_cmyk")
add("j_cmyk.jpg", "j_cmyk", "lossy")

# ---------------------------------------------------------------- PNG


def png_chunk(t, d):
    return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)


ADAM7 = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)]


def pack_row(samples, depth):
    """samples: 1-D array of sample values for one row."""
    if depth == 16:
        return samples.astype(">u2").tobytes()
    if depth == 8:
        return samples.astype(np.uint8).tobytes()
    per = 8 // depth
    n = len(samples)
    out = bytearray((n * depth + 7) // 8)
    for i, s in enumerate(samples):
        out[i // per] |= int(s) << (8 - depth * (i % per + 1))
    return bytes(out)


def filt(row, prev, bpp, ft):
    row = np.frombuffer(row, np.uint8).astype(np.int32)
    prev = np.frombuffer(prev, np.uint8).astype(np.int32) if prev is not None else np.zeros_like(row)
    a = np.concatenate([np.zeros(bpp, np.int32), row[:-bpp]]) if len(row) > bpp else np.zeros_like(row)
    c = np.concatenate([np.zeros(bpp, np.int32), prev[:-bpp]]) if len(row) > bpp else np.zeros_like(row)
    if len(row) <= bpp:
        a = np.zeros_like(row)
        c = np.zeros_like(row)
    b = prev
    if ft == 0:
        o = row
    elif ft == 1:
        o = row - a
    elif ft == 2:
        o = row - b
    elif ft == 3:
        o = row - (a + b) // 2
    else:
        p = a + b - c
        pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
        pr = np.where((pa <= pb) & (pa <= pc), a, np.where(pb <= pc, b, c))
        o = row - pr
    return bytes([ft]) + (o & 255).astype(np.uint8).tobytes()


def write_png(path, samples, ct, depth, interlace=False, plte=None, trns=None, level=6, strategy=None):
    """samples: (h, w, channels) array of raw sample values."""
    h, w, ch = samples.shape
    bpp = max(1, ch * depth // 8)
    raw = bytearray()
    passes = ADAM7 if interlace else [(0, 0, 1, 1)]
    k = 0
    for x0, y0, dx, dy in passes:
        sub = samples[y0::dy, x0::dx]
        if sub.shape[0] == 0 or sub.shape[1] == 0:
            continue
        prev = None
        for row in sub:
            r = pack_row(row.reshape(-1), depth)
            raw += filt(r, prev, bpp, k % 5)
            k += 1
            prev = r
    if strategy is None:
        z = zlib.compress(bytes(raw), level)
    else:
        co = zlib.compressobj(level, zlib.DEFLATED, 15, 9, strategy)
        z = co.compress(bytes(raw)) + co.flush()
    out = b"\x89PNG\r\n\x1a\n"
    out += png_chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, depth, ct, 0, 0, 1 if interlace else 0))
    out += png_chunk(b"tEXt", b"Comment\0orpheus-img test picture")
    if plte is not None:
        out += png_chunk(b"PLTE", bytes(np.asarray(plte, np.uint8).reshape(-1)))
    if trns is not None:
        out += png_chunk(b"tRNS", trns)
    # Split the data over several IDAT chunks of odd sizes.
    i, n = 0, 0
    while i < len(z):
        step = [1, 7, 1000, 8192][n % 4]
        out += png_chunk(b"IDAT", z[i : i + step])
        i += step
        n += 1
    out += png_chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(out)


def png_case(name, w, h, ct, depth, interlace=False, trns=False, **kw):
    r = np.random.default_rng(len(name) * 7919 + w * 31 + h)
    base = photo(w, h, len(name))
    maxv = (1 << depth) - 1
    plte = None
    trns_bytes = None
    if ct == 3:
        n = min(256, 1 << depth)
        plte = r.integers(0, 256, (n, 3))
        idx = (base[..., 0].astype(np.int32) * n // 256).astype(np.int64)
        samples = idx[..., None]
        rgba = np.concatenate([plte[idx], np.full((h, w, 1), 255)], axis=2)
        if trns:
            alpha = r.integers(0, 256, n)
            alpha[0] = 0
            alpha[1 % n] = 255
            trns_bytes = bytes(alpha.astype(np.uint8)[: max(1, n - 1)])
            full_a = np.full(n, 255)
            full_a[: max(1, n - 1)] = alpha[: max(1, n - 1)]
            rgba[..., 3] = full_a[idx]
    else:
        if depth == 16:
            lum = base.astype(np.int64) * 257 + r.integers(0, 257, base.shape)
        else:
            lum = base.astype(np.int64) * maxv // 255
        gray = lum[..., 1:2]
        alpha = (np.arange(w)[None, :, None] * maxv // max(1, w - 1)).repeat(h, axis=0)
        alpha = (alpha + np.arange(h)[:, None, None] * 0).astype(np.int64)
        samples = {0: gray, 2: lum, 4: np.concatenate([gray, alpha], 2), 6: np.concatenate([lum, alpha], 2)}[ct]

        def to8(v):
            if depth == 16:
                return v >> 8
            return v * 255 // maxv

        if ct in (0, 4):
            g8 = to8(gray)
            rgb = np.concatenate([g8, g8, g8], 2)
        else:
            rgb = to8(lum)
        a8 = to8(alpha) if ct in (4, 6) else np.full((h, w, 1), 255)
        rgba = np.concatenate([rgb, a8], 2)
        if trns and ct in (0, 2):
            key = samples[h // 2, w // 2].copy()
            if ct == 0:
                trns_bytes = struct.pack(">H", int(key[0]))
            else:
                trns_bytes = struct.pack(">HHH", *[int(v) for v in key])
            hit = np.all(samples == key, axis=2)
            rgba[hit, 3] = 0
    fname = name + ".png"
    write_png(os.path.join(OUT, fname), samples, ct, depth, interlace, plte, trns_bytes, **kw)
    write_ref(name, rgba)
    add(fname, name, "exact")
    # Cross-check our writer with Pillow where it can represent the result.
    if depth == 8 and ct in (2, 6) and not trns:
        im = np.asarray(Image.open(os.path.join(OUT, fname)).convert("RGBA"))
        assert (im == rgba).all(), name


for depth in (1, 2, 4, 8, 16):
    png_case(f"p_gray{depth}", 61, 37, 0, depth)
for depth in (1, 2, 4, 8):
    png_case(f"p_pal{depth}", 53, 29, 3, depth)
png_case("p_pal8_trns", 53, 29, 3, 8, trns=True)
png_case("p_pal4_trns", 33, 17, 3, 4, trns=True)
png_case("p_rgb8", 160, 120, 2, 8)
png_case("p_rgb16", 97, 41, 2, 16)
png_case("p_ga8", 77, 33, 4, 8)
png_case("p_ga16", 77, 33, 4, 16)
png_case("p_rgba8", 120, 90, 6, 8)
png_case("p_rgba16", 64, 48, 6, 16)
png_case("p_gray8_trns", 40, 30, 0, 8, trns=True)
png_case("p_rgb8_trns", 40, 30, 2, 8, trns=True)
png_case("p_stored", 50, 40, 2, 8, level=0)
png_case("p_fixed", 70, 40, 2, 8, strategy=zlib.Z_FIXED)
png_case("p_big_rgb", 1600, 1200, 2, 8, level=1)
for name, ct, depth, w, h in [
    ("p_i_rgb8", 2, 8, 160, 120),
    ("p_i_gray1", 0, 1, 37, 23),
    ("p_i_pal4", 3, 4, 45, 31),
    ("p_i_rgba16", 6, 16, 33, 19),
    ("p_i_tiny", 2, 8, 3, 2),
    ("p_i_big", 2, 8, 1024, 768),
]:
    png_case(name, w, h, ct, depth, interlace=True)

# A real-world encoder's output (ImageMagick, max compression, palette chosen
# by the encoder).
src = os.path.join(OUT, "m_src.ppm")
Image.fromarray(photo(200, 150, 21)).save(src)
subprocess.run(["magick", src, "-quality", "95", os.path.join(OUT, "p_magick.png")], check=True)
ref_from_pil(os.path.join(OUT, "p_magick.png"), "p_magick")
add("p_magick.png", "p_magick", "exact")

# ---------------------------------------------------------------- BMP


def bmp_pil(name, im):
    path = os.path.join(OUT, name + ".bmp")
    im.save(path, "BMP")
    ref_from_pil(path, name)
    add(name + ".bmp", name, "exact")


def bmp_magick(name, args, out_prefix="", kind="exact"):
    path = os.path.join(OUT, name + ".bmp")
    subprocess.run(["magick", src, *args, out_prefix + path], check=True)
    ref_from_pil(path, name)
    add(name + ".bmp", name, kind)


B = Image.fromarray(photo(83, 51, 31))
bmp_pil("b_24", B)
bmp_pil("b_8", B.quantize(256))
bmp_pil("b_1", B.convert("1"))
bmp_pil("b_32a", Image.fromarray(np.concatenate([photo(41, 23, 32), RNG.integers(0, 256, (23, 41, 1), np.uint8)], 2)))
bmp_magick("b_4", ["-colors", "16", "-type", "Palette"], "BMP3:")
bmp_magick("b_rle8", ["-colors", "200", "-type", "Palette", "-compress", "RLE"], "BMP3:")
# Pillow widens 5/6-bit fields by truncation, the decoder rounds: ±1.
bmp_magick("b_565", ["-define", "bmp:subtype=RGB565"], kind="near")
bmp_magick("b_555", ["-define", "bmp:subtype=RGB555"], kind="near")


def bmp_raw(name, w, h, top_down, core):
    """24-bit top-down (negative height) or OS/2 core 8-bit palette."""
    rgb = photo(w, h, 33)
    if core:
        pal = np.array([[i, 255 - i, (i * 7) & 255] for i in range(256)], np.uint8)
        idx = rgb[..., 0]
        stride = (w + 3) & ~3
        px = b"".join(bytes(idx[y]) + b"\0" * (stride - w) for y in range(h - 1, -1, -1))
        hdr = struct.pack("<IHHHH", 12, w, h, 1, 8)
        palb = b"".join(bytes([p[2], p[1], p[0]]) for p in pal)
        off = 14 + 12 + len(palb)
        data = b"BM" + struct.pack("<IHHI", off + len(px), 0, 0, off) + hdr + palb + px
        rgba = np.concatenate([pal[idx], np.full((h, w, 1), 255, np.uint8)], 2)
    else:
        stride = (w * 3 + 3) & ~3
        rows = range(h) if top_down else range(h - 1, -1, -1)
        px = b"".join(bytes(rgb[y][:, ::-1].reshape(-1)) + b"\0" * (stride - 3 * w) for y in rows)
        hdr = struct.pack("<IiiHHIIiiII", 40, w, -h if top_down else h, 1, 24, 0, len(px), 2835, 2835, 0, 0)
        off = 14 + 40 + 6  # a gap before the pixels
        data = b"BM" + struct.pack("<IHHI", off + len(px), 0, 0, off) + hdr + b"\xAA" * 6 + px
        rgba = np.concatenate([rgb, np.full((h, w, 1), 255, np.uint8)], 2)
    with open(os.path.join(OUT, name + ".bmp"), "wb") as f:
        f.write(data)
    write_ref(name, rgba)
    add(name + ".bmp", name, "exact")


bmp_raw("b_topdown", 45, 27, True, False)
bmp_raw("b_core8", 37, 21, False, True)


def bmp_rle(name, w, h, four, delta):
    """RLE4/RLE8 with encoded runs, absolute runs and (if `delta`) one delta
    escape; pixels skipped by the delta are palette index 0."""
    n = 16 if four else 256
    pal = np.array([[(i * 37) & 255, (i * 91) & 255, (255 - i * 13) & 255] for i in range(n)], np.uint8)
    idx = (photo(w, h, 34)[..., 1].astype(np.int32) * n // 256).astype(np.uint8)
    idx[:, w // 2 : w // 2 + 9] = 3  # a long run
    out = bytearray()
    for y in range(h - 1, -1, -1):
        row = idx[y]
        x = 0
        while x < w:
            if delta and y == h - 4 and x == 5:
                out += bytes([0, 2, 4, 0])
                idx[y, 5:9] = 0
                x = 9
                continue
            run = 1
            while x + run < w and run < 255 and row[x + run] == row[x]:
                run += 1
            if run >= 3 or w - x < 3:
                v = int(row[x])
                out += bytes([run, (v << 4) | v if four else v])
                x += run
                continue
            lit = 0
            while x + lit < w and lit < 255:
                r2 = 1
                while x + lit + r2 < w and row[x + lit + r2] == row[x + lit]:
                    r2 += 1
                if r2 >= 3:
                    break
                lit += 1
            if delta and y == h - 4 and x < 5 < x + lit:
                lit = 5 - x
            if lit < 3:
                for k in range(lit):
                    v = int(row[x + k])
                    out += bytes([1, (v << 4) if four else v])
                x += lit
                continue
            vals = [int(v) for v in row[x : x + lit]]
            if four:
                data = bytes((vals[i] << 4) | (vals[i + 1] if i + 1 < lit else 0) for i in range(0, lit, 2))
            else:
                data = bytes(vals)
            if len(data) % 2:
                data += b"\0"
            out += bytes([0, lit]) + data
            x += lit
        out += b"\0\0"
    out += b"\0\1"
    palb = b"".join(bytes([p[2], p[1], p[0], 0]) for p in pal)
    hdr = struct.pack("<IiiHHIIiiII", 40, w, h, 1, 4 if four else 8, 2 if four else 1, len(out), 0, 0, n, 0)
    off = 14 + 40 + len(palb)
    with open(os.path.join(OUT, name + ".bmp"), "wb") as f:
        f.write(b"BM" + struct.pack("<IHHI", off + len(out), 0, 0, off) + hdr + palb + out)
    write_ref(name, np.concatenate([pal[idx], np.full((h, w, 1), 255, np.uint8)], 2))
    add(name + ".bmp", name, "exact")
    if not delta and not four:
        # Cross-check the encoder. (Pillow 11.3 reads 4 bytes for the delta
        # escape instead of 2 and drops the last pixel of odd RLE4 absolute
        # runs, so those files are checked only
        # against the source.)
        check = np.asarray(Image.open(os.path.join(OUT, name + ".bmp")).convert("RGB"))
        assert (check == pal[idx]).all(), name


bmp_rle("b_rle4_own", 57, 19, True, False)
bmp_rle("b_rle8_own", 61, 17, False, False)
bmp_rle("b_rle4_delta", 57, 19, True, True)
bmp_rle("b_rle8_delta", 61, 17, False, True)
subprocess.run(["magick", os.path.join(OUT, "p_rgba8.png"), "BMP:" + os.path.join(OUT, "b_v5a.bmp")], check=True)
ref_from_pil(os.path.join(OUT, "b_v5a.bmp"), "b_v5a")
add("b_v5a.bmp", "b_v5a", "exact")

# ---------------------------------------------------------------- GIF


def gif_pil(name, im, **kw):
    path = os.path.join(OUT, name + ".gif")
    im.save(path, "GIF", **kw)
    ref_from_pil(path, name)
    add(name + ".gif", name, "exact")


G = Image.fromarray(photo(150, 100, 41))
gif_pil("g_256", G.quantize(256))
gif_pil("g_2", G.convert("1").convert("P"))
gif_pil("g_16_trans", G.quantize(16), transparency=3)
subprocess.run(["magick", src, "-colors", "64", "-interlace", "GIF", os.path.join(OUT, "g_interlaced.gif")], check=True)
ref_from_pil(os.path.join(OUT, "g_interlaced.gif"), "g_interlaced")
add("g_interlaced.gif", "g_interlaced", "exact")
subprocess.run(["magick", src, "-resize", "37x23!", "-colors", "8", os.path.join(OUT, "g_small.gif")], check=True)
ref_from_pil(os.path.join(OUT, "g_small.gif"), "g_small")
add("g_small.gif", "g_small", "exact")

# ---------------------------------------------------------------- QOI

for name, im in [("q_rgb", Image.fromarray(photo(97, 61, 51))), ("q_rgba", Image.open(os.path.join(OUT, "p_rgba8.png")))]:
    png = os.path.join(OUT, name + "_src.png")
    im.save(png)
    path = os.path.join(OUT, name + ".qoi")
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", png, path], check=True)
    os.remove(png)
    ref_from_pil(path, name)
    add(name + ".qoi", name, "exact")

os.remove(src)
with open(os.path.join(OUT, "manifest.txt"), "w") as f:
    f.write("\n".join(manifest) + "\n")
