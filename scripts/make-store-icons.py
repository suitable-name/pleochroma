#!/usr/bin/env python3
"""Generates the store visual assets: the MSIX logos for pleochroma-cut and the Google Play
icon and feature graphic for the Android app.

Run from the workspace root (or anywhere; writes to ./msix/assets and ./play/assets by
default, or pass explicit output paths as the first and second argument):

    python scripts/make-store-icons.py [msix_out_dir] [play_out_dir]

Writes:
    msix/assets/StoreLogo.png        (50x50 px   -- AppxManifest Properties/Logo)
    msix/assets/Square44x44Logo.png  (44x44 px   -- Taskbar, Start list, file associations)
    msix/assets/Square71x71Logo.png  (71x71 px   -- Start menu small tile)
    msix/assets/Square150x150Logo.png(150x150 px -- Start menu medium tile)
    msix/assets/StoreListing300.png  (300x300 px -- Partner Center Store listing upload)
    play/assets/play-icon-512.png    (512x512 px -- Google Play "App icon": full-bleed square,
                                      opaque PNG; Play applies its own rounded mask)
    play/assets/play-feature-graphic-1024x500.png
                                     (1024x500 px -- Google Play "Feature graphic": opaque
                                      24-bit PNG, the gem with the name and a tag line)
"""

import math
import os
import sys

from PIL import Image, ImageDraw, ImageFont

SUPERSAMPLE = 8
FACET_DETAIL_MIN_PX = 32

GROUND = (15, 21, 35, 255)  # #0f1523 -- the app's own darkest surface
ACCENT_CUT = "#38bdf8"      # pleochroma-cut (viewer, cyan)

# Required MSIX manifest logos + 300x300 Store listing icon
MSIX_ASSETS = {
    "StoreLogo.png": 50,
    "Square44x44Logo.png": 44,
    "Square71x71Logo.png": 71,
    "Square150x150Logo.png": 150,
    "StoreListing300.png": 300,
    "StoreListing500.png": 500,
    "StoreListing512.png": 512,
}

# Google Play listing graphics (Play Console > Main store listing > Graphics)
PLAY_ICON = ("play-icon-512.png", 512)
PLAY_FEATURE = ("play-feature-graphic-1024x500.png", (1024, 500))
PLAY_NAME = "Pleochroma"
PLAY_TAGLINE = ["Design, render and cut", "faceted gemstones"]

# Bold and regular sans fonts tried in order (Windows, Linux, macOS); the first that exists is used.
FONT_BOLD = [
    "C:/Windows/Fonts/segoeuib.ttf",
    "C:/Windows/Fonts/arialbd.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
    "/Library/Fonts/Arial Bold.ttf",
    "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
]
FONT_REGULAR = [
    "C:/Windows/Fonts/segoeui.ttf",
    "C:/Windows/Fonts/arial.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/Library/Fonts/Arial.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
]


def _lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def _rgb(hex_str):
    h = hex_str.lstrip("#")
    return tuple(int(h[i : i + 2], 16) for i in (0, 2, 4))


def _ring(cx, cy, r, n, phase=0.0):
    """`n` points evenly spaced on a circle, starting at `phase` radians."""
    return [
        (
            cx + r * math.cos(phase + i * 2 * math.pi / n),
            cy + r * math.sin(phase + i * 2 * math.pi / n),
        )
        for i in range(n)
    ]


def draw_gem(size, accent_hex, facets=True, rounded=True, pad_frac=0.06):
    """Renders one square RGBA icon of `size` pixels.

    `rounded` draws the ground as a rounded square with transparent corners (the MSIX tiles);
    without it the ground fills the whole square (Google Play masks the corners itself).
    `pad_frac` is the margin between the edge and the gem, as a fraction of the size.
    """
    s = size * SUPERSAMPLE
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    accent = _rgb(accent_hex)
    highlight = _lerp(accent, (255, 255, 255), 0.55)
    shadow = _lerp(accent, GROUND[:3], 0.55)

    pad = s * pad_frac
    r = (s - 2 * pad) / 2
    cx = cy = s / 2

    if rounded:
        d.rounded_rectangle([0, 0, s - 1, s - 1], radius=s * 0.18, fill=GROUND)
    else:
        d.rectangle([0, 0, s - 1, s - 1], fill=GROUND)

    steps = max(48, int(r / 2))
    for i in range(steps, 0, -1):
        t = i / steps
        d.ellipse(
            [cx - r * t, cy - r * t, cx + r * t, cy + r * t],
            fill=_lerp(highlight, shadow, t) + (255,),
        )

    table_r = r * 0.40
    table = _ring(cx, cy, table_r, 8, phase=math.pi / 8)

    if facets:
        line = max(1, int(s * 0.006))
        edge = _lerp(GROUND[:3], accent, 0.25) + (255,)
        star_r = r * 0.72
        star = _ring(cx, cy, star_r, 8, phase=math.pi / 8 + math.pi / 8)
        bezel_tip = _ring(cx, cy, r, 8, phase=math.pi / 8)

        for i in range(8):
            d.line(
                [table[i][0], table[i][1], star[i][0], star[i][1]],
                fill=edge,
                width=line,
            )
            d.line(
                [table[(i + 1) % 8][0], table[(i + 1) % 8][1], star[i][0], star[i][1]],
                fill=edge,
                width=line,
            )
            d.line(
                [star[i][0], star[i][1], bezel_tip[i][0], bezel_tip[i][1]],
                fill=edge,
                width=line,
            )
            d.line(
                [
                    star[i][0],
                    star[i][1],
                    bezel_tip[(i + 1) % 8][0],
                    bezel_tip[(i + 1) % 8][1],
                ],
                fill=edge,
                width=line,
            )
        d.polygon(table, fill=_lerp(highlight, (255, 255, 255), 0.35) + (255,))
        d.line(table + [table[0]], fill=edge, width=line)
    else:
        d.polygon(table, fill=_lerp(highlight, (255, 255, 255), 0.35) + (255,))

    rim = max(1, int(s * 0.012))
    d.ellipse(
        [cx - r, cy - r, cx + r, cy + r],
        outline=_lerp(accent, (255, 255, 255), 0.3) + (255,),
        width=rim,
    )

    return img.resize((size, size), Image.LANCZOS)


