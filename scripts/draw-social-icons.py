#!/usr/bin/env python3
"""Renders the social apps' icon sources: assets/instagram-app.png,
assets/whatsapp-app.png and assets/discord-app.png, from the SVGs beside them.

Each SVG is its service's app icon as the user knows it: Instagram's
gradient with the camera glyph, WhatsApp's green with the speech bubble and
handset, Discord's blurple with Clyde. The glyphs are the services' own
outlines (Simple Icons, CC0-1.0), so they are exact at any size. librsvg's
`rsvg-convert` renders them at 1024x1024 (Moon rounds the corners itself);
prepare-assets.py then makes the BGRX the launcher loads.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[1]
ICONS = ("instagram-app", "whatsapp-app", "discord-app")
SIZE = 1024


def main() -> None:
    renderer = shutil.which("rsvg-convert")
    if renderer is None:
        raise SystemExit("rsvg-convert (librsvg) is required")
    for name in ICONS:
        source = PROJECT_ROOT / "assets" / f"{name}.svg"
        output = PROJECT_ROOT / "assets" / f"{name}.png"
        subprocess.run(
            [renderer, "--width", str(SIZE), "--height", str(SIZE), "--background-color", "black",
             "--output", str(output), str(source)],
            check=True,
        )
        print(f"rendered {output.relative_to(PROJECT_ROOT)}")


if __name__ == "__main__":
    main()
