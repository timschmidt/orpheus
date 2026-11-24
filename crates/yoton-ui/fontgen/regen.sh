#!/bin/sh
# Rebuild the committed bitmap fonts in crates/yoton-ui/fonts/ from the
# outline fonts installed on the development host (Fedora packages
# dejavu-sans-fonts and mozilla-fira-sans-fonts). Licences: fonts/README.md.
set -e
here=$(dirname "$0")
out="$here/../fonts"
dejavu=/usr/share/fonts/dejavu-sans-fonts
fira=/usr/share/fonts/mozilla-fira

# Latin, Latin-1, Latin Extended-A, a few Latin Extended-B letters used by
# Romanian, Greek and Coptic, Cyrillic, general punctuation and symbols.
UI_RANGES="20-7E,A0-17F,192,218-21B,2C6-2C7,2C9,2D8-2DD,370-3FF,400-4FF,2010-2027,2030,2032-2033,2039-203A,2044,20AC,20BD,2116,2122,2190-2193,2212,2215,221E,2248,2260,2264-2265,25B2,25B4,25B6,25B8,25BC,25BE,25C0,25C2"
# Bold is for titles and the clock: Latin-1, Extended-A, basic Greek and
# Cyrillic. Anything else falls back to the regular face.
BOLD_RANGES="20-7E,A0-17F,218-21B,370-3CE,400-45F,490-491,2013-2014,2018-201E,2022,2026,2039-203A,20AC"
NUM_RANGES="20,25,2B-2E,2F-3A"

python3 "$here/fontgen.py" --font "$dejavu/DejaVuSansCondensed.ttf" --size 12 --bpp 2 \
    --ranges "$UI_RANGES" --out "$out/ui12.yfnt"
python3 "$here/fontgen.py" --font "$dejavu/DejaVuSansCondensed-Bold.ttf" --size 12 --bpp 2 \
    --ranges "$BOLD_RANGES" --out "$out/ui12b.yfnt"
python3 "$here/fontgen.py" --font "$fira/FiraSansCondensed-Regular.otf" --digit-height 28 --bpp 4 \
    --tabular --ranges "$NUM_RANGES" --space 9 --out "$out/num28.yfnt"
python3 "$here/fontgen.py" --font "$fira/FiraSansCondensed-Medium.otf" --digit-height 16 --bpp 4 \
    --tabular --ranges "$NUM_RANGES" --space 5 --out "$out/num16.yfnt"
