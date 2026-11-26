#!/usr/bin/env python3
"""Generate the 24 x 24 two-bit app icons (05 §6.2) as Rust source.

    python3 crates/orpheus-apps/icongen/icons.py crates/orpheus-apps/src/icons_gen.rs [preview.png]

Each icon is drawn with Pillow at 8x scale (192 x 192, strokes 16 px = 2 px
at 1x, round caps), box-filtered down to 24 x 24 and quantised to four
coverage levels. Style: 2 px outlines, filled accents, 2 px margin, optical
centre at (12, 12). Original drawings for this project (MIT OR Apache-2.0).
"""

import math
import sys

from PIL import Image, ImageDraw

S = 8
N = 24 * S
W = 2 * S  # stroke


def canvas():
    im = Image.new("L", (N, N), 0)
    return im, ImageDraw.Draw(im)


def p(x, y):
    return (x * S, y * S)


def line(d, pts, w=W):
    pts = [p(*q) for q in pts]
    d.line(pts, fill=255, width=w, joint="curve")
    for q in (pts[0], pts[-1]):
        d.ellipse([q[0] - w / 2, q[1] - w / 2, q[0] + w / 2, q[1] + w / 2], fill=255)


def rrect(d, x0, y0, x1, y1, r, fill=False, w=W):
    box = [x0 * S, y0 * S, x1 * S, y1 * S]
    if fill:
        d.rounded_rectangle(box, radius=r * S, fill=255)
    else:
        d.rounded_rectangle(box, radius=r * S, outline=255, width=w)


def circle(d, cx, cy, r, fill=False, w=W):
    box = [(cx - r) * S, (cy - r) * S, (cx + r) * S, (cy + r) * S]
    if fill:
        d.ellipse(box, fill=255)
    else:
        d.ellipse(box, outline=255, width=w)


def music():
    im, d = canvas()
    # Two beamed quavers.
    line(d, [(9, 17), (9, 5)])
    line(d, [(19, 15), (19, 3)])
    d.polygon([p(8, 4), p(20, 1.5), p(20, 6), p(8, 8.5)], fill=255)
    d.ellipse([p(3.5, 14.5)[0], p(3.5, 14.5)[1], p(10, 20)[0], p(10, 20)[1]], fill=255)
    d.ellipse([p(13.5, 12.5)[0], p(13.5, 12.5)[1], p(20, 18)[0], p(20, 18)[1]], fill=255)
    return im


def lyre():
    """The Orpheus logo: a lyre (sound box, two curved arms with outward
    horn tips, crossbar, three strings)."""
    im, d = canvas()
    rrect(d, 6, 17.5, 18, 21, 1.5, fill=True)
    line(d, [(7.5, 18), (5.2, 14.5), (4.2, 10.5), (4.8, 7), (6.6, 4.6), (5.0, 2.6)])
    line(d, [(16.5, 18), (18.8, 14.5), (19.8, 10.5), (19.2, 7), (17.4, 4.6), (19.0, 2.6)])
    line(d, [(6.2, 5.4), (17.8, 5.4)])
    for x in (9.6, 12, 14.4):
        line(d, [(x, 5.6), (x, 17.6)], w=S)
    return im


def files():
    im, d = canvas()
    line(d, [(3, 19.5), (3, 4.5), (9, 4.5), (11, 7), (21, 7), (21, 19.5), (3, 19.5)])
    line(d, [(3, 11), (21, 11)])
    return im


def radio():
    im, d = canvas()
    line(d, [(6, 7.5), (17, 2.5)])
    rrect(d, 2, 7, 22, 21, 2.5)
    circle(d, 15.5, 14, 3.5)
    for y in (11, 14, 17):
        line(d, [(6, y), (8.5, y)], w=int(1.6 * S))
    return im


def bluetooth():
    im, d = canvas()
    line(d, [(6.5, 7.5), (17, 16), (12, 21), (12, 3), (17, 8), (6.5, 16.5)])
    return im


def recorder():
    im, d = canvas()
    rrect(d, 8.5, 2, 15.5, 14, 3.5, fill=True)
    d.arc([p(5, 6)[0], p(5, 6)[1], p(19, 17)[0], p(19, 17)[1]], 0, 180, fill=255, width=W)
    line(d, [(12, 17), (12, 21)])
    line(d, [(8, 21.5), (16, 21.5)])
    return im


def pictures():
    im, d = canvas()
    rrect(d, 2, 4, 22, 20, 2)
    d.polygon([p(4, 18.5), p(9.5, 11), p(13.5, 16), p(16, 13.5), p(20, 18.5)], fill=255)
    circle(d, 16.5, 8.5, 2, fill=True)
    return im


