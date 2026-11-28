//! Line breaking and pagination over the normalised stream (05 §7.15).
//!
//! [`Liner`] turns the stream into lines no wider than the text width:
//! breaks at spaces, before and after CJK characters (never before closing
//! punctuation), after hyphens and dashes, and inside a word only when the
//! word alone is wider than the line. [`Pager`] stacks lines into pages
//! of the page height. Both are streaming (any piece sizes, a character
//! cut between pieces is carried) and keep a few dozen bytes of state.
//!
//! A line's breaks depend only on where it starts and the stream's state
//! there ([`Mark`]: offset, paragraph kind, inline style, first line of a
//! paragraph). So the index pass over the whole book and the reader's
//! drawing of one page from its mark find the same lines (the stable
//! index), and any offset that starts a line can be shown as a page top.

use crate::norm::{BOLD, ITALIC, Item, Kind, item_at};

/// The bytes one page spans at most: a page ends early rather than grow
/// past it, so the reader's text window (2 KiB) always holds a page and
/// the start of the next.
pub const MAX_PAGE_BYTES: u32 = 1536;
/// The bytes one line spans at most (zero-width characters cannot make a
/// line without end).
pub const MAX_LINE_BYTES: u32 = 256;

/// Widths of characters, from the fonts the reader draws with.
pub trait Metrics {
    /// The advance of `c` in pixels drawn with `style` ([`BOLD`],
    /// [`ITALIC`]; see [`draw_style`]).
    fn advance(&self, c: char, style: u8) -> u16;
}

/// The style a paragraph's text is drawn with: headings and subtitles
/// bold, the author line italic.
#[must_use]
pub fn draw_style(kind: Kind, style: u8) -> u8 {
    match kind {
        Kind::H1 | Kind::H2 | Kind::H3 | Kind::Subtitle => style | BOLD,
        Kind::Author => style | ITALIC,
        _ => style,
    }
}

/// How a line is placed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
    /// Spread to the width (the setting; not a paragraph's last line).
    Justify,
}

/// The page geometry and spacing (from the reader's settings).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Params {
    /// Text width and page height in pixels.
    pub width: u16,
    pub height: u16,
    /// Line pitch.
    pub line: u16,
    /// Space before a paragraph (not at a page's top).
    pub gap: u16,
    /// First-line indent of body paragraphs.
    pub indent: u16,
    /// Left inset of quotes and verse.
    pub inset: u16,
    pub justify: bool,
    /// The font the metrics are of (part of the key).
    pub font: u16,
}

impl Params {
    /// What the page index depends on (a changed key lays the book out
    /// again). Justification does not move breaks.
    #[must_use]
    pub fn key(&self) -> u32 {
        let v = [self.width, self.height, self.line, self.gap, self.indent, self.inset, self.font];
        let mut h = 0x811C_9DC5u32 ^ LAYOUT_VERSION;
        for x in v {
            for b in x.to_le_bytes() {
                h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
            }
        }
        h
    }

    /// Left inset of a line of `kind` (the first line of a body
    /// paragraph is indented).
    #[must_use]
    pub fn left(&self, kind: Kind, first: bool) -> u16 {
        match kind {
            Kind::Quote | Kind::Verse => self.inset,
            Kind::Body if first => self.indent,
            _ => 0,
        }
    }

    /// The width a line of `kind` may fill.
    #[must_use]
    pub fn avail(&self, kind: Kind, first: bool) -> u16 {
        self.width.saturating_sub(self.left(kind, first)).max(8)
    }

    /// Height of a line of `kind`.
    #[must_use]
    pub fn line_h(&self, kind: Kind) -> u16 {
        if kind == Kind::Break { self.line / 2 } else { self.line }
    }

    /// Space before the first line of a paragraph of `kind`.
    #[must_use]
    pub fn gap_before(&self, kind: Kind) -> u16 {
        match kind {
            Kind::H1 | Kind::H2 | Kind::H3 => self.gap + self.line / 2,
            Kind::Verse | Kind::Pre | Kind::Item => self.gap / 2,
            _ => self.gap,
        }
    }

    #[must_use]
    pub fn align(&self, kind: Kind, last: bool) -> Align {
        match kind {
            Kind::H1 | Kind::Subtitle => Align::Center,
            Kind::Author => Align::Right,
            Kind::Body | Kind::Quote if self.justify && !last => Align::Justify,
            _ => Align::Left,
        }
    }
}

/// Bumped when breaking rules change (old page indexes are laid out again).
pub const LAYOUT_VERSION: u32 = 1;

