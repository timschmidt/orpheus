# 04 — Audio and media

The audio pipeline, codecs, DSP, recording, the on-card indexes that make a
large library usable with almost no RAM, and picture decoding. Hardware
registers are in [docs/hw/audio.md](../hw/audio.md) and 03-hal-drivers
§"DAC", §"ADC"; task and memory structure in 02-architecture §1.3, §4.

## 1. Goals, in priority order

1. **Never glitch.** No underrun while the user scrolls, seeks, plugs things
   in, or while a background job runs. The HIL `audio` suite counts underruns;
   the acceptable number is zero.
2. **Bit-transparent by default.** With every effect off, the samples that
   reach the DAC are the decoder's samples. Volume is applied in hardware
   (§3.3), not by scaling samples.
3. **Lowest power for the job.** Each stream runs at the lowest clock level
   that leaves 50 % headroom; nothing in the chain runs unless enabled.
4. **Wide format support within fixed memory.** Every decoder declares its
   arena need up front and is refused cleanly ("unsupported") when a file
   would exceed it. No decoder allocates.

## 2. Pipeline

```
card ─► stream ring ─► demux ─► decoder ─► [DSP stages] ─► PCM ring ─► DMA ─► DAC ─► headphone amp
        32 KiB          │        arena       only those      12 KiB            (hardware volume)
        (mode pool)     │        ≤ 72 KiB    enabled         (SRAM3)
                        └─ tags, seek tables, gapless info
```

* **`audio::reader`** (RT executor) keeps the stream ring filled by multi-block
  card reads that land directly in the ring by DMA (sector-aligned, split at
  the wrap). It refills when the ring is below half, in bursts, and reads
  ahead across the track boundary into the next file of the queue (a boundary
  record in a side queue tells the decoder where one stream ends).
* **`audio::decode`** (RT executor) loop: wait for PCM-ring space; ask the
  decoder for one frame; run enabled DSP stages; push. If the decoder answers
  `NeedData(n)`, wait until the ring holds `n` bytes (or end of stream) and
  call again. One frame per iteration, then yield.
* **`audio::sink`** owns the DAC DMA. Preferred: the DMA channel in
  reload/circular mode over the PCM ring so there is no refill deadline at all
  and software only tracks the read position (whether the DMA controller has
  such a mode: datasheet ch. 5 and docs/hw/audio.md §6; U-DMA-2). Fallback, as the
  Rockbox port does: chained fixed-size transfers restarted from the
  completion interrupt, with the pager's poll hook restarting them during a
  burst of page faults.
* **Underrun policy**: the ring going empty inserts silence with a 5 ms fade,
  increments a counter and logs the cause (decoder late, stream ring empty,
  card error). It never repeats stale audio.

### 2.1 Interfaces

```rust
pub trait ByteSource {
    fn peek(&self, n: usize) -> Option<(&[u8], &[u8])>; // contiguous view over the ring wrap
    fn consume(&mut self, n: usize);
    fn position(&self) -> u64;
    fn len(&self) -> Option<u64>;
    fn at_end(&self) -> bool;
}

pub struct StreamInfo { pub rate: u32, pub channels: u8, pub bits: u8,
                        pub frames: Option<u64>, pub delay: u32, pub padding: u32,
                        pub bitrate: u32, pub codec: CodecId }

pub enum Step<'a> { Pcm(PcmBlock<'a>), NeedData(usize), EndOfStream }

pub trait Decoder {
    const ARENA_MIN: usize;                                   // refuse to open with less
    fn open(src: &mut dyn ByteSource, arena: &'static mut [u8], hint: &ContainerInfo)
        -> Result<(Self, StreamInfo), Unsupported> where Self: Sized;
    fn decode<'a>(&'a mut self, src: &mut dyn ByteSource) -> Result<Step<'a>, DecodeError>;
    fn seek(&mut self, src: &mut dyn SeekSource, frame: u64) -> Result<u64, DecodeError>;
}
```

* Decoders are synchronous and restartable at frame boundaries: `decode` must
  not consume input unless it can finish the frame. Frames larger than the
  stream ring are an `Unsupported` condition detected at open where possible.
* `PcmBlock` is interleaved `i16` or `i32` (left-justified 24-bit); a decoder
  with large blocks (FLAC) keeps the decoded block in its arena and returns it
  in slices on successive calls.
* Decoder selection is an `enum` over the built codecs (static dispatch at the
  frame call; one indirect branch per frame is irrelevant).
* Errors: a corrupt frame is skipped with a resync and counted; ten in a row
  end the track with a toast (05 §6.6).

### 2.2 Gapless, seek, resume

* **Gapless**: `delay`/`padding` from the LAME/Xing tag, iTunSMPB, Vorbis and
  Opus granule positions, FLAC total samples. The reader has already queued
  the next stream; the decoder is re-opened in the same arena. If a decoder's
  open time (Vorbis codebook set-up is the long one) exceeds what the PCM ring
  holds, the governor is raised to the top level for the hand-over; the HIL
  suite measures the join (zero inserted samples is the pass criterion for
  MP3, FLAC, WAV, Opus; Vorbis is reported and must be < 30 ms if not zero).
  Settings › Playback › Gapless off (VER-13, 2026-11-26): the player reads
  nothing ahead (`orpheus_player::Command::Gapless`), so every track that
  ends is followed by a fresh start (the PCM ring plays out, the DAC stops,
  the next file opens and fills the ring: a short silence). `status`
  `gapless=1|0|-` says how the last track change at a track's end went.
* **Seek**: by time. MP3: Xing/VBRI table, else bisection with frame-header
  resync, bit reservoir primed by decoding one discarded frame. FLAC: seek
  table, else bisection on frame headers with CRC check. Ogg: bisection on
  page granule positions. MP4: sample tables read at open into a bounded
  sparse index. The stream ring is flushed and refilled; the PCM ring is
  faded out and in (10 ms) to avoid a click.
* **Resume record**: file locator (§6.3), byte offset, frame position, queue
  position, shuffle seed. Written at pause, stop, track change, every 60 s of
  play, and at shutdown. Position must be restored within 1 s.

## 3. Output

### 3.1 DAC path

Start from the verified sequence in the Rockbox port's `pcm-yotonym03.c`
(analogue power, audio PLL, clocks, the vendor's pop-free amplifier ramp, DMA
channel 5 with mode `0x00148b00` into the FIFO at `0xC018000C`). The driver
must add what the port left out, from docs/hw/audio.md: power-down with the
matching pop-free ramp, mute, hardware volume, the complete rate table, and
24-bit transfers if the FIFO format allows them.

### 3.2 Sample rates and width

Native DAC rates are 8, 11.025, 12, 16, 22.05, 24, 32, 44.1 and 48 kHz (both
PLL families; verified by ear at 48 kHz); the datasheet also lists 96 kHz for
DAC and ADC, which the vendor driver gates by IC type (U-AUD-2). The pipeline switches the DAC to the
stream's rate at track start (mute ramp around the switch) and **never
resamples a natively supported rate**. Higher rates (88.2/96/176.4/192 kHz)
are decimated by 2 or 4 with a half-band FIR unless docs/hw/audio.md shows the
DAC accepts them directly. 24-bit sources are dithered to 16 bits (TPDF)
unless a 24-bit FIFO mode exists, in which case they pass through.

### 3.3 Volume

User volume is 0–40 in steps that are equal in dB. It maps to the DAC's own
gain controls (digital volume and/or analogue amplifier gain; register fields
and step sizes from docs/hw/audio.md), changed in small increments at 1 ms
spacing to avoid zipper noise. ReplayGain and the EQ pre-amp are the only
software gains, applied only when enabled, with a look-ahead-free soft limiter
when the result can exceed full scale. The volume limit setting caps the
mapping. Key click and alarm tones are mixed in software only while they
sound (or use the hardware tone generator if one exists).

### 3.4 Other sinks

* **Bluetooth**: not a separate sink. The transmitter module takes the
  analogue output of the DAC (06 §4.2), so the pipeline is unchanged; volume
  maps to the module's absolute-volume command when the headset supports it.
* **Recorder monitor** and **FM**: §5.

## 4. Codecs

### 4.1 Format list and memory plan

Arena figures are the decoder's state, work buffers and one output block; they
are budgets to design against and are replaced by measured numbers in
`budgets.toml`. Priority A is the release minimum; B follows; C is optional.

| Pri | Format (container) | Arena | Notes |
|---|---|---|---|
| A | WAV, AIFF, AU: PCM 8/16/24/32-bit, float, A-law, µ-law, IMA and MS ADPCM | 4 KiB | Also the test vehicle for the pipeline |
| A | MP3, MP2, MP1 (raw, ID3) | 28 KiB | 7 KiB state + 16 KiB scratch + 4.6 KiB output; VBR, free format, MPEG-2/2.5 |
| A | FLAC (native, Ogg) | 38 KiB for block ≤ 4608; 66 KiB for ≤ 8192 | 16/24-bit, up to 2 channels played (more are down-mixed); larger blocks refused |
| A | Ogg Vorbis | ≤ 72 KiB | Low-memory decoder design (compact codebooks); streams whose set-up exceeds the arena are refused with a clear message |
| A | Opus (Ogg) | ≤ 64 KiB | Fixed-point; output always 48 kHz; R128 gain honoured |
| A | Speex (Ogg) | 12 KiB | Narrow/wide/ultra-wide band |
| B | AAC-LC (MP4/M4A, ADTS) | 40 KiB | No SBR/PS in v1 (HE-AAC plays its LC core at half rate) |
| B | ALAC (MP4) | 40 KiB | Block ≤ 4096 |
| B | WavPack (lossless and hybrid lossy) | 24 KiB | |
| B | Monkey's Audio (APE) | 40 KiB | Fast, Normal, High levels; Extra High/Insane refused (CPU) |
| C | WMA Standard | — | Only if a permissively licensed reference can be ported; the stock firmware has it, so parity is desirable but not required |
| C | Tracker modules (MOD, S3M, XM) | file size | Files that fit the arena only |
| C | CUE sheets, `.m3u/.m3u8`, `.pls` | — | Index and playlist formats, not codecs |

The stock firmware's own decoders (ADMP3, ADFLAC, ADOGG, ADAPE, ADAAC, ADWMA,
ADWAV; sizes and working memory in docs/hw/audio.md §8: about 17 KiB for APE/WAV,
36 KiB for FLAC/WMA/AAC, 70 KiB for Vorbis) prove
that each of these fits this RAM when written for it.

### 4.2 Where the decoders come from

"Pure Rust" means no C in the firmware. For each format, WP-AU-n starts by
checking [docs/research/rust-ecosystem.md](../research/rust-ecosystem.md) §3
for an existing crate that is `no_std`, allocation-free (or adaptable to a
fixed arena), and fast enough. Where none exists, the decoder is **written in
Rust from the format specification, using a permissively licensed reference
implementation for cross-checking**; a line-by-line port is acceptable where
the reference licence allows it and is noted in the crate's header. Known
starting points:

