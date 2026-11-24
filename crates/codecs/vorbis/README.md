# yoton-codec-vorbis

Ogg Vorbis decoder for yotonrs (04-audio §4.1, priority A, WP AU-7), with
the Ogg layer in `crates/codecs/ogg` (`yoton-ogg`).

## Decision: written from the specification

`lewton` (MIT/Apache-2.0, the only maintained pure-Rust Vorbis decoder) is
`std` and allocates per stream and per packet (codebook trees, expanded VQ
tables, `Vec` residue vectors); restructuring it would touch nearly every
line. `stb_vorbis` and Tremor are C. The decoder is therefore written from
the Xiph Vorbis I specification, with lewton read for cross-checking; the
spec's `floor1_inverse_dB_table` is taken from lewton's copy (MIT/Apache).
No GPL/LGPL code.

## Low-memory design

| Part | How | Typical size |
|---|---|---|
| Codebooks | binary decode tree, 4 B per used entry; VQ multiplicands unexpanded (`u16`), vectors computed per lookup | 15–20 KiB |
| Floors, residues, mappings, modes | fixed `Pod` records | 2–4 KiB |
| Per channel | spectrum/DCT-IV buffer n/2 + overlap tail n/4 floats (the IMDCT output is never stored: it is a folding of the DCT-IV output) | 6 KiB at n = 2048 |
| Transform | one quarter-wave cosine table (n_long + 1 floats) for the FFT and both DCT-IV twiddles; FFT scratch n/4 complex; two window slopes | 14 KiB at n = 2048 |
| Output | 512 frames of `i16` | 2 KiB stereo |

The set-up header is parsed twice (count, then fill) so the arena need is
exact: `inspect` reports it when the three headers are buffered, else
(`ARENA_MAX`, 72 KiB) when a comment packet larger than the ring hides the
set-up; such a comment is skipped as it streams past.

Measured arena high-water (corpus): **55 140 B** (q10, 44.1 kHz stereo),
48–54 KiB for q0–q8 stereo, 34 KiB mono 22 kHz, 23 KiB mono 8–11 kHz.
Budget (04 §4.1): ≤ 72 KiB. Six channels need ~104 KB and are refused
(`ArenaTooSmall`; no in-decoder down-mix). Decoder struct: 1 224 B.

Arithmetic: `f32` (the M4F's FPU); no `f64`, no `libm` (sine for the
tables by a series, `ldexp` by exponent arithmetic).

## Ogg

Packets are read as byte ranges of the ring (no copy). Pages may be larger
than the ring (ffmpeg writes one-second pages, ~40 KB at q10): the reader
keeps the page header and consumes the body packet by packet; CRCs are
checked whenever a page is wholly buffered (always for pages ≤ 28 KiB).
Foreign logical streams are skipped; chaining ends the stream.

## Gapless and seeking

Exact by the spec's arithmetic: the first packet yields nothing, each later
one the span between block centres; the end is trimmed to the last page's
granule position. Seeking bisects on page granule positions (header pages
and pages without packet ends ignored), lands on the last page whose
granule ≤ target, decodes with output dropped until a granule position
fixes the position, then hands out from the exact target sample. Targets
before the first audio granule restart at the first audio packet.

## Conformance (`cargo xtask test codecs --filter vorbis`)

20 libvorbis files (q0–q10, 64k ABR, 8–96 kHz, mono/stereo, silence,
impulses, square, noise, sweep, a 42 KB comment): RMS difference to
ffmpeg's decoder −114.7 to −122.3 dBFS, at most 1 LSB; lengths equal to
ffmpeg's (gapless) on every file; 23/23 seeks exact per file;
truncation/bit-flip/splice variants terminate; a three-track album split
decodes back to back in one arena to exactly the source length
(−121.2 dBFS vs per-track references). Fuzz targets `vorbis` and `ogg`.

Not done: floor type 0 (refused; libvorbis has not written it since 2001),
Vorbis in other containers.
