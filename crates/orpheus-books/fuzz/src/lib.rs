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
//! * [`books_zip`] (BOOK-2) — any bytes as a zip and as DEFLATE: the end
//!   record searched in every 512-byte window as the EPUB job does, the
//!   central directory walked in pieces (byte 0 the piece size), the bytes
//!   from 1 inflated with a 32 KiB and a 1 KiB window in 1-, 7- and
//!   whole-input pieces with small output limits. Oracles: no panic, no
//!   stall; the same bytes or the same error whatever the pieces; never
//!   more than 1 MiB out (a bomb is cut there).
//! * [`books_epub`] (BOOK-2) — any bytes as an EPUB file through the whole
//!   job ([`convert_epub`]: an in-memory card, the 32 KiB scratch, the
//!   1 KiB sink the shelf empties). Oracles: no panic, no stall (steps
//!   bounded by the inflate caps); at most one 512-byte book read and
//!   [`WRITES_MAX`] table writes per step, nothing over 1 KiB of text per
//!   step; a well-formed stream; contents entries inside the text, each on
//!   a paragraph byte (or at the end).

#![forbid(unsafe_code)]

use orpheus_books::convert::{Converter, Format, MetaReader, ROOM};
use orpheus_books::enc::{Decoder, Encoding};
use orpheus_books::epub::{Epub, Fail, STEP_OUT, Step, Store, Table};
use orpheus_books::files::{TOC_ENTRY, decode_toc};
use orpheus_books::inflate::{self, Inflate, Scratch, Status};
use orpheus_books::layout::{self, MAX_PAGE_BYTES, Mark, Metrics, Paginator, Params, Skip, breaks_around};
use orpheus_books::norm::{BOLD, Item, Kind, PARA, Sink, TocEntry, item_at};
use orpheus_books::opf::Metadata;
use orpheus_books::zip::{CdWalk, find_end};

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
pub fn check_stream(s: &[u8]) {
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

// ----- BOOK-2: zip, inflate, EPUB -------------------------------------------

/// Poll a future that never waits (the in-memory store) to its end.
pub fn block_on<F: core::future::Future>(f: F) -> F::Output {
    let mut f = core::pin::pin!(f);
    let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
    loop {
        if let core::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

/// The card as the EPUB job sees it, in memory; counts what a step does.
#[derive(Default)]
pub struct MemStore {
    pub book: Vec<u8>,
    pub tables: [Vec<u8>; 4],
    /// Book reads and their bytes, table writes (since cleared).
    pub reads: u32,
    pub read_bytes: usize,
    pub puts: u32,
    pub put_bytes: usize,
}

fn table(t: Table) -> usize {
    match t {
        Table::Manifest => 0,
        Table::Spine => 1,
        Table::Targets => 2,
        Table::Toc => 3,
    }
}

impl Store for MemStore {
    async fn read(&mut self, at: u32, buf: &mut [u8]) -> usize {
        self.reads += 1;
        let at = (at as usize).min(self.book.len());
        let n = buf.len().min(self.book.len() - at);
        buf[..n].copy_from_slice(&self.book[at..at + n]);
        self.read_bytes += n;
        n
    }
    async fn get(&mut self, t: Table, at: u32, buf: &mut [u8]) -> bool {
        let v = &self.tables[table(t)];
        let at = at as usize;
        match v.get(at..at + buf.len()) {
            Some(s) => {
                buf.copy_from_slice(s);
                true
            }
            None => false,
        }
    }
    async fn put(&mut self, t: Table, at: u32, data: &[u8]) -> bool {
        let v = &mut self.tables[table(t)];
        let at = at as usize;
        if v.len() < at + data.len() {
            v.resize(at + data.len(), 0);
        }
        v[at..at + data.len()].copy_from_slice(data);
        self.puts += 1;
        self.put_bytes += data.len();
        true
    }
}

/// Table writes one step makes at most (records and offsets; a few
/// hundred bytes).
pub const WRITES_MAX: u32 = 20;

/// An EPUB converted by the job.
pub struct Converted {
    pub text: Vec<u8>,
    pub toc: Vec<TocEntry>,
    pub meta: Metadata,
    pub damaged: u16,
    pub steps: u32,
    /// The most any one step did.
    pub max_reads: u32,
    pub max_puts: u32,
    pub max_text: usize,
}

/// Convert `book` with a scratch of `scratch` bytes (0: none lent), the
/// sink emptied after every step as the shelf does. `Err` with the job's
/// refusal; panics on a stall, on a step doing more than bounded work, or
/// on a wait for the scratch while it is lent.
pub fn convert_epub(book: &[u8], scratch: usize) -> Result<Converted, Fail> {
    let mut st = MemStore { book: book.to_vec(), ..MemStore::default() };
    let mut mem = vec![0u8; scratch];
    let mut e = Epub::new(book.len() as u32);
    let mut sink = Bounded { buf: Vec::new() };
    let mut c = Converted {
        text: Vec::new(),
        toc: Vec::new(),
        meta: Metadata::default(),
        damaged: 0,
        steps: 0,
        max_reads: 0,
        max_puts: 0,
        max_text: 0,
    };
    // Every step reads, writes, or makes output; output is capped.
    let limit = 200_000 + book.len() as u64 * 8 + (orpheus_books::epub::BOOK_CAP / STEP_OUT as u64) * 4;
    loop {
        (st.reads, st.read_bytes, st.puts) = (0, 0, 0);
        let r = block_on(e.step(&mut st, (scratch > 0).then_some(&mut mem[..]), &mut sink));
        c.steps += 1;
        assert!(u64::from(c.steps) < limit, "the EPUB job stalls");
        c.max_reads = c.max_reads.max(st.reads);
        c.max_puts = c.max_puts.max(st.puts);
        c.max_text = c.max_text.max(sink.buf.len());
        assert!(st.reads <= 1 && st.read_bytes <= 512, "a step reads {} times", st.reads);
        assert!(st.puts <= WRITES_MAX, "a step writes {} times", st.puts);
        c.text.append(&mut sink.buf);
        match r {
            Step::Busy => {}
            Step::Done => break,
            Step::Failed(f) => return Err(f),
            Step::NeedScratch | Step::Lost if scratch == 0 => return Err(Fail::NoPackage),
            Step::NeedScratch | Step::Lost => panic!("{r:?} with the scratch lent"),
        }
    }
    assert_eq!(e.text_len() as usize, c.text.len(), "the job's length is the stream's");
    let toc = &st.tables[3];
    for i in 0..e.toc_n as usize {
        c.toc.push(decode_toc(&toc[i * TOC_ENTRY..(i + 1) * TOC_ENTRY]).expect("an entry"));
    }
    c.meta = *e.metadata();
    c.damaged = e.damaged;
    Ok(c)
}

/// Contents entries inside the text, on a paragraph byte or at its end.
pub fn check_toc(text: &[u8], toc: &[TocEntry]) {
    for e in toc {
        let at = e.offset as usize;
        assert!(at <= text.len(), "entry at {at} of {}", text.len());
        if at < text.len() {
            assert!((PARA..PARA + Kind::COUNT).contains(&text[at]), "entry at {at} not on a paragraph");
        }
        assert!((1..=3).contains(&e.level));
    }
}

pub fn books_epub(data: &[u8]) {
    if let Ok(c) = convert_epub(data, inflate::SCRATCH) {
        check_stream(&c.text);
        check_toc(&c.text, &c.toc);
        assert!(c.meta.title.as_str().len() <= 64);
    }
}

/// Inflate `z` in `piece`-byte pieces, at most `max` out per call, with a
/// window of `window` bytes; stops at 1 MiB out.
pub fn inflate_pieces(z: &[u8], piece: usize, max: usize, window: usize) -> Result<Vec<u8>, inflate::Error> {
    let mut mem = vec![0u8; window + inflate::TABLES];
    let mut sc = Scratch::new(&mut mem).expect("scratch");
    let mut d = Inflate::new();
    let mut out = Vec::new();
    let mut at = 0;
    let mut idle = 0;
    loop {
        let end = (at + piece).min(z.len());
        let r = d.run(&mut sc, &z[at..end], end == z.len(), max)?;
        let (a, b) = d.made(&sc, r.made);
        out.extend_from_slice(a);
        out.extend_from_slice(b);
        at += r.used;
        if r.status == Status::Done || out.len() >= 1 << 20 {
            out.truncate(1 << 20);
            return Ok(out);
        }
        idle = if r.used == 0 && r.made == 0 { idle + 1 } else { 0 };
        assert!(idle < 3, "inflate stalls");
    }
}

pub fn books_zip(data: &[u8]) {
    let [p, rest @ ..] = data else { return };
    // The end record in every window the job would read.
    let len = data.len() as u32;
    let mut at = data.len().saturating_sub(512);
    loop {
        let w = &data[at..(at + 512).min(data.len())];
        if let Some(Ok(e)) = find_end(w, at as u32, len) {
            assert!(u64::from(e.cd_at) + u64::from(e.cd_size) <= u64::from(len));
            break;
        }
        if at == 0 {
            break;
        }
        at = at.saturating_sub(491);
    }
    // The bytes as a central directory, in pieces.
    let piece = usize::from(*p) + 1;
    let mut walk = CdWalk::new(0);
    let mut name = orpheus_books::zip::Name::new();
    for c in data.chunks(piece) {
        let mut i = 0;
        let mut rounds = 0;
        while i < c.len() {
            let (n, _) = walk.feed(&c[i..], Some(&mut name));
            i += n;
            rounds += 1;
            assert!(rounds <= c.len() + 1, "the walk stalls");
        }
    }
    // The rest as DEFLATE: pieces and windows do not change the result
    // (beyond the 1 KiB window's refusal).
    let whole = inflate_pieces(rest, rest.len().max(1), 1 << 20, inflate::WINDOW);
    assert_eq!(inflate_pieces(rest, 1, 7, inflate::WINDOW), whole, "1-byte pieces");
    assert_eq!(inflate_pieces(rest, 7, 4096, inflate::WINDOW), whole, "7-byte pieces");
    match inflate_pieces(rest, 64, 333, 1024) {
        Err(inflate::Error::Window) => {}
        small => assert_eq!(small, whole, "a 1 KiB window"),
    }
}
