#!/usr/bin/env python3
"""Deterministically generate 220x220 BGRX icon bytes from the source PNG."""

from __future__ import annotations

import argparse
from pathlib import Path

import PIL
from PIL import Image


PROJECT_ROOT = Path(__file__).resolve().parents[1]
SOURCE = PROJECT_ROOT / "assets" / "system-app.png"
OUTPUT = PROJECT_ROOT / "assets" / "system-app.bgrx"
SIZE = (220, 220)
PINNED_PILLOW = "12.1.1"


def generated_bytes() -> bytes:
    """Return tightly packed B,G,R,0 pixels using a fixed resize operation."""
    if PIL.__version__ != PINNED_PILLOW:
        raise SystemExit(
            f"Pillow {PINNED_PILLOW} is required; found {PIL.__version__}"
        )

    with Image.open(SOURCE) as source:
        image = source.convert("RGB").resize(SIZE, Image.Resampling.LANCZOS)

    rgb = image.tobytes()
    bgrx = bytearray(SIZE[0] * SIZE[1] * 4)
    output_index = 0
    for input_index in range(0, len(rgb), 3):
        red, green, blue = rgb[input_index : input_index + 3]
        bgrx[output_index : output_index + 4] = bytes((blue, green, red, 0))
        output_index += 4
    return bytes(bgrx)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail instead of writing when the committed output differs",
    )
    arguments = parser.parse_args()

    expected = generated_bytes()
    if arguments.check:
        try:
            actual = OUTPUT.read_bytes()
        except FileNotFoundError as error:
            raise SystemExit("generated BGRX asset is missing") from error
        if actual != expected:
            raise SystemExit("generated BGRX asset is stale")
        print("BGRX asset is reproducible and current")
        return

    OUTPUT.write_bytes(expected)
    print(f"wrote {len(expected)} bytes to {OUTPUT.relative_to(PROJECT_ROOT)}")


if __name__ == "__main__":
    main()