/// Where a line starts and the stream's state there.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Mark {
    pub offset: u32,
    pub kind: Kind,
    pub style: u8,
    /// The first line of its paragraph.
    pub first: bool,
}

impl Mark {
    /// The start of the book.
    pub const START: Mark = Mark { offset: 0, kind: Kind::Body, style: 0, first: true };

    /// The state as one byte (page index, positions, bookmarks).
    #[must_use]
    pub fn state(&self) -> u8 {
        (self.kind as u8 & 0x0F) | ((self.style & 3) << 4) | (u8::from(self.first) << 6)
    }

    #[must_use]
    pub fn from_state(offset: u32, s: u8) -> Mark {
        Mark { offset, kind: Kind::from_u8(s & 0x0F), style: (s >> 4) & 3, first: s & 0x40 != 0 }
    }

    /// A heading's mark (the contents: its paragraph byte).
    #[must_use]
    pub fn para(offset: u32, kind: Kind) -> Mark {
        Mark { offset, kind, style: 0, first: true }
    }
}

/// One laid-out line: bytes `[mark.offset, end)` of the stream.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Line {
    pub mark: Mark,
    pub end: u32,
    /// The paragraph's last line (not justified).
    pub last: bool,
    /// Natural width of its text.
    pub width: u16,
}

#[derive(Clone, Copy, Debug)]
struct Brk {
    /// The line would end here, with this width.
    end: u32,
    w_end: u16,
    /// The next line would start here, with the width measured up to it
    /// and the characters before it.
    next: u32,
    w_next: u16,
    n_next: u16,
    style_next: u8,
}

