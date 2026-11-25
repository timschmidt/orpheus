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
| Tap for meters/spectrum | Peak detector always; 256-point FFT at ≤ 15 Hz only while the Levels view is visible | — |

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
  English from the public-domain 1976 NRL rule set plus a compressed
  exception lexicon built from a permissively licensed dictionary (CMUdict,
  BSD-style), Russian by its largely phonemic orthography with a stress
  dictionary for common words and a heuristic otherwise, Japanese kana
  only (kanji are not read in v1); a spell-out mode for unknown strings.
* **Synthesiser**: own formant synthesiser (cascade/parallel resonators
  after the published Klatt design; written from the papers, no code
  copied), 16 kHz internal rate resampled to the DAC rate, phoneme
  durations and formant targets per language, prosody (declination,
  phrase-final lengthening, question rise, emphasis), two or three voices as
  parameter sets (pitch, formant scaling, breathiness).
* **Output**: a speech voice in the audio mixer next to the music path,
  with ducking or pausing of the music, its own volume and the DSP chain
  bypassed; works during playback, radio and with the screen off.
* **Budgets**: ≤ 64 KiB paged code and tables (lexicons paged as data),
  ≤ 8 KiB RAM, ≤ 20 % CPU at the Base clock level, first sound ≤ 150 ms
  after a request; numbers in `budgets.toml`.
* **Not used**: eSpeak NG (GPL) and SAM (no licence) — licence D-1. Flite
  (BSD-style) may be studied for its data only if the sizes fit.
* **Tests**: phoneme-string goldens for word lists per language; the
  normaliser and the rule engine fuzzed; intelligibility measured on the
  host by running the synthesised audio through an offline speech
  recogniser (word error rate below a budget on a fixed sentence set);
  device output checked through the M13 capture path.

## 9. Work packages

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
