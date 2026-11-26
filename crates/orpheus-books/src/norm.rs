//! The normalised text every book is converted to once (05 §7.15
//! "extracted once to a plain cached form"): UTF-8 with a few control
//! bytes, so that layout and reading never parse a format again.
//!
//! * `0x10 + kind` starts a paragraph of [`Kind`] `kind`; style is plain.
//! * `0x08 | flags` sets the inline style ([`BOLD`], [`ITALIC`]).
//! * Everything else is text: no other byte below 0x20, single spaces, no
//!   space at a paragraph's start or end, no empty paragraph (except
//!   [`Kind::Break`], which has no text).
//!
//! Offsets in this stream (relative to the text, after the cache file's
//! header) are a book's positions: they do not change with the layout.

/// Bold text.
pub const BOLD: u8 = 1;
/// Italic text.
pub const ITALIC: u8 = 2;
/// Style byte base (`STYLE | flags`).
pub const STYLE: u8 = 0x08;
/// Paragraph byte base (`PARA + kind`).
pub const PARA: u8 = 0x10;

/// What a paragraph is.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Kind {
    #[default]
    Body = 0,
    /// Headings: book or part, chapter, section and below.
    H1 = 1,
    H2 = 2,
    H3 = 3,
    /// FB2 `<subtitle>`: centred.
    Subtitle = 4,
    /// Epigraphs, citations, Markdown block quotes: indented.
    Quote = 5,
    /// A line of verse.
    Verse = 6,
    /// Preformatted (Markdown code): one source line per paragraph.
    Pre = 7,
    /// A list item (its bullet or number is in the text).
    Item = 8,
    /// FB2 `<text-author>`: right-aligned.
    Author = 9,
    /// A blank line or scene break (FB2 `<empty-line/>`, Markdown rule).
    Break = 10,
}

impl Kind {
    pub const COUNT: u8 = 11;

    #[must_use]
    pub fn from_u8(v: u8) -> Kind {
        match v {
            1 => Kind::H1,
            2 => Kind::H2,
            3 => Kind::H3,
            4 => Kind::Subtitle,
            5 => Kind::Quote,
            6 => Kind::Verse,
            7 => Kind::Pre,
            8 => Kind::Item,
            9 => Kind::Author,
            10 => Kind::Break,
            _ => Kind::Body,
        }
    }

    /// A heading of level 1–3 (the contents).
    #[must_use]
    pub fn level(self) -> Option<u8> {
        match self {
            Kind::H1 => Some(1),
            Kind::H2 => Some(2),
            Kind::H3 => Some(3),
            _ => None,
        }
    }

    /// The heading kind of `level` (1–3; deeper is 3).
    #[must_use]
    pub fn heading(level: u8) -> Kind {
        match level {
            0 | 1 => Kind::H1,
            2 => Kind::H2,
            _ => Kind::H3,
        }
    }
}

/// One item of the stream.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    Para(Kind),
    Style(u8),
    Char(char),
}

/// The item at `b[i..]` and its length in bytes; `None` at the end or if
/// the character is cut by the end of `b`. Invalid bytes (never written
/// by [`Writer`], but the cache is a file on a card) read as U+FFFD, one
/// byte each.
#[must_use]
pub fn item_at(b: &[u8], i: usize) -> Option<(Item, usize)> {
    let first = *b.get(i)?;
    if first < 0x20 {
        let it = if (PARA..PARA + Kind::COUNT).contains(&first) {
            Item::Para(Kind::from_u8(first - PARA))
        } else if (STYLE..STYLE + 4).contains(&first) {
            Item::Style(first - STYLE)
        } else {
            Item::Char(' ')
        };
        return Some((it, 1));
    }
    let n = match first {
        0x20..=0x7F => return Some((Item::Char(char::from(first)), 1)),
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return Some((Item::Char('\u{FFFD}'), 1)),
    };
    let seq = b.get(i..i + n)?;
    match core::str::from_utf8(seq).ok().and_then(|s| s.chars().next()) {
        Some(c) => Some((Item::Char(c), n)),
        None => Some((Item::Char('\u{FFFD}'), 1)),
    }
}

