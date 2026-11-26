//! A book file to the normalised stream, in pieces of any size: encoding
//! detection from the head, the format's parser, the [`Writer`]; and the
//! library's title and author from a file's head ([`MetaReader`]).

use crate::enc::{Decoder, Encoding, bom, detect, xml_declared};
use crate::fb2::{Fb2, MetaText};
use crate::md::Md;
use crate::norm::{Emit, Events, ITALIC, Kind, Sink, TocEntry, Writer};
use crate::txt::{self, Txt};
use crate::xhtml::Xhtml;

/// The book formats (BOOK-1; EPUB BOOK-2).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Format {
    #[default]
    Txt = 0,
    Md = 1,
    Fb2 = 2,
    Epub = 3,
}

impl Format {
    /// From a file name's extension.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Format> {
        let ext = name.rsplit_once('.')?.1;
        let is = |e: &str| ext.eq_ignore_ascii_case(e);
        if is("txt") || is("text") {
            Some(Format::Txt)
        } else if is("md") || is("markdown") {
            Some(Format::Md)
        } else if is("fb2") {
            Some(Format::Fb2)
        } else if is("epub") {
            Some(Format::Epub)
        } else {
            None
        }
    }

    #[must_use]
    pub fn from_u8(v: u8) -> Format {
        match v {
            1 => Format::Md,
            2 => Format::Fb2,
            3 => Format::Epub,
            _ => Format::Txt,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Parser {
    Txt(Txt),
    Md(Md),
    Fb2(Fb2),
    Xhtml(Xhtml),
}

impl Parser {
    fn char(&mut self, c: char, ev: &mut impl Events) {
        match self {
            Parser::Txt(p) => p.char(c, ev),
            Parser::Md(p) => p.char(c, ev),
            Parser::Fb2(p) => p.char(c, ev),
            Parser::Xhtml(p) => p.char(c, ev),
        }
    }

    fn finish(&mut self, ev: &mut impl Events) {
        match self {
            Parser::Txt(p) => p.finish(ev),
            Parser::Md(p) => p.finish(ev),
            Parser::Fb2(p) => p.finish(ev),
            Parser::Xhtml(p) => p.finish(ev),
        }
    }
}

/// Free room the converter wants in its sink before it takes another
/// input byte: the most one byte can make (a held line of plain text of
/// 48 characters, a paragraph and a style byte, a list number).
pub const ROOM: usize = 256;

/// Contents entries a part's anchors may resolve at once (EPUB).
pub const WATCH: usize = 16;

/// EPUB contents entries waiting for the anchor (an element's id) they
/// point at in the part being converted: the first one found gives the
/// entry its offset ([`Watch::take_hit`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct Watch {
    /// (fragment hash, contents entry), in contents order.
    slots: [(u32, u32); WATCH],
    n: u8,
    hit: Option<(u32, u32)>,
    last: Option<(u32, u32)>,
}

impl Watch {
    pub fn clear(&mut self) {
        *self = Watch::default();
    }

    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.n)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Watch for `frag` on behalf of contents entry `toc`; false if full.
    pub fn add(&mut self, frag: u32, toc: u32) -> bool {
        let Some(s) = self.slots.get_mut(usize::from(self.n)) else { return false };
        *s = (frag, toc);
        self.n += 1;
        true
    }

    fn anchor(&mut self, id: u32, at: u32) {
        if self.hit.is_none() {
            self.last = Some((id, at));
            self.find();
        }
    }

    fn find(&mut self) {
        let Some((id, at)) = self.last else { return };
        let n = usize::from(self.n);
        match self.slots[..n].iter().position(|s| s.0 == id) {
            Some(k) => {
                self.hit = Some((self.slots[k].1, at));
                self.slots.copy_within(k + 1..n, k);
                self.n -= 1;
            }
            None => self.last = None,
        }
    }

    /// A resolved entry: (contents entry, offset). Others watching the
    /// same anchor follow on the next calls.
    pub fn take_hit(&mut self) -> Option<(u32, u32)> {
        let h = self.hit.take();
        if h.is_some() {
            self.find();
        }
        h
    }

    /// An entry still watched (its anchor not found), taken out.
    pub fn pop_unresolved(&mut self) -> Option<u32> {
        if self.n == 0 {
            return None;
        }
        let toc = self.slots[0].1;
        self.slots.copy_within(1..usize::from(self.n), 0);
        self.n -= 1;
        Some(toc)
    }
}

/// The parser's events with anchors told to the watch (if any).
struct Watched<'a> {
    e: Emit<'a>,
    watch: Option<&'a mut Watch>,
}