def steps():
    im, d = canvas()
    # Two shoe prints, offset.
    for (cx, cy) in ((8, 9), (16, 14)):
        d.ellipse([p(cx - 3, cy - 6)[0], p(cx - 3, cy - 6)[1], p(cx + 3, cy + 2)[0], p(cx + 3, cy + 2)[1]], fill=255)
        d.ellipse([p(cx - 2.3, cy + 3)[0], p(cx - 2.3, cy + 3)[1], p(cx + 2.3, cy + 7)[0], p(cx + 2.3, cy + 7)[1]], fill=255)
    return im


def stopwatch():
    im, d = canvas()
    circle(d, 12, 13.5, 8)
    line(d, [(10, 2.5), (14, 2.5)])
    line(d, [(12, 3), (12, 5)])
    line(d, [(18.5, 5.5), (20, 7)])
    line(d, [(12, 13.5), (12, 9)])
    circle(d, 12, 13.5, 1.6, fill=True)
    return im


def authenticator():
    im, d = canvas()
    # Shield with a keyhole.
    pts = [(12, 2), (20.5, 5), (20.5, 11)]
    for i in range(1, 11):
        t = i / 10
        pts.append((20.5 - 8.5 * t * t, 11 + 10.5 * t))
    pts_l = [(24 - x, y) for (x, y) in reversed(pts[1:])]
    poly = pts + pts_l[1:]
    d.polygon([p(x, y) for (x, y) in poly], fill=255)
    hole = Image.new("L", (N, N), 0)
    hd = ImageDraw.Draw(hole)
    hd.ellipse([p(9.8, 7.5)[0], p(9.8, 7.5)[1], p(14.2, 11.9)[0], p(14.2, 11.9)[1]], fill=255)
    hd.polygon([p(10.8, 10.5), p(13.2, 10.5), p(14, 16.5), p(10, 16.5)], fill=255)
    im.paste(0, (0, 0), hole)
    return im


def usb_drive():
    im, d = canvas()
    # Thumb drive, diagonal like a status glyph is too small; upright.
    rrect(d, 6, 9, 18, 22, 2.5, fill=True)
    rrect(d, 8, 2, 16, 9.5, 1)
    for x in (10.3, 13.7):
        d.rectangle([p(x - 0.9, 4.5)[0], p(x - 0.9, 4.5)[1], p(x + 0.9, 6.5)[0], p(x + 0.9, 6.5)[1]], fill=255)
    hole = Image.new("L", (N, N), 0)
    ImageDraw.Draw(hole).ellipse([p(10, 14)[0], p(10, 14)[1], p(14, 18)[0], p(14, 18)[1]], fill=255)
    im.paste(0, (0, 0), hole)
    return im


def settings():
    im, d = canvas()
    cx, cy = 12, 12
    teeth = 8
    pts = []
    for i in range(teeth * 4):
        a = 2 * math.pi * i / (teeth * 4)
        r = 10 if (i % 4) in (1, 2) else 7.6
        pts.append((cx + r * math.cos(a + math.pi / teeth / 4), cy + r * math.sin(a + math.pi / teeth / 4)))
    d.polygon([p(x, y) for (x, y) in pts], fill=255)
    hole = Image.new("L", (N, N), 0)
    ImageDraw.Draw(hole).ellipse([p(8.6, 8.6)[0], p(8.6, 8.6)[1], p(15.4, 15.4)[0], p(15.4, 15.4)[1]], fill=255)
    im.paste(0, (0, 0), hole)
    return im


def games():
    """A hand-held game pad: rounded body, cross keys left, two buttons
    right."""
    im, d = canvas()
    rrect(d, 1.5, 5, 22.5, 19.5, 6)
    # Cross keys.
    line(d, [(7.5, 9), (7.5, 15.5)], w=S * 2)
    line(d, [(4.3, 12.25), (10.7, 12.25)], w=S * 2)
    # Buttons.
    circle(d, 15.2, 14.3, 1.9, fill=True)
    circle(d, 18.4, 10.4, 1.9, fill=True)
    return im


def books():
    """An open book: two pages from a spine, lines of text on each."""
    im, d = canvas()
    line(d, [(12, 6.5), (12, 20)])
    line(d, [(12, 6.5), (8, 4.6), (2.5, 4.6), (2.5, 18.2), (8, 18.2), (12, 20)])
    line(d, [(12, 6.5), (16, 4.6), (21.5, 4.6), (21.5, 18.2), (16, 18.2), (12, 20)])
    for y in (8.6, 11.6, 14.6):
        line(d, [(5.4, y), (9.2, y)], w=S)
        line(d, [(14.8, y), (18.6, y)], w=S)
    return im


