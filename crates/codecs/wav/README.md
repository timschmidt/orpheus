# yoton-codec-wav

WAV-family decoder for yotonrs (04-audio §4.1, priority A, WP AU-1).

| Container | Encodings |
|---|---|
| RIFF/WAVE (incl. `WAVE_FORMAT_EXTENSIBLE`) | PCM u8/16/24/32 (any valid-bit count in its container), IEEE float 32/64, A-law, µ-law, IMA ADPCM (`0x0011`), MS ADPCM (`0x0002`) |
| AIFF / AIFF-C | `NONE`/`twos`, `sowt`, `raw `, `fl32`, `fl64`, `alaw`, `ulaw`, `ima4` |
| Sun AU | µ-law, 8/16/24/32-bit PCM, float, double, A-law |

Not supported: RF64 (refused with `Unsupported::Encoding`), more than eight
channels, MS ADPCM with more than two channels, ADPCM blocks above 16 KiB.

## Decision: written here, no third-party crate

The candidates (`nanoaudio-wav`, `nanoaudio-adpcm`, `audio-codec-algorithms`,
`hound`) were evaluated against the arena model and the ring interface:

* none reads from a split ring view (`ByteSource::peek` returns two slices);
* none walks chunks incrementally (large `LIST`/`id3 `/`bext` chunks before
  the data must be skipped without buffering them);
* the ADPCM decoders must match ffmpeg's arithmetic bit for bit (IMA:
  multiplication form of the step; MS: truncating division of the
  prediction; QuickTime: predictor kept across packets) for the conformance
  check to be "bit-exact" rather than "close".

The whole decoder is about 600 lines written from the format descriptions,
so owning it is cheaper than adapting a crate.

## Memory

| | Bytes |
|---|---|
| Arena (`ARENA`, all streams) | 3 075 (one 768-word output buffer + alignment slack); measured high-water 3 072 on every corpus file |
| Budget (04 §4.1) | 4 096 |
| Decoder struct (outside the arena) | about 200 (ADPCM channel state) |
| Stack | < 200 B (a 64-byte buffer for a frame that straddles the ring wrap) |

Output is `i16` for sources of up to 16 bits and for the companded and ADPCM
encodings; left-justified `i32` for 24/32-bit integer and float sources
(float is converted from its bit pattern with integer arithmetic — no FPU,
ties to even, saturating — bit-identical to ffmpeg's `libswresample`).

## Conformance

`cargo xtask test codecs --filter wav/` (and `aif`, `au/`): every corpus file
is bit-exact against ffmpeg's decode, decodes identically through the 32 KiB
device ring and a trickling ring, seeks exactly (23 positions per file), and
survives 17 truncated/bit-flipped/spliced variants. QuickTime IMA (`ima4`)
seeks carry a DC offset below 128 LSB: the format keeps the full predictor
across packets while each packet header stores only its top nine bits, so
only a decode from the start is exact.