impl Events for Watched<'_> {
    fn para(&mut self, kind: Kind) {
        self.e.para(kind);
    }
    fn end_para(&mut self) {
        self.e.end_para();
    }
    fn style(&mut self, flags: u8) {
        self.e.style(flags);
    }
    fn text(&mut self, c: char) {
        self.e.text(c);
    }
    fn title(&mut self, open: bool) {
        self.e.title(open);
    }
    fn anchor(&mut self, id: u32) {
        let at = self.e.w.anchor_at();
        if let Some(w) = self.watch.as_deref_mut() {
            w.anchor(id, at);
        }
    }
}

/// The converter's state (≈ 400 bytes; the XHTML parser is the largest).
#[derive(Clone, Copy, Debug)]
pub struct Converter {
    dec: Decoder,
    parser: Parser,
    w: Writer,
    /// Bytes of a byte-order mark still to skip.
    skip: u8,
    /// The part's encoding was named (a byte-order mark, the XML
    /// declaration): a `<meta>` does not change it.
    named: bool,
    pub format: Format,
}

impl Converter {
    /// A converter for a file of `format` whose first bytes are `head`
    /// (the whole file if `complete`). The head is not consumed: feed the
    /// file from its start.
    #[must_use]
    pub fn new(format: Format, head: &[u8], complete: bool) -> Converter {
        let (enc, bom) = detect(head, complete);
        let parser = match format {
            Format::Txt => {
                let mut d = Decoder::new(enc);
                let mut mode = txt::Mode::Join;
                let mut len = 0usize;
                let mut f = |c: char| {
                    if c == '\n' || c == '\r' {
                        len = 0;
                    } else {
                        len += 1;
                        if len > txt::LONG_LINE {
                            mode = txt::Mode::Lines;
                        }
                    }
                };
                d.feed(head.get(bom..).unwrap_or(&[]), &mut f);
                Parser::Txt(Txt::new(mode))
            }
            Format::Md => Parser::Md(Md::new()),
            Format::Fb2 => Parser::Fb2(Fb2::new()),
            Format::Epub => Parser::Xhtml(Xhtml::new()),
        };
        Converter {
            dec: Decoder::new(enc),
            parser,
            w: Writer::new(),
            skip: u8::try_from(bom).unwrap_or(0),
            named: true,
            format,
        }
    }

    /// EPUB: the next content document starts (`head`: its first bytes;
    /// the whole of it if `complete`). The writer goes on: one stream.
    pub fn begin_part(&mut self, head: &[u8], complete: bool) {
        let (enc, mark) = detect(head, complete);
        self.dec = Decoder::new(enc);
        self.skip = u8::try_from(mark).unwrap_or(0);
        self.named = bom(head).is_some()
            || xml_declared(head).is_some()
            || matches!(enc, Encoding::Utf16Le | Encoding::Utf16Be);
        // The watch was loaded for this part before its first bytes.
        self.parser = Parser::Xhtml(Xhtml::new());
    }

    /// EPUB: the content document ends (the sink needs [`ROOM`] bytes
    /// free).
    pub fn end_part(&mut self, out: &mut dyn Sink) {
        let (dec, parser, w) = (&mut self.dec, &mut self.parser, &mut self.w);
        let mut ev = Watched { e: Emit { w, out }, watch: None };
        dec.finish(&mut |c| parser.char(c, &mut ev));
        parser.finish(&mut ev);
        ev.end_para();
    }

    /// A paragraph of its own in italics (EPUB: `[damaged]` where a part
    /// could not be read); the sink needs [`ROOM`] bytes free.
    pub fn note(&mut self, text: &str, out: &mut dyn Sink) {
        self.w.para(out, Kind::Body);
        self.w.style(ITALIC);
        for c in text.chars() {
            self.w.text(out, c);
        }
        self.w.end_para(out);
    }

    /// Headings make contents entries (on unless the book has its own).
    pub fn headings(&mut self, on: bool) {
        self.w.collect = on;
    }

    /// Where an anchor here points (EPUB: a part's start).
    #[must_use]
    pub fn anchor_at(&self) -> u32 {
        self.w.anchor_at()
    }

    #[must_use]
    pub fn encoding(&self) -> Encoding {
        self.dec.encoding()
    }

    /// Bytes of normalised text written so far.
    #[must_use]
    pub fn offset(&self) -> u32 {
        self.w.offset
    }

