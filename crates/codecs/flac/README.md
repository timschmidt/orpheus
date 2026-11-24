# yoton-codec-flac

FLAC decoder for yotonrs (04-audio §4.1, priority A, WP AU-3), written from
RFC 9639.

Supported: native FLAC; block sizes 16–8192 (fixed and variable blocking);
4–24 bits per sample (8, 12, 16, 20, 24 tested); 1–8 channels; independent,
left/side, right/side and mid/side stereo; constant, verbatim, fixed
(orders 0–4) and LPC (orders 1–32) subframes; Rice and Rice2 residuals with
escape partitions; wasted bits; CRC-8 header and CRC-16 frame checks with
resync; SEEKTABLE; any metadata (pictures, padding, tags) streamed past.

Refused: STREAMINFO maximum block size above 8192
(`Unsupported::FrameTooLarge`; 04 §4.1 "larger blocks refused"), 32-bit
samples (`Unsupported::BitDepth`), STREAMINFO maximum frame size above
`MAX_PEEK` (28 KiB). Ogg FLAC needs the Ogg layer (WP AU-7) and is refused
by the probe/facade until then.

## Decision: written here

`nanoaudio-flac` 0.1.2 (MIT, allocation-free) was evaluated: its block
buffer and channel count are const generics (one compile-time size instead
of an arena sized from STREAMINFO, so a 4096-block file would pay for the
8192 worst case), it reads contiguous input only (no split ring view, no
`NeedData` restart contract), and it brings its own pipeline traits
(`nanoaudio-core`) and the `wide` SIMD crate on x86. `claxon` is `std`.
A FLAC decoder is about 700 lines; owning it fits the arena model exactly.

## Memory

Arena = channels × max block × 4 (planar `i32` block) + 1 KiB (interleaving
buffer) + 512 B (32 seek points) + alignment slack, from STREAMINFO:

| Stream | Arena | Budget (04 §4.1) |
|---|---|---|
| stereo, max block 4608 | 38 413 B | 38 912 (38 KiB) |
| stereo, max block 4096 | 34 317 B | 38 912 |
| stereo, max block 1152 | 10 765 B | 38 912 |
| stereo, max block 8192 | 67 085 B | 67 584 (66 KiB) |
| 6 channels, 4096 | 99 853 B | over: refused within these budgets (no in-decoder down-mix yet) |

Decoder struct outside the arena: about 200 bytes. Stack: the LPC
coefficient array (128 B) and a 16-byte header copy.

Output is handed out in pieces of up to 256 stereo frames (`i16`) or 128
(`i32`), interleaved from the planar block.

## Conformance (`cargo xtask test codecs --filter flac/`)

Every file bit-exact against ffmpeg and its STREAMINFO MD5 verified: flac
levels 0–8, block sizes 192–8192, ffmpeg's encoder (LPC order 32, fixed
predictors, each stereo mode, exact Rice parameters, 512-sample frames),
8/12/16/20/24 bits, mono and stereo, 22.05–96 kHz, with and without
seek table, padding, tags and a picture; 23 seeks per file sample-exact;
truncation/bit-flip/splice variants terminate. 16384-sample blocks, 32-bit
samples, six channels at 4096 (arena) and Ogg FLAC are refused as specified.

Seeking: the seek table brackets the target, interpolation on frame-header
sample numbers narrows it (each probe syncs on a header whose CRC-8 and
parameters check), the last stretch walks headers, and the decoder discards
up to the exact sample.
