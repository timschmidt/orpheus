# yoton-codec-mp3

MP3 decoder for yotonrs (04-audio §4.1, priority A, WP AU-2): MPEG-1,
MPEG-2 and MPEG-2.5 Layer III, CBR/VBR/ABR, joint/mid-side/intensity/dual
stereo, CRC-protected frames, Xing/Info/VBRI headers, LAME gapless data,
ID3v1/ID3v2/APEv2 tags inside the stream skipped.

Refused (`Unsupported::Encoding`): Layer I and II, free-format streams.

## Decision: adopt mbop3's frame decoder, write the stream layer

Candidates (docs/research/rust-ecosystem.md §3), all MIT OR Apache-2.0:

| Crate | Verdict |
|---|---|
| `nanomp3-core` 0.2.0 | Bit-exact minimp3 port, layers I–III. Keeps per-frame scratch (~16 KiB) on the stack — more than the 12 KiB firmware stack — and pulls `wide` by default |
| `mbop3` 0.1.0 | minimp3 port tuned for microcontrollers (priorities: stack, memory, CPU, code size); `#![forbid(unsafe_code)]`; all memory, scratch included, in one 12 KiB struct; < 3 KiB stack; differentially fuzzed against C minimp3. Layer III only |
| `mp3dec` 0.1.0 | Safe minimp3 port; no memory figures; std-oriented API |
| `nanoaudio-mp3` | Wrapper over `nanomp3-core` with its own pipeline traits |

Adopted: **mbop3's frame decoder, vendored** (its `bits`, `header`,
`layer3`, `synth`, `tables` files, changes marked `yoton:`; `state.rs` is its
`decoder.rs` reworked). Vendoring rather than depending is needed because
the state must be a `Pod` struct carved out of the arena (no `unsafe`), and
frame sync must work on a split ring view with the `NeedData` contract.
The stream layer (sync and lock, tags, Xing/LAME/VBRI, gapless trims,
sample-exact seek with a sparse index) is written here.

Arithmetic: single-precision float (minimp3's). The Cortex-M4F has an FPU
for `f32`; no `f64` is used. A fixed-point variant would need a different
decoder; whether float is fast enough at the base clock is measured on the
device (04 §4.3). The `exact` feature evaluates sums in minimp3's order.

## Memory

| | Bytes |
|---|---|
| Arena (`ARENA`) | 21 102: state 12 388, split-frame buffer 1 536, PCM block 4 608, seek index 2 048, seek walk ring 512, alignment slack |
| Measured high-water | 21 092 on every corpus file (everything is taken at open) |
| Budget (04 §4.1) | 28 672 |
| Decoder struct (outside the arena) | about 150 |
| Stack | mbop3 states < 3 KiB on ESP32-S3; not measured here |

## Conformance (`cargo xtask test codecs --filter mp3/`)

29 encoded files (CBR 8–320 kbit/s, VBR q0–q9, ABR, L/R stereo, every MPEG
version and rate, mono and stereo, ID3v1, ID3v2 with a 480 KB picture,
no Xing header, silence, impulses, full-scale square, sweeps, noise) against
ffmpeg's float decoder: RMS difference −99 to −120 dBFS, at most 1 LSB per
sample (ISO 11172-4 "full accuracy" is RMS < −101 dBFS and ±2 LSB at 16 bits
measured against the ISO reference); decoded length equals the source length
to the sample on every gapless file; 23 seeks per file sample-exact against
the linear decode; truncation/bit-flip/splice variants all terminate.

Seeking: positions are counted in the trimmed time line. The decoder walks
frame headers from the nearest index entry (built while playing and while
walking, 256 entries with doubling stride), then restarts decoding early
enough that a full bit reservoir (511/255 bytes) precedes the two frames
before the target, and discards up to the exact sample. A far seek in a file
not yet played reads the file up to the target once (headers only).