    /// Feed `input`. Stops early (returning the bytes taken) when `out`
    /// has less than [`ROOM`] bytes free or a contents entry waits for
    /// [`Converter::take_toc`].
    pub fn feed(&mut self, input: &[u8], out: &mut dyn Sink) -> usize {
        self.feed_watched(input, out, None)
    }

    /// [`Converter::feed`], anchors told to `watch` (EPUB): stops early
    /// when one resolves an entry ([`Watch::take_hit`]).
    pub fn feed_watched(&mut self, input: &[u8], out: &mut dyn Sink, mut watch: Option<&mut Watch>) -> usize {
        let mut used = 0;
        for &b in input {
            if out.room() < ROOM || self.w.done.is_some() || watch.as_ref().is_some_and(|w| w.hit.is_some()) {
                break;
            }
            used += 1;
            if self.skip > 0 {
                self.skip -= 1;
                continue;
            }
            let (dec, parser, w) = (&mut self.dec, &mut self.parser, &mut self.w);
            let mut ev = Watched { e: Emit { w, out: &mut *out }, watch: watch.as_deref_mut() };
            dec.feed(&[b], &mut |c| parser.char(c, &mut ev));
            if let Parser::Xhtml(x) = parser
                && let Some(e) = x.take_charset()
                && !self.named
            {
                // `<meta charset>`: the bytes after the tag in its encoding.
                self.dec = Decoder::new(e);
                self.named = true;
            }
        }
        used
    }

    /// A finished contents entry, if one waits.
    pub fn take_toc(&mut self) -> Option<TocEntry> {
        self.w.done.take()
    }

    /// End of the file (the sink needs [`ROOM`] bytes free).
    pub fn finish(&mut self, out: &mut dyn Sink) {
        let (dec, parser, w) = (&mut self.dec, &mut self.parser, &mut self.w);
        {
            let mut ev = Emit { w, out: &mut *out };
            dec.finish(&mut |c| parser.char(c, &mut ev));
            parser.finish(&mut ev);
        }
        self.w.finish(out);
    }
}

/// Bytes of a file's head [`MetaReader`] looks at, at most.
pub const META_HEAD: usize = 4096;

/// A book's title and author for the library.
#[derive(Clone, Copy, Debug, Default)]
pub struct Meta {
    pub title: MetaText<64>,
    pub author: MetaText<48>,
}

/// Events nobody wants (the meta reader runs the FB2 parser for its
/// description only).
struct Nothing;

impl Events for Nothing {
    fn para(&mut self, _: Kind) {}
    fn end_para(&mut self) {}
    fn style(&mut self, _: u8) {}
    fn text(&mut self, _: char) {}
    fn title(&mut self, _: bool) {}
}

/// Bytes of a line looked at (a `Title:` line longer is cut).
const LINE: usize = 160;

/// Title and author from the head of a file, in pieces: FB2's
/// `<title-info>`; Markdown's front matter (`title:`, `author:`) or first
/// `#` heading; plain text's `Title:` and `Author:` lines (Project
/// Gutenberg's header). Empty if the head does not say.
#[derive(Clone, Copy, Debug)]
pub struct MetaReader {
    format: Format,
    dec: Decoder,
    skip: u8,
    fb2: Fb2,
    /// The current line's first bytes (UTF-8).
    line: [u8; LINE],
    line_n: u8,
    lines: u16,
    front: u8,
    /// Bytes fed so far.
    pub fed: u32,
    pub meta: Meta,
}

impl MetaReader {
    #[must_use]
    pub fn new(format: Format, head: &[u8], complete: bool) -> MetaReader {
        let (enc, bom) = detect(head, complete);
        MetaReader {
            format,
            dec: Decoder::new(enc),
            skip: u8::try_from(bom).unwrap_or(0),
            fb2: Fb2::new(),
            line: [0; LINE],
            line_n: 0,
            lines: 0,
            front: 0,
            fed: 0,
            meta: Meta::default(),
        }
    }

    /// Whether more of the file could still tell more.
    #[must_use]
    pub fn wants_more(&self) -> bool {
        // EPUB's title is in its package document ([`crate::epub`]).
        self.format != Format::Epub
            && (self.fed as usize) < META_HEAD
            && (self.front == 1
                || self.meta.title.is_empty()
                || (self.format != Format::Md && self.meta.author.is_empty()))
    }