| Format | Reference to port or check against | Licence of the reference |
|---|---|---|
| MP3 | minimp3 | CC0 |
| FLAC | Format specification (RFC 9639); libFLAC for cross-checks | BSD-3 |
| Vorbis | Tremor (low-memory branch) | BSD-3 |
| Opus | libopus fixed-point decoder (RFC 6716) | BSD-3 |
| Speex | libspeex fixed-point decoder | BSD-3 |
| AAC-LC | ISO reference behaviour; Symphonia's decoder as Rust prior art | MPL-2.0 (file-level copyleft; decide per D-1) |
| ALAC | Apple ALAC | Apache-2.0 |
| WavPack | WavPack "tiny decoder" | BSD-3 |
| APE | Monkey's Audio SDK (check current licence terms before use) | to verify |

Crates the research found that may save the porting work (all young; adopt
only after they pass our tests inside the fixed arena): `nanomp3-core` or
`mbop3` (MP3), `nanoaudio-flac` (FLAC), `opuscule` (Opus), `nanoaudio-adpcm`,
`nanoaudio-sbc`. No `no_std` pure-Rust decoder was found for Vorbis, AAC,
Speex, WavPack, APE, ALAC or WMA: those are ports.

Do not take code from the Rockbox tree (GPL) unless decision D-1 in PLAN.md
makes the project GPL (the research report suggests Rockbox's fixed-point
codecs as porting sources; that is only an option under a GPL decision).

Every codec crate: `#![no_std]`, `#![forbid(unsafe_code)]` unless a measured
hot loop needs it, no allocation, arena passed in, fuzzed, conformance-tested
(07 §3.3), and benchmarked on the device.

### 4.3 CPU budget

To be filled from the on-device benchmark (`xtask dev hil audio --bench`) for
each codec at each clock level: decode time per second of audio, page faults
per second after warm-up, resulting minimum level.

| Codec / stream | Low | Base (48 MHz) | High | Max | Minimum level |
|---|---|---|---|---|---|
| MP3 128 kbit/s 44.1 kHz | | | | | |
| MP3 320 kbit/s 48 kHz | | (Rockbox port: ~85 % with UI, float minimp3) | | | |
| FLAC 16/44.1 level 5 | | | | | |
| FLAC 24/96 | | | | | |
| Vorbis q5 | | | | | |
| Opus 128 kbit/s | | | | | |
| AAC-LC 256 kbit/s | | | | | |

Design consequences already known: MP3 at 48 MHz leaves little headroom, so
(1) unlocking a higher clock level is on the critical path for comfortable
playback with effects, and (2) the MP3 inner loops (IMDCT, polyphase
synthesis, Huffman) are the first candidates for hand optimisation (fixed
point with `smull`/`smlal` versus the FPU: benchmark both). Hardware offload
for codec mathematics: docs/hw/audio.md §7 found none usable for decoding (the vendor decoders are
integer code on the core), so the Cortex-M4 DSP instructions and the FPU are the
only accelerators and are used through plain Rust arithmetic that LLVM lowers
to them (checked in disassembly for the hot loops).

### 4.4 Metadata

One parser crate (`orpheus-audio::meta`), bounded output (each field truncated
to 127 bytes UTF-8): ID3v1, ID3v2.2/2.3/2.4 (Latin-1 with configurable code
page, UTF-16, UTF-8, unsynchronisation, compressed frames skipped), APEv2,
Vorbis comments, FLAC metadata blocks, MP4 `ilst`, WAV `LIST/INFO`. Fields:
title, artist, album, album artist, genre, year, track, disc, duration,
ReplayGain (track/album gain and peak), R128 gains, embedded picture location
(offset and length only), lyrics location. Parsers are fuzzed and never read
more than they are given.

Lyrics: `.lrc` next to the file (time-tagged, UTF-8 or the tag code page) or
embedded USLT/`LYRICS`. Album art: embedded picture, else `cover.jpg`,
`folder.jpg`, `front.jpg`, `AlbumArt*.jpg` in the file's directory; decoded
once to a 56 × 56 RGB565 thumbnail cached under `.orpheus/cache/art/`.

## 5. DSP, recording and radio audio

### 5.1 DSP stages (each inserted only when enabled)

| Stage | Method | Cost target at 44.1 kHz stereo |
|---|---|---|
| ReplayGain / pre-amp | Q15 gain, soft limiter | < 1 % at Base |
| Equaliser, bass | 5 biquads per channel (peaking/shelving), 32-bit state, coefficients recomputed on change | < 6 % |
| Crossfeed | Two first-order filters and a cross-mix | < 2 % |
| Balance, mono, swap | Trivial | — |
| Speed (0.5×–2×, pitch kept) | WSOLA, 20 ms windows, 4 KiB of state | < 15 % |
| Decimator (> 48 kHz sources) | Half-band FIR, 2 stages | < 8 % |
| Dither | TPDF when reducing word length | < 1 % |
| Fades | 5–60 ms linear ramps for pause, seek, skip, underrun | — |
| Key click | 3 ms 2.5 kHz triangle, linear decay, added after the chain (VER-13) | — |
| Tap for meters/spectrum | Peak detector always; 256-point FFT at ≤ 15 Hz only while the Levels view is visible | — |

**As built (VER-13, 2026-11-26).** The pause fade is the chain's fade stage:
`Chain::begin_pause(fade)` / `end_pause(fade)` (`PAUSE_FADE_MS` = 60, 05
§7.1), driven by Settings › Sound › Fade on pause. The firmware decodes on
after a pause until the fade is done, lets the faded end play out
(`PcmOut::finish`) and then stops the DAC; `status` `fade=` is the last
pause's fade (60, or 0 when it stopped at once). Fades around seek, skip and
underrun are not built. The key click (`orpheus_dsp::click`) is mixed into
each piece after the chain only while it sounds. Speed (the WSOLA row) is
not built: Now Playing's Speed has no effect (05 §7.1 "As built
(VER-13)").

Fixed point first; the FPU where the benchmark says so. The DAC path itself
has no EQ, tone or rate-conversion hardware (audio.md §7), but the SoC has an
FFT/FIR/IIR engine that the stock firmware never uses: if WP-DSP-1 (03 §14)
brings it up, the equaliser's biquad cascade, the spectrum FFT and possibly
the decimator move onto it, with the software stages kept as the reference
and fallback. What the hardware already does for free and the pipeline must
use: volume and its ramps (DAC), 16-bit-to-FIFO widening and planar/
interleaved conversion (DMA), buffer looping (DMA reload), FM and line-in
monitoring (analogue bypass).

### 5.2 Recording

```
ADC ─DMA─► capture ring ─► [low-cut, AGC, VOX] ─► encoder ─► write-behind buffer ─► card
           (SRAM3 ring)                           arena 24 KiB    80 KiB (mode pool)
```

* Sources and gains (microphone single-ended/differential, FM, line-in;
  analogue gain steps, digital gain, bias) per docs/hw/audio.md.
* Formats: WAV PCM 16-bit (8–48 kHz, mono/stereo); WAV IMA ADPCM; FLAC (fixed
  predictors, Rice coding: lossless at about 55–65 % of PCM, light on CPU);
  Opus or Speex in Ogg for long voice recordings (complexity chosen by the
  benchmark). MP3 encoding is offered only if a permissively licensed
  fixed-point encoder is available to port (the common small encoder, Shine,
  is LGPL); otherwise it is absent, since FLAC and Opus are better at both
  ends.
* The writer flushes whole clusters, pre-allocates in 8 MiB steps, rewrites
  the header every 10 s, and closes cleanly at 1 MiB free. A card stall longer
  than the buffer drops the oldest audio, marks the file (cue marker
  "dropout"), counts it, and carries on.
* Pre-record keeps the last 2 s in the capture path while armed. Voice
  activation gates writing with a hang time; AGC is a slow feed-forward gain
  with a limiter.
* Monitoring routes the input to the headphones (analogue bypass if the
  hardware has it, otherwise ADC→DAC through the PCM ring with ~20 ms latency).

**As built (AU-13, 2026-11-24).** `orpheus-rec` (host-tested) holds the
encoders (WAV PCM 16-bit; IMA ADPCM in WAV, 256/512-byte blocks per channel;
FLAC levels 0–2 with a fixed 1152-frame block, fixed predictors 0–4, Rice
partitions, verbatim fallback), the chain (first-order 100 Hz low-cut; AGC:
attack straight to −12 dBFS, release 3 dB/s, +24 dB at most, holds below
−50 dBFS; meter with 1.5 s hold and clip at −0.1 dBFS; voice activation at
−50/−40/−30 dBFS with 2 s hang, listening before the AGC), file names, the
write-behind ring and the recording session. Differences from the plan above:
the arena is the decoder arena of the card task (64 KiB less the music
library's state), cut into a 16 KiB capture ring (two halves, DMA channel 6 in
reload mode), the encoder's block buffer, one encoded block and a
write-behind ring of about 37 KiB (8 KiB writes, 1 s latency bound): about
0.2 s of card stall at 48 kHz stereo WAV, 0.4 s mono, longer for ADPCM and
FLAC. A block that does not fit is dropped **whole and newest first** (the
oldest is the one the card is writing), counted and marked with a `dropout`
cue point (WAV; FLAC frame numbers stay contiguous); audio the capture DMA
overwrote is counted the same way. The header is rewritten every 10 s with
the counts of what is on the card; recording stops with 1 MiB (+64 KiB
allowance) left. Not built: Opus/Speex voice, MP3, pre-record, pre-allocation,
monitoring the microphone in the headphones (the radio is heard through its
bypass while recording it).

### 5.3 FM radio audio

The tuner's audio path (analogue into the SoC mixer versus through the ADC)
is specified by docs/hw/fm-i2c-imu.md and docs/hw/audio.md. Preferred: the
analogue bypass with hardware volume — no CPU, the core sleeps at `Low`
while the radio plays. Recording from FM and FM over Bluetooth use the ADC
path.

## 6. Indexes on the card

Large libraries must be browsable with a few kilobytes of RAM. Everything
here lives under `/.orpheus/` on the card, is versioned, and is disposable:
delete it and the firmware rebuilds it.

### 6.1 Collation

One comparison function for all lists: Unicode simple case folding for Latin,
Greek and Cyrillic; digits compared as numbers ("Track 2" < "Track 10");
leading articles ignored for artists and albums when enabled. Each indexed
string gets an 8-byte **sort key prefix** computed once; comparisons use the
prefix and read the full strings only on ties.

### 6.2 Directory index

Directories with ≤ 128 entries are sorted in RAM (2 KiB). Larger ones get a
cache file `cache/dir/<hash>.idx`: header (signature of the directory: entry
count, total name bytes, newest timestamp, first cluster) and an array of
directory-entry positions in sorted order. Built by an external merge sort in
borrowed pool memory (runs of 1024–4096 entries), in the background, with the
unsorted listing usable meanwhile. A list page is then two sector reads.

