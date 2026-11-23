#!/usr/bin/env python3
"""Extract the GNU Unifont glyphs the simulator's sample text needs.

    python3 sim/assets/make_subset.py unifont-17.0.05.hex sim/src > sim/assets/unifont-subset.hex

Reads every `.rs` file under the given directories, collects the code points
above U+00FF that occur in them, and prints those lines of the Unifont `.hex`
file (format `XXXX:hexbits`, 32 hex digits for 8-wide glyphs, 64 for
16-wide). GNU Unifont is dual-licensed; the subset is used under the SIL Open
Font License 1.1 (see `OFL-unifont.txt`). The full 16 px font pack for the
device is generated the same way (02-architecture §2.4), not committed.
"""

import os
import sys


def main():
    hexfile, *dirs = sys.argv[1:]
    want = set()
    for d in dirs:
        for root, _, files in os.walk(d):
            for f in files:
                if f.endswith(".rs"):
                    with open(os.path.join(root, f), encoding="utf-8") as fh:
                        for ch in fh.read():
                            cp = ord(ch)
                            if 0x100 <= cp <= 0xFFFF:
                                want.add(cp)
    out = []
    with open(hexfile, encoding="ascii") as fh:
        for line in fh:
            cp = int(line.split(":", 1)[0], 16)
            if cp in want:
                out.append(line.strip())
    print("# GNU Unifont 17.0.05 subset for yoton-sim sample text. SIL OFL 1.1, see OFL-unifont.txt.")
    for line in out:
        print(line)


if __name__ == "__main__":
    main()