def warning():
    im, d = canvas()
    d.polygon([p(12, 2), p(22.5, 21), p(1.5, 21)], fill=255)
    hole = Image.new("L", (N, N), 0)
    hd = ImageDraw.Draw(hole)
    hd.polygon([p(11, 8), p(13, 8), p(12.6, 15), p(11.4, 15)], fill=255)
    hd.ellipse([p(10.9, 16.3)[0], p(10.9, 16.3)[1], p(13.1, 18.5)[0], p(13.1, 18.5)[1]], fill=255)
    im.paste(0, (0, 0), hole)
    return im


def card():
    im, d = canvas()
    d.polygon([p(6, 2), p(16, 2), p(20, 6), p(20, 22), p(4, 22), p(4, 4)], outline=None, fill=255)
    hole = Image.new("L", (N, N), 0)
    hd = ImageDraw.Draw(hole)
    hd.polygon([p(6.2, 4), p(15.2, 4), p(18, 6.8), p(18, 20), p(6, 20), p(6, 4.2)], fill=255)
    im.paste(0, (0, 0), hole)
    for x in (8.5, 11.5, 14.5):
        d.rectangle([p(x - 0.8, 4.5)[0], p(x - 0.8, 4.5)[1], p(x + 0.8, 8)[0], p(x + 0.8, 8)[1]], fill=255)
    return im


def usb_plug():
    im, d = canvas()
    # The USB trident.
    line(d, [(12, 21), (12, 5)])
    d.polygon([p(12, 1.5), p(14.5, 5.5), p(9.5, 5.5)], fill=255)
    line(d, [(12, 17), (6.5, 12), (6.5, 9.5)])
    line(d, [(12, 14.5), (17.5, 10.5), (17.5, 8.5)])
    circle(d, 6.5, 8.5, 1.8, fill=True)
    d.rectangle([p(16, 6.2)[0], p(16, 6.2)[1], p(19, 9.2)[0], p(19, 9.2)[1]], fill=255)
    circle(d, 12, 20.5, 2.2, fill=True)
    return im


def power():
    im, d = canvas()
    d.arc([p(4, 4.5)[0], p(4, 4.5)[1], p(20, 20.5)[0], p(20, 20.5)[1]], -50, 230, fill=255, width=W)
    line(d, [(12, 2.5), (12, 11)])
    return im


ICONS = [
    ("MUSIC", music),
    ("FILES", files),
    ("RADIO", radio),
    ("BLUETOOTH", bluetooth),
    ("RECORDER", recorder),
    ("PICTURES", pictures),
    ("STEPS", steps),
    ("STOPWATCH", stopwatch),
    ("AUTHENTICATOR", authenticator),
    ("USB_DRIVE", usb_drive),
    ("SETTINGS", settings),
    ("GAMES", games),
    ("WARNING", warning),
    ("CARD", card),
    ("USB", usb_plug),
    ("POWER", power),
    ("LYRE", lyre),
    ("BOOKS", books),
]


def quantise(im):
    small = im.resize((24, 24), Image.BOX)
    return [min(3, (small.getpixel((x, y)) * 3 + 127) // 255) for y in range(24) for x in range(24)]


def pack(levels):
    out = []
    for i in range(0, len(levels), 4):
        b = 0
        for v in levels[i:i + 4]:
            b = (b << 2) | v
        out.append(b)
    return out


def main():
    out = sys.argv[1]
    lines = ["// Generated by crates/orpheus-apps/icongen/icons.py. Do not edit.",
             "// 24 x 24, 2 bits per pixel; original drawings, MIT OR Apache-2.0.", "",
             "use orpheus_ui::icons::Icon2;", ""]
    previews = []
    for name, f in ICONS:
        lv = quantise(f())
        previews.append(lv)
        data = ", ".join(f"0x{b:02X}" for b in pack(lv))
        lines.append(f"pub static {name}: Icon2 = Icon2 {{ w: 24, h: 24, data: &[{data}] }};")
    with open(out, "w") as fh:
        fh.write("\n".join(lines) + "\n")
    if len(sys.argv) > 2:
        cols = len(previews)
        sheet = Image.new("RGB", (cols * 28 + 4, 2 * 28 + 4), (20, 22, 28))
        for i, lv in enumerate(previews):
            for row, (bg, fg) in enumerate((((20, 22, 28), (238, 240, 244)), ((32, 110, 220), (255, 255, 255)))):
                ox, oy = 4 + i * 28, 4 + row * 28
                for y in range(24):
                    for x in range(24):
                        a = lv[y * 24 + x] / 3
                        c = tuple(int(bg[k] * (1 - a) + fg[k] * a) for k in range(3))
                        sheet.putpixel((ox + x, oy + y), c)
        sheet = sheet.resize((sheet.width * 4, sheet.height * 4), Image.NEAREST)
        sheet.save(sys.argv[2])


if __name__ == "__main__":
    main()