### 6.3 File locators

A file is referenced by `(volume serial, parent directory first cluster,
entry position, name hash)`, 16 bytes, which can be opened without a path
walk and verified against the name hash. Queues, the resume record, bookmarks
and index rows store locators; exported playlists store paths.

### 6.4 Library index (`library.ydb`)

* **Scan**: walk the tree (iterative, depth ≤ 16), skip directories whose
  signature is unchanged since the last scan (their rows are copied from the
  old file), read tags for new or changed files. Target ≥ 50 files/s for
  files with tags at the front; the scanner yields whenever the audio stream
  ring is below half and pauses on battery in Battery-saver mode.
* **Tables**: tracks (fixed 48-byte rows: locator, string offsets, numbers,
  duration, gains, sort-key prefixes), string pool (de-duplicated through a
  small hash cache), then sorted id arrays: by title; by album + disc +
  track; by artist + album + disc + track; by genre + artist; by date added.
  Group tables (artists, albums, genres) hold `(name, first position, count)`
  so a list of 2000 albums is a flat array.
* **Sort**: external merge sort over ids with the 8-byte prefixes as keys, in
  borrowed pool memory; with nothing lendable it falls back to 2 KiB runs and
  more merge passes. Capacity target: 50 000 tracks.
* **Atomicity**: built as `library.new`, renamed over the old file at the end.
  A header checksum and a "complete" flag guard against using a partial file.
* **Queries** from the UI are `(table, first row, count)` reads; results are
  cached per page by the `ListView`.

### 6.5 Queue and shuffle

The queue is a list of locators in `/.orpheus/queue.bin` (a library query
result or a directory listing materialised when playback starts), of which
only a window is in RAM. Shuffle is a keyed permutation of the index range
(a 4-round Feistel network with cycle walking), so any queue length shuffles
with O(1) memory, "previous" works exactly, and the order survives a restart
via the stored seed. "Play next" and "Add to queue" edit the file.

## 7. Picture decoding

All image decoders are streaming: source bytes come from a `ByteSource`,
pixels go to a `RowSink` that scales and writes into the frame buffer (or a
thumbnail file). None holds a full image.

| Format | Method | Memory |
|---|---|---|
| JPEG baseline | MCU-row decode with DCT-domain scaling (1/1, 1/2, 1/4, 1/8) chosen so the result is ≥ the target size, then box filter | Tables 4 KiB + one MCU row at the scaled size (a 6000 px wide photo at 1/8: about 4.5 KiB) |
| JPEG progressive | DC scan only, streamed: a 1/8-scale image with no coefficient storage (full detail for sources ≥ 1024 px wide on this screen) | Same as baseline at 1/8 |
| PNG | Inflate with a 32 KiB window, two scan lines, de-filter, palette/gamma ignored except tRNS → background blend | 32 KiB + 2 lines (≤ 4096 px RGBA → 32 KiB); interlaced only if the whole image fits |
| GIF | LZW (16 KiB), canvas at logical size ≤ 256 × 256, frame timing honoured | ≤ 84 KiB |
| BMP, QOI | Row streaming | 1 line |
| Scaler | Box filter down-scaling with per-column accumulators; bilinear up-scaling for small images; EXIF orientation applied at the sink | 2 KiB |

With audio playing, the picture viewer has only the codec arena's lendable
remainder; images that need more show at reduced size or wait with a "pause
music to view" hint (PNG wider than the remainder allows, animated GIF).
Thumbnails use the JPEG DC path and are cached as RGB565 files.

## 8. Speech synthesis (M15; owner's requirement 2026-11-25)

A tiny text-to-speech engine, crate `orpheus-tts` (no_std, no alloc,
host-tested, in the simulator and the WASM site too), for the screen reader
and read-aloud (05 §7.14–7.15).

* **Front end**: text normalisation (numbers, ordinals, dates, times,
  units, currency, abbreviations, initials, Roman numerals in titles, file
  names, punctuation to pauses and intonation); letter-to-sound by rules —
  **English only** (D-14, owner 2026-11-25): the public-domain 1976 NRL
  rule set plus a compressed exception lexicon built from a permissively
  licensed dictionary (CMUdict, BSD-style); a spell-out mode for unknown
  strings; text in other scripts is skipped or spelled by character
  name.
* **Synthesiser**: own formant synthesiser (cascade/parallel resonators
  after the published Klatt design; written from the papers, no code
  copied), 16 kHz internal rate resampled to the DAC rate, phoneme
  durations and formant targets, prosody (declination,
  phrase-final lengthening, question rise, emphasis), two or three voices as
  parameter sets (pitch, formant scaling, breathiness).
* **Speed** (owner, 2026-11-25): **0.25× to 10× in 0.25× steps** (40
  values; an earlier "39" was a miscount; default 1×), a setting of the
  screen reader and, separately, of
  read-aloud. Implemented in the front end and the synthesiser, not by
  resampling, so the pitch stays: phoneme durations scale with the speed,
  with per-class minimum durations (stops and fricatives keep enough length
  to be heard); pauses compress faster than speech above 2×; above 4× the
  prosody flattens (fewer pitch movements) and vowel reduction applies, as
  in fast human-like speech; below 1× vowels and pauses lengthen, not
  stops. Synthesis cost scales with the audio produced, so 10× needs no
  more CPU per second of output; the text front end must keep 10 ahead
  (budget checked at 10× on the player). Tests: phoneme timing goldens at
  0.25×, 1×, 4× and 10×; intelligibility (offline recogniser) at 1×, 2×
  and 4× within budget, and measured, not gated, above; the speed setting
  is in the VER-13 manifest (screen reader, read-aloud, every context
  that speaks).
* **Output**: a speech voice in the audio mixer next to the music path,
  with ducking or pausing of the music, its own volume and the DSP chain
  bypassed; works during playback, radio and with the screen off.
* **Budgets** (revised 2026-11-25 from TTS-2's measurements: ≈ 70 KB of
  code and tables, 22 % of Base at 48 kHz output): ≤ 80 KiB paged code and
  tables (lexicons paged as data; flash-paged, not RAM), ≤ 8 KiB RAM; while
  speech plays the governor's clock floor is Mid (96 MHz; speech ≈ 11 % of
  it at 48 kHz), and speech alone (nothing else playing) may run the DAC at
  16 kHz; first sound ≤ 150 ms
  after a request; numbers in `budgets.toml`.
* **Not used**: eSpeak NG (GPL) and SAM (no licence) — licence D-1. Flite
  (BSD-style) may be studied for its data only if the sizes fit.
* **Tests**: phoneme-string goldens for word lists; the
  normaliser and the rule engine fuzzed; intelligibility measured on the
  host by running the synthesised audio through an offline speech
  recogniser (word error rate below a budget on a fixed sentence set);
  device output checked through the M13 capture path.

**As built (TTS-1, 2026-11-25): the front end, host side.**
`crates/orpheus-tts` (`no_std`, no `alloc`, `#![forbid(unsafe_code)]`, no
dependencies; feature `build` adds the std lexicon tool). Module docs are
the reference; in short:

* **API.** `Converter::new(Options { spell, lines }, Speed)`;
  `feed(&[u8]) -> usize` (takes what fits, pieces may split UTF-8),
  `space()`, `finish()`, `next_token() -> Option<Token>` (`None`: feed more,
  or done), `is_done()`, `reset()`, `set_speed`, `set_options`,
  `word_text()` (the normalised word just started), `convert(text, f)`.
  `Token::Word(Word { pos, phrase_final, spelled })` (`pos`: byte offset of
  the source chunk in the fed text — for highlighting and resume),
  `Token::Phone(Seg { phone: Phone { ph, stress }, ms })`,
  `Token::Break(Pause { kind, contour, ms })`. Phonemes: CMUdict's 39
  ARPAbet phonemes (`Ph`), stress 0/1/2 on vowels (`AH` unstressed is the
  schwa), classes (`Class`: vowel, diphthong, stop, fricative, affricate,
  nasal, liquid, glide, aspirate). Breaks: group, quote, paren open/close,
  comma, dash, colon, semicolon, line, period, question, exclamation,
  ellipsis, paragraph; contours level, continue, fall, rise, exclaim (a
  question rises, or falls when the sentence began with a wh-word). The
  output does not depend on how the text is cut into feeds (property test).
* **RAM**: one `Converter` is 1,408 B on the host (input 256 B, normaliser
  queue 768 B, one word's phones 48 × 4 B; slightly less on the player with
  4-byte pointers); nothing grows with the text. **Code/data** (thumbv7em,
  opt "s", no LTO, whole crate): text 31.5 KB, read-only data 50.2 KB of
  which the lexicon is 32,766 B and the rule table 3.4 KB — 49 KB without
  the lexicon (paged as data), inside the 64 KiB budget; TTS-2 adds the
  synthesiser. Host speed: 4.4 µs per word (≈ 100,000× real time at 1×);
  the player's figure is TTS-2's to measure.
* **Normaliser** (`normalise`, tests `tests/normalise.rs`): the text is cut
  at white space into chunks of ≤ 48 B, each read with two chunks of
  look-ahead. Numbers (cardinals to 15 digits, ordinals, decimals, signs,
  comma groups, versions, IP addresses, years 1100–2099, decades,
  fractions, ranges, phone/serial numbers and long digit strings digit by
  digit), times, dates (ISO, M/D/Y and D/M/Y by value, "25 Nov 2026",
  "November 25th, 2026"), currency ($ € £ ¥ ¢, before or after, with
  k/M/bn), percent, 51 units, 60 abbreviations (titles never end a
  sentence; "St." is Saint before a capital, else Street; "No." only before
  a number), initials and dotted acronyms, Roman numerals after title
  words (cardinal) and capitalised names (regnal ordinal), file names (44
  extensions dropped; `_`, `-`, `.` and camelCase split), URLs and e-mail
  ("dot", "slash", "at", short TLDs spelled), all-capitals words of ≤ 3
  letters spelled unless common, unpronounceable strings spelled, accented
  Latin folded, Greek letters and symbols by name, other scripts and emoji
  skipped, `Options::spell` spells everything ("capital" before capitals),
  `Options::lines` makes a line end a pause (menus, lists).
* **Letter-to-sound**: the NRL rule set (Elovitz et al. 1976; 305 rules in
  `rules::TABLE`, written from the report's description, US-government
  public domain), a primary-stress heuristic (neutral and stress-taking
  endings, endings that stress the syllable before them, unstressed
  prefixes, else the first full vowel), regular endings on lexicon stems
  (`-s -'s -es -ies -ed -ing -ly -er -est`), and the exception lexicon.