/// Bytes of a contents title kept (UTF-8, cut at a character).
pub const TITLE_MAX: usize = 42;

/// A heading for the contents: where it starts (its paragraph byte), its
/// level and its text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TocEntry {
    pub offset: u32,
    pub level: u8,
    pub len: u8,
    pub title: [u8; TITLE_MAX],
}

impl TocEntry {
    pub const EMPTY: TocEntry = TocEntry { offset: 0, level: 0, len: 0, title: [0; TITLE_MAX] };

    #[must_use]
    pub fn title(&self) -> &str {
        let b = self.title.get(..usize::from(self.len)).unwrap_or(&[]);
        match core::str::from_utf8(b) {
            Ok(s) => s,
            Err(e) => core::str::from_utf8(b.get(..e.valid_up_to()).unwrap_or(&[])).unwrap_or(""),
        }
    }

    fn push_char(&mut self, c: char) {
        let mut tmp = [0u8; 4];
        let s = c.encode_utf8(&mut tmp).as_bytes();
        let at = usize::from(self.len);
        if at + s.len() <= TITLE_MAX {
            self.title[at..at + s.len()].copy_from_slice(s);
            self.len += s.len() as u8;
        }
    }

    fn push_str(&mut self, s: &str) {
        for c in s.chars() {
            self.push_char(c);
        }
    }
}

/// Where the writer's bytes go.
pub trait Sink {
    fn put(&mut self, b: &[u8]);
    /// Bytes that still fit (the converter stops before it runs out).
    fn room(&self) -> usize {
        usize::MAX
    }
}

/// The events a format parser produces; [`Writer`] turns them into the
/// stream.
pub trait Events {
    /// A paragraph of `kind` starts (the previous one ends).
    fn para(&mut self, kind: Kind);
    /// The paragraph ends (text after it, before the next `para`, starts a
    /// body paragraph).
    fn end_para(&mut self);
    /// Inline style from here.
    fn style(&mut self, flags: u8);
    fn text(&mut self, c: char);
    /// A heading of several paragraphs (FB2 `<title>`) starts or ends:
    /// its paragraphs make one contents entry.
    fn title(&mut self, open: bool);
}

/// Turns parser events into the normalised stream: collapses white space,
/// drops empty paragraphs, writes style bytes only where the style of the
/// text changes, and collects headings for the contents (`toc`, emitted
/// when a heading ends).
#[derive(Clone, Copy, Debug)]
pub struct Writer {
    /// Bytes written so far (the next offset).
    pub offset: u32,
    kind: Kind,
    /// The paragraph byte is written (the paragraph has text).
    open: bool,
    /// Inside a paragraph (events since `para`).
    in_para: bool,
    space: bool,
    /// Style asked for / style written in this paragraph.
    want: u8,
    have: u8,
    /// A heading being collected, and whether a multi-paragraph title is
    /// open.
    toc: TocEntry,
    collecting: bool,
    in_title: bool,
    /// A finished contents entry for the caller to take.
    pub done: Option<TocEntry>,
    /// Paragraphs written.
    pub paragraphs: u32,
}

impl Default for Writer {
    fn default() -> Self {
        Writer::new()
    }
}

impl Writer {
    #[must_use]
    pub const fn new() -> Writer {
        Writer {
            offset: 0,
            kind: Kind::Body,
            open: false,
            in_para: false,
            space: false,
            want: 0,
            have: 0,
            toc: TocEntry::EMPTY,
            collecting: false,
            in_title: false,
            done: None,
            paragraphs: 0,
        }
    }

    fn put(&mut self, out: &mut dyn Sink, b: &[u8]) {
        out.put(b);
        self.offset += b.len() as u32;
    }