    pub fn feed(&mut self, input: &[u8]) {
        for &b in input {
            self.fed += 1;
            if self.skip > 0 {
                self.skip -= 1;
                continue;
            }
            let mut tmp = [' '; 2];
            let mut n = 0;
            self.dec.feed(&[b], &mut |c| {
                if let Some(s) = tmp.get_mut(n) {
                    *s = c;
                    n += 1;
                }
            });
            for &c in tmp.iter().take(n) {
                self.char(c);
            }
        }
    }

    /// End of what was read: the title and author found.
    #[must_use]
    pub fn finish(mut self) -> Meta {
        if self.format != Format::Fb2 && self.line_n > 0 {
            self.line_done();
        }
        if self.format == Format::Fb2 {
            (self.meta.title, self.meta.author) = self.fb2.meta();
        }
        self.meta
    }

    fn char(&mut self, c: char) {
        if self.format == Format::Fb2 {
            self.fb2.char(c, &mut Nothing);
            return;
        }
        if self.format == Format::Epub {
            return;
        }
        if c == '\n' || c == '\r' {
            self.line_done();
            return;
        }
        let mut tmp = [0u8; 4];
        let b = c.encode_utf8(&mut tmp).as_bytes();
        let at = usize::from(self.line_n);
        if let Some(dst) = self.line.get_mut(at..at + b.len()) {
            dst.copy_from_slice(b);
            self.line_n += b.len() as u8;
        }
    }

