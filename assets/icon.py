#!/usr/bin/env python3
"""Draws RenameIt's application icon.

The mark is ours, drawn here rather than borrowed. Committing the generator
alongside the output is what makes that checkable — the `.ico` in the tree is
reproducible from this file and from nothing else.

What it says: a dim bar (the name a file has), a chevron, a bright bar (the name
it is about to get). Two bars and one chevron is as much as survives 16x16,
which is the size that decides an icon.

    python3 assets/icon.py

Writes into `crates/ren-gui/assets/`:

* `renameit.ico`     — the Windows executable resource, 16-256 px
* `renameit-256.png` — for documentation
* `renameit-64.rgba` — raw RGBA, embedded for the runtime window icon so the
  taskbar is right on Linux too, where there is no `.ico` resource

Requires Pillow.
"""

import pathlib

from PIL import Image, ImageDraw

OUT = pathlib.Path(__file__).resolve().parent.parent / "crates" / "ren-gui" / "assets"

# The app's indigo, top to bottom.
BG_TOP = (72, 92, 210)
BG_BOTTOM = (46, 58, 150)
BRIGHT = (255, 255, 255, 255)
DIM = (255, 255, 255, 125)

# Every size Windows asks for, plus the two the docs use.
SIZES = [16, 24, 32, 48, 64, 128, 256]

# Supersampling factor. The shapes are drawn large and resampled down, which is
# what keeps the chevron from breaking up at 16 px.
SCALE = 8


def plate(size: int) -> Image.Image:
    """The rounded square, with a vertical gradient."""
    gradient = Image.new("RGBA", (size, size))
    draw = ImageDraw.Draw(gradient)
    for y in range(size):
        t = y / max(size - 1, 1)
        colour = tuple(int(a + (b - a) * t) for a, b in zip(BG_TOP, BG_BOTTOM))
        draw.line([(0, y), (size, y)], fill=colour + (255,))

    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [0, 0, size - 1, size - 1], radius=int(size * 0.22), fill=255
    )
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.paste(gradient, (0, 0), mask)
    return out


def icon(size: int) -> Image.Image:
    s = size * SCALE
    img = plate(s)
    draw = ImageDraw.Draw(img)

    # The name it has: wider, dimmer.
    height = s * 0.115
    draw.rounded_rectangle(
        [s * 0.22, s * 0.225 - height / 2, s * 0.74, s * 0.225 + height / 2],
        radius=height / 2,
        fill=DIM,
    )

    # Becomes.
    draw.line(
        [(s * 0.355, s * 0.42), (s * 0.50, s * 0.585), (s * 0.645, s * 0.42)],
        fill=BRIGHT,
        width=int(s * 0.075),
        joint="curve",
    )

    # The name it gets: shorter, solid.
    height = s * 0.15
    draw.rounded_rectangle(
        [s * 0.28, s * 0.785 - height / 2, s * 0.72, s * 0.785 + height / 2],
        radius=height / 2,
        fill=BRIGHT,
    )

    return img.resize((size, size), Image.LANCZOS)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    images = {size: icon(size) for size in SIZES}

    images[256].save(OUT / "renameit.ico", sizes=[(s, s) for s in SIZES])
    images[256].save(OUT / "renameit-256.png")
    (OUT / "renameit-64.rgba").write_bytes(images[64].convert("RGBA").tobytes())

    for name in ("renameit.ico", "renameit-256.png", "renameit-64.rgba"):
        print(f"wrote {OUT / name}")


if __name__ == "__main__":
    main()