    /// The pending contents entry ends (a heading paragraph ended outside
    /// a title, or the title closed).
    fn flush_toc(&mut self) {
        if self.collecting {
            self.collecting = false;
            if self.toc.len > 0 {
                self.done = Some(self.toc);
            }
        }
    }

    pub fn para(&mut self, out: &mut dyn Sink, kind: Kind) {
        self.end_para(out);
        self.kind = kind;
        self.in_para = true;
        if kind == Kind::Break {
            // No text: written at once.
            self.put(out, &[PARA + Kind::Break as u8]);
            self.paragraphs += 1;
            self.in_para = false;
        }
    }

    pub fn end_para(&mut self, _out: &mut dyn Sink) {
        if self.open && self.kind.level().is_some() && !self.in_title {
            self.flush_toc();
        }
        self.open = false;
        self.in_para = false;
        self.space = false;
        self.want = 0;
        self.have = 0;
    }

    pub fn style(&mut self, flags: u8) {
        self.want = flags & (BOLD | ITALIC);
    }

    pub fn title(&mut self, out: &mut dyn Sink, open: bool) {
        self.end_para(out);
        if !open {
            self.flush_toc();
        }
        self.in_title = open;
    }

    pub fn text(&mut self, out: &mut dyn Sink, c: char) {
        let c = match c {
            '\t' | '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{2028}' | '\u{2029}' | '\u{3000}' | ' ' => ' ',
            '\u{00AD}' | '\u{200B}' | '\u{FEFF}' => return,
            c if u32::from(c) < 0x20 || (0x7F..0xA0).contains(&u32::from(c)) => return,
            c => c,
        };
        if c == ' ' {
            if self.open {
                self.space = true;
            }
            return;
        }
        if !self.in_para {
            // Text outside a paragraph: a body paragraph of its own.
            self.kind = Kind::Body;
            self.in_para = true;
        }
        if !self.open {
            let kind = self.kind;
            if kind == Kind::Break {
                self.kind = Kind::Body;
            }
            let at = self.offset;
            self.put(out, &[PARA + self.kind as u8]);
            self.open = true;
            self.paragraphs += 1;
            if let Some(level) = self.kind.level() {
                if self.collecting && self.in_title {
                    // The next paragraph of a multi-paragraph title.
                    self.toc.push_str(". ");
                } else {
                    self.flush_toc();
                    self.toc = TocEntry { offset: at, level, ..TocEntry::EMPTY };
                    self.collecting = true;
                }
            }
        } else if self.space {
            // A style that only ends before the space ends before it (the
            // space is not bold); one that starts, after it.
            if self.want != self.have && self.want & !self.have == 0 {
                self.put(out, &[STYLE | self.want]);
                self.have = self.want;
            }
            self.put(out, b" ");
            if self.collecting && self.kind.level().is_some() {
                self.toc.push_char(' ');
            }
        }
        self.space = false;
        if self.want != self.have {
            self.put(out, &[STYLE | self.want]);
            self.have = self.want;
        }
        let mut tmp = [0u8; 4];
        let s = c.encode_utf8(&mut tmp);
        self.put(out, s.as_bytes());
        if self.collecting && self.kind.level().is_some() {
            self.toc.push_char(c);
        }
    }

    /// End of the book.
    pub fn finish(&mut self, out: &mut dyn Sink) {
        self.end_para(out);
        self.in_title = false;
        self.flush_toc();
    }
}

/// [`Events`] over a [`Writer`] and a [`Sink`] (what the parsers drive).
pub struct Emit<'a> {
    pub w: &'a mut Writer,
    pub out: &'a mut dyn Sink,
}

