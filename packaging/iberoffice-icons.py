#!/usr/bin/env python3
"""IberOffice app icons (the "azulejo" tile): one source for every icon file of the five apps.

The tile is an ivory rounded square with a quarter circle in each corner, like a glazed wall
tile, and the app's glyph in the middle. The suite mark carries all four office app colours.
Original artwork for IberOffice, MIT OR Apache-2.0 like the code.

    python3 packaging/iberoffice-icons.py preview DIR            # every icon, every size, into DIR
    python3 packaging/iberoffice-icons.py apply REPO KIND BASE   # rewrite REPO/assets/app-icon

KIND is pdf, texto, calculo, presenta, correo or suite. Needs cairosvg and Pillow.
"""
import io
import os
import struct
import sys

import cairosvg
from PIL import Image

IVORY = "#F3EDE1"
EDGE = "#D8CBB2"
INK = "#1E2547"
COLORS = {"pdf": "#B03A2E", "texto": "#2D4BA0", "calculo": "#4F7A28", "presenta": "#C7780F", "correo": "#1F7A8C"}
SLUG = {"suite": "iberoffice", "pdf": "iber-pdf", "texto": "iber-texto", "calculo": "iber-calculo", "presenta": "iber-presenta", "correo": "iber-correo"}
SMALL_MAX = 32  # sizes up to this use the simplified tile


def glyph(kind, small):
    c = COLORS.get(kind, INK)
    if kind == "suite":
        if small:
            return f'<circle cx="50" cy="33" r="7.5" fill="{c}"/><rect x="43" y="45" width="14" height="25" rx="2.5" fill="{c}"/>'
        return f'<circle cx="50" cy="37" r="5.5" fill="{c}"/><rect x="45" y="47" width="10" height="21" rx="2" fill="{c}"/>'
    if kind == "pdf":
        if small:
            return f'<path d="M35 31 L54 31 L65 42 L65 69 L35 69 Z" fill="{c}"/>'
        return f'<path d="M39 35 L53 35 L61 43 L61 65 L39 65 Z" fill="{c}"/><path d="M53 35 L53 43 L61 43 Z" fill="#E7B3AB"/>'
    if kind == "texto":
        if small:
            return "".join(f'<rect x="34" y="{y}" width="{w}" height="8" rx="2" fill="{c}"/>' for y, w in [(33, 32), (46, 32), (59, 21)])
        return "".join(f'<rect x="37" y="{y}" width="{w}" height="4.5" rx="2.25" fill="{c}"/>' for y, w in [(37, 26), (45, 26), (53, 26), (61, 16)])
    if kind == "calculo":
        if small:
            return "".join(f'<rect x="{x}" y="{y}" width="13" height="13" rx="2" fill="{c}"/>' for y in (35, 52) for x in (35, 52))
        return (f'<rect x="37.5" y="37.5" width="25" height="25" rx="3" fill="none" stroke="{c}" stroke-width="3"/>'
                f'<path d="M37.5 46 H62.5 M37.5 54 H62.5 M46 37.5 V62.5 M54 37.5 V62.5" stroke="{c}" stroke-width="2.5"/>'
                f'<rect x="37.5" y="37.5" width="25" height="8.5" fill="{c}"/>')
    if kind == "presenta":
        if small:
            return f'<path d="M40 33 L64 50 L40 67 Z" fill="{c}" stroke="{c}" stroke-width="4" stroke-linejoin="round"/>'
        return f'<path d="M44 39 L60 50 L44 61 Z" fill="{c}" stroke="{c}" stroke-width="3" stroke-linejoin="round"/>'
    if kind == "correo":
        if small:
            return (f'<rect x="32" y="35" width="36" height="30" rx="3" fill="{c}"/>'
                    f'<path d="M34 38 L50 52 L66 38" fill="none" stroke="{IVORY}" stroke-width="4" stroke-linejoin="round" stroke-linecap="round"/>')
        return (f'<rect x="36" y="39" width="28" height="22" rx="2.5" fill="{c}"/>'
                f'<path d="M38 41.5 L50 51 L62 41.5" fill="none" stroke="{IVORY}" stroke-width="2.4" stroke-linejoin="round" stroke-linecap="round"/>')
    raise ValueError(kind)


