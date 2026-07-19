# System app icon

`system-app.png` is the source image for the launcher icon.
`system-app.bgrx` is the generated 220x220 X11-native asset: each pixel is four
bytes in B,G,R,X order, with the unused X byte set to zero.

Run `scripts/prepare-assets.py` to regenerate it, or pass `--check` to verify
that the committed bytes match the source. The script requires the Pillow
version pinned in `requirements-assets.txt` so resizing remains reproducible.

The image does not contain the one-pixel launcher selection border; the
launcher draws that UI element separately.