impl Events for Emit<'_> {
    fn para(&mut self, kind: Kind) {
        self.w.para(self.out, kind);
    }
    fn end_para(&mut self) {
        self.w.end_para(self.out);
    }
    fn style(&mut self, flags: u8) {
        self.w.style(flags);
    }
    fn text(&mut self, c: char) {
        self.w.text(self.out, c);
    }
    fn title(&mut self, open: bool) {
        self.w.title(self.out, open);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use std::string::String;
    use std::vec::Vec;

    impl Sink for Vec<u8> {
        fn put(&mut self, b: &[u8]) {
            self.extend_from_slice(b);
        }
    }

    /// The stream as readable text: `¶n` paragraphs, `{b}`/`{i}`/`{bi}`/`{}`
    /// style changes.
    pub fn show(b: &[u8]) -> String {
        let mut s = String::new();
        let mut i = 0;
        while let Some((it, n)) = item_at(b, i) {
            match it {
                Item::Para(k) => {
                    s.push_str(&std::format!("¶{}", k as u8));
                }
                Item::Style(f) => s.push_str(match f {
                    0 => "{}",
                    BOLD => "{b}",
                    ITALIC => "{i}",
                    _ => "{bi}",
                }),
                Item::Char(c) => s.push(c),
            }
            i += n;
        }
        s
    }

    #[test]
    fn writer_collapses_and_drops() {
        let mut out = Vec::new();
        let mut w = Writer::new();
        w.para(&mut out, Kind::Body);
        for c in "  Hello \t\n  world  ".chars() {
            w.text(&mut out, c);
        }
        w.para(&mut out, Kind::Body); // empty: dropped
        w.para(&mut out, Kind::H2);
        for c in "Chapter  One".chars() {
            w.text(&mut out, c);
        }
        w.para(&mut out, Kind::Body);
        w.style(ITALIC);
        w.text(&mut out, 'x');
        w.style(0);
        w.text(&mut out, ' ');
        w.style(BOLD);
        w.text(&mut out, 'y');
        w.para(&mut out, Kind::Break);
        w.finish(&mut out);
        assert_eq!(show(&out), "¶0Hello world¶2Chapter One¶0{i}x {b}y¶10");
        assert_eq!(w.offset as usize, out.len());
        assert_eq!(w.paragraphs, 4);
    }

    #[test]
    fn writer_collects_headings() {
        let mut out = Vec::new();
        let mut w = Writer::new();
        w.para(&mut out, Kind::Body);
        w.text(&mut out, 'a');
        w.title(&mut out, true);
        w.para(&mut out, Kind::H1);
        "Часть первая".chars().for_each(|c| w.text(&mut out, c));
        w.para(&mut out, Kind::H1);
        "Глава I".chars().for_each(|c| w.text(&mut out, c));
        assert!(w.done.is_none(), "still in the title");
        w.title(&mut out, false);
        let e = w.done.take().unwrap();
        assert_eq!(e.title(), "Часть первая. Глава I");
        assert_eq!((e.offset, e.level), (2, 1));
        w.para(&mut out, Kind::H3);
        "A very long heading that is longer than the contents keep".chars().for_each(|c| w.text(&mut out, c));
        w.para(&mut out, Kind::Body);
        let e = w.done.take().unwrap();
        assert_eq!(e.level, 3);
        assert!(e.title().len() <= TITLE_MAX && e.title().starts_with("A very long heading"));
    }

    #[test]
    fn items() {
        let b = [PARA + 2, b'a', STYLE | 3, 0xD0, 0x96, 0xE2, 0x80];
        assert_eq!(item_at(&b, 0), Some((Item::Para(Kind::H2), 1)));
        assert_eq!(item_at(&b, 2), Some((Item::Style(3), 1)));
        assert_eq!(item_at(&b, 3), Some((Item::Char('Ж'), 2)));
        assert_eq!(item_at(&b, 5), None, "cut");
        assert_eq!(item_at(&[0xFF], 0), Some((Item::Char('\u{FFFD}'), 1)));
        assert_eq!(item_at(&[0x01], 0), Some((Item::Char(' '), 1)));
    }
}
