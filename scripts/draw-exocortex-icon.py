#!/usr/bin/env python3
"""Draws the Exocortex app's icon source, assets/exocortex-app.png.

The System and Browser icons' flat style: a navy field, a thick blue outline,
teal detail and one white accent. Here a speech bubble holds a small network
of connected nodes (a mind outside the head), with a spark at its corner.
Drawn at 4x and reduced, so the edges are smooth. prepare-assets.py then
makes the 220x220 BGRX the launcher loads.
"""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw

PROJECT_ROOT = Path(__file__).resolve().parents[1]
OUTPUT = PROJECT_ROOT / "assets" / "exocortex-app.png"
SIZE = 1254
SCALE = 4

NAVY = (16, 27, 51)
BLUE = (81, 159, 248)
TEAL = (100, 226, 213)
WHITE = (242, 246, 253)


def main() -> None:
    s = SCALE
    image = Image.new("RGB", (SIZE * s, SIZE * s), NAVY)
    draw = ImageDraw.Draw(image)

    def box(x0: float, y0: float, x1: float, y1: float) -> list[float]:
        return [x0 * s, y0 * s, x1 * s, y1 * s]

    # The bubble: a blue rounded body with a tail, hollowed to an outline.
    stroke = 50
    body = (232, 282, 1022, 872)
    radius = 190
    tail = [(372, 812), (300, 1032), (560, 860)]
    draw.rounded_rectangle(box(*body), radius=radius * s, fill=BLUE)
    draw.polygon([(x * s, y * s) for x, y in tail], fill=BLUE)
    inner = (
        body[0] + stroke,
        body[1] + stroke,
        body[2] - stroke,
        body[3] - stroke,
    )
    draw.rounded_rectangle(box(*inner), radius=(radius - stroke) * s, fill=NAVY)
    inner_tail = [(410, 800), (352, 960), (520, 824)]
    draw.polygon([(x * s, y * s) for x, y in inner_tail], fill=NAVY)

    # The network: four nodes, five links.
    nodes = [(452, 640), (618, 452), (812, 560), (720, 730)]
    links = [(0, 1), (1, 2), (2, 3), (3, 0), (1, 3)]
    for a, b in links:
        (xa, ya), (xb, yb) = nodes[a], nodes[b]
        draw.line([(xa * s, ya * s), (xb * s, yb * s)], fill=TEAL, width=26 * s)
    for x, y in nodes:
        r = 52
        draw.ellipse(box(x - r, y - r, x + r, y + r), fill=TEAL)
        r = 20
        draw.ellipse(box(x - r, y - r, x + r, y + r), fill=NAVY)

    # The spark: a four-pointed star over the bubble's corner, ringed in navy.
    cx, cy = 1000, 300
    for size, color in ((205, NAVY), (160, WHITE)):
        waist = size * 0.22
        points = [
            (cx, cy - size),
            (cx + waist, cy - waist),
            (cx + size, cy),
            (cx + waist, cy + waist),
            (cx, cy + size),
            (cx - waist, cy + waist),
            (cx - size, cy),
            (cx - waist, cy - waist),
        ]
        draw.polygon([(x * s, y * s) for x, y in points], fill=color)

    image = image.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
    image.save(OUTPUT, optimize=True)
    print(f"wrote {OUTPUT.relative_to(PROJECT_ROOT)}")


if __name__ == "__main__":
    main()
