#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["pillow", "click"]
# ///
"""Render Mermoz's app icon (light-blue squircle, white paper plane) to AppIcon.icns.

The paper plane nods to Jean Mermoz, the Aéropostale airmail pilot. Drawn at 4x and
downsampled for clean edges, then packed with `iconutil` into the macOS icon format.

Usage:
  scripts/make_icon.py                 # writes app/Resources/AppIcon.icns
  scripts/make_icon.py --png out.png   # also keep the 1024px master
"""

from __future__ import annotations

import logging
import shutil
import subprocess
import tempfile
from pathlib import Path

import click
from PIL import Image, ImageDraw, ImageFilter

log = logging.getLogger("make_icon")
REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "app" / "Resources" / "AppIcon.icns"
SS = 4  # supersampling factor
SIZE = 1024


def squircle_mask(size: int, inset: int) -> Image.Image:
    """Apple-like continuous-corner square: superellipse with n=5."""
    mask = Image.new("L", (size, size), 0)
    px = mask.load()
    c = size / 2
    r = c - inset
    n = 5.0
    for y in range(size):
        dy = abs((y + 0.5 - c) / r)
        if dy > 1:
            continue
        # x extent of the superellipse at this row
        dx = (1 - dy**n) ** (1 / n)
        x0 = int(c - dx * r)
        x1 = int(c + dx * r)
        for x in range(max(x0, 0), min(x1, size)):
            px[x, y] = 255
    return mask


def vertical_gradient(size: int, top: tuple[int, int, int], bottom: tuple[int, int, int]) -> Image.Image:
    img = Image.new("RGB", (size, size))
    draw = ImageDraw.Draw(img)
    for y in range(size):
        t = y / (size - 1)
        col = tuple(int(top[i] + (bottom[i] - top[i]) * t) for i in range(3))
        draw.line([(0, y), (size, y)], fill=col)
    return img


def render() -> Image.Image:
    s = SIZE * SS
    # macOS icon grid: the shape sits inside ~824px of the 1024 canvas.
    inset = int(100 * SS)
    mask = squircle_mask(s, inset)
    bg = vertical_gradient(s, (214, 234, 251), (110, 168, 228)).convert("RGBA")
    icon = Image.new("RGBA", (s, s), (0, 0, 0, 0))

    # Soft drop shadow under the tile.
    shadow = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    shadow.paste((31, 111, 181, 70), mask=mask.filter(ImageFilter.GaussianBlur(14 * SS)))
    icon.alpha_composite(shadow, (0, 10 * SS))
    tile = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    tile.paste(bg, mask=mask)
    icon.alpha_composite(tile)

    # Glass highlight across the upper half.
    hl = Image.new("L", (s, s), 0)
    ImageDraw.Draw(hl).ellipse([inset - 200 * SS, inset - 520 * SS, s - inset + 200 * SS, s // 2 - 40 * SS], fill=90)
    hl = Image.composite(hl, Image.new("L", (s, s), 0), mask).filter(ImageFilter.GaussianBlur(30 * SS))
    white = Image.new("RGBA", (s, s), (255, 255, 255, 0))
    white.putalpha(hl)
    icon.alpha_composite(white)

    # Paper plane, pointing up-right.
    def p(x: float, y: float) -> tuple[int, int]:
        return (int(x * SS), int(y * SS))

    nose, tail_top, tail_bot, fold, keel = p(742, 300), p(268, 470), p(470, 742), p(468, 540), p(560, 640)
    plane_shadow = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    ImageDraw.Draw(plane_shadow).polygon([nose, tail_top, fold, tail_bot], fill=(22, 79, 130, 90))
    icon.alpha_composite(plane_shadow.filter(ImageFilter.GaussianBlur(18 * SS)), (int(10 * SS), int(22 * SS)))
    d = ImageDraw.Draw(icon)
    d.polygon([nose, tail_top, fold], fill=(255, 255, 255, 255))  # upper wing
    d.polygon([nose, fold, tail_bot], fill=(232, 242, 252, 255))  # lower wing, slightly shaded
    d.polygon([nose, fold, keel], fill=(203, 225, 246, 255))  # keel fold
    return icon.resize((SIZE, SIZE), Image.LANCZOS)


@click.command()
@click.option("--png", type=click.Path(path_type=Path), help="Also save the 1024px master PNG here.")
def main(png: Path | None) -> None:
    logging.basicConfig(level=logging.INFO, format="%(message)s")
    master = render()
    if png:
        master.save(png)
    with tempfile.TemporaryDirectory() as tmp:
        iconset = Path(tmp) / "AppIcon.iconset"
        iconset.mkdir()
        for base in (16, 32, 128, 256, 512):
            master.resize((base, base), Image.LANCZOS).save(iconset / f"icon_{base}x{base}.png")
            master.resize((base * 2, base * 2), Image.LANCZOS).save(iconset / f"icon_{base}x{base}@2x.png")
        OUT.parent.mkdir(parents=True, exist_ok=True)
        if shutil.which("iconutil") is None:
            raise click.ClickException("iconutil not found (macOS only)")
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(OUT)], check=True)
    log.info("wrote %s", OUT.relative_to(REPO))


if __name__ == "__main__":
    main()