def _font(candidates, size):
    for path in candidates:
        if os.path.exists(path):
            return ImageFont.truetype(path, size)
    print(f"warning: no TrueType font found, using PIL's default font at size {size}")
    return ImageFont.load_default(size=size)


def build_msix_assets(out_dir, accent_hex):
    os.makedirs(out_dir, exist_ok=True)
    for filename, size in MSIX_ASSETS.items():
        out_path = os.path.join(out_dir, filename)
        img = draw_gem(size, accent_hex, facets=size >= FACET_DETAIL_MIN_PX)
        img.save(out_path, format="PNG")
        print(f"wrote {out_path} ({size}x{size})")


def build_play_icon(out_dir, accent_hex):
    """The 512x512 Play icon: full-bleed, opaque. The gem keeps clear of the corners Play rounds
    off (about 20 % of the size), hence the wider margin."""
    filename, size = PLAY_ICON
    img = draw_gem(size, accent_hex, facets=True, rounded=False, pad_frac=0.14)
    out_path = os.path.join(out_dir, filename)
    img.convert("RGB").save(out_path, format="PNG")
    print(f"wrote {out_path} ({size}x{size})")


def build_play_feature(out_dir, accent_hex):
    """The 1024x500 feature graphic: the gem on the left, the name and tag line on the right,
    on the app's ground with a soft accent glow behind the gem. Opaque 24-bit PNG."""
    filename, (w, h) = PLAY_FEATURE
    accent = _rgb(accent_hex)
    # A radial glow behind the gem, drawn as concentric ellipses on the ground.
    img = Image.new("RGB", (w, h), GROUND[:3])
    g = ImageDraw.Draw(img)
    gx, gy, gr = 270, h // 2, 330
    for i in range(gr, 0, -4):
        t = i / gr
        g.ellipse([gx - i, gy - i, gx + i, gy + i], fill=_lerp(accent, GROUND[:3], 0.82 + 0.18 * t))

    gem_size = 340
    gem = draw_gem(gem_size, accent_hex, facets=True, rounded=False, pad_frac=0.0)
    # Drop the gem's square ground: keep only the round stone.
    mask = Image.new("L", (gem_size, gem_size), 0)
    ImageDraw.Draw(mask).ellipse([1, 1, gem_size - 2, gem_size - 2], fill=255)
    img.paste(gem.convert("RGB"), (gx - gem_size // 2, gy - gem_size // 2), mask)

    d = ImageDraw.Draw(img)
    name_font = _font(FONT_BOLD, 96)
    tag_font = _font(FONT_REGULAR, 34)
    text_x = 500
    d.text((text_x, 130), PLAY_NAME, font=name_font, fill=(240, 244, 250))
    for row, line in enumerate(PLAY_TAGLINE):
        d.text(
            (text_x, 262 + row * 48),
            line,
            font=tag_font,
            fill=_lerp(accent, (255, 255, 255), 0.35),
        )
        # Each tag line must stay inside the graphic (a fallback font can run wide).
        if d.textlength(line, font=tag_font) > w - text_x - 40:
            print(f"warning: tag line {line!r} is wider than the graphic; shorten it")

    out_path = os.path.join(out_dir, filename)
    img.save(out_path, format="PNG")
    print(f"wrote {out_path} ({w}x{h})")


if __name__ == "__main__":
    msix_dir = sys.argv[1] if len(sys.argv) > 1 else os.path.join(".", "msix", "assets")
    play_dir = sys.argv[2] if len(sys.argv) > 2 else os.path.join(".", "play", "assets")
    build_msix_assets(msix_dir, ACCENT_CUT)
    os.makedirs(play_dir, exist_ok=True)
    build_play_icon(play_dir, ACCENT_CUT)
    build_play_feature(play_dir, ACCENT_CUT)
