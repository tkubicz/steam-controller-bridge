#!/usr/bin/env python3
"""Render packaging/macos/AppIcon.icns.

The generated .icns is committed, so building the app needs no Python. Run this
only when the artwork changes:

    python3 tools/make-app-icon.py

Requires Pillow and macOS's iconutil.

The controller silhouette reuses the geometry that apps/sc-bridge-menu/src/macos.rs
draws for the menu-bar template, so the Dock icon and the menu-bar icon stay the
same shape. Keep the two in sync if either changes.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

try:
    from PIL import Image, ImageDraw
except ImportError:
    sys.exit("Pillow is required: python3 -m pip install --user Pillow")

REPO = Path(__file__).resolve().parent.parent
OUTPUT = REPO / "packaging" / "macos" / "AppIcon.icns"

# Master canvas. The tile fills it edge to edge; see render().
CANVAS = 1024

# Anti-aliasing comes from drawing large and downsampling.
OVERSAMPLE = 4

# Diagonal gradient, top-left to bottom-right.
GRADIENT_FROM = (0x8B, 0x7C, 0xF6)
GRADIENT_TO = (0x43, 0x38, 0xCA)

# The controller outline from macos.rs, in its 24x18 logical space.
# ("c", ...) is a cubic with two control points; ("l", ...) a line.
CONTROLLER = [
    ("m", 5.5, 2.4),
    ("c", 3.7, 2.4, 2.3, 3.6, 1.9, 5.3),
    ("l", 0.65, 11.6),
    ("c", 0.2, 13.8, 1.3, 15.9, 3.05, 16.45),
    ("c", 4.5, 16.9, 5.5, 15.8, 6.25, 14.35),
    ("l", 7.25, 12.5),
    ("c", 7.55, 11.9, 7.95, 11.7, 8.55, 11.7),
    ("l", 9.35, 11.7),
    ("c", 9.95, 11.7, 10.35, 11.9, 10.65, 12.5),
    ("l", 11.65, 14.35),
    ("c", 12.4, 15.8, 13.4, 16.9, 14.85, 16.45),
    ("c", 16.6, 15.9, 17.7, 13.8, 17.25, 11.6),
    ("l", 16.0, 5.3),
    ("c", 15.6, 3.6, 14.2, 2.4, 12.4, 2.4),
]

# Knocked out of the silhouette so the background shows through.
D_PAD = [((5.15, 6.4), (5.15, 9.6)), ((3.55, 8.0), (6.75, 8.0))]
D_PAD_WIDTH = 1.45
BUTTONS = [(12.2, 7.1, 0.78), (14.2, 8.7, 0.78)]

ICONSET = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
]


def cubic(start, control_a, control_b, end, steps=48):
    """Flatten one cubic bezier to a list of points, excluding the start."""
    points = []
    for step in range(1, steps + 1):
        t = step / steps
        u = 1.0 - t
        points.append(
            (
                u * u * u * start[0]
                + 3 * u * u * t * control_a[0]
                + 3 * u * t * t * control_b[0]
                + t * t * t * end[0],
                u * u * u * start[1]
                + 3 * u * u * t * control_a[1]
                + 3 * u * t * t * control_b[1]
                + t * t * t * end[1],
            )
        )
    return points


def controller_polygon():
    """Flatten the controller outline into a single closed polygon."""
    points: list[tuple[float, float]] = []
    cursor = (0.0, 0.0)
    for segment in CONTROLLER:
        kind = segment[0]
        if kind == "m":
            cursor = (segment[1], segment[2])
            points.append(cursor)
        elif kind == "l":
            cursor = (segment[1], segment[2])
            points.append(cursor)
        else:
            end = (segment[5], segment[6])
            points.extend(
                cubic(cursor, (segment[1], segment[2]), (segment[3], segment[4]), end)
            )
            cursor = end
    return points


def superellipse(size, exponent=5.0, steps=720):
    """Apple-style continuous-corner square, closer than a rounded rectangle."""
    radius = size / 2.0
    points = []
    for step in range(steps):
        angle = 2.0 * 3.141592653589793 * step / steps
        cos_a = __import__("math").cos(angle)
        sin_a = __import__("math").sin(angle)
        x = radius * (abs(cos_a) ** (2.0 / exponent)) * (1 if cos_a >= 0 else -1)
        y = radius * (abs(sin_a) ** (2.0 / exponent)) * (1 if sin_a >= 0 else -1)
        points.append((radius + x, radius + y))
    return points


def gradient(size):
    """Diagonal linear gradient as an RGB image."""
    image = Image.new("RGB", (size, size))
    pixels = image.load()
    for y in range(size):
        for x in range(size):
            # Projection onto the top-left → bottom-right diagonal.
            t = (x + y) / (2.0 * (size - 1))
            pixels[x, y] = tuple(
                round(GRADIENT_FROM[c] + (GRADIENT_TO[c] - GRADIENT_FROM[c]) * t)
                for c in range(3)
            )
    return image


def glyph_fraction_for(size):
    """How much of the tile the controller fills.

    The controller collapses into a blob at 16 and 32 pixels at the proportion
    that reads well large, so the small renders get a bigger glyph. Carrying
    per-size artwork like this is the point of an iconset.
    """
    if size <= 16:
        return 0.74
    if size <= 32:
        return 0.68
    return 0.56


def render(size):
    """Render the icon at one pixel size."""
    big = size * OVERSAMPLE
    shape_px = size
    glyph_fraction = glyph_fraction_for(size)

    # The tile fills the whole canvas. macOS 26 composites app artwork into a
    # system-drawn container, so artwork that shapes and insets itself ends up
    # as a small tile floating inside that container, with the system's own
    # shape visible as a frame around it. Filling the canvas puts our edge where
    # the mask falls. Older macOS does not mask at all and draws this shape
    # as-is, which is why the superellipse is still ours to draw.
    mask = Image.new("L", (big, big), 0)
    outline = [(x * OVERSAMPLE, y * OVERSAMPLE) for x, y in superellipse(shape_px)]
    ImageDraw.Draw(mask).polygon(outline, fill=255)
    mask = mask.resize((size, size), Image.LANCZOS)

    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))

    # No drop shadow: macOS draws the container shadow, and one of ours would
    # spill past the tile edge as a grey halo.
    body = gradient(size)
    body.putalpha(mask)
    canvas.alpha_composite(body)

    # Controller silhouette, sized against the squircle rather than the canvas.
    polygon = controller_polygon()
    min_x = min(p[0] for p in polygon)
    max_x = max(p[0] for p in polygon)
    min_y = min(p[1] for p in polygon)
    max_y = max(p[1] for p in polygon)
    glyph_scale = (shape_px * glyph_fraction) / (max_x - min_x)
    offset_x = (size - (max_x - min_x) * glyph_scale) / 2.0 - min_x * glyph_scale
    offset_y = (size - (max_y - min_y) * glyph_scale) / 2.0 - min_y * glyph_scale

    def place(x, y):
        return (
            (x * glyph_scale + offset_x) * OVERSAMPLE,
            (y * glyph_scale + offset_y) * OVERSAMPLE,
        )

    glyph = Image.new("L", (big, big), 0)
    pen = ImageDraw.Draw(glyph)
    pen.polygon([place(x, y) for x, y in polygon], fill=255)

    # Below roughly 32px the cut-outs collapse into noise, so the silhouette
    # alone reads better than a muddy one.
    if size >= 32:
        stroke = D_PAD_WIDTH * glyph_scale * OVERSAMPLE
        for (x1, y1), (x2, y2) in D_PAD:
            start, end = place(x1, y1), place(x2, y2)
            pen.line([start, end], fill=0, width=round(stroke))
            # Round caps, which ImageDraw.line does not provide.
            for cx, cy in (start, end):
                pen.ellipse(
                    [cx - stroke / 2, cy - stroke / 2, cx + stroke / 2, cy + stroke / 2],
                    fill=0,
                )
        for x, y, radius in BUTTONS:
            cx, cy = place(x, y)
            r = radius * glyph_scale * OVERSAMPLE
            pen.ellipse([cx - r, cy - r, cx + r, cy + r], fill=0)

    glyph = glyph.resize((size, size), Image.LANCZOS)
    white = Image.new("RGBA", (size, size), (255, 255, 255, 255))
    white.putalpha(glyph)
    canvas.alpha_composite(white)
    return canvas


def main():
    if not shutil.which("iconutil"):
        sys.exit("iconutil not found; this script requires macOS")

    with tempfile.TemporaryDirectory() as work:
        iconset = Path(work) / "AppIcon.iconset"
        iconset.mkdir()
        cache: dict[int, Image.Image] = {}
        for name, size in ICONSET:
            if size not in cache:
                cache[size] = render(size)
                print(f"  rendered {size}x{size}")
            cache[size].save(iconset / name)

        OUTPUT.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            ["iconutil", "--convert", "icns", "--output", str(OUTPUT), str(iconset)],
            check=True,
        )

    print(f"wrote {OUTPUT.relative_to(REPO)} ({OUTPUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
