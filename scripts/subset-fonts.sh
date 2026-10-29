#!/usr/bin/env bash
# Regenerates the JetBrains Mono NL cuts in assets/fonts from the upstream
# release (2.304, OFL; https://github.com/JetBrains/JetBrainsMono) with
# fonttools' pyftsubset: no hinting, no layout features (the app does no
# shaping): ASCII and Latin-1, box drawing and blocks, and the punctuation,
# arrows, maths, geometric shapes and dingbats the app or common replies
# use. Few glyphs keep each face quick to parse (fontdue parses every glyph
# on first use); everything else falls back to Moon's Noto fonts at run time
# (src/text.rs).
#
#   scripts/subset-fonts.sh DIR   DIR holds JetBrainsMonoNL-{Regular,Medium,
#                                 SemiBold,Bold,Italic}.ttf (e.g. Arch's
#                                 ttf-jetbrains-mono: /usr/share/fonts/TTF)
set -euo pipefail

SOURCE="${1:?usage: scripts/subset-fonts.sh DIR}"
PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
UNICODES="U+0020-007E,U+00A0-00AC,U+00AE-00FF,U+2010-2027,U+2030,U+2032-2033,U+2039-203A,U+203C,U+2044"
UNICODES+=",U+20AC,U+2116,U+2122,U+2190-2199,U+21A9-21AA,U+21D0-21D5,U+21E5,U+21E7"
UNICODES+=",U+2200,U+2202-2203,U+2205-2209,U+220F,U+2211-2213,U+2218-221A,U+221E,U+2227-222B"
UNICODES+=",U+2248,U+2260-2261,U+2264-2265,U+2295,U+2297,U+22C5,U+22EF"
UNICODES+=",U+2303-2304,U+2318,U+2325,U+232B,U+23CE,U+2500-259F"
UNICODES+=",U+25A0-25A1,U+25AA-25AB,U+25B2-25B3,U+25B6,U+25B8-25B9,U+25BC-25BE,U+25C0,U+25C6-25C7,U+25CB,U+25CF,U+25E6"
UNICODES+=",U+26A0-26A1,U+2713,U+2715,U+2717,U+2736,U+276E-276F,U+2794,U+279C"

for cut in Regular Medium SemiBold Bold Italic; do
    pyftsubset "$SOURCE/JetBrainsMonoNL-$cut.ttf" \
        --unicodes="$UNICODES" \
        --no-hinting \
        --layout-features='' \
        --output-file="$PROJECT_ROOT/assets/fonts/JetBrainsMonoNL-$cut.ttf"
done