* **Lexicon** (`cargo xtask tts-lexicon [--check]`, `lexgen`): CMUdict
  (pinned commit, SHA-256) ranked by word counts over 57 public-domain
  Gutenberg books (6.8 M tokens; 97.7 % of them are CMUdict words), plus
  forced vocabularies at the weight of the 300th word: every word the
  normaliser says (331) and the interface's strings (748 words from
  `orpheus-apps`/`orpheus-ui`); 30 normaliser words CMUdict lacks have
  hand-written entries. Greedy in weight order: a word gets an entry only
  if the rules + heuristic + stems get it wrong (a 4-bit stress-only entry
  when only the stress is wrong); entries made redundant by later stems
  are pruned; the largest prefix within the budget is kept. Format: sorted
  words, front-coded in blocks of 32, canonical Huffman codes for letters
  and phones, a binary search over the blocks. **Shipped: 4,487 entries
  (4,148 full, 339 stress-only) in 32,766 B.**
* **Accuracy against CMUdict** (right = phonemes equal to a CMUdict
  variant and primary stress on the same vowel), all 38,010 CMUdict words of
  the corpus: **rules alone 39.3 % of words (phonemes; 35.6 % with
  stress), 79.8 % of tokens (78.5 %)**; top 1,000 words 66.9 %. **Rules +
  lexicon: 98.1 % of tokens** (top 5,000 words 99.8 %; forced vocabulary
  100 %); words outside the lexicon stay at the rules' level (all words
  55.4 %). Curve (tokens right, phonemes + stress): 0 B 78.5 %, 2 KB
  84.4 %, 7 KB 91.6 %, 14 KB 95.1 %, 21 KB 96.7 %, 27 KB 97.6 %, **32 KB
  98.1 %**, 42 KB 98.6 %, 56 KB 99.1 %, 111 KB 99.6 %
  (`crates/orpheus-tts/data/lexicon-report.txt`). The selection and the
  measurement use the same corpus; the fixture `data/eval.tsv` (top 1,500,
  every 20th word to rank 40,000, the forced vocabulary) carries the
  `tests/accuracy.rs` thresholds (rules > 36 % of words, > 78 % of tokens;
  with the lexicon > 97.5 % of tokens, > 99.5 % of the top 1,500, 100 % of
  the forced vocabulary).
* **Duration model** (`duration`; the inputs TTS-2 uses): `Speed` in
  quarters (1–40, 0.25×–10×); per phoneme inherent/minimum durations after
  the MITalk table of Klatt's rules, Klatt-style percentage rules
  (phrase-final ×1.4, non-final ×0.6, non-word-final ×0.85, polysyllabic
  ×0.8, unstressed ×0.5, following-consonant effects, non-initial and
  cluster consonants); faster: the compressible part scales 1/s, the
  minimum 1/s for vowels/sonorants but 2/(1+s) for obstruents, class floors
  (stops 22, affricates 30, fricatives 25, vowels 15, diphthongs 25 ms …)
  up to 4×, lowered by up to 37 % at 10×; slower: vowels ×1/s, sonorants
  ×(1+(1/s−1)/2), obstruents ×(1+(1/s−1)/4); pauses 1/s up to 2×, 2/s²
  above, sentence ends ≥ 30 ms; above 4× unstressed vowels reduce to
  schwa/`IH` and `pitch_depth_pct` falls from 100 to 40 % at 10×. Integer
  arithmetic only. Goldens at 0.25×/1×/4×/10×
  (`tests/goldens/durations.txt`). A test utterance lasts 12.7 / 6.9 /
  4.0 / 2.1 / 1.0 / 0.51 s at 0.25/0.5/1/2/4/10×: **the 10× setting is
  ≈ 7.7× faster in practice** (the floors).
* **Tests**: unit tests per module; `tests/normalise.rs` (166 written
  forms); `tests/goldens.rs` (phoneme strings of 143 inputs: common words,
  numbers, dates, currency, names, file names, sentences; duration
  tables; `ORPHEUS_WRITE_GOLDENS=1` rewrites); `tests/accuracy.rs`;
  `tests/properties.rs` (feed-size independence over 300 random texts at
  every speed, arbitrary bytes and UTF-8, pathological inputs, ≤ 24 tokens
  per input byte, well-formed tokens, monotonic durations, floors,
  0.25× ratios, pause compression, reduction, RAM); fuzz targets
  `tts_text` and `tts_lexicon` (`crates/orpheus-tts/fuzz`; 9 committed
  seeds replayed by `tests/fuzz_corpus.rs`). The fuzz seeds found an endless
  loop in the lexicon decoder on truncated data (fixed: decoding stops at
  the end of the data).