/// CJK and other wide scripts: a line may break before and after each.
#[must_use]
pub fn breaks_around(c: char) -> bool {
    matches!(u32::from(c), 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

/// Characters that never begin a line (closing punctuation; kinsoku).
#[must_use]
pub fn no_line_start(c: char) -> bool {
    matches!(
        c,
        '。' | '、'
            | '，'
            | '．'
            | '！'
            | '？'
            | '：'
            | '；'
            | '）'
            | '」'
            | '』'
            | '】'
            | '〕'
            | '〉'
            | '》'
            | 'ー'
            | '…'
            | '・'
            | ','
            | '.'
            | ';'
            | ':'
            | '!'
            | '?'
            | ')'
            | ']'
            | '»'
    )
}

/// A line may break after these (between words joined by them).
fn breaks_after(c: char) -> bool {
    matches!(c, '-' | '–' | '—' | '/')
}

/// The line breaker.
#[derive(Clone, Copy, Debug)]
pub struct Liner {
    p: Params,
    kind: Kind,
    style: u8,
    /// The current line: its mark (state at its start), width and
    /// characters so far, the last break chance.
    mark: Mark,
    w: u16,
    n: u16,
    brk: Option<Brk>,
    prev_wide: bool,
    /// The last item was a space: its break chance stands (2026-11-28).
    after_space: bool,
    /// The next byte's offset and the bytes of a character cut between
    /// two pieces.
    pos: u32,
    pend: [u8; 4],
    pend_n: u8,
    stop: bool,
}

impl Liner {
    /// A liner whose first line starts at `mark`.
    #[must_use]
    pub fn new(p: Params, mark: Mark) -> Liner {
        Liner {
            p,
            kind: mark.kind,
            style: mark.style,
            mark,
            w: 0,
            n: 0,
            brk: None,
            prev_wide: false,
            after_space: false,
            pos: mark.offset,
            pend: [0; 4],
            pend_n: 0,
            stop: false,
        }
    }

    /// The offset of the next byte it expects.
    #[must_use]
    pub fn pos(&self) -> u32 {
        self.pos
    }

    fn emit(&mut self, end: u32, last: bool, width: u16, out: &mut impl FnMut(&Line) -> bool) {
        // Refused once: no more lines (one item can end two: a paragraph's
        // last line and a break).
        if self.stop {
            return;
        }
        let l = Line { mark: self.mark, end, last, width };
        if !out(&l) {
            self.stop = true;
        }
    }

    fn avail(&self) -> u16 {
        self.p.avail(self.mark.kind, self.mark.first)
    }

    /// The current line's width without its trailing spaces (they take no
    /// room at a paragraph's or the text's end; 2026-11-28).
    fn natural(&self) -> u16 {
        match self.brk {
            Some(b) if self.after_space => b.w_end,
            _ => self.w,
        }
    }

    fn new_line(&mut self, offset: u32, style: u8, first: bool) {
        self.mark = Mark { offset, kind: self.kind, style, first };
        self.w = 0;
        self.n = 0;
        self.brk = None;
        self.prev_wide = false;
        self.after_space = false;
    }

    fn item(&mut self, it: Item, o: u32, len: u32, m: &dyn Metrics, out: &mut impl FnMut(&Line) -> bool) {
        match it {
            Item::Para(k) => {
                if self.n > 0 {
                    let w = self.natural();
                    self.emit(o, true, w, out);
                }
                self.kind = k;
                self.style = 0;
                self.new_line(o, 0, true);
                if k == Kind::Break {
                    self.emit(o + len, true, 0, out);
                    // Text never follows a break; if it did, a body
                    // paragraph.
                    self.kind = Kind::Body;
                    self.new_line(o + len, 0, true);
                }
            }
            Item::Style(f) => self.style = f,
            Item::Char(c) => self.char(c, o, len, m, out),
        }
    }

    fn char(&mut self, c: char, o: u32, len: u32, m: &dyn Metrics, out: &mut impl FnMut(&Line) -> bool) {
        let st = draw_style(self.kind, self.style);
        if c == ' ' {
            if self.n == 0 {
                // A line never starts with a space.
                self.mark.offset = o + len;
                self.mark.style = self.style;
                return;
            }
            let a = m.advance(' ', st);
            let w_next = self.w.saturating_add(a);
            match self.brk.as_mut().filter(|_| self.after_space) {
                // More spaces (a damaged stream; the normaliser collapses
                // them): the line still ends before the first, and the
                // next starts after the last (2026-11-28).
                Some(b) => {
                    b.next = o + len;
                    b.w_next = w_next;
                    b.n_next = self.n;
                    b.style_next = self.style;
                }
                None => {
                    self.brk = Some(Brk {
                        end: o,
                        w_end: self.w,
                        next: o + len,
                        w_next,
                        n_next: self.n,
                        style_next: self.style,
                    });
                }
            }
            self.w = w_next;
            // The space is the break chance: no wide-character break right
            // after it (it would carry the space's width; 2026-11-28).
            self.prev_wide = false;
            self.after_space = true;
            return;
        }
        let a = m.advance(c, st);
        let wide = breaks_around(c);
        let after_space = core::mem::take(&mut self.after_space);
        if self.n > 0 && (wide || self.prev_wide) && !no_line_start(c) && !after_space {
            self.brk = Some(Brk {
                end: o,
                w_end: self.w,
                next: o,
                w_next: self.w,
                n_next: self.n,
                style_next: self.style,
            });
        }
        let too_long = o + len - self.mark.offset > MAX_LINE_BYTES;
        // The width so far without trailing spaces (a cut here does not
        // carry them; 2026-11-28).
        let natural = match self.brk {
            Some(b) if after_space => b.w_end,
            _ => self.w,
        };
        if self.n > 0 && (self.w.saturating_add(a) > self.avail() || too_long) {
            match self.brk.take().filter(|b| b.next > self.mark.offset && !too_long) {
                Some(b) => {
                    self.emit(b.end, false, b.w_end, out);
                    let (w, n) = (self.w - b.w_next.min(self.w), self.n - b.n_next.min(self.n));
                    self.new_line(b.next, b.style_next, false);
                    self.w = w;
                    self.n = n;
                    // A word longer than the line: break inside it.
                    if self.n > 0 && self.w.saturating_add(a) > self.avail() {
                        self.emit(o, false, self.w, out);
                        self.new_line(o, self.style, false);
                    }
                }
                None => {
                    self.emit(o, false, natural, out);
                    self.new_line(o, self.style, false);
                }
            }
        }
        self.w = self.w.saturating_add(a);
        self.n = self.n.saturating_add(1);
        self.prev_wide = wide;
        if breaks_after(c) {
            self.brk = Some(Brk {
                end: o + len,
                w_end: self.w,
                next: o + len,
                w_next: self.w,
                n_next: self.n,
                style_next: self.style,
            });
        }
    }

    /// Lay out `bytes`, which start at [`Liner::pos`]. `out` gets every
    /// finished line; returning `false` stops (the rest is not taken).
    /// Returns the bytes taken.
    pub fn feed(&mut self, bytes: &[u8], m: &dyn Metrics, out: &mut impl FnMut(&Line) -> bool) -> usize {
        let mut i = 0usize;
        // Bytes held from the last piece (a character cut at its end):
        // taken item by item, with bytes of this piece added as needed.
        while self.pend_n > 0 && !self.stop {
            let k = usize::from(self.pend_n);
            let held = self.pend;
            match item_at(&held[..k], 0) {
                Some((it, n)) => {
                    // `n` may be less than held (an invalid sequence reads
                    // byte by byte): the rest stays held.
                    let start = self.pos;
                    self.pos += n as u32;
                    self.pend.copy_within(n..k, 0);
                    self.pend_n -= n as u8;
                    self.item(it, start, n as u32, m, out);
                }
                None => {
                    let Some(&b) = bytes.get(i) else { return i };
                    if let Some(slot) = self.pend.get_mut(k) {
                        *slot = b;
                        self.pend_n += 1;
                        i += 1;
                    }
                }
            }
        }
        while !self.stop {
            match item_at(bytes, i) {
                Some((it, n)) => {
                    let o = self.pos;
                    i += n;
                    self.pos += n as u32;
                    self.item(it, o, n as u32, m, out);
                }
                None => {
                    let rest = bytes.get(i..).unwrap_or(&[]);
                    if !rest.is_empty() {
                        self.pend[..rest.len()].copy_from_slice(rest);
                        self.pend_n = rest.len() as u8;
                        i = bytes.len();
                    }
                    break;
                }
            }
        }
        i
    }

    /// The end of the stream: the last line.
    pub fn finish(&mut self, m: &dyn Metrics, out: &mut impl FnMut(&Line) -> bool) {
        if self.pend_n > 0 {
            let start = self.pos;
            self.pos += u32::from(self.pend_n);
            self.pend_n = 0;
            self.char('\u{FFFD}', start, 1, m, out);
        }
        if self.n > 0 && !self.stop {
            let end = self.pos;
            let w = self.natural();
            self.emit(end, true, w, out);
            self.n = 0;
        }
    }

    /// Lines have been refused (`out` returned `false`).
    #[must_use]
    pub fn stopped(&self) -> bool {
        self.stop
    }
}

/// Stacks lines into pages.
#[derive(Clone, Copy, Debug)]
pub struct Pager {
    p: Params,
    y: u16,
    lines: u16,
    /// The current page's first line.
    pub page: Mark,
}

impl Pager {
    #[must_use]
    pub fn new(p: Params, first: Mark) -> Pager {
        Pager { p, y: 0, lines: 0, page: first }
    }

    /// Place `l`: its top on its page, and the new page's mark if it
    /// starts one.
    pub fn place(&mut self, l: &Line) -> (u16, Option<Mark>) {
        let h = self.p.line_h(l.mark.kind);
        let mut gap = if l.mark.first && self.lines > 0 { self.p.gap_before(l.mark.kind) } else { 0 };
        let mut new = None;
        if self.lines > 0
            && (self.y + gap + h > self.p.height || l.end.saturating_sub(self.page.offset) > MAX_PAGE_BYTES)
        {
            self.page = l.mark;
            self.y = 0;
            self.lines = 0;
            gap = 0;
            new = Some(l.mark);
        }
        let top = self.y + gap;
        self.y = top + h;
        self.lines += 1;
        (top, new)
    }
}

/// The whole book's pages, streaming: every page's first mark.
#[derive(Clone, Copy, Debug)]
pub struct Paginator {
    liner: Liner,
    pager: Pager,
    /// Pages found (the first is [`Mark::START`]).
    pub pages: u32,
}

impl Paginator {
    #[must_use]
    pub fn new(p: Params) -> Paginator {
        Paginator { liner: Liner::new(p, Mark::START), pager: Pager::new(p, Mark::START), pages: 1 }
    }

    /// Lay out the next `bytes`; `page` gets the mark of each page after
    /// the first. Takes all of `bytes`.
    pub fn feed(&mut self, bytes: &[u8], m: &dyn Metrics, page: &mut impl FnMut(Mark)) {
        let pager = &mut self.pager;
        let pages = &mut self.pages;
        self.liner.feed(bytes, m, &mut |l: &Line| {
            if let (_, Some(mk)) = pager.place(l) {
                *pages += 1;
                page(mk);
            }
            true
        });
    }

    pub fn finish(&mut self, m: &dyn Metrics, page: &mut impl FnMut(Mark)) {
        let pager = &mut self.pager;
        let pages = &mut self.pages;
        self.liner.finish(m, &mut |l: &Line| {
            if let (_, Some(mk)) = pager.place(l) {
                *pages += 1;
                page(mk);
            }
            true
        });
    }

    /// Bytes of the stream laid out so far.
    #[must_use]
    pub fn pos(&self) -> u32 {
        self.liner.pos()
    }

    /// The offset of the next byte to feed (after a character cut at the
    /// end of the last piece, which is held).
    #[must_use]
    pub fn next_byte(&self) -> u32 {
        self.liner.pos() + u32::from(self.liner.pend_n)
    }
}

/// What [`page`] found.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PageInfo {
    /// Lines shown.
    pub lines: u16,
    /// The first shown line's mark (the reading position).
    pub first: Option<Mark>,
    /// Where the next page starts (the first line not shown); `None` at
    /// the end of the book or when the window ended first.
    pub next: Option<Mark>,
    /// The window ended before the page did (more text is needed).
    pub short: bool,
    /// The lines laid out before the first shown one.
    pub skip: u16,
}

/// Where a page view starts.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Skip {
    /// At the mark.
    #[default]
    None,
    /// At the last line of the mark's page that starts before this offset
    /// (one line up from a view whose top is there).
    Before(u32),
}

