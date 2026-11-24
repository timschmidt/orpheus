# yoton-meta

Tag, album-art and lyrics parsing for yotonrs (04-audio §4.4, WP AU-4).

## Why a crate of its own

04 §4.4 names the parser `yoton-audio::meta`. It is a separate crate so
that the library scanner (AU-12) can read tags without linking any codec,
the decode path links no tag parser, and each is fuzzed and paged on its
own. It depends on `yoton-audio` only for `ByteSource`/`SeekSource`/`View`.

No third-party crate is used: `nanoaudio-metadata` (MIT) parses from
in-memory slices only, and `id3-core` needs `alloc`. Streaming over the
ring with bounded memory and `NeedData` restarts is the requirement here.

## What is read

| Source | Fields |
|---|---|
| ID3v2.2/2.3/2.4 (leading; also inside WAV `id3 ` and AIFF `ID3 ` chunks) | `TIT2 TPE1 TALB TPE2 TCON TYER/TDRC TRCK TPOS` (and v2.2 ids), `TXXX` (ReplayGain, any Vorbis-style key), `COMM iTunSMPB` (gapless), `APIC`/`PIC` (location; front cover preferred), `USLT`, `SYLT` (locations). Latin-1 in a configurable code page (ISO-8859-1, Windows-1252, Windows-1251), UTF-16 with/without BOM, UTF-16BE, UTF-8. Tag-level (v2.2/2.3) and frame-level (v2.4) unsynchronisation, data-length indicators, grouping bytes, extended headers; compressed and encrypted frames skipped |
| APEv2 (end of file) | Text items by key, `Lyrics`, `Cover Art (Front)` (location) |
| ID3v1/1.1 (end of file) | Title, artist, album, year, track, genre (80 standard genres) |
| FLAC metadata | `VORBIS_COMMENT` (all keys below, `LYRICS`/`UNSYNCEDLYRICS` as a location), `PICTURE` (location) |
| RIFF `LIST/INFO` | `INAM IART IPRD IGNR ICRD ITRK/IPRT` |
| AIFF | `NAME`, `AUTH` |

Keys understood everywhere (case-insensitive): `TITLE ARTIST ALBUM
ALBUMARTIST GENRE DATE/YEAR TRACKNUMBER TRACKTOTAL DISCNUMBER DISCTOTAL
REPLAYGAIN_{TRACK,ALBUM}_{GAIN,PEAK} R128_{TRACK,ALBUM}_GAIN ITUNSMPB`.

Priority: the leading ID3v2 tag (or the FLAC Vorbis comment) sets fields;
APEv2 then ID3v1 only fill fields still empty.

Lyrics readers: `lrc::LrcReader` (`[mm:ss.xx]` tags, several per line,
`[offset:±ms]`, UTF-8 or the code page per line) and `sylt::SyltReader`
(ID3 `SYLT` bodies, millisecond or MPEG-frame stamps).

Not here yet: MP4 `ilst` (with the MP4 container, AU-10) and Ogg comment
headers (with the Ogg layer, AU-7); the Vorbis comment parser is ready for
the latter.

## Memory and restarts

* `Tags`: five 127-byte text fields plus numbers and locations: 808
  bytes. Pictures and lyrics are never copied: `Located { offset, skip,
  len, unsync }` says where to read them.
* `TagReader`: 600 bytes (a 528-byte work buffer and the parser state). One unit
  (frame header + at most 512 bytes of content, a 32-byte APE footer, a
  128-byte ID3v1 tag) is peeked at a time; the rest is skipped by
  consuming or, for large blocks, by seeking.
* `step` answers `NeedData(n)` (call again with `n` bytes buffered; nothing
  needed was consumed), `Continue` (work bound of one call reached: 256
  units) or `Done`. No recursion; every pass consumes, seeks forward or
  changes phase, which the fuzz target checks (`Continue` count bounded by
  the file length).

## Tests

`cargo test -p yoton-meta`: hand-built ID3v2.2/2.3/2.4 tags (all
encodings, unsynchronised whole tag, unsynchronised v2.4 frame with a data
length indicator), APEv2 + ID3v1 priority, FLAC comments and picture, RIFF
INFO, LRC and SYLT; every case also through a small simulated ring with
trickling refills, compared with the whole-file result; picture and lyric
locations checked by reading the bytes they point at; 3 000 mutated tags
per container; corpus files (ffmpeg- and flac-written tags) when the
corpus exists. Fuzz targets `meta` and `lyrics` (`cargo xtask test
fuzz-smoke`).
