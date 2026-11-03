# App icons

`system-app.png` is the source image for the System app's launcher icon;
`exocortex-app.png` is the Exocortex app's, drawn by
`scripts/draw-exocortex-icon.py`. The social apps' sources
(`instagram-app.png`, `whatsapp-app.png`, `discord-app.png`) are rendered from
the SVGs beside them by `scripts/draw-social-icons.py` (librsvg's
`rsvg-convert`); their glyphs are the services' own outlines from Simple Icons
(CC0-1.0, <https://simpleicons.org>), the marks themselves their owners'.

Each `.bgrx` beside them is the generated 336x336 X11-native asset Moon
loads: each pixel is four bytes in B,G,R,X order, with the unused X byte set
to zero. 336 px is twice Moon's home-screen icon; Moon reduces the artwork to
every size it draws by exact area averaging, so the icons stay crisp at all
of them. (Moon before the change that does this, OSS-Phone-V1 `bf06bf0`,
loads only 220x220 assets.)

Run `scripts/prepare-assets.py` to regenerate them, or pass `--check` to
verify that the committed bytes match the sources. The script requires the
Pillow version pinned in `requirements-assets.txt` so resizing remains
reproducible.

The images do not contain the rounded corners or the one-pixel launcher
selection border; the launcher draws those itself.

`fonts/` holds the embedded Inter (OFL.txt) and, for the Exocortex app,
JetBrains Mono NL in five cuts (Regular, Medium, SemiBold, Bold, Italic;
OFL-JetBrainsMono.txt), subset by `scripts/subset-fonts.sh` to the
characters Inter's subset has plus box drawing, unhinted.
