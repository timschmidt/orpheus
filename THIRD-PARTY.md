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

Its demo card (`sim/web/demo/card/`, served as `try/pkg/demo.bin`; the
player's version is built by `cargo xtask demo-card`) holds third-party
works, each pinned with its source, SHA-256, licence and changes in
[`sim/web/demo/sources.toml`](sim/web/demo/sources.toml) and credited in
[`sim/web/demo/CREDITS.md`](sim/web/demo/CREDITS.md) and on the card in
`CREDITS.txt`. They keep their own terms, which are not the project's
licence:

| Files under `sim/web/demo/card/` | Work | Licence |
|---|---|---|
| `Music/Bach/Goldberg Variations/` | Bach, Goldberg Variations: Aria, Variatio 4; Kimiko Ishizaka (2012) | CC0 1.0 |
| `Music/Beethoven/` | Beethoven, Symphony No. 5, i.; Skidmore College Orchestra (Musopen) | Public domain |
| `Music/Caruso/*.opus` | Leoncavallo, “Vesti la giubba”; Enrico Caruso (1907) | Public domain |
| `Music/Caruso/*.lrc` | Subtitles by Wikimedia Commons user Pagliaccious (libretto public domain) | CC BY-SA 3.0 |
| `Music/Chopin/` | Chopin, Nocturne Op. 9 No. 2; Peter Johnston (Musopen) | CC0 1.0 |
| `Music/Satie/` | Satie, Gymnopédie No. 1; Daria Baiocchi (2021) | CC BY-SA 4.0 |
| `Music/Sousa/` | Sousa, The Stars and Stripes Forever; Sousa's Band (Edison, 1909) | Public domain |
| `Music/Bulfinch/` | Bulfinch, The Age of Fable ch. 24; LibriVox, read by roolynninms | Public domain |
| `MIDI/` | Bach, Prelude BWV 846; Mutopia Project, Tobias Erbsland | Public domain |
| `Pictures/Ring Nebula in Lyra.png` | Judy Schmidt, from Hubble data | CC BY 2.0 |
| `Pictures/` (the others) | Corot (1861), Hokusai (c. 1831), NASA Earthrise (1968), Muybridge (1878) | Public domain |
| `Books/` | Bulfinch, The Age of Fable (PG #3327); Browning, The Pied Piper of Hamelin (PG #18343) | Public domain; Project Gutenberg files unmodified, with the Project Gutenberg License |

`About this card.txt` and `Music/Demo tour.m3u8` are the project's own.

The Rockbox port in the companion repository is GPL and is used only as a
hardware reference; no Rockbox code is copied (decision D-1).