/// Lay out one page from `mark` out of `window` (the stream's bytes from
/// offset `base`; `at_end`: the window reaches the end of the book). With
/// [`Skip::Before`] the lines before the shown one are laid out but not
/// shown (the view goes on past the page's end until it is full); `f`
/// gets each shown line and its top.
#[allow(clippy::too_many_arguments)]
pub fn page(
    window: &[u8],
    base: u32,
    at_end: bool,
    mark: Mark,
    skip: Skip,
    p: Params,
    m: &dyn Metrics,
    f: &mut impl FnMut(&Line, u16),
) -> PageInfo {
    let mut info = PageInfo::default();
    let Some(bytes) =
        mark.offset.checked_sub(base).and_then(|d| usize::try_from(d).ok()).and_then(|d| window.get(d..))
    else {
        info.short = true;
        return info;
    };
    let skip = match skip {
        Skip::None => 0,
        Skip::Before(before) => {
            // The lines of the page from `mark` that start before `before`.
            let mut pager = Pager::new(p, mark);
            let mut n = 0u16;
            let mut count = |l: &Line| {
                if l.mark.offset >= before || pager.place(l).1.is_some() {
                    return false;
                }
                n += 1;
                true
            };
            let mut liner = Liner::new(p, mark);
            liner.feed(bytes, m, &mut count);
            if !liner.stopped() && at_end {
                liner.finish(m, &mut count);
            }
            n.saturating_sub(1)
        }
    };
    info.skip = skip;
    let mut liner = Liner::new(p, mark);
    let mut view = Pager::new(p, mark);
    let mut seen = 0u16;
    let mut on_line = |l: &Line| {
        if seen < skip {
            seen += 1;
            return true;
        }
        let (top, new) = view.place(l);
        if new.is_some() {
            info.next = new;
            return false;
        }
        if info.first.is_none() {
            info.first = Some(l.mark);
        }
        info.lines += 1;
        f(l, top);
        true
    };
    liner.feed(bytes, m, &mut on_line);
    if !liner.stopped() {
        if at_end {
            liner.finish(m, &mut on_line);
        } else {
            info.short = true;
        }
    }
    info
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use crate::norm::{Emit, Writer};
    use std::string::String;
    use std::vec::Vec;

    /// Every character 6 px (bold 7), wide ones 12, space 4.
    pub struct Mono;
    impl Metrics for Mono {
        fn advance(&self, c: char, style: u8) -> u16 {
            let base = if breaks_around(c) {
                12
            } else if c == ' ' {
                4
            } else {
                6
            };
            base + u16::from(style & BOLD != 0)
        }
    }

    pub const P: Params =
        Params { width: 100, height: 64, line: 16, gap: 4, indent: 8, inset: 10, justify: false, font: 0 };

    /// A normalised stream from plain text through the TXT parser.
    pub fn stream(text: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let mut w = Writer::new();
        let mut t = crate::txt::Txt::new(crate::txt::Mode::Join);
        {
            let mut ev = Emit { w: &mut w, out: &mut out };
            for c in text.chars() {
                t.char(c, &mut ev);
            }
            t.finish(&mut ev);
        }
        w.finish(&mut out);
        out
    }

    pub fn sample() -> String {
        let mut s = String::from("CHAPTER I\n\n");
        let words = [
            "It",
            "was",
            "the",
            "best",
            "of",
            "times,",
            "it",
            "was",
            "the",
            "worst",
            "of",
            "times;",
            "a",
            "remarkably-long-hyphenated-word",
            "and",
            "supercalifragilisticexpialidocious",
            "words",
            "夜に駆ける沈むように溶けてゆくように",
            "—",
            "end.",
        ];
        for p in 0..12 {
            for (i, w) in words.iter().enumerate() {
                if (i + p) % 7 != 3 {
                    s.push_str(w);
                    s.push(' ');
                }
            }
            s.push_str("\n\n");
            if p % 5 == 4 {
                s.push_str("* * *\n\nChapter II\n\n");
            }
        }
        s
    }

    /// All pages' marks, fed in pieces of `piece` bytes.
    /// Regression (VER-3 fuzz finding, `books_convert`, 2026-11-28): a line
    /// of wide characters followed by a space came out wider than the page
    /// (70 px on 67): the space did not clear "the last character was
    /// wide", so the next character set a break chance after the space,
    /// with the space's width, over the space's own break before it.
    #[test]
    fn regress_wide_characters_then_a_space_overflowed_the_line() {
        let text = "夜に駆ける沈\u{2000}むよ うに溶けてゆく ように abc 夜に駆ける沈む def\n\n";
        // A letter after a space still lets the next wide character break
        // before itself (the flag is the last item's, not the line's).
        let more = stream("ab cd夜夜夜夜夜夜夜夜\n\n");
        let mut l = Liner::new(Params { width: 60, ..P }, Mark::START);
        let mut ends = std::vec::Vec::new();
        l.feed(&more, &Mono, &mut |line: &Line| {
            ends.push(line.width);
            true
        });
        l.finish(&Mono, &mut |line: &Line| {
            ends.push(line.width);
            true
        });
        assert!(ends.iter().all(|&w| w <= 60), "{ends:?}");
        assert!(ends.len() >= 2 && ends[0] > 30, "cd and wide characters share the first line: {ends:?}");
        // A run of spaces (a damaged cache: fuzz `books_layout`, 347 px on
        // 66) neither widens the line before it nor its paragraph's end.
        let mut raw = std::vec::Vec::from(&stream("abc def\n\n")[..]);
        let at = raw.iter().position(|&c| c == b' ').unwrap();
        raw.splice(at..=at, core::iter::repeat_n(b' ', 80));
        raw.splice(raw.len() - 1..raw.len() - 1, core::iter::repeat_n(b' ', 80));
        let p = Params { width: 66, ..P };
        let mut l = Liner::new(p, Mark::START);
        let mut check = |line: &Line| {
            assert!(line.width <= p.avail(line.mark.kind, line.mark.first), "a line of {} px", line.width);
            true
        };
        l.feed(&raw, &Mono, &mut check);
        l.finish(&Mono, &mut check);
        let b = stream(text);
        for width in 40..140u16 {
            let p = Params { width, ..P };
            let mut l = Liner::new(p, Mark::START);
            let mut check = |line: &Line| {
                assert!(
                    line.width <= p.avail(line.mark.kind, line.mark.first) || line.width <= 12,
                    "width {width}: a line of {} px, {} available",
                    line.width,
                    p.avail(line.mark.kind, line.mark.first)
                );
                true
            };
            l.feed(&b, &Mono, &mut check);
            l.finish(&Mono, &mut check);
        }
    }

    pub fn pages(b: &[u8], p: Params, piece: usize) -> Vec<Mark> {
        let mut pg = Paginator::new(p);
        let mut marks = std::vec![Mark::START];
        for c in b.chunks(piece) {
            pg.feed(c, &Mono, &mut |m| marks.push(m));
        }
        pg.finish(&Mono, &mut |m| marks.push(m));
        assert_eq!(pg.pages as usize, marks.len());
        marks
    }

    /// The text of the lines of the page at `mark` drawn from the stream.
    fn page_lines(b: &[u8], mark: Mark, p: Params) -> (Vec<String>, PageInfo) {
        let mut lines = Vec::new();
        let info = page(b, 0, true, mark, Skip::None, p, &Mono, &mut |l, _| {
            let mut s = String::new();
            let mut i = l.mark.offset as usize;
            while i < l.end as usize {
                let (it, n) = item_at(b, i).unwrap();
                if let Item::Char(c) = it {
                    s.push(c);
                }
                i += n;
            }
            lines.push(s);
        });
        (lines, info)
    }

    fn words_of(b: &[u8]) -> Vec<String> {
        let mut s = String::new();
        let mut i = 0;
        while let Some((it, n)) = item_at(b, i) {
            match it {
                Item::Char(c) => s.push(c),
                Item::Para(_) => s.push(' '),
                Item::Style(_) => {}
            }
            i += n;
        }
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn page_breaks_do_not_depend_on_piece_sizes() {
        let b = stream(&sample());
        let whole = pages(&b, P, b.len());
        assert!(whole.len() > 10, "{} pages", whole.len());
        for piece in [1, 2, 3, 7, 64, 512] {
            assert_eq!(pages(&b, P, piece), whole, "piece {piece}");
        }
    }

    /// No word lost or duplicated: the lines of every page, page after
    /// page, are the book's words in order. Every page drawn from its
    /// mark ends where the index says the next page starts, every line
    /// fits the width.
    #[test]
    fn no_word_lost_or_duplicated() {
        let b = stream(&sample());
        let marks = pages(&b, P, 37);
        let mut text = String::new();
        for (k, m) in marks.iter().enumerate() {
            let (lines, info) = page_lines(&b, *m, P);
            assert!(!lines.is_empty(), "page {k} empty");
            assert_eq!(info.next, marks.get(k + 1).copied(), "page {k}'s end");
            for l in &lines {
                text.push_str(l);
                text.push(' ');
            }
        }
        // Lines broken inside a word (the long one, CJK) join without a
        // space: compare the text without spaces, and the words where no
        // forced break happened.
        let joined: String = text.split_whitespace().collect();
        let expect: String = words_of(&b).concat();
        assert_eq!(joined, expect);
    }

    #[test]
    fn lines_fit() {
        let b = stream(&sample());
        let mut liner = Liner::new(P, Mark::START);
        let mut n = 0;
        liner.feed(&b, &Mono, &mut |l: &Line| {
            assert!(l.width <= P.avail(l.mark.kind, l.mark.first), "{l:?}");
            assert!(l.end >= l.mark.offset);
            n += 1;
            true
        });
        liner.finish(&Mono, &mut |_| true);
        assert!(n > 40);
    }

    #[test]
    fn breaks() {
        let p = Params { width: 60, ..P };
        let b = stream(
            "aaaa bbbb cccc dddd\n\nxxxxxxxxxxxxxxxxxxxxxx\n\nwell-known words\n\n夜に駆ける沈むように。溶け",
        );
        let mut got: Vec<String> = Vec::new();
        let mut liner = Liner::new(p, Mark::START);
        let mut f = |l: &Line| {
            let s: String = std::str::from_utf8(&b[l.mark.offset as usize..l.end as usize])
                .unwrap()
                .chars()
                .filter(|c| *c >= ' ')
                .collect();
            got.push(s);
            true
        };
        liner.feed(&b, &Mono, &mut f);
        liner.finish(&Mono, &mut f);
        assert_eq!(
            got,
            [
                // Indent 8 + 6 × 8 + 4 = 60.
                "aaaa bbbb",
                "cccc dddd",
                // A word wider than the line: 8 + 7 × 6 = 50 (+ 6 > 60),
                // then 10 × 6 = 60.
                "xxxxxxxx",
                "xxxxxxxxxx",
                "xxxx",
                "well-",
                "known",
                "words",
                // Wide characters: 8 + 4 × 12 = 56, then 5 × 12 = 60;
                // "。" may not start a line, so it stays with "に".
                "夜に駆け",
                "る沈むよう",
                "に。溶け",
            ]
        );
    }

    /// Regression (BOOK-1 development, 2026-11-25): the layout job read
    /// each next 512-byte piece from `Paginator::pos`, which stops before
    /// a character cut at the end of the last piece (held by the liner):
    /// its bytes were fed twice, and pages after the first cut Cyrillic
    /// letter started a few bytes off, so the reader's page 3 ended before
    /// the index's page 4 (page number 3 shown on page 4). The job reads
    /// from `Paginator::next_byte`.
    #[test]
    fn regress_layout_pieces_resume_after_a_cut_character() {
        let b = stream(&"Жили-были дед да баба. ".repeat(300));
        let whole = pages(&b, P, b.len());
        let mut pg = Paginator::new(P);
        let mut marks = std::vec![Mark::START];
        while (pg.next_byte() as usize) < b.len() {
            let at = pg.next_byte() as usize;
            pg.feed(&b[at..(at + 511).min(b.len())], &Mono, &mut |m| marks.push(m));
        }
        pg.finish(&Mono, &mut |m| marks.push(m));
        assert_eq!(marks, whole);
    }

    /// Regression (BOOK-1 development, 2026-11-25, the `books_layout`
    /// fuzz seed `garbage`): an invalid sequence cut between two pieces
    /// (`E2 82` held, then a space) read its first byte as U+FFFD and gave
    /// the other held byte back to the new piece: the piece's index went
    /// below zero (subtract overflow). Held bytes are now a queue taken
    /// item by item.
    #[test]
    fn regress_invalid_sequence_cut_between_pieces() {
        let b = [crate::norm::PARA, b'a', 0xE2, 0x82, b' ', b'b', 0xF0, 0x9F, 0x98, b'c'];
        let whole = pages(&b, P, b.len());
        for piece in 1..b.len() {
            let mut pg = Paginator::new(P);
            let mut marks = std::vec![Mark::START];
            for c in b.chunks(piece) {
                pg.feed(c, &Mono, &mut |m| marks.push(m));
            }
            pg.finish(&Mono, &mut |m| marks.push(m));
            assert_eq!(marks, whole, "piece {piece}");
            assert_eq!(pg.next_byte() as usize, b.len());
        }
    }

    /// Regression (BOOK-1 development, 2026-11-25, found by the
    /// `books_layout` fuzz seed `fb2`): one item can finish two lines (a
    /// paragraph's last line, then a scene break's). When the page view
    /// refused the first (the page was full), the liner still gave it
    /// the second, and the view took that as its page end: the page drawn
    /// from its start ended at the break instead of where the index's
    /// next page starts (Next skipped the paragraph's last line). The
    /// liner gives nothing more once a line was refused.
    #[test]
    fn regress_no_line_after_a_refused_one() {
        let p = Params {
            width: 106,
            height: 25,
            line: 22,
            gap: 4,
            indent: 12,
            inset: 6,
            justify: false,
            font: 0,
        };
        let mut b = stream("A first line of text that wraps\n\nend");
        // A scene break right after the first paragraph.
        let at = b.iter().rposition(|&x| x == crate::norm::PARA).unwrap();
        b.insert(at, crate::norm::PARA + Kind::Break as u8);
        let marks = pages(&b, p, 64);
        for (k, m) in marks.iter().enumerate() {
            let info = page(&b, 0, true, *m, Skip::None, p, &Mono, &mut |_, _| {});
            assert_eq!(info.next, marks.get(k + 1).copied(), "page {k}");
        }
    }

    /// One line up: the view from the previous page's start with
    /// `Skip::Before(top)` shows the line before `top` first, then `top`'s.
    #[test]
    fn line_up() {
        let b = stream(&sample());
        let marks = pages(&b, P, 512);
        let top = marks[3];
        let mut first = Vec::new();
        let info = page(&b, 0, true, marks[2], Skip::Before(top.offset), P, &Mono, &mut |l, y| {
            first.push((l.mark, y));
        });
        assert_eq!(first[0].1, 0, "the shown line is at the top");
        assert_eq!(first[1].0, top, "then the old top");
        assert!(info.skip > 0 && first[0].0.offset < top.offset);
        assert_eq!(info.first, Some(first[0].0));
        // The window ends early: short.
        let cut = page(&b[..top.offset as usize + 10], 0, false, top, Skip::None, P, &Mono, &mut |_, _| {});
        assert!(cut.short);
        // A mark before the window: short, nothing shown.
        let before = page(&b[100..], 100, false, Mark::START, Skip::None, P, &Mono, &mut |_, _| {});
        assert!(before.short && before.lines == 0);
    }

    #[test]
    fn marks_pack() {
        for kind in 0..Kind::COUNT {
            for style in 0..4 {
                for first in [false, true] {
                    let m = Mark { offset: 77, kind: Kind::from_u8(kind), style, first };
                    assert_eq!(Mark::from_state(77, m.state()), m);
                }
            }
        }
    }

    /// Re-layout keeps the position: the mark the reader is at, laid out
    /// with other parameters, is inside the page the new index finds for
    /// its offset.
    #[test]
    fn relayout_keeps_the_position() {
        let b = stream(&sample());
        let old = pages(&b, P, 512);
        let at = old[old.len() / 2];
        for p in [Params { width: 70, ..P }, Params { line: 20, height: 100, ..P }, Params { indent: 0, ..P }]
        {
            let new = pages(&b, p, 512);
            let k = new.iter().rposition(|m| m.offset <= at.offset).unwrap();
            let end = new.get(k + 1).map_or(u32::MAX, |m| m.offset);
            assert!(new[k].offset <= at.offset && at.offset < end);
            // And the page drawn from the old mark starts with its text.
            let (lines, _) = page_lines(&b, at, p);
            assert!(!lines.is_empty());
        }
    }

    #[test]
    fn page_is_bounded_in_bytes() {
        // Tiny glyphs: many characters per line.
        struct Tiny;
        impl Metrics for Tiny {
            fn advance(&self, _: char, _: u8) -> u16 {
                0
            }
        }
        let b = stream(&"ж".repeat(5000));
        let mut pg = Paginator::new(P);
        let mut marks = std::vec![Mark::START];
        pg.feed(&b, &Tiny, &mut |m| marks.push(m));
        pg.finish(&Tiny, &mut |m| marks.push(m));
        for w in marks.windows(2) {
            assert!(w[1].offset - w[0].offset <= MAX_PAGE_BYTES);
        }
        assert!(marks.len() >= 7);
    }
}
