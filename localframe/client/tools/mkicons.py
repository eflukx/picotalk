#!/usr/bin/env python3
"""Generates the Finder resources of the Mac applications, as Rez sources:
src/lftest.r, src/teletekst.r and src/tanks.r, each with the application's
ICN# (32 x 32 icon and mask), ics# (16 x 16), FREF, BNDL and signature
resource, and a SIZE resource asking for far less memory than Retro68's
default of 1 MB, so the applications start on a 1 MB Mac. The icons are drawn here with Pillow; the masks are the icons'
silhouettes. With --preview FILE, it also writes a PNG of all of them.

    python3 tools/mkicons.py [--preview icons.png]

The Finder shows these only when the file's bundle bit is set; the build
sets it on the disk images (hattrib -b).
"""
import os
import sys

from PIL import Image, ImageDraw

# Memory partition, in KB: preferred and minimum. About 72 KB of code and
# at most a few tens of KB of data (Tanks: a 20 KB offscreen copy of the
# screen and two snapshots).
SIZE_KB = (384, 256)

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "..", "src")

GREY = [[(x + y) % 2 == 0 for x in range(32)] for y in range(32)]  # 50 % dither


def canvas(size=32):
    img = Image.new("1", (size, size), 0)  # 1 = ink
    return img, ImageDraw.Draw(img)


def dither(img, box, every=2):
    """Ink every other pixel inside box (x0, y0, x1, y1), inclusive."""
    x0, y0, x1, y1 = box
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            if (x + y) % every == 0:
                img.putpixel((x, y), 1)


def compact_mac(img, d, x, y):
    """A compact Mac, 13 x 17, top left at (x, y)."""
    d.rectangle((x, y, x + 12, y + 14), outline=1, fill=0)    # case
    d.rectangle((x + 2, y + 2, x + 10, y + 8), outline=1)     # screen
    d.line((x + 6, y + 11, x + 9, y + 11), fill=1)            # floppy slot
    d.rectangle((x + 1, y + 15, x + 11, y + 16), outline=1)   # foot


def lftest():
    img, d = canvas()
    compact_mac(img, d, 1, 8)
    compact_mac(img, d, 18, 8)
    # what is on their screens: a ping going out, one coming back
    d.point([(5, 13), (7, 13), (9, 13)], fill=1)
    d.line((22, 13, 26, 13), fill=1)
    # LocalTalk between them, over the top
    d.line((7, 7, 7, 3), fill=1)
    d.line((7, 3, 24, 3), fill=1)
    d.line((24, 3, 24, 7), fill=1)
    d.rectangle((14, 1, 17, 5), outline=1, fill=1)            # the connector box
    # the ping: an arrow left to right below
    d.line((5, 29, 26, 29), fill=1)
    d.line((23, 27, 26, 29), fill=1)
    d.line((23, 31, 26, 29), fill=1)
    return img


def teletekst():
    img, d = canvas()
    d.line((10, 0, 15, 5), fill=1)                            # antenna
    d.line((21, 0, 16, 5), fill=1)
    d.rounded_rectangle((0, 5, 31, 28), radius=4, outline=1, fill=0)
    d.rectangle((3, 8, 28, 25), outline=1, fill=1)            # the screen, black
    # the page: a header, a block-graphics logo, lines of text, fastext
    for x in range(15, 27, 2):
        img.putpixel((x, 10), 0)
    d.rectangle((5, 12, 26, 14), fill=0)
    for x in (8, 12, 16, 20, 24):
        img.putpixel((x, 13), 1)
    for y in (17, 19):
        d.line((5, y, 20 if y == 17 else 24, y), fill=0)
        img.putpixel((26, y), 0)
    for i in range(4):                                        # the colour keys
        x = 5 + i * 6
        if i % 2:
            d.rectangle((x, 22, x + 4, 23), fill=0)
        else:
            img.putpixel((x, 22), 0)
            img.putpixel((x + 2, 22), 0)
            img.putpixel((x + 4, 22), 0)
            img.putpixel((x + 1, 23), 0)
            img.putpixel((x + 3, 23), 0)
    d.rectangle((6, 29, 9, 30), fill=1)                       # feet
    d.rectangle((22, 29, 25, 30), fill=1)
    return img


def cylinder(img, d, x, w, top, bottom, level):
    """A tank from top to bottom, filled to level (0..1)."""
    d.rectangle((x, top + 2, x + w - 1, bottom - 2), fill=0)
    d.ellipse((x, bottom - 4, x + w - 1, bottom), outline=1, fill=0)
    surface = round(bottom - 2 - (bottom - top - 4) * level)
    dither(img, (x + 1, surface, x + w - 2, bottom - 2))      # the liquid
    d.line((x + 1, surface, x + w - 2, surface), fill=1)
    d.line((x, top + 2, x, bottom - 2), fill=1)
    d.line((x + w - 1, top + 2, x + w - 1, bottom - 2), fill=1)
    d.ellipse((x, top, x + w - 1, top + 4), outline=1, fill=0)


