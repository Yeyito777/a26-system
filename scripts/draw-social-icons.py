#!/usr/bin/env python3
"""Draws the social apps' icon sources: assets/instagram-app.png,
assets/whatsapp-app.png and assets/discord-app.png.

Each in its service's colours, so it is found on the home screen where the
user looks for the official app: Instagram's warm gradient with a camera
outline, WhatsApp's green with a speech bubble holding a handset, Discord's
blurple with its controller face. Drawn at 4x and reduced, so the edges are
smooth (Moon rounds the corners itself). prepare-assets.py then makes the
220x220 BGRX the launcher loads.
"""

from __future__ import annotations

import math
from pathlib import Path

from PIL import Image, ImageDraw

PROJECT_ROOT = Path(__file__).resolve().parents[1]
SIZE = 1024
SCALE = 4
WHITE = (255, 255, 255)


def gradient(size: int, stops: list[tuple[float, tuple[int, int, int]]], angle: float) -> Image.Image:
    """A linear gradient across the square at `angle` degrees (0: left to right)."""
    image = Image.new("RGB", (size, size))
    pixels = image.load()
    dx, dy = math.cos(math.radians(angle)), math.sin(math.radians(angle))
    # Project corners to find the span.
    corners = [(0, 0), (size, 0), (0, size), (size, size)]
    values = [x * dx + y * dy for x, y in corners]
    low, high = min(values), max(values)
    # One column of colours per projected value, sampled coarsely and reused.
    steps = 1024
    table = []
    for i in range(steps + 1):
        t = i / steps
        for (t0, c0), (t1, c1) in zip(stops, stops[1:]):
            if t0 <= t <= t1:
                f = 0 if t1 == t0 else (t - t0) / (t1 - t0)
                table.append(tuple(round(a + (b - a) * f) for a, b in zip(c0, c1)))
                break
    for y in range(size):
        for x in range(size):
            t = ((x * dx + y * dy) - low) / (high - low)
            pixels[x, y] = table[int(t * steps)]
    return image


def instagram() -> Image.Image:
    s = SIZE * SCALE
    small = gradient(SIZE // 4, [
        (0.0, (254, 218, 117)),
        (0.25, (250, 126, 30)),
        (0.5, (214, 41, 118)),
        (0.75, (150, 47, 191)),
        (1.0, (79, 91, 213)),
    ], -45.0)
    image = small.resize((s, s), Image.Resampling.BICUBIC)
    draw = ImageDraw.Draw(image)
    stroke = 70 * SCALE
    inset = 230 * SCALE
    draw.rounded_rectangle((inset, inset, s - inset, s - inset), radius=170 * SCALE, outline=WHITE, width=stroke)
    c = s / 2
    r = 120 * SCALE
    draw.ellipse((c - r, c - r, c + r, c + r), outline=WHITE, width=stroke)
    dot = 38 * SCALE
    dx, dy = s - inset - 150 * SCALE, inset + 150 * SCALE
    draw.ellipse((dx - dot, dy - dot, dx + dot, dy + dot), fill=WHITE)
    return image


def whatsapp() -> Image.Image:
    s = SIZE * SCALE
    green = (37, 211, 102)
    image = Image.new("RGB", (s, s), green)
    draw = ImageDraw.Draw(image)
    c = s / 2
    r = 300 * SCALE
    ring = 56 * SCALE
    # The bubble: a ring with a tail at its lower left.
    tail = [
        (c - r * 0.80, c + r * 0.45),
        (c - r * 1.10, c + r * 1.10),
        (c - r * 0.35, c + r * 0.88),
    ]
    draw.polygon(tail, fill=WHITE)
    draw.ellipse((c - r, c - r, c + r, c + r), fill=WHITE)
    inner = r - ring
    draw.ellipse((c - inner, c - inner, c + inner, c + inner), fill=green)
    # The handset: a thick curved grip with a rounded piece at each end.
    handset = Image.new("L", (s, s), 0)
    hd = ImageDraw.Draw(handset)
    hc = (c + 75 * SCALE, c - 75 * SCALE)
    hr = 210 * SCALE
    grip = 72 * SCALE
    hd.arc((hc[0] - hr, hc[1] - hr, hc[0] + hr, hc[1] + hr), start=82, end=188, fill=255, width=grip)
    for angle in (82, 188):
        a = math.radians(angle)
        ex = hc[0] + (hr - grip / 2) * math.cos(a)
        ey = hc[1] + (hr - grip / 2) * math.sin(a)
        piece = 64 * SCALE
        hd.ellipse((ex - piece, ey - piece, ex + piece, ey + piece), fill=255)
    image.paste(WHITE, (0, 0), handset)
    return image


def discord() -> Image.Image:
    s = SIZE * SCALE
    blurple = (88, 101, 242)
    image = Image.new("RGB", (s, s), blurple)
    draw = ImageDraw.Draw(image)
    c = s / 2
    # The face: a wide head on two short legs, a dip in its brow and its chin.
    draw.rounded_rectangle((c - 310 * SCALE, c - 230 * SCALE, c + 310 * SCALE, c + 170 * SCALE), radius=160 * SCALE, fill=WHITE)
    for side in (-1, 1):
        draw.polygon([
            (c + side * 310 * SCALE, c - 20 * SCALE),
            (c + side * 335 * SCALE, c + 200 * SCALE),
            (c + side * 200 * SCALE, c + 215 * SCALE),
            (c + side * 150 * SCALE, c + 150 * SCALE),
        ], fill=WHITE)
        draw.ellipse((c + side * 290 * SCALE - 45 * SCALE, c + 170 * SCALE, c + side * 290 * SCALE + 45 * SCALE, c + 255 * SCALE), fill=WHITE)
    draw.ellipse((c - 130 * SCALE, c - 275 * SCALE, c + 130 * SCALE, c - 200 * SCALE), fill=blurple)
    draw.ellipse((c - 165 * SCALE, c + 120 * SCALE, c + 165 * SCALE, c + 290 * SCALE), fill=blurple)
    for side in (-1, 1):
        ex = c + side * 120 * SCALE
        draw.ellipse((ex - 60 * SCALE, c - 30 * SCALE - 70 * SCALE, ex + 60 * SCALE, c - 30 * SCALE + 70 * SCALE), fill=blurple)
    return image


def main() -> None:
    for name, draw in (("instagram-app", instagram), ("whatsapp-app", whatsapp), ("discord-app", discord)):
        image = draw().resize((SIZE, SIZE), Image.Resampling.LANCZOS)
        image.save(PROJECT_ROOT / "assets" / f"{name}.png", optimize=True)


if __name__ == "__main__":
    main()