def icon(kind, small=False, uid=None, standalone=True):
    """The full-bleed tile in a 512 viewBox (rx 112, like the upstream masters).

    `small` is the variant for 32 px and below: smaller corners and a bolder glyph."""
    uid = uid or f"{kind}{'-s' if small else ''}"
    fill = [COLORS["pdf"], COLORS["texto"], COLORS["calculo"], COLORS["presenta"]] if kind == "suite" else [COLORS[kind]] * 4
    r = 22 if small else 32
    k = 1.15 if small else 1.25
    rx = 112 / 5.12
    dots = "".join(f'<circle cx="{x}" cy="{y}" r="{r}" fill="{c}"/>' for (x, y), c in zip([(0, 0), (100, 0), (0, 100), (100, 100)], fill))
    ns = ' xmlns="http://www.w3.org/2000/svg"' if standalone else ""
    return (f'<svg{ns} viewBox="0 0 512 512"><g transform="scale(5.12)">'
            f'<defs><clipPath id="c-{uid}"><rect width="100" height="100" rx="{rx:.3f}"/></clipPath></defs>'
            f'<rect width="100" height="100" rx="{rx:.3f}" fill="{IVORY}"/>'
            f'<g clip-path="url(#c-{uid})">{dots}</g>'
            f'<rect x="0.75" y="0.75" width="98.5" height="98.5" rx="{rx - 0.75:.3f}" fill="none" stroke="{EDGE}" stroke-width="1.5"/>'
            f'<g transform="translate(50 50) scale({k}) translate(-50 -50)">{glyph(kind, small)}</g></g></svg>')


def render(svg, size):
    return Image.open(io.BytesIO(cairosvg.svg2png(bytestring=svg.encode(), output_width=size, output_height=size))).convert("RGBA")


def tile(kind, size):
    """Full bleed (Windows, Linux), the simplified tile at small sizes."""
    return render(icon(kind, small=size <= SMALL_MAX), size)


def mac(kind, size):
    """On Apple's grid: an 824/1024 body centred on a transparent canvas."""
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    body = round(size * 824 / 1024)
    off = (size - body) // 2
    canvas.alpha_composite(tile(kind, body), (off, off))
    return canvas


def png_bytes(im):
    buf = io.BytesIO()
    im.save(buf, "PNG")
    return buf.getvalue()


def write_ico(path, kind):
    """A Windows .ico of PNG-compressed entries, 16-256 px (Vista and later read these)."""
    sizes = (16, 20, 24, 32, 40, 48, 64, 128, 256)
    blobs = [png_bytes(tile(kind, s)) for s in sizes]
    head = struct.pack("<HHH", 0, 1, len(blobs))
    entries, offset = b"", 6 + 16 * len(blobs)
    for s, b in zip(sizes, blobs):
        entries += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(b), offset)
        offset += len(b)
    with open(path, "wb") as f:
        f.write(head + entries + b"".join(blobs))


def apply(repo, kind, base):
    """Rewrite the app icon files that REPO/assets/app-icon already has, keeping their names."""
    d = os.path.join(repo, "assets", "app-icon")
    app_id = os.environ.get("ICON_APP_ID", f"ai.storyteller.{base}")
    done = []

    def out(rel, write):
        path = os.path.join(d, rel)
        if os.path.exists(path):
            write(path)
            done.append(rel)

    def text(content):
        return lambda p: open(p, "w").write(content + "\n")

    out(f"{base}.svg", text(icon(kind)))
    out(f"{base}-small.svg", text(icon(kind, small=True)))
    # PdfCraft's 1024 PNG sits on Apple's grid; the other apps' 1024 PNG is full bleed.
    out(f"{base}-1024.png", lambda p: (mac if base == "pdfcraft" else tile)(kind, 1024).save(p))
    out(f"{base}-macos-512.png", lambda p: mac(kind, 512).save(p))
    out(f"{base}.icns", lambda p: mac(kind, 1024).save(p))
    out(f"{base}.ico", lambda p: write_ico(p, kind))
    for s in (16, 24, 32, 48, 64, 128, 256, 512):
        out(f"hicolor/{s}x{s}/apps/{app_id}.png", lambda p, s=s: tile(kind, s).save(p))
    out(f"hicolor/scalable/apps/{app_id}.svg", text(icon(kind)))
    return done


def preview(out_dir):
    for kind, slug in SLUG.items():
        d = os.path.join(out_dir, slug)
        os.makedirs(d, exist_ok=True)
        open(os.path.join(d, f"{slug}.svg"), "w").write(icon(kind) + "\n")
        open(os.path.join(d, f"{slug}-small.svg"), "w").write(icon(kind, small=True) + "\n")
        for s in (16, 24, 32, 48, 64, 128, 256, 512, 1024):
            tile(kind, s).save(os.path.join(d, f"{slug}-{s}.png"))
        write_ico(os.path.join(d, f"{slug}.ico"), kind)
        mac(kind, 1024).save(os.path.join(d, f"{slug}.icns"))


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "preview":
        preview(sys.argv[2])
    elif len(sys.argv) == 5 and sys.argv[1] == "apply":
        for rel in apply(sys.argv[2], sys.argv[3], sys.argv[4]):
            print("wrote", rel)
    else:
        sys.exit(__doc__)
