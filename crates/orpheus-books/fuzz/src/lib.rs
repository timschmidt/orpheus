//! Fuzz-target bodies of `orpheus-books` (BOOK-1, VER-4), shared by the
//! cargo-fuzz targets in `fuzz_targets/` (nightly) and the stable replay
//! of the committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! * [`books_convert`] — a book file of any bytes through encoding
//!   detection, a format's parser and the writer, in pieces through a
//!   bounded sink as the card job feeds it; the library's title reader and
//!   every decoder over the same bytes. Input: byte 0 the format (mod 3:
//!   TXT, Markdown, FB2), byte 1 the piece size (1–256); the rest is the
//!   file. Oracles: no panic, no stall (every piece is taken in bounded
//!   rounds); the stream is well formed (control bytes only paragraph and
//!   style bytes, nothing cut at its end, its offset its length, no empty
//!   paragraph but a break, no space at a paragraph's start or end, no two
//!   spaces); contents entries in order, inside the text, on heading
//!   paragraph bytes of their level; at most 8 bytes out per byte in
//!   (+ 64); the same stream for one piece and for the asked pieces; laid
//!   out, pages of at most `MAX_PAGE_BYTES` (+ the one space a page may
//!   start after) and the stable index of [`books_layout`].
//! * [`books_layout`] — a normalised stream of any bytes (a damaged cache
//!   file) laid out: byte 0 the width (16 + 0–255 px), byte 1 the line
//!   pitch and page height. Oracles: no panic; page starts strictly
//!   increasing; the same pages
//!   for 1-, 7- and 64-byte pieces; every page drawn from its start ends
//!   where the next starts (the stable index); lines never wider than the
//!   width unless a single character is.

#![forbid(unsafe_code)]

use orpheus_books::convert::{Converter, Format, MetaReader, ROOM};
use orpheus_books::enc::{Decoder, Encoding};
use orpheus_books::layout::{self, MAX_PAGE_BYTES, Mark, Metrics, Paginator, Params, Skip, breaks_around};
use orpheus_books::norm::{BOLD, Item, Kind, PARA, Sink, TocEntry, item_at};

/// A sink of 1 KiB like the card job's, emptied into `all` when the
/// converter stops.
struct Bounded {
    buf: Vec<u8>,
}

impl Sink for Bounded {
    fn put(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
        assert!(self.buf.len() <= 1024, "the converter overran its sink");
    }
    fn room(&self) -> usize {
        1024 - self.buf.len()
    }
}

fn convert(format: Format, file: &[u8], piece: usize) -> (Vec<u8>, Vec<TocEntry>) {
    let head = &file[..file.len().min(512)];
    let mut c = Converter::new(format, head, file.len() <= 512);
    let mut sink = Bounded { buf: Vec::new() };
    let mut all = Vec::new();
    let mut toc = Vec::new();
    let mut at = 0;
    let mut idle = 0;
    while at < file.len() {
        let end = (at + piece).min(file.len());
        let used = c.feed(&file[at..end], &mut sink);
        at += used;
        if let Some(e) = c.take_toc() {
            toc.push(e);
        }
        if used == 0 || sink.room() < ROOM {
            all.append(&mut sink.buf);
        }
        idle = if used == 0 { idle + 1 } else { 0 };
        assert!(idle < 4, "the converter stalls");
    }
    all.append(&mut sink.buf);
    c.finish(&mut sink);
    if let Some(e) = c.take_toc() {
        toc.push(e);
    }
    all.append(&mut sink.buf);
    assert_eq!(c.offset() as usize, all.len(), "offset is the length");
    (all, toc)
}

/// The stream's well-formedness.
fn check_stream(s: &[u8]) {
    let mut i = 0;
    let mut para: Option<(Kind, usize)> = None;
    let mut prev_space = false;
    let mut chars_in_para = 0usize;
    while i < s.len() {
        let (it, n) = item_at(s, i).expect("nothing cut at the end");
        assert!(core::str::from_utf8(&s[i..i + n]).is_ok() || n == 1, "valid UTF-8 at {i}");
        match it {
            Item::Para(k) => {
                if let Some((pk, _)) = para {
                    assert!(pk == Kind::Break || chars_in_para > 0, "empty paragraph at {i}");
                    assert!(!prev_space, "space at a paragraph's end before {i}");
                }
                para = Some((k, i));
                chars_in_para = 0;
                prev_space = false;
            }
            Item::Style(f) => assert!(f <= 3),
            Item::Char(c) => {
                assert!(para.is_some(), "text before the first paragraph");
                assert!(u32::from(s[i]) >= 0x20, "a control byte at {i}");
                if c == ' ' {
                    assert!(chars_in_para > 0, "space at a paragraph's start at {i}");
                    assert!(!prev_space, "two spaces at {i}");
                }
                assert!(para.map(|p| p.0) != Some(Kind::Break), "text in a break at {i}");
                prev_space = c == ' ';
                chars_in_para += 1;
            }
        }
        i += n;
    }
    if let Some((pk, _)) = para {
        assert!(pk == Kind::Break || chars_in_para > 0, "empty last paragraph");
        assert!(!prev_space, "space at the end");
    }
}

