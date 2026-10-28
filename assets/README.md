# App icons

`system-app.png` is the source image for the System app's launcher icon;
`exocortex-app.png` is the Exocortex app's, drawn by
`scripts/draw-exocortex-icon.py`. Each `.bgrx` beside them is the generated
220x220 X11-native asset Moon loads: each pixel is four bytes in B,G,R,X
order, with the unused X byte set to zero.

Run `scripts/prepare-assets.py` to regenerate them, or pass `--check` to
verify that the committed bytes match the sources. The script requires the
Pillow version pinned in `requirements-assets.txt` so resizing remains
reproducible.

The images do not contain the one-pixel launcher selection border; the
launcher draws that UI element separately.

`fonts/` holds the embedded Inter (OFL.txt) and, for the Exocortex app's
markdown, Inter Italic and JetBrains Mono NL (OFL-JetBrainsMono.txt), both
subset to the characters Inter's subset has (plus box drawing for Mono).
