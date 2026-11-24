# Add a codec

Decoders are `no_std`, allocation-free crates in `crates/codecs/<name>`,
host-tested first, then linked into the firmware. The design is
[04 §2 and §4](../../plan/04-audio.md).

1. **Licence first.** Port only permissively licensed sources (MIT,
   Apache-2.0, BSD); never GPL code (decision D-1). Record the origin and
   licence in the crate's README and in [THIRD-PARTY.md](../../../THIRD-PARTY.md)
   if its licence differs from MIT OR Apache-2.0.
2. **The crate.** `crates/codecs/<name>` (the workspace picks it up),
   depending on `orpheus-audio` (and `orpheus-ogg` for Ogg streams). Use
   `#![no_std]` and `#![forbid(unsafe_code)]`.
3. **Implement `orpheus_audio::Decoder`**: `ARENA_MIN`, `inspect` (parse
   the header without consuming), `open` (take every buffer from the
   arena; no allocation later), `decode` (one frame per call, then return
   so the task can yield), `seek`, `arena_used`. Read input only through
   `ByteSource`; it may return less than asked for (`NeedData`).
4. **Register it** in `crates/codecs/lib.rs`: a Cargo feature, a variant
   of `AnyDecoder`, and the arm in the probe/open `match`; add the format
   to `orpheus_audio::Format` if it is new, and to the probe.
5. **Conformance.** Add files for the codec to the corpus generator in
   `xtask/src/test.rs` (`cargo xtask test codecs --gen` encodes them with
   ffmpeg, sox or flac and decodes a reference with ffmpeg; the
   `orpheus-codec-conformance` binary compares within the stated
   tolerance), and seeds for the fuzz targets (`cargo xtask test fuzz-seeds`; see
   [add a fuzz target](add-fuzz-target.md)).
6. **Size and speed.** `cargo xtask test codec-size` reports the code
   size; the arena must fit the decoder arena of the mode pool. On the
   player, `cargo xtask dev cmd 'play /.orpheus/test/x.ext'` then
   `cargo xtask dev cmd status` shows decode time and pager line loads per
   call (`dec:`). If it pages heavily, pack the frame loop into a `hot.toml`
   cluster ([profile](profile.md)).
7. **Firmware**: enable the feature in `fw/firmware/Cargo.toml`.

Write down measured numbers (decode time per second of audio at each clock
level, arena size, code size) in STATUS and in 04 §4.