    fn line_done(&mut self) {
        let n = usize::from(self.line_n);
        self.line_n = 0;
        let buf = self.line;
        let line = core::str::from_utf8(&buf[..n]).unwrap_or("").trim();
        if line.is_empty() {
            return;
        }
        let first = self.lines == 0;
        self.lines = self.lines.saturating_add(1);
        let value = |key: &str| -> Option<&str> {
            let (k, v) = line.split_once(':')?;
            k.trim().eq_ignore_ascii_case(key).then(|| v.trim().trim_matches(&['"', '\''][..]))
        };
        match self.format {
            Format::Md => {
                if first && line == "---" {
                    self.front = 1;
                    return;
                }
                if self.front == 1 {
                    if line == "---" || line == "..." {
                        self.front = 2;
                    } else if let Some(v) = value("title") {
                        self.meta.title.clear();
                        self.meta.title.push_str(v);
                    } else if let Some(v) = value("author") {
                        self.meta.author.clear();
                        self.meta.author.push_str(v);
                    }
                    return;
                }
                if self.meta.title.is_empty()
                    && let Some(h) = line.strip_prefix("# ")
                {
                    self.meta.title.push_str(h.trim());
                }
            }
            _ => {
                if self.meta.title.is_empty()
                    && let Some(v) = value("title")
                {
                    self.meta.title.push_str(v);
                } else if self.meta.author.is_empty()
                    && let Some(v) = value("author")
                {
                    self.meta.author.push_str(v);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::norm::tests::show;
    use std::string::String;
    use std::vec::Vec;

    /// A sink with a fixed room, like the card job's buffer.
    struct Bounded(Vec<u8>, usize);
    impl Sink for Bounded {
        fn put(&mut self, b: &[u8]) {
            self.0.extend_from_slice(b);
        }
        fn room(&self) -> usize {
            self.1.saturating_sub(self.0.len())
        }
    }

    /// Convert in pieces of `piece` bytes through a sink of 1 KiB that is
    /// emptied whenever the converter stops: the same stream as at once.
    pub fn convert_pieces(format: Format, file: &[u8], piece: usize) -> (Vec<u8>, Vec<TocEntry>) {
        let mut c = Converter::new(format, &file[..file.len().min(512)], file.len() <= 512);
        let mut all = Vec::new();
        let mut toc = Vec::new();
        let mut sink = Bounded(Vec::new(), 1024);
        let mut at = 0;
        while at < file.len() {
            let end = (at + piece).min(file.len());
            let used = c.feed(&file[at..end], &mut sink);
            at += used;
            if let Some(e) = c.take_toc() {
                toc.push(e);
            }
            if used == 0 || sink.room() < ROOM {
                all.append(&mut sink.0);
            }
        }
        c.finish(&mut sink);
        if let Some(e) = c.take_toc() {
            toc.push(e);
        }
        all.append(&mut sink.0);
        (all, toc)
    }

    #[test]
    fn pieces_do_not_matter() {
        let text = crate::fb2::tests::SAMPLE.repeat(5);
        let (whole, toc) = convert_pieces(Format::Fb2, text.as_bytes(), usize::MAX / 2);
        for piece in [1, 3, 64, 511] {
            let (p, t) = convert_pieces(Format::Fb2, text.as_bytes(), piece);
            assert_eq!(show(&p), show(&whole), "piece {piece}");
            assert_eq!(t, toc);
        }
        assert_eq!(toc.len(), 20);
    }

    #[test]
    fn utf16_txt_with_bom() {
        let t = "Chapter 1\r\n\r\nПривет, мир.\r\n";
        let mut b = std::vec![0xFF, 0xFE];
        b.extend(t.encode_utf16().flat_map(u16::to_le_bytes));
        let (out, toc) = convert_pieces(Format::Txt, &b, 5);
        assert_eq!(show(&out), "¶2Chapter 1¶0Привет, мир.");
        assert_eq!(toc[0].title(), "Chapter 1");
    }

    #[test]
    fn formats_from_names() {
        assert_eq!(Format::from_name("a.TXT"), Some(Format::Txt));
        assert_eq!(Format::from_name("b.markdown"), Some(Format::Md));
        assert_eq!(Format::from_name("War.fb2"), Some(Format::Fb2));
        assert_eq!(Format::from_name("x.EPUB"), Some(Format::Epub));
        assert_eq!(Format::from_u8(3), Format::Epub);
        assert_eq!(Format::from_name("noext"), None);
    }

    fn meta(format: Format, b: &[u8]) -> (String, String) {
        let mut r = MetaReader::new(format, b, true);
        for p in b.chunks(7) {
            if !r.wants_more() {
                break;
            }
            r.feed(p);
        }
        let m = r.finish();
        (m.title.as_str().into(), m.author.as_str().into())
    }

    #[test]
    fn metadata() {
        assert_eq!(
            meta(Format::Fb2, crate::fb2::tests::SAMPLE.as_bytes()),
            ("War & Peace".into(), "Lev N. Tolstoy".into())
        );
        assert_eq!(
            meta(Format::Txt, b"The Project Gutenberg eBook\r\n\r\nTitle: A Tale\r\nAuthor: C. Dickens\r\n"),
            ("A Tale".into(), "C. Dickens".into())
        );
        assert_eq!(
            meta(Format::Md, b"---\ntitle: \"Notes\"\nauthor: Me\n---\n# Other"),
            ("Notes".into(), "Me".into())
        );
        assert_eq!(meta(Format::Md, b"Intro\n\n# The Heading\n"), ("The Heading".into(), String::new()));
        assert_eq!(meta(Format::Txt, b"just text"), (String::new(), String::new()));
    }

    /// BOOK-2, 2026-11-25: `begin_part` cleared the watch the EPUB job had
    /// loaded for the part just before its first bytes: every contents
    /// entry with a fragment pointed at its chapter's start instead of
    /// its heading.
    #[test]
    fn regress_epub_watch_survives_part_start() {
        let mut c = Converter::new(Format::Epub, &[], false);
        c.headings(false);
        let mut out = Vec::new();
        let mut watch = Watch::default();
        let page = b"<html><body><p>Intro.</p><h2 id=\"s2\">Two</h2><p>Text.</p></body></html>";
        assert!(watch.add(crate::xml::hash32(b"s2"), 7));
        c.begin_part(page, true);
        let mut at = 0;
        let mut hit = None;
        while at < page.len() {
            at += c.feed_watched(&page[at..], &mut out, Some(&mut watch));
            hit = hit.or(watch.take_hit());
        }
        c.end_part(&mut out);
        assert_eq!(show(&out), "¶0Intro.¶2Two¶0Text.");
        assert_eq!(hit, Some((7, 7)), "entry 7 at the heading's paragraph byte");
    }

    /// A `<meta>` charset switches the decoder after the tag when nothing
    /// named the encoding before; a byte-order mark or the XML
    /// declaration wins.
    #[test]
    fn meta_charset_switch() {
        let run = |page: &[u8]| {
            let mut c = Converter::new(Format::Epub, &[], false);
            let mut out = Vec::new();
            c.begin_part(page, true);
            let mut at = 0;
            while at < page.len() {
                at += c.feed(&page[at..], &mut out);
            }
            c.end_part(&mut out);
            show(&out)
        };
        let mut p = b"<head><meta charset=\"windows-1251\"/></head><body><p>".to_vec();
        p.extend_from_slice(&[0xCC, 0xE8, 0xF0]);
        p.extend_from_slice(b"</p></body>");
        assert_eq!(run(&p), "¶0Мир");
        let mut q = b"<?xml version=\"1.0\" encoding=\"iso-8859-1\"?>".to_vec();
        q.extend_from_slice(&p);
        assert_eq!(run(&q), "¶0Ìèð");
    }
}