def tanks():
    img, d = canvas()
    cylinder(img, d, 0, 10, 6, 30, 0.7)
    cylinder(img, d, 11, 10, 2, 30, 0.9)
    cylinder(img, d, 22, 10, 10, 30, 0.45)
    return img


def small(img):
    """The 16 x 16 version: the 32 x 32 one halved, any ink counts."""
    out = Image.new("1", (16, 16), 0)
    for y in range(16):
        for x in range(16):
            px = [img.getpixel((2 * x + dx, 2 * y + dy)) for dx in (0, 1) for dy in (0, 1)]
            out.putpixel((x, y), 1 if sum(px) >= 2 else 0)
    return out


def mask(img):
    """The silhouette: everything not reachable from the border through
    white pixels."""
    n = img.size[0]
    outside = set()
    todo = [(x, y) for x in range(n) for y in (0, n - 1)] + [(x, y) for y in range(n) for x in (0, n - 1)]
    while todo:
        x, y = todo.pop()
        if (x, y) in outside or not (0 <= x < n and 0 <= y < n) or img.getpixel((x, y)):
            continue
        outside.add((x, y))
        todo += [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]
    m = Image.new("1", (n, n), 0)
    for y in range(n):
        for x in range(n):
            if (x, y) not in outside:
                m.putpixel((x, y), 1)
    return m


def hexrows(img):
    n = img.size[0]
    out = []
    for y in range(n):
        v = 0
        for x in range(n):
            v = v << 1 | (1 if img.getpixel((x, y)) else 0)
        out.append("%0*X" % (n // 4, v))
    return out


def rez(name, creator, img):
    sm = small(img)
    lines = [
        "/* Generated by tools/mkicons.py. Do not edit. */",
        '#include "Multiverse.r"',
        "",
        "resource 'ICN#' (128) {",
        "    {",
    ]
    for part in (img, mask(img)):
        rows = hexrows(part)
        lines.append("        $\"" + "\"\n        $\"".join(" ".join(rows[i:i + 2]) for i in range(0, 32, 2)) + "\",")
    lines[-1] = lines[-1].rstrip(",")
    lines += ["    }", "};", "", "resource 'ics#' (128) {", "    {"]
    for part in (sm, mask(sm)):
        rows = hexrows(part)
        lines.append("        $\"" + "\"\n        $\"".join(" ".join(rows[i:i + 4]) for i in range(0, 16, 4)) + "\",")
    lines[-1] = lines[-1].rstrip(",")
    lines += [
        "    }",
        "};",
        "",
        "resource 'FREF' (128) { 'APPL', 0, \"\" };",
        "",
        "resource 'BNDL' (128) {",
        "    '%s', 0," % creator,
        "    {",
        "        'ICN#', { 0, 128 },",
        "        'FREF', { 0, 128 }",
        "    }",
        "};",
        "",
        "/* Memory: replaces Retro68's default of 1 MB. */",
        "resource 'SIZE' (-1) {",
        "    reserved, ignoreSuspendResumeEvents, reserved, cannotBackground,",
        "    needsActivateOnFGSwitch, backgroundAndForeground, dontGetFrontClicks,",
        "    ignoreChildDiedEvents, is32BitCompatible, notHighLevelEventAware,",
        "    onlyLocalHLEvents, notStationeryAware, dontUseTextEditServices,",
        "    reserved, reserved, reserved,",
        "    %d * 1024, /* preferred */" % SIZE_KB[0],
        "    %d * 1024  /* minimum */" % SIZE_KB[1],
        "};",
        "",
        "/* The signature (owner) resource. */",
        "data '%s' (0, \"Owner resource\") { $\"%02X\" $\"%s\" };" % (creator, len(name), name.encode().hex().upper()),
        "",
    ]
    return "\n".join(lines)


APPS = [("LFTest", "LFts", "lftest", lftest), ("Teletekst", "LFtt", "teletekst", teletekst), ("Tanks", "LFtk", "tanks", tanks)]

if __name__ == "__main__":
    icons = []
    for name, creator, base, draw in APPS:
        img = draw()
        icons.append(img)
        with open(os.path.join(SRC, base + ".r"), "w") as f:
            f.write(rez(name, creator, img))
    if "--preview" in sys.argv:
        path = sys.argv[sys.argv.index("--preview") + 1]
        sheet = Image.new("L", (3 * 48 + 3 * 24, 40), 255)
        for i, img in enumerate(icons):
            sheet.paste(img.point(lambda v: 0 if v else 255).convert("L"), (8 + i * 48, 4))
            sheet.paste(small(img).point(lambda v: 0 if v else 255).convert("L"), (3 * 48 + 4 + i * 24, 12))
        sheet.resize((sheet.size[0] * 6, sheet.size[1] * 6), Image.NEAREST).save(path)