* **For TTS-2**: the synthesiser consumes `Token`s; it needs (1) a look-ahead
  of at least the current word to put the contour of the following
  `Break` on the last stressed syllable (`Word::phrase_final` marks the
  word; the break follows its phones directly), declination reset after
  `kind.ends_sentence()`, asides lower/faster between paren breaks;
  (2) formant targets per `Ph` and voice; (3) `duration::pitch_depth_pct`
  for flattening above 4×; (4) the CPU check at 10× on the player and the
  intelligibility test (offline recogniser) at 1×/2×/4×; (5) the lexicon
  paged as data (it is one `&'static [u8]`; `Converter::with_lexicon`
  takes another). Not done here: function-word de-accenting, emphasis
  marks, homograph choice (the lexicon keeps CMUdict's first variant).

**As built (TTS-2, 2026-11-25): the formant synthesiser and prosody, host
side.** In `crates/orpheus-tts` (still `no_std`, no `alloc`,
`forbid(unsafe_code)`; new dependency `orpheus-synth` for its f32 helpers
`sin_turns`/`exp2`, xorshift noise and soft clip). Module docs are the
reference; in short:

* **API.** `Synth::new(Config { rate, channels, voice, speed, seed })`
  (8–48 kHz, mono or interleaved stereo); `push(Token)` /
  `push_with_text(Token, word_text)` (`false` = queue full: render first;
  `space()`), `finish()`, `render(&mut [i16], |frame, Event|)` and
  `render_f32` (fewer frames = starved or done; `is_starved`, `is_idle`),
  `stop(Stop::Now | Stop::AtWord)`, `set_voice`, `set_speed`, `pitch()`
  (F0 of the last tick, for tests/displays). Events: `Sentence { pos }`,
  `Word { pos, spelled }` at the output frame where the word starts
  (±0.5 ms resampler latency), `End`. `Speaker::new(Config, Options)`
  wraps a `Converter` and a `Synth`: `feed`/`space`/`finish`/`render`/
  `stop` (drops the unspoken text)/`set_speed`/`set_voice`/`is_idle`; text
  after an `End` starts a new utterance. Example: `cargo run -p
  orpheus-tts --example say -- [-s SPEED] [-v neutral|low|bright] [-r
  RATE] [-m] [-p SEMITONES] [--words] "text"` writes a WAV.
* **Synthesiser** (`klatt`, written from Klatt 1980's description): 16 kHz,
  1 ms control ticks. Source: KLGLOTT88-like polynomial flow derivative
  (Klatt & Klatt 1990) with the closure step band-limited by a two-sample
  polyBLEP at its fractional position (F0 not quantised to samples), F0 and
  OQ latched per period, flutter (three slow sines, ±0.35 %), aspiration
  noise ×½ in the closed phase, a one-zero pre-emphasis (0.65: the pulse
  alone left F2/F3 of /i/ 28 dB under F1, natural ≈ 20 dB) and a one-pole
  tilt. Cascade: nasal pole/zero (270 Hz; the zero moves to 450 Hz in
  nasals and over the transition of a neighbouring vowel), F6…F1 (F4–F6
  fixed at 3.3/3.75/4.9 kHz × the voice's scale; F6 added for 16 kHz).
  Parallel (frication and bursts): white noise into P2–P4 (tracking
  F2–F4), P5 4.7 kHz, P6 6 kHz (peak-normalised), alternating signs, and a
  bypass. Each filter runs over the whole tick with its state in
  registers, coefficients are redesigned only when a frequency or
  bandwidth changed; amplitudes ramp per sample over the tick.
* **Targets** (`targets`, sources cited in the module): vowels Peterson &
  Barney 1952 (men's means), schwa 500/1500/2500, diphthongs start/end
  after Holbrook & Fairbanks 1962, sonorants, nasal murmurs and fricatives
  after Klatt 1980 / MITalk / Fujimura 1962, frication spectra after Hughes
  & Halle 1956 and Jongman et al. 2000, burst spectra after Stevens &
  Blumstein 1978, VOT after Lisker & Abramson 1964. Loci as linear locus
  equations per place (after Lindblom 1963, Sussman et al. 1991; velar
  front/back split and F2–F3 pinch). Levels set by measurement: /s/ −8 dB,
  /ʃ/ −5, /f/ −20, /θ/ −24, nasals −9, /h/ −15 re the vowel RMS.
* **Tracks** (`speak`): per tick, from the previous, current and next unit:
  formant targets (diphthongs glide over 20–90 % of the segment, /h/ takes
  the neighbouring vowel's), boundary values (locus onset when a placed
  consonant meets a vocalic segment, else the mean), linear transitions of
  45 ms (stops), 40 (fricatives), 35 (nasals), 50 (vowel–vowel), 60
  (vowel next to a liquid/glide; the liquid itself 22 ms so it reaches its
  target), each at most half the segment (fast speech undershoots
  naturally); 3 ms amplitude cross-fades (6/8 ms after/before a pause).
  Stops: closure (holding the incoming formants; voice bar after a voiced
  sound, 20 ms prevoicing otherwise for b d g), burst at the end (4–10 ms ×
  2/(1+s)), none before another stop or affricate; aspiration at the start
  of the next vocalic segment (VOT 55/65/75 ms for p t k at 1×, ×0.6 when
  unstressed, ≤ 12 ms after /s/, ≤ 45 % of the segment, widened B1–B3);
  liquids/glides after a stop start at the vowel's level. Affricates:
  45 % closure, then /ʃ/-like frication. Pauses hold the last filters (no
  coefficient jump while they ring).
* **Prosody**: a word is final when the next word or break arrives (the
  queue holds 64 units ≥ a word's 48 phones + its break). Accent on the
  first primary-stressed vowel of each content word (+20 % of the
  baseline, first accent of a phrase +25 %, spelled letters ×0.6;
  secondary stress +7 %); function words (66, `is_function_word`) flat.
  Contours on the last stressed vowel of the phrase-final word, ending on
  its last voiced sound: fall (+19 % → −17 %), wh-question fall (+32 % →
  −17 %), rise (−6 % → +52 %), continue (→ +23 %), exclaim (+46 % →
  −15 %). Baseline ×1.10 declining 9 % per second of speech (in 1×
  seconds, so the shape is the same at every speed), floor ×0.86, full
  reset after a sentence end, half after a phrase break. Asides: baseline
  ×0.9, movements ×0.6, units 15 % shorter. All movements × the voice's
  range × `pitch_depth_pct` (40 % at 10×); F0 smoothed (22 ms at 1×, 8 ms
  at ≥ 8×).
* **Voices** (`voice`): Neutral (118 Hz, formants ×1, OQ 0.55, tilt 3 dB,
  breath −34 dB), Low (92 Hz, ×0.93, OQ 0.5, tilt 6, range 0.85), Bright
  (195 Hz, ×1.16, OQ 0.62, tilt 2, breath −26, range 1.2);
  `Voice::with_pitch(semitones)` for the screen reader's pitch setting.
* **Speed**: durations are TTS-1's, exactly: a phone of `ms` lasts `ms`
  ticks; an utterance lasts `frames_for(16·Σms, rate)` frames (tested at
  0.25/1/4/10× and 16/44.1/48 kHz; word events at 16 kHz on the exact
  sample). Speed only changes the pitch depth, the F0 smoothing and the
  burst/VOT scaling: the mean F0 stays within 8 % from 0.25× to 10× (test).
* **Resampler** (`resample`): 16-tap Kaiser (β 6) windowed sinc, cutoff
  7.36 kHz, 48 phases + linear interpolation (3,136 B table, built by
  `const fn`), exact rational phase: 48/32/24 kHz use table phases, 44.1/
  22.05 interpolate; 16 kHz passes through; below 16 kHz a 4th-order
  Butterworth pre-filter. Image at 10 kHz from a 6 kHz tone < −30 dB
  (test); 1 kHz gain ±2 %.
* **RAM** (`speak::ram`, test): `Synth` 1,936 B (signal path 460 B,
  resampler 208 B, unit queue 64 × 16 B); `Speaker` (converter + synth)
  **3,352 B** (≤ 8 KiB). **Code/data** (thumbv7em, opt "s", no LTO, whole
  crate, generic `render` paths not counted): text 47.5 KB, rodata
  55.5 KB (TTS-1: 31.5 + 50.2), so TTS-2 adds ≈ 16 KB code (largest
  `Synth::frame` 4.7 KB, `Dsp::render` 2.1 KB) and 5.3 KB tables;
  **without the lexicon ≈ 70 KB, over the 64 KiB plan budget** (open).
* **Cost.** Host (x86-64, `cargo bench -p orpheus-tts`, release): 31 ns
  per 16 kHz sample (synthesiser and control, ≈ 2,000× real time); 20 ns
  per 48 kHz stereo frame and 26 ns per 44.1 kHz frame including the
  resampler; front end 12 µs per second of speech at 1×, 86 µs at 10×.
  **Player estimate** (thumbv7em disassembly, instructions + 1 per
  load/store + 2 per taken branch, ±30 %): per 16 kHz sample source ≈ 90
  cycles, each of the 8 cascade filters ≈ 15, the parallel branch ≈ 120
  when frication sounds: ≈ 210 (vowels) / 330 (fricatives) / 60 (pauses),
  plus ≈ 2,000 per tick for the tracks and filter design (≈ 125 per
  sample) → ≈ 5.5 M cycles per second of speech. Resampler + i16 stereo
  out ≈ 110 cycles per frame at 48 kHz, ≈ 175 at 44.1 kHz. **Total ≈ 10.8 M
  cycles/s at 48 kHz (7.5 % of 144 MHz, 22 % of Base 48 MHz), ≈ 13 M at
  44.1 kHz (9 % / 27 %), ≈ 6 M with 16 kHz output (4 % / 12.5 %)**: the
  "≤ 20 % at Base" budget holds only at 16–32 kHz output; at 44.1/48 kHz
  the governor needs one step up (96 MHz) while speech plays, or the
  resampler a shorter kernel. Front end at 10×: ≈ 1.3–1.7 M cycles per
  second of speech (host time × 15–20, SYN-1's ratio), ≈ 1 % of 144 MHz
  (lexicon NOR reads not counted): it keeps up.
* **Tests**: unit tests (resonator/antiresonator responses, glottal period
  and DC, resampler table/pass band/images/counts, tilt, voices, function
  words, accents and contours per word, full queue); `tests/speech.rs`
  (durations exact at 4 speeds × 3 rates and word-event samples; asides
  faster; output independent of block sizes 1/7/256/1023/4096 and of
  feeding text in 5-byte pieces; LPC formants of the 10 monophthongs in
  Neutral and Low within 15 % (F1) / 8 % (F2, F3) — measured within 12 %
  / 3 %; /s/ > 60 % of its energy above 4 kHz, /ʃ/ > 50 % in 1.8–4 kHz,
  /f/ weaker than /s/; question rise, statement fall and wh-question fall
  on the audio (Neutral, pitch tracker) and on `pitch()` (all voices);
  mean F0 within 8 % from 0.25× to 10×; no NaN and peak < 0.8 (soft-clip
  knee) over 3 voices × 4 speeds × 6 sentences — measured −3.1 dBFS;
  events, stop now (≤ 3 ms + latency), stop at word (no further word,
  ends at the next word's start), RAM, every phoneme at 0.25×/10× in
  three positions); `tests/golden_speech.rs` (34 FNV-1a hashes: 4
  sentences × 1/0.25/4/10× × Neutral/Bright at 48 kHz stereo, Low at
  44.1 kHz, 16 kHz mono; `tests/goldens/speech.txt`); fuzz target
  `tts_synth` (tokens or text, every rate/voice/speed, stops; oracle:
  finite, within ±1, i16 = f32 rounded, ends within the pushed duration,
  exactly it without asides or stops) with 6 seeds and one regression
  input, fuzzed 4 min (2,793 inputs, ASan): no findings.
* **Intelligibility** (`tests/intelligibility.rs`; runs with
  `ORPHEUS_ASR_URL` = a whisper.cpp server or `ORPHEUS_ASR_CMD`, "not
  run" otherwise; 24 own sentences, 187 words): whisper.cpp 1.9.4
  `ggml-base.en`, greedy: **Neutral 13.9 % WER at 1×, 30.5 % at 2×,
  93 % at 4×** (6× 98 %, 10× 100 %, measured, not gated); Low 13.4 /
  31.6 %, Bright 7.5 / 27.8 % at 1× / 2×. Same recogniser and sentences:
  espeak-ng 5.9 / 63 / 89 % at 1/2/4×, flite 7.5 % at 1×; our 1× audio
  time-compressed by WSOLA scores 41 / 86 / 96 % at 2/3/4×: **the 4×
  figure is the recogniser's limit**, not the synthesiser's (a human
  listener check is the real measure there). Budgets `budgets.toml
  [tts]`: 18 / 36 / 97 %. Typical 1× errors: stop + /r/ onsets ("green
  umbrella", the same for espeak-ng), word-boundary runs ("water and
  heat").
* **For the firmware (TTS-3 / A11Y-1 / BOOK-3)**: a speech voice in the
  mixer (own volume, DSP bypassed, ducking or pausing the music), fed from
  `Speaker::render` at the DAC rate (16 kHz natively when nothing else
  plays); `Speaker` (3.4 KB) as one static; the lexicon paged as data
  (`Converter::with_lexicon`); `Dsp::render`, `Resonator::block`, the
  resampler and its table hot/resident (per-sample paths); a clock floor
  of 96 MHz at 44.1/48 kHz output; check the golden hashes on the player
  (FZ = 0) and measure cycles and first-sound latency (≤ 150 ms) into
  `budgets.toml`; decide the code-size budget (≈ 70 KB vs 64 KiB). The
  screen reader uses `Stop::Now` on a key, `Voice::with_pitch`,
  `set_speed`; read-aloud feeds text by `space()`, highlights on
  `Event::Sentence`/`Word` (`pos` = byte offset of the source chunk) and
  resumes from a word's `pos`. Not done: emphasis marks, homographs,
  intonation of lists, a male/female choice beyond the three voices.

## 9. Synthesis and MIDI (M17; owner's requirement 2026-11-25)

One synthesis crate, `orpheus-synth` (no_std, no alloc, host-tested, in
the simulator and on the website), shared by MIDI playback, the Sequencer
app (05 §7.16), the games' sound effects (M14) and the speech engine's
resonators (M15).

* **Voices**: band-limited oscillators (saw, square/pulse with width,
  triangle via PolyBLEP; sine; small wavetables; noise), 2–4 operator FM,
  sample playback (one-shot and looped, from WAV files on the card, streamed
  through a small cache), drum models (kick, snare, hats, clap, toms,
  cymbal, percussion: oscillators, noise and envelopes), per voice: pitch
  and amplitude envelopes (ADSR), a state-variable filter (low/band/high
  pass, resonance, envelope amount), LFOs, velocity, pan.
* **Engine**: up to 16 voices at 44.1/48 kHz; **measured (SYN-1, 2026-11-25):
  about 6 saw-lead voices fit the Base clock (48 MHz) and 16 take ≈ 79 % at
  144 MHz, so the synth raises the governor's clock floor to High (144 MHz)
  when more than ~6 voices sound (as the codecs raise it for heavy formats);
  the 16-voice target applies at High** — the earlier target was 16 voices at the Base clock (the number is measured and becomes a budget; voice stealing by
  age and level), events scheduled at sample offsets inside a block (no
  timing jitter from the UI), per-channel mixing, send effects (delay,
  chorus, a small reverb), a soft limiter; f32 without fused multiply-add
  so the host and the player render the same samples; denormals flushed.
* **Output**: rendered blocks go to the DAC ring like a decoder's (the
  DSP chain and volume apply; Bluetooth follows); the synth takes the
  decoder arena while it plays, so music playback and the synth do not run
  at once; the governor raises the clock floor by the voice count.
* **MIDI playback**: Standard MIDI Files type 0 and 1 (`.mid`, `.midi`,
  `.kar` with lyrics shown), running status, tempo and time-signature
  changes, all channel messages, pitch bend, the common controllers (volume,
  expression, pan, sustain, modulation, RPN bend range), program changes
  and drum channel 10 mapped to an **own General MIDI sound set** of
  synthesised patches (no copied sound banks); optional user SoundFont
  (SF2) samples from the card for better instruments, streamed with a
  sample cache sized to the free RAM (stretch). Played from Files and the
  Music app like other tracks: seek, pause, gapless next file, ReplayGain
  not applicable; a channel mute/solo and transpose/tempo view in Now
  Playing.
* **Licences**: synthesis is written from published descriptions; no GPL
  synthesiser code (TinySoundFont is MIT and may be studied); patches and
  drum kits are our own data files.
* **Tests**: golden renders (hash of the samples) for every patch, drum
  voice and a set of MIDI files on host and player (bit-identical by the
  f32 rule); band-limiting checked by spectrum (aliasing below a budget);
  the MIDI parser fuzzed and adversarial (truncated tracks, huge deltas,
  bad running status, 0-length meta events); a scheduling test that every
  event lands on its sample; CPU per voice per clock level into VER-10;
  M13 measures the rendered output.

**As built (SYN-1, 2026-11-25): the engine core, host side.**
`crates/orpheus-synth` (`no_std`, no `alloc`, `#![forbid(unsafe_code)]`;
depends only on `orpheus-dsp` for `exp2`/`log2`/`sin`/`cos` at control
rate). Module docs are the reference; in short:

* **API.** `Engine::new(rate, seed)` (8–48 kHz); `set_patch(part, &Patch)`
  on 16 parts; `render_i16(&mut [i16], &[Event])` and `render_f32` fill an
  interleaved stereo block and apply each `Event { offset, kind }` at its
  sample (`NoteOn`, `NoteOff`, `Param { part, Param::…, value }` for
  volume, pan, cutoff, resonance, bend, glide, LFO rate/depths, sustain,
  level, the three sends; `Fx { FxParam::…, value }`; `AllNotesOff`,
  `AllSoundOff`, `Tempo`, `Master`); `apply(kind)` between blocks;
  `set_voice_limit`, `set_delay/chorus/reverb`, `voices()`, `reset(seed)`.
  A `Patch` is plain `Copy` data: `Kind::Subtractive` (2 oscillators +
  noise), `Kind::Fm` (2–4 operators, 8 algorithms as operator graphs,
  feedback, key scaling), `Kind::Drum(DrumParams)` or `Kind::Kit` (GM
  notes → drum voices); filter (`Off`/LP/BP/HP, cutoff, resonance, envelope
  amount, key tracking, velocity), amplitude, filter and pitch ADSRs
  (linear or exponential), one LFO (sine/triangle/square/saw/S&H, Hz or
  cycles per beat; pitch, cutoff, amplitude, pulse-width depths), glide,
  mono/legato, level, velocity sensitivity, bend range. Every value is
  clamped on installation (NaN included). The building blocks (`Osc`,
  `Svf`, `Env`, `Lfo`, `FmState`, `DrumVoice`, `Delay`, `Chorus`,
  `Reverb`, `Limiter`) are public for the signal generator (§7.17), the
  games' effects and the speech resonators.
* **Determinism.** `f32` with IEEE basic operations only (no `mul_add`, no
  `libm`), 32-bit phase accumulators, seeded xorshift noise. Requirement on
  the player: `FPSCR.FZ` left at its reset value 0. Rendering is cut into
  segments at event offsets and at the control ticks of the engine's own
  sample clock (every 32 samples); gains and the master ramp over a fixed
  32 samples, tiny state is flushed and drum voices decide their end only
  on ticks, the chorus phase is integer, so **the output does not depend
  on the block sizes** (tested at 1, 7, 16, 256, 1023, 4096 frames). The golden hashes
  are equal in the test build (opt-level 1) and the release build
  (opt-level "s", fat LTO; `render --check`).
* **Oscillators.** Saw and pulse by two-sample polyBLEP, triangle by
  polyBLAMP, sine by a degree-11 polynomial on the integer phase (error
  < 2.5·10⁻⁷), 4 built-in wavetables × 7 mip levels (const-built, 14 KiB
  read-only), white noise, pink noise (Voss–McCartney, 12 rows). The pulse
  is zero-mean (its DC moved with PWM: a ±0.6 sub-audio wander in the first
  render). Fundamentals at or above 0.45 fs are muted.
* **Aliasing** (`tests/spectrum.rs`, 65 536-point Blackman–Harris FFT,
  alias = everything outside ±8 bins of the harmonics, relative to the
  total): saw 1046/2794/4186 Hz −31/−27/−24 dB in all, **−51/−44/−43 dB
  below 12 kHz** (naive −16/−12/−10); pulse similar; triangle −60/−46 dB
  (below 12 kHz −82/−62); wavetables −47 to −87 dB; sine at the window's
  floor (−94 dB). The two-sample polyBLEP leaves its residue in the octave
  below Nyquist; budgets are the measured values + ~2 dB.
* **Envelopes, filter, LFO, glide.** The amplitude envelope runs per
  sample (attack 0 = full level on the note-on sample, release 0 = silent
  on the note-off sample); filter and pitch envelopes, LFO, glide and the
  SVF design run at control rate (32 samples) with one-step 32-sample
  coefficients; FM operator envelopes run at control rate one period ahead
  and are interpolated per sample. SVF: trapezoidal (Zavalishin/Simper
  descriptions), cutoff 16 Hz–0.49 fs, resonance to Q 50, stable under
  per-period cutoff jumps to the top (test).
* **Voices.** 16 slots + 4 hidden tails: a stolen or retriggered voice
  fades out over 256 samples (5.3 ms; 64 left visible clicks in the
  spectrogram) in a tail while the new note starts on its sample. Order: the
  same part+note, a free slot, the quietest released or < −40 dB voice, the
  oldest. Mono parts: held-note stack (8), last-note priority, legato glide
  without retrigger, return to the held note. Sustain pedal. Drums ignore
  note-off; a closed hat chokes open hats of its part.
* **Drum voices** (kick, snare, closed/open hat, clap, tom; tune, decay,
  tone, snap, level): sines with pitch drops, six band-limited squares at
  the analogue hat's frequencies, noise through the SVF, exponential decays.
* **Mixing and effects.** Constant-power pan; mono sends (delay, chorus,
  reverb) returning stereo; delay 12 288 × `i16` (**256 ms at 48 kHz**,
  tempo sync in beats; 16 384 would put the engine over the 72 KiB arena),
  chorus 1 024 × `f32` (two quadrature taps around 7 ms), reverb 4 LP-combs
  (Schroeder's 29.7–43.7 ms) + 2×2 all-passes in 8 192 × `i16`; the `i16`
  lines truncate toward zero so tails reach exact silence and the effect
  then skips its work. Master: peak follower (instant, 100 ms release),
  gain smoothed down over 1 ms (an instant gain drop splattered), soft clip
  from −1.9 dBFS: output always within ±1.
* **RAM** (`ram_table` test): voice slot 328 B (drum source 140 B,
  oscillator 56 B), patch 292 B, part ≈ 1 KiB with its coefficients;
  delay 24.6 KiB, chorus 4.1 KiB, reverb 16.3 KiB; core (20 slots, 16
  parts, buses) 23.6 KiB; **engine 69,040 B** (≤ the 72 KiB decoder
  arena, asserted). Code (thumbv7em, opt "s", no LTO, whole crate incl.
  3.9 KB demo scenarios) 38.6 KB, read-only data 22.4 KB (wavetables 14.3).
* **Cost.** Host (x86-64, `cargo bench -p orpheus-synth`, opt "s"), ns per
  voice-sample: sine 4.4, saw 3.6, saw+LP 7.2, pulse+LP 8.6, wavetable+LP
  9.3, saw-lead (2 saws, LP, envelopes, LFO) 9.8, PWM pad 11.7, FM 2-op
  20.6, FM 4-op 41.7; drums 5–25; per sample: delay 4.4, chorus 9.7,
  reverb 19.1, empty engine (limiter, i16) 10.1 ns/frame. **Player
  estimate** (thumbv7em disassembly: instructions + 1 per load/store + 2
  per taken branch; ±30 %, to be replaced by SYN-3's measurement), cycles
  per voice-sample including the amplitude envelope, mix and amortised
  control: sine ≈ 75, saw-lead ≈ 140, PWM pad ≈ 190, FM 2-op ≈ 120, FM
  4-op ≈ 260; per sample: delay ≈ 95, chorus ≈ 120, reverb ≈ 190, engine
  + limiter ≈ 135. At 48 kHz that is **16 saw-lead voices ≈ 114 MHz (79 %
  of 144 MHz)**, with all three effects ≈ 93 %; 16 sine voices ≈ 40 %;
  4-op FM ≈ 10 voices at 144 MHz. At Base (48 MHz) ≈ 6 saw-lead or 3
  4-op FM voices: **the "16 voices at Base" target is not met**; the
  governor floor by voice count (SYN-3) must go to 144 MHz for full
  polyphony, and the inner loops (fused passes, state in registers) are
  the place to win.
* **Tests**: unit tests per module; `tests/engine.rs` (every note-on/off
  on its exact sample over random block sizes; parameter changes start on
  their sample; block-size independence; seeds; stealing order; 16-voice
  cap; mono legato; sustain; SVF stability under extreme modulation; RAM
  table); `tests/golden.rs` (28 scenarios, 33 s: every preset's phrase,
  every drum, kit groove, glide, stealing, each effect, LFO sync,
  parameter sweeps, a 4 s demo at 48 and 44.1 kHz; FNV-1a hashes in
  `tests/goldens.txt`, rewritten with `ORPHEUS_WRITE_GOLDENS=1`);
  `tests/spectrum.rs`; fuzz target `synth_events` (`crates/orpheus-synth/
  fuzz`; event streams, any `f32` parameters, wild patches, odd blocks;
  oracle: finite, within ±1, ≤ 20 voices) with 4 committed seeds replayed
  by `tests/fuzz_corpus.rs`. `cargo run --release -p orpheus-synth
  --example render -- <dir>` writes the scenarios as WAVs (not committed).
* **Left for SYN-2**: more drum models (cymbal, percussion) and the full
  kit, sample playback from the card, presets and kits as data files,
  richer effects (a longer delay from memory the synth may take while it
  plays, e.g. the idle stream ring; reverb quality); listening to the
  kits. **For SYN-3**: place the 69 KB engine in the decoder arena (it is
  one value; `reset` re-initialises in place), feed the DAC ring from
  `render_i16`, check the golden hashes on the player (FZ = 0), measure
  cycles per voice type into `budgets.toml`, clock floor by voice count,
  keep the wavetables and hot loops resident or clustered (they are read
  per sample).

**As built (SYN-2, 2026-11-25): drums, kits, samples, effects, banks;
host side.** Same crate, same rules (`no_std`, no `alloc`, no `unsafe`,
IEEE basic `f32` only — the thumbv7em build has 0 `vfma`/`vfms`/`vfnma`/
`vfnms`; its 349 `vmla`/`vmls` are the M4's chained, separately rounded
multiply-accumulate). Module docs are the reference; in short:

* **Drum models** (`drum`): 11 kinds, each with `tune`, `decay`, `tone`,
  `snap`, `level` (per-kind meaning in the module table), written from
  published descriptions of analogue drum circuits (facts such as the
  six metal-oscillator frequencies, the rim shot's two resonators at
  455/1667 Hz, the cowbell's two squares at 540/800 Hz; no code or
  samples copied): kick, snare, closed/open hat, clap, tom (congas and
  bongos are tuned toms) from SYN-1 unchanged (their goldens kept), and
  new **rim** (two sines, click, high pass, drive), **cowbell** (two
  band-limited squares, band pass then a 3.5 kHz low pass, fast + slow
  decay), **cymbal** (six squares and noise in two bands: 3.4 kHz band
  pass τ 0.9 s, 7 kHz high pass τ 0.35 s; `tone` ride → crash), **clave**
  (2.5 kHz sine, `tone` toward a wood block), **shaker** (high-passed
  noise with an attack). One `#[inline(never)]` method per kind (own
  symbol for the estimate and for placing hot code).
* **Kits as data** (`kit`): a `Kit` is ≤ 32 `KitEntry`s of 9 bytes
  (note, sound = drum code or sample slot, choke group, pan %, tune in
  ¼ semitones, decay ×50, tone/snap/level %); the engine holds 2 kit
  slots (`set_kit`), `Kind::Kit(slot)` plays one (`Patch::KIT` = slot 0,
  default `STANDARD`). **Choke groups**: a hit in group g ≠ 0 chokes the
  part's sounding voices of group g (τ 3 ms for drums and sample hits;
  symmetric: closed, pedal and open hats share group 1). Factory kits:
  `standard` (General MIDI map, the SYN-1 kit completed: 32 notes incl.
  cymbals, ride, splash, tambourine, cowbell, bongos, congas, claves,
  wood block), `analog`, `punch`, `tight`, `percussion`.
* **Sample playback** (`sample`, `wav`): `SampleSource` reads 512-byte
  blocks of numbered files (the card in SYN-3; `SliceSource` for tests,
  the simulator, flash); `SampleCache<'m>` borrows its page memory
  (`&mut [i16]`, 512 B pages of 256 mono frames, ≤ 256 pages, 32 slots;
  its own state 4.9 KB on the host) — on the player, memory the synth may
  use while it plays (the idle 32 KiB stream ring is 64 pages).
  `load(src, slot, file, head_pages)` parses the WAV and pins the first
  pages; `service(src, &wants, max_pages)` between render blocks loads
  what the play heads (`Engine::sample_wants`) reach within 1024 output
  frames (wrapping at loop ends), evicting least-recently-wanted unpinned
  pages; a missing page plays as silence and counts
  (`Engine::sample_misses`) without shifting time. The head must cover a
  render block at the highest speed (4 pages: 512-frame blocks to an
  octave up). WAV: PCM 8/16/24/32 and float 32, mono/stereo (stereo is
  mixed to mono on loading), `WAVE_FORMAT_EXTENSIBLE`, `smpl` loop and
  unity note; sizes never trusted (chunks bounded by the file, ≤ 64
  chunks). Player: 32.32 fixed-point position, 4-point Hermite
  interpolation (Niemitalo 2001), one-shot or forward loop (reading
  across the seam), increment ≤ 8; at unit speed the output is the stored
  samples bit for bit (tested). `Kind::Sample(SampleParams)` runs through
  the patch's filter, envelopes and LFO (root note from the file or the
  patch, loop off/file/whole, one-shot ignores note-off, start offset);
  kit sample hits (`KitEntry::sample`) have tune, fade, a one-pole low
  pass and pan. `render_*_with(&mut out, &events, &cache)`; the plain
  `render_*` play sample voices silent. Sample voices end on control
  ticks, as drums do (block-size independence; a first version freed the
  slot mid-segment and the kit render differed at block size 1).
* **Effects** (`fx`). *Delay*: 12 288 × `i16`; times to 256 ms at full
  rate, exact to the sample (beats × 60 / BPM × rate rounded); longer
  (to 1.02 s at 48 kHz) by storing every k-th sample (k = 2…4, the mean
  of k) read back by linear interpolation, with two-pole low passes at
  0.4·rate/k before the line and on the taps (images of the cowbell
  echoes reached 20 kHz without them) whose delay is subtracted from the
  read position: the echo's centre of mass within a sample of the time
  (tested at 60–174 BPM × ¼–1½ beats); feedback path: damping low pass
  and a new `low_cut` high pass (default 120 Hz; `FxParam::DelayLowCut`).
  *Chorus*: unchanged. *Reverb*: replaced the Schroeder combs by an
  **8-line feedback delay network** (Jot & Chaigne 1991): 3 series
  all-pass diffusers, 8 lines of 11.3–28.9 ms with damping and RT60
  gains, an 8 × 8 Hadamard mix, two orthogonal output sign patterns,
  the input on a sign pattern orthogonal to both; **at half the engine
  rate** (pairs averaged in, linear interpolation out) in 4 096 × `i16`
  (8 KiB, half SYN-1's 16 KiB) and computed line by line over blocks of
  ≤ 16 steps (no line is shorter than a block: the same values as step by
  step; tested at 8–48 kHz). Lines hold ±1.0 at 1/32768 and are written
  rounded above T = ⌈0.5/(1 − g) + 1⌉ steps and truncated toward zero
  below, so the decay is at the set rate (measured RT60 0.80/2.46 s for
  0.8/2.5 s) and the tail still ends in exact zeros; truncation alone
  measured 0.48 s for 0.8 s (the tail of a single hit lives at tens of
  steps), error feedback did not fix it (its shaped noise biased the
  highs). A single hit's tail keeps the rate for 30 dB at ≤ 2.5 s and
  15 dB at 10 s, then drains.
* **Banks** (`bank`): `OSB1` files of records (tag, length, 16-byte name,
  body; unknown tags skipped): patches (every field, `f32` bit for bit:
  subtractive 165 B, 4-op FM 259 B, drum 138 B) and kits (1 + 9 per
  entry); decoding checks every code and length and sanitises. Factory
  patches stay `const` (`presets`: 16 melodic — new `acid-bass`,
  `strings`, `brass`, `sub-bass` — 11 drums, `SAMPLER`); all round-trip
  exactly (tested).
* **RAM** (`ram_table`, host sizes): voice slot 328 B (unchanged: drum
  voice 156 B, sample player 48 B fit the slot), kit slot 289 B × 2,
  delay 24,888 B, chorus 4,144 B, reverb 8,600 B, core 24,688 B,
  **engine 62,320 B** (was 69,040; asserted ≤ 64 KiB), plus the sample
  cache state 4,904 B and its borrowed pages. Code (thumbv7em, opt "s",
  no LTO, whole crate incl. demo scenarios): 64.3 KB, read-only data
  26.7 KB (wavetables 14.3 KB).
* **Cost.** Host ns per voice-sample (x86-64, `cargo bench`): kick 12.2,
  snare 18.0, hats 23.6/24.9, clap 5.6, tom 10.4, rim 10.6, cowbell 9.9,
  cymbal 24.2, clave 7.0, shaker 6.0; sample voice 7.4 (unit or pitched),
  pitched + low pass 10.6, kit sample hit 7.3; effects per sample: delay
  5.1 (1 s, k = 4: 10.9), chorus 9.9, reverb 26.6 (full-rate FDN: 42.7).
  **Player estimate** (thumbv7em disassembly, SYN-1's rule, inner loops
  less their rarely taken branches, + ≈ 15 for mixing and amortised
  control; ±30 %), cycles per voice-sample: kick ≈ 145, snare ≈ 230,
  hats ≈ 295, clap ≈ 105, tom ≈ 155, rim ≈ 120, cowbell ≈ 155, cymbal
  ≈ 315, clave ≈ 100, shaker ≈ 75, sample voice with low pass ≈ 95, kit
  sample hit ≈ 90; per sample: delay ≈ 90 (k = 1) / ≈ 230 (k > 1), chorus
  ≈ 120, **reverb ≈ 290** (step by step at full rate it was ≈ 900; SYN-1's
  four combs ≈ 190). A kit groove keeps ≈ 6–8 drum voices sounding (a
  retriggered note replaces itself): ≈ 1,000–1,600 cycles/sample, 48–77
  MHz at 48 kHz, plus ≈ 14 MHz of reverb: **drum patterns need the High
  clock (144 MHz)**, not Base. Drums end at −80 dB; a −60 dB end would
  free voices ~25 % sooner (SYN-3 to judge).
* **Tests**: unit tests per module (27); `tests/drums_fx.rs` (every
  drum's hit is identical at any offset and block cutting; choke groups
  cut members only, both ways, not other groups or parts; every factory
  kit entry sounds; 16 extreme drums with every effect at maximum: finite,
  no subnormal, within ±1, limiter engaged, tails end in exact zeros;
  delay exact at tempo; reverb energy decays window by window at the
  set RT60 and drains; RAM); `tests/samples.rs` (golden hashes of 4
  sample scenarios in `tests/goldens-samples.txt`; streaming through 48
  pages equals the pinned render; block-size independence; sample notes
  start on their sample; WAV formats and conversion; malformed files;
  cache limits); `tests/golden.rs` now 45 scenarios (all drums, the 5
  kits playing two bars, tempo-synced and 1 s delays, a hall); fuzz
  targets `wav_parse` (parser + cache + playback; 8 seeds) and
  `bank_decode` (decoder, round trip, playback; 5 seeds) replayed by
  `tests/fuzz_corpus.rs`; release `render --check`: 49 scenarios, 0
  mismatches.
* **Left for SYN-3**: the card `SampleSource` and where the cache pages
  live (stream ring), `service` in the synth task between blocks, the
  measured cycles (the estimates above), the clock floor for drum
  patterns, the reverb and drum loops clustered/resident. **MIDI-1**:
  map GM programs to `presets` (16 melodic now; a fuller sound set), drum
  channel 10 → `Kit` slot 0 (`STANDARD` covers 32 GM notes), SF2 samples
  through the same cache (WAV is the only file format so far). **SEQ-1**:
  projects store patches and kits as bank records (`bank::Writer`,
  `records`); kit editing is `KitEntry` fields in UI units already; a
  drum track is a part with `Kind::Kit(slot)` (2 slots: more kits per
  project need more slots, 289 B each).

**As built (MIDI-1, 2026-11-25): MIDI files; host and simulator.** New
crate `crates/orpheus-midi` (`no_std`, no `alloc`, `#![forbid(unsafe_code)]`,
no `core::fmt`), on `orpheus-synth`. Module docs are the reference; in
short:

* **Parser** (`smf`). `MThd` (also inside `RIFF RMID`), formats 0, 1, 2;
  running status (cancelled by meta and SysEx); tempo, time and key
  signatures, the seven text kinds (the first 48 bytes kept), end of
  track; SysEx (`F0`/`F7`) and other metas skipped by their length; SMPTE
  divisions 24, 25, 29.97 (30000/1001) and 30 fps. Format 0/1 tracks are
  merged by tick (ties by track number, then file order); format 2
  patterns play one after the other from where the previous ended, with a
  `Pattern` event (the tempo resets to 120 BPM there). A format-0 file
  with several tracks plays like format 1.
* **Bounded streaming.** `Smf` holds ≤ 64 track cursors, each a 64-byte
  window onto its track (≈ 97 B; 6,216 B in all) and never the file. A call
  that needs bytes answers `Pull::Need { offset, len ≤ 64 }` and consumes
  nothing; the caller reads them and calls `supply` (an `async` card task
  can answer; `MidiSource`/`SliceSource` drive it synchronously). Each
  `next_event` parses at most one delta time per live track and one event.
  **Nothing is trusted**: chunk lengths are clamped to the file (≤ 320
  chunks walked, the header's track count ignored); more than 64 `MTrk`
  chunks are refused (`MidiError::TooManyTracks`, message "MIDI file has
  more than 64 tracks"); meta/SysEx lengths are clamped to the track; a
  variable-length number of 5 bytes, a data byte without running status, a
  data byte ≥ 0x80 and the status bytes `F1`–`FE` end their track (the
  rest of the file plays); a track without an end-of-track ends where its
  bytes do (a synthetic `EndOfTrack`); ticks add in 64 bits, saturating.
* **Timing** (`timing`). `frame(t) = ⌊(acc₀ + (t − t₀)·mul) / den⌋` with
  `den` fixed by the division (tpq·10⁶, or the SMPTE frame rate) and `acc₀`
  the exact time of the last tempo change in 1/`den` frames (128-bit): every
  event lands on the floor of its exact time however many tempo changes
  precede it (tested against an independent rational computation, and the
  audio against the same notes placed by hand). A song ends at the first
  event after 24 h (huge deltas). `TempoMap` (128 points) converts frames
  back to ticks and BPM for a tempo view.
* **Sequencer** (`seq`). Channel *n* = engine part *n*. Note on/off
  (velocity 0 = off; muted channels drop note-ons); program change →
  the GM sound, installed (`set_patch`, bend range ±24) only when the
  sound changes; CC 7 × 11 → `Volume` = (v/127)²(e/127)² (the GM 40 log₁₀
  curve); CC 10 → `Pan`; CC 64 → `Sustain`; CC 1 and channel pressure →
  `LfoPitch` = the patch's vibrato + 0.6 semitone × max/127; RPN 0 (CC
  101/100, 6/38, 96/97) → bend range in semitones and cents, `Bend` scaled
  to the ±24 of the installed patch; CC 91/93 → reverb/chorus sends (option
  `effects`); CC 121 resets per RP-015 (volume and pan stay); CC 120,
  123–127 → `AllNotesOff` of the part (it lifts the pedal). Channel 10 →
  `Patch::KIT` on kit slot 0; its program picks the factory kit (16–23
  `punch`, 24–31 `analog`, 32–47 `tight`, else `standard`). Polyphonic
  pressure ignored. The state can be kept without emitting (a seek scan,
  remembering ≤ 32 held notes) and emitted at once (`restore`).
* **General MIDI sound set** (`gm`): 32 sounds of our own for the 128
  programs, families kept (bass stays bass, plucked stays plucked), CPU in
  mind (one or two oscillators through the filter, 2–3 operator FM; four
  operators only for the e-piano):

  | # | Sound | GM programs (0-based) | # | Sound | GM programs |
  |---|---|---|---|---|---|
  | 0 | piano (FM, 3 op) | 0–3 | 16 | solo strings | 40–43, 110 |
  | 1 | e-piano (FM, 4 op) | 4, 5 | 17 | strings | 44, 48–51 |
  | 2 | harpsichord | 6, 7 | 18 | harp | 45, 46 |
  | 3 | bell (FM 1:3.5) | 8–10, 14, 31, 98, 112 | 19 | timpani | 47, 116–118 |
  | 4 | mallet (FM 1:4) | 11–13, 108, 113–115 | 20 | choir | 52–54, 85, 91, 101 |
  | 5 | pluck | 15, 104–107 | 21 | brass | 55–59, 61 |
  | 6 | organ | 16–18 | 22 | horn | 60, 62, 63 |
  | 7 | church organ | 19 | 23 | sax | 64–67 |
  | 8 | reed organ | 20–23, 109 | 24 | reed | 68–71, 111 |
  | 9 | nylon guitar | 24 | 25 | flute | 72–79, 82, 83 |
  | 10 | steel guitar | 25–28 | 26 | square lead | 80 |
  | 11 | drive guitar | 29, 30, 84 | 27 | saw lead | 81, 86, 87 |
  | 12 | bass | 32–35 | 28 | pad (PWM) | 88, 90, 93, 95, 99, 100, 103 |
  | 13 | slap bass | 36, 37 | 29 | warm pad | 89, 92, 94, 96, 97, 102 |
  | 14 | synth bass (acid) | 38 | 30 | noise fx | 119–122, 125–127 |
  | 15 | synth bass 2 (FM) | 39 | 31 | chirp | 123, 124 |

  Nine reuse SYN presets (e-piano, pluck, organ, synth bass 2 and flute
  with their levels adjusted), 23 are new. Levels were balanced on the
  tour's per-bar RMS (−22 … −30 dBFS; the noise swell −33). The set is an
  `OSB1` bank, `crates/orpheus-midi/data/gm-sounds.osb` (7,312 B: the 32
  patches in this order, then the 5 factory kits), kept current and
  decoded back bit for bit by a test.
* **Player** (`MidiPlayer`, 70,224 B with the engine; the decoder-like
  source). `open(len, kar)` then `step(budget)` until ready: header,
  tracks, and one silent scan of the whole file (duration = last event +
  2 s, channels and notes, title from `@T` or track 0's name, the lyric
  mode), then a seek to 0. `render(&mut [i16])`/`render_f32`: interleaved
  stereo at the engine rate; engine events at their frame offsets, the
  block cut at program changes (a patch is installed between engine
  calls) and when the 32-event buffer is full; any block size gives the
  same samples. `seek(frame)` + `step`: rewind, silent scan to the target
  in ≤ budget events per step, `Engine::reset_at(seed, target)` (new in
  `orpheus-synth`: the control grid stays on the samples of a render from
  the start), the whole channel state installed, the melodic notes held
  at the target re-struck (option `chase`). Starting is a seek to 0, so a
  seek leaves the engine as playing from the start would, except what
  sounds across the target, the noise seeds and the voice slots' free-
  running LFO phases: **bit-exact after a seek into a silence** on patches
  without noise and LFO (tested at frame offsets 0, 17, 31 and from the
  next phrase after a seek into a phrase); with drums and effects the
  levels follow within 1–3 dB. Pause = not calling `render`. `position()`,
  `duration()`, `bpm()`; `set_mute`/`set_solo` (note-ons dropped, sounding
  notes released); lyric pieces queued with their frames (`pop_lyric`);
  `summary()` gives duration, title, tempo map and every lyric piece
  without an engine. Master gain 0.5: a fff six-note chord stays below
  full scale on every program (only the tuba reaches the limiter, −0.5 dB).
* **Lyrics** (`kar`). Karaoke files (`@K`/`@T` headers or a `.kar` name):
  text events not starting with `@`, `\` = new paragraph, `/` = new line;
  else lyric events (CR/LF and `/`/`\` as breaks). Bytes are passed on (the
  simulator decodes UTF-8, else Latin-1).
* **Simulator.** Feature `midi` of `orpheus-apps` (simulator only; off in
  the firmware) makes `.mid`, `.midi`, `.kar`, `.rmi` audio in Files and
  the library; the simulated player takes duration and title from the scan
  and shows `.kar` lyrics as timed lines in Now Playing (golden
  `media_np_kar_lyrics_*`); `set:queue=midi` writes the demo songs to
  `/Music/MIDI`. The built card image is unchanged.
* **Tests**: `tests/parse.rs` (16), `tests/play.rs` (16: exact tempo
  timing, golden hashes in `tests/goldens.txt` — also checked in a release
  build by `cargo run --release -p orpheus-midi --example render --
  --check` —, block-size independence, the GM sweep without NaN or full
  scale, seeks, `.kar` timing, mute/solo, duration/position, RAM ≤ 72 KiB,
  the bank), fuzz target `midi_parse` (`crates/orpheus-midi/fuzz`; 11
  seeds replayed by `tests/fuzz_corpus.rs`; 70,882 runs under ASan, no
  finding). `--example render <dir>` writes the three demo songs of our
  own (`demo`: an etude, a karaoke song, a tour of the sound set) as
  `.mid`/`.kar` and WAV.
* **Code** (thumbv7em, opt "s", no LTO): the play path ≈ 10 KB, the GM
  table 9.6 KB read-only; 0 fused multiply-add.
* **CPU estimate** (`--example estimate`: the demos' real voice mix ×
  SYN-1/2's per-voice disassembly cycles by sound, ±30 %, + engine/limiter
  135 and reverb 290 per sample): a 10–11-voice song ≈ 2,430 cycles/sample
  = **≈ 107 MHz at 44.1 kHz, 117 MHz at 48 kHz** (worst block, 16 voices +
  4 tails, ≈ 186/203 MHz); a 5–6-voice song ≈ 60–65 MHz; parser, clock
  and sequencer < 1 MHz. **MIDI needs the High clock (144 MHz)**; full
  polyphony peaks need Turbo/Max or a voice limit of ≈ 12 at High.
* **Left for SYN-3**: a card source answering the parser's `Need`s
  (64-byte reads at arbitrary offsets: a sector cache), the player in the
  decoder arena, `render` into the DAC ring, the clock floor and voice
  limit (measured), the `midi` feature in the firmware, lyrics and a
  mute/solo/tempo view in Now Playing. **SF2** (stretch): not done; the
  sample cache and `Kind::Sample` are the way in, but SoundFont parsing
  (RIFF, presets, zones, generators, modulators) is a package of its own.

## 10. Work packages

* **WP-AU-1 Pipeline core**: rings, pool leases, `ByteSource`, `Decoder`
  trait, reader/decode/sink tasks, WAV family, host simulation of timing,
  underrun accounting. DAC driver with hardware volume and pop-free
  power-up/down (with 03). HIL: tone and WAV CRC tests.
* **WP-AU-2 MP3** incl. gapless, seek, VBR headers. On-device benchmark and
  the first `mp3-frame` packing cluster.
* **WP-AU-3 FLAC** incl. Ogg FLAC, seek, MD5 check in tests.
* **WP-AU-4 Metadata, album art, lyrics.**
* **WP-AU-5 Playback service**: queue, locators, shuffle, repeat, resume,
  bookmarks, A–B, sleep timer hooks.
* **WP-AU-6 DSP**: ReplayGain, EQ, bass, crossfeed, balance, fades, limiter,
  decimator, dither, meters; then speed control.
* **WP-AU-7 Ogg + Vorbis.**  **WP-AU-8 Opus.**  **WP-AU-9 Speex.**
* **WP-AU-10 MP4 + AAC-LC + ALAC.**  **WP-AU-11 WavPack, APE.**
* **WP-AU-12 Directory index and library index, playlists.**
* **WP-AU-13 Recorder**: ADC driver (with 03), WAV/ADPCM, FLAC encoder,
  Opus/Speex voice encoder, AGC/VOX/markers, write-behind.
* **WP-AU-14 Pictures**: decoders, scaler, thumbnails, album-art path.
* **WP-AU-15 Optional formats** (WMA, trackers, CUE) as time allows.

Each codec package ends with: conformance and fuzz tests green, arena
high-water mark recorded, benchmark row filled in §4.3, packing cluster in
`hot.toml`, HIL CRC test on the device.