pub fn books_convert(data: &[u8]) {
    let [f, p, file @ ..] = data else { return };
    let format = Format::from_u8(f % 3);
    let piece = usize::from(*p) + 1;
    let (out, toc) = convert(format, file, piece);
    check_stream(&out);
    assert!(out.len() <= file.len() * 8 + 64, "{} bytes from {}", out.len(), file.len());
    let mut last = None;
    for e in &toc {
        assert!((e.offset as usize) < out.len());
        assert!(last.is_none_or(|l| l < e.offset), "contents in order");
        last = Some(e.offset);
        let b = out[e.offset as usize];
        assert!((PARA..PARA + Kind::COUNT).contains(&b));
        assert_eq!(Kind::from_u8(b - PARA).level(), Some(e.level), "a heading of its level");
        assert!(e.title().len() <= orpheus_books::norm::TITLE_MAX);
    }
    let (whole, whole_toc) = convert(format, file, file.len().max(1));
    assert_eq!(whole, out, "piece size changes the stream");
    assert_eq!(whole_toc, toc);
    // Laid out: bounded pages, the stable index.
    let p = Params {
        width: 60 + u16::from(*p % 64),
        height: 70,
        line: 14,
        gap: 2,
        indent: 6,
        inset: 6,
        justify: true,
        font: 0,
    };
    let marks = lay_out(&out, p);
    for w in marks.windows(2) {
        assert!(w[1].offset - w[0].offset <= MAX_PAGE_BYTES + 1, "a page of at most MAX_PAGE_BYTES");
    }
    stable(&out, p, &marks);
    // The library's reader and the decoders.
    let mut m = MetaReader::new(format, &file[..file.len().min(512)], file.len() <= 512);
    for c in file.chunks(piece) {
        if !m.wants_more() {
            break;
        }
        m.feed(c);
    }
    let meta = m.finish();
    assert!(meta.title.as_str().len() <= 64 && meta.author.as_str().len() <= 48);
    for e in Encoding::ALL {
        let mut d = Decoder::new(e);
        let mut n = 0usize;
        d.feed(file, &mut |_| n += 1);
        d.finish(&mut |_| n += 1);
        assert!(n <= file.len() + 1);
    }
}

/// Every character 6 px (bold 7), wide ones 12, space 4.
struct Mono;
impl Metrics for Mono {
    fn advance(&self, c: char, style: u8) -> u16 {
        let base = if breaks_around(c) {
            12
        } else if c == ' ' {
            4
        } else if u32::from(c) < 0x20 {
            0
        } else {
            6
        };
        base + u16::from(style & BOLD != 0)
    }
}

fn pages(s: &[u8], p: Params, piece: usize) -> Vec<Mark> {
    let mut pg = Paginator::new(p);
    let mut marks = vec![Mark::START];
    for c in s.chunks(piece) {
        pg.feed(c, &Mono, &mut |m| marks.push(m));
    }
    pg.finish(&Mono, &mut |m| marks.push(m));
    marks
}

pub fn books_layout(data: &[u8]) {
    let [w, l, s @ ..] = data else { return };
    let line = 8 + u16::from(l % 32);
    let p = Params {
        width: 16 + u16::from(*w),
        height: line * (1 + u16::from(l / 32)) + 3,
        line,
        gap: u16::from(l % 5),
        indent: u16::from(w % 13),
        inset: u16::from(w % 7),
        justify: false,
        font: 0,
    };
    let marks = lay_out(s, p);
    assert_eq!(pages(s, p, 1), marks, "1-byte pieces");
    assert_eq!(pages(s, p, 7), marks, "7-byte pieces");
    stable(s, p, &marks);
}

/// Pages in 64-byte pieces, their starts increasing.
fn lay_out(s: &[u8], p: Params) -> Vec<Mark> {
    let marks = pages(s, p, 64);
    for w in marks.windows(2) {
        assert!(w[0].offset < w[1].offset, "page starts increase");
    }
    marks
}

/// Every page drawn from its start ends where the next starts.
fn stable(s: &[u8], p: Params, marks: &[Mark]) {
    for (k, m) in marks.iter().enumerate() {
        let info = layout::page(s, 0, true, *m, Skip::None, p, &Mono, &mut |line, _| {
            let widest = 12 + 1;
            assert!(
                line.width <= p.avail(line.mark.kind, line.mark.first) || line.width <= widest,
                "a line wider than the page"
            );
        });
        assert_eq!(info.next, marks.get(k + 1).copied(), "page {k} drawn from its start ends at the next");
    }
}
