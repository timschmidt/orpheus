# Third-party material

Orpheus is licensed under MIT OR Apache-2.0 (`LICENSE-MIT`,
`LICENSE-APACHE`) except for the parts below, which keep their own licences.
All of them are permissive (or, for fonts, the SIL Open Font License) and
their notices travel with the code.

| Path | What | Licence | Notice |
|---|---|---|---|
| `crates/codecs/opus/` | Rust port of the Opus reference decoder (libopus) | BSD-3-Clause (Xiph.Org and contributors) | `crates/codecs/opus/LICENSE-XIPH` |
| `crates/codecs/speex/` | Rust port of the Speex decoder (libspeex 1.2.1) | BSD-3-Clause (Xiph.Org, Jean-Marc Valin and contributors) | `crates/codecs/speex/LICENSE-XIPH-SPEEX` |
| `crates/codecs/mp3/` | MP3 decoder crate (vendored, adapted) | MIT OR Apache-2.0 | `crates/codecs/mp3/LICENSE-MIT`, `LICENSE-APACHE` |
| `crates/codecs/vorbis/` | Written from the Vorbis I specification; the floor1 inverse-dB table as copied by `lewton` | MIT OR Apache-2.0 (`lewton`) | see the crate's README |
| `crates/orpheus-ui/fonts/` | DejaVu and Fira glyph data | DejaVu (Bitstream Vera) licence; SIL OFL 1.1 | `LICENSE-DejaVu.txt`, `LICENSE-Fira.txt` |
| `sim/assets/unifont-subset.hex` | GNU Unifont glyph subset | SIL OFL 1.1 | `sim/assets/OFL-unifont.txt` |
| `crates/orpheus-tts/src/lexicon.bin`, `crates/orpheus-tts/data/eval.tsv` | Pronunciations from the CMU Pronouncing Dictionary (cmudict, commit `74790861`), selected and re-encoded by `cargo xtask tts-lexicon` (30 entries for words cmudict lacks are the project's own, `lexgen::EXTRA`) | BSD-2-Clause-style (Carnegie Mellon University) | `crates/orpheus-tts/data/LICENSE-CMUDICT` |

The speech engine's letter-to-sound rules (`crates/orpheus-tts/src/rules.rs`)
are written from NRL Report 7948 (Elovitz, Johnson, McHugh, Shore, Naval
Research Laboratory, 1976), a work of the US government in the public
domain; no implementation of them was copied. The word counts that rank the
lexicon come from public-domain Project Gutenberg books (only counts are
used; nothing of the texts is kept).

The browser build (`sim/web`, the website's Try it page) contains the
codecs, fonts and glyphs above in compiled form; the page links this file.
Its demo card (`sim/web/demo/`, `sim/web/src/demo.rs`) is the project's own
work: music synthesised by `sim/web/src/synth.rs` (encoded with `ffmpeg`),
texts written for it, pictures drawn by code; no third-party content.

The Rockbox port in the companion repository is GPL and is used only as a
hardware reference; no Rockbox code is copied (decision D-1).
