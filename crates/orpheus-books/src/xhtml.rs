//! EPUB content documents (XHTML, and HTML as written in the wild)
//! reduced to the normalised stream's events (05 §7.15, BOOK-2), over the
//! shared XML tokenizer ([`crate::xml`]).
//!
//! * Blocks (`p`, `div`, `section`, table cells, …) are paragraphs;
//!   `h1`/`h2` headings H1/H2, `h3`–`h6` H3 (several paragraphs of one
//!   heading, `<br/>` between them, make one contents entry);
//!   `blockquote` and `dd` quotes, `center` centred, `pre` one
//!   preformatted paragraph per line, `li` list items with their bullet or
//!   number (`ol start` honoured; nested lists flattened); `br` ends a
//!   paragraph (the next one has the same kind), `hr` is a break.
//! * `em`/`i`/`cite`/`dfn`/`var` italic, `strong`/`b` bold; other inline
//!   elements are their text.
//! * Not shown: `head` (and its `title`), `script`, `style`, `template`,
//!   `svg`, `video`, `audio`, `object`, ruby annotations (`rt`, `rp`), and
//!   images (skipped, without a marker: 05 §7.15); CSS is not read.
//! * Every element's `id` (and `<a name>`) is told as an anchor (the
//!   contents point at them). `<meta charset>` and `<meta http-equiv
//!   content="…; charset=…">` are told to the converter, which switches
//!   the decoder unless a byte-order mark or the XML declaration named
//!   the encoding.
//! * Unclosed and stray end tags are tolerated; elements nested deeper
//!   than 32 are counted, not told apart.

use crate::enc::Encoding;
use crate::norm::{BOLD, Events, ITALIC, Kind};
use crate::xml::{Handler, Xml};

const DEPTH: usize = 32;
/// Lists nested deeper share the innermost counter.
const LISTS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
enum El {
    Other,
    A,
    Head,
    Body,
    /// Not shown, nor anything in it.
    Skip,
    /// A paragraph of the kind around it.
    Block,
    H1,
    H2,
    H3,
    Pre,
    Quote,
    Center,
    Ul,
    Ol,
    Li,
    Italic,
    Bold,
    Br,
    Hr,
    Meta,
}

fn element(name: &[u8]) -> El {
    match name {
        b"a" => El::A,
        b"head" => El::Head,
        b"body" => El::Body,
        b"script" | b"style" | b"template" | b"svg" | b"video" | b"audio" | b"object" | b"rt" | b"rp"
        | b"title" => El::Skip,
        b"p" | b"div" | b"section" | b"article" | b"aside" | b"header" | b"footer" | b"main" | b"nav"
        | b"figure" | b"figcaption" | b"address" | b"hgroup" | b"details" | b"summary" | b"caption"
        | b"dl" | b"dt" | b"table" | b"tr" | b"td" | b"th" | b"thead" | b"tbody" | b"tfoot" | b"fieldset"
        | b"legend" | b"form" => El::Block,
        b"h1" => El::H1,
        b"h2" => El::H2,
        b"h3" | b"h4" | b"h5" | b"h6" => El::H3,
        b"pre" | b"listing" | b"xmp" | b"plaintext" => El::Pre,
        b"blockquote" | b"dd" => El::Quote,
        b"center" => El::Center,
        b"ul" | b"menu" | b"dir" => El::Ul,
        b"ol" => El::Ol,
        b"li" => El::Li,
        b"em" | b"i" | b"cite" | b"dfn" | b"var" => El::Italic,
        b"strong" | b"b" => El::Bold,
        b"br" => El::Br,
        b"hr" => El::Hr,
        b"meta" => El::Meta,
        _ => El::Other,
    }
}

/// HTML's void elements: never closed, never on the stack.
fn void(name: &[u8]) -> bool {
    matches!(
        name,
        b"br"
            | b"hr"
            | b"img"
            | b"image"
            | b"meta"
            | b"link"
            | b"input"
            | b"col"
            | b"area"
            | b"base"
            | b"wbr"
            | b"source"
            | b"embed"
            | b"param"
            | b"track"
    )
}

impl El {
    /// A block's own paragraph kind (`None`: the kind around it).
    fn kind(self) -> Option<Kind> {
        Some(match self {
            El::H1 => Kind::H1,
            El::H2 => Kind::H2,
            El::H3 => Kind::H3,
            El::Pre => Kind::Pre,
            El::Quote => Kind::Quote,
            El::Center => Kind::Subtitle,
            El::Li => Kind::Item,
            _ => return None,
        })
    }

    fn block(self) -> bool {
        matches!(
            self,
            El::Block
                | El::H1
                | El::H2
                | El::H3
                | El::Pre
                | El::Quote
                | El::Center
                | El::Ul
                | El::Ol
                | El::Li
        )
    }

    fn heading(self) -> bool {
        matches!(self, El::H1 | El::H2 | El::H3)
    }
}

/// `charset=…` in a `content` value.
fn charset_in(content: &[u8]) -> Option<Encoding> {
    let at = content.windows(8).position(|w| w.eq_ignore_ascii_case(b"charset="))? + 8;
    let rest = content.get(at..)?;
    let rest = rest.strip_prefix(b"\"").unwrap_or(rest);
    let end = rest.iter().position(|&c| c == b';' || c == b'"' || c == b' ').unwrap_or(rest.len());
    Encoding::from_label(rest.get(..end)?)
}

/// The XHTML parser: one character at a time.
#[derive(Clone, Copy, Debug)]
pub struct Xhtml {
    xml: Xml<64>,
    p: Parse,
}

#[derive(Clone, Copy, Debug)]
struct Parse {
    /// The start tag being read.
    cur: El,
    cur_void: bool,
    cur_id: Option<u32>,
    ol_start: Option<u16>,
    meta_ct: bool,
    meta_enc: Option<Encoding>,
    content_enc: Option<Encoding>,
    stack: [El; DEPTH],
    depth: u8,
    skip: u8,
    head: u8,
    bold: u8,
    italic: u8,
    pre: u8,
    headings: u8,
    lists: [u16; LISTS],
    list_n: u8,
    /// An encoding a `<meta>` named, for the converter.
    charset: Option<Encoding>,
}

impl Default for Xhtml {
    fn default() -> Self {
        Xhtml::new()
    }
}

struct On<'a, E: Events> {
    p: &'a mut Parse,
    ev: &'a mut E,
}

impl<E: Events> Handler for On<'_, E> {
    fn start(&mut self, name: &[u8]) {
        let p = &mut *self.p;
        p.cur = element(name);
        p.cur_void = void(name);
        p.cur_id = None;
        p.ol_start = None;
        p.meta_ct = false;
        p.meta_enc = None;
        p.content_enc = None;
    }

    fn attr(&mut self, name: &[u8], v: &[u8], hash: u32) {
        let p = &mut *self.p;
        match (p.cur, name) {
            (_, b"id") | (El::A, b"name") => p.cur_id = Some(hash),
            (El::Ol, b"start") => {
                p.ol_start = core::str::from_utf8(v).ok().and_then(|s| s.trim().parse::<u16>().ok());
            }
            (El::Meta, b"charset") => p.meta_enc = Encoding::from_label(v.trim_ascii()),
            (El::Meta, b"http-equiv") => p.meta_ct = v.trim_ascii().eq_ignore_ascii_case(b"content-type"),
            (El::Meta, b"content") => p.content_enc = charset_in(v),
            _ => {}
        }
    }

    fn start_end(&mut self, empty: bool) {
        let p = &mut *self.p;
        let el = p.cur;
        if el == El::Meta {
            let enc = p.meta_enc.or(p.content_enc.filter(|_| p.meta_ct));
            if enc.is_some() {
                p.charset = enc;
            }
        }
        p.open(el, empty || p.cur_void, self.ev);
        if let Some(id) = p.cur_id {
            self.ev.anchor(id);
        }
    }

    fn end(&mut self, name: &[u8]) {
        self.p.end_tag(element(name), self.ev);
    }

    fn text(&mut self, c: char) {
        let p = &mut *self.p;
        if p.skip > 0 || p.head > 0 {
            return;
        }
        if p.pre > 0 && c == '\n' {
            self.ev.end_para();
            p.reopen(self.ev);
            return;
        }
        self.ev.text(c);
    }
}

impl Xhtml {
    #[must_use]
    pub fn new() -> Xhtml {
        Xhtml {
            xml: Xml::new(),
            p: Parse {
                cur: El::Other,
                cur_void: false,
                cur_id: None,
                ol_start: None,
                meta_ct: false,
                meta_enc: None,
                content_enc: None,
                stack: [El::Other; DEPTH],
                depth: 0,
                skip: 0,
                head: 0,
                bold: 0,
                italic: 0,
                pre: 0,
                headings: 0,
                lists: [0; LISTS],
                list_n: 0,
                charset: None,
            },
        }
    }

    pub fn char(&mut self, c: char, ev: &mut impl Events) {
        self.xml.char(c, &mut On { p: &mut self.p, ev });
    }

    /// The encoding a `<meta>` just named, once.
    pub fn take_charset(&mut self) -> Option<Encoding> {
        self.p.charset.take()
    }

    pub fn finish(&mut self, ev: &mut impl Events) {
        self.xml.finish(&mut On { p: &mut self.p, ev });
        let p = &mut self.p;
        while p.depth > 0 {
            p.pop(ev);
        }
        ev.end_para();
    }
}

impl Parse {
    fn flags(&self) -> u8 {
        (if self.bold > 0 { BOLD } else { 0 }) | (if self.italic > 0 { ITALIC } else { 0 })
    }

    fn tracked(&self) -> &[El] {
        &self.stack[..usize::from(self.depth).min(DEPTH)]
    }

    /// The kind of a paragraph here: the innermost block that has one.
    fn para_kind(&self) -> Kind {
        self.tracked().iter().rev().find_map(|e| e.kind()).unwrap_or(Kind::Body)
    }

    /// A paragraph of the kind here starts (empty ones are dropped).
    fn reopen(&self, ev: &mut impl Events) {
        ev.para(self.para_kind());
        ev.style(self.flags());
    }

    fn push(&mut self, el: El) {
        if let Some(s) = self.stack.get_mut(usize::from(self.depth)) {
            *s = el;
        }
        self.depth = self.depth.saturating_add(1);
    }

    fn open(&mut self, el: El, void: bool, ev: &mut impl Events) {
        match el {
            El::Body => {
                // The head ends here even if it was not closed.
                self.end_tag(El::Head, ev);
            }
            El::Br => {
                ev.end_para();
                self.reopen(ev);
            }
            El::Hr => {
                ev.para(Kind::Break);
                self.reopen(ev);
            }
            _ => {}
        }
        if void {
            if el.block() {
                // `<p/>`, `<div/>`: a boundary.
                self.reopen(ev);
            }
            return;
        }
        let tracked = usize::from(self.depth) < DEPTH;
        match el {
            El::Head if tracked => self.head += 1,
            El::Skip if tracked => self.skip += 1,
            El::Italic => self.italic = self.italic.saturating_add(1),
            El::Bold => self.bold = self.bold.saturating_add(1),
            El::Pre if tracked => self.pre += 1,
            El::Ul | El::Ol => {
                let first = if el == El::Ol { self.ol_start.unwrap_or(1) } else { 0 };
                let k = usize::from(self.list_n).min(LISTS - 1);
                self.lists[k] = first;
                self.list_n = self.list_n.saturating_add(1);
            }
            _ => {}
        }
        if el.heading() && tracked {
            if self.headings == 0 {
                ev.title(true);
            }
            self.headings += 1;
        }
        self.push(el);
        if el.block() {
            self.reopen(ev);
        } else if matches!(el, El::Italic | El::Bold) {
            ev.style(self.flags());
        }
        if el == El::Li && !self.skipping() {
            self.bullet(ev);
        }
    }

    fn skipping(&self) -> bool {
        self.skip > 0 || self.head > 0
    }

    fn bullet(&mut self, ev: &mut impl Events) {
        let k = usize::from(self.list_n).min(LISTS).checked_sub(1);
        let n = k.map_or(0, |k| self.lists[k]);
        if n == 0 {
            ev.text('•');
        } else {
            let mut digits = [0u8; 5];
            let mut d = 0;
            let mut v = n;
            while v > 0 || d == 0 {
                digits[d] = b'0' + (v % 10) as u8;
                v /= 10;
                d += 1;
            }
            for &c in digits[..d].iter().rev() {
                ev.text(char::from(c));
            }
            ev.text('.');
            if let Some(k) = k {
                self.lists[k] = n.saturating_add(1);
            }
        }
        ev.text(' ');
    }

    /// The top element closes.
    fn pop(&mut self, ev: &mut impl Events) {
        let d = usize::from(self.depth);
        self.depth -= 1;
        let Some(&el) = self.stack.get(d - 1) else { return };
        match el {
            El::Head => self.head = self.head.saturating_sub(1),
            El::Skip => self.skip = self.skip.saturating_sub(1),
            El::Italic => self.italic = self.italic.saturating_sub(1),
            El::Bold => self.bold = self.bold.saturating_sub(1),
            El::Pre => self.pre = self.pre.saturating_sub(1),
            El::Ul | El::Ol => self.list_n = self.list_n.saturating_sub(1),
            _ => {}
        }
        if el.heading() {
            self.headings = self.headings.saturating_sub(1);
            if self.headings == 0 {
                ev.title(false);
            }
        }
        if el.block() || el == El::Body {
            ev.end_para();
            self.reopen(ev);
        } else if matches!(el, El::Italic | El::Bold) {
            ev.style(self.flags());
        }
    }

    /// The end tag `el`: pop to its open tag (tolerating unclosed tags
    /// inside it, ignoring a stray end tag).
    fn end_tag(&mut self, el: El, ev: &mut impl Events) {
        let Some(at) = self.tracked().iter().rposition(|&e| e == el) else {
            if usize::from(self.depth) > DEPTH {
                // An end tag of an untracked element (no telling which).
                self.depth -= 1;
            }
            return;
        };
        while usize::from(self.depth) > at {
            self.pop(ev);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use crate::norm::tests::show;
    use crate::norm::{Emit, Sink, TocEntry, Writer};
    use std::string::{String, ToString};
    use std::vec::Vec;

    /// Ids told, with the stream offset they point at.
    struct Anchors<'a> {
        e: Emit<'a>,
        ids: &'a mut Vec<(u32, u32)>,
    }

    impl Events for Anchors<'_> {
        fn para(&mut self, k: Kind) {
            self.e.para(k);
        }
        fn end_para(&mut self) {
            self.e.end_para();
        }
        fn style(&mut self, f: u8) {
            self.e.style(f);
        }
        fn text(&mut self, c: char) {
            self.e.text(c);
        }
        fn title(&mut self, open: bool) {
            self.e.title(open);
        }
        fn anchor(&mut self, id: u32) {
            let at = self.e.w.anchor_at();
            self.ids.push((id, at));
        }
    }

    /// The stream shown, contents entries, anchors (id, offset), the
    /// stream.
    type Converted = (String, Vec<TocEntry>, Vec<(u32, u32)>, Vec<u8>);

    pub fn convert(text: &str) -> Converted {
        let mut out = Vec::new();
        let mut w = Writer::new();
        let mut p = Xhtml::new();
        let mut toc = Vec::new();
        let mut ids = Vec::new();
        for c in text.chars() {
            p.char(c, &mut Anchors { e: Emit { w: &mut w, out: &mut out as &mut dyn Sink }, ids: &mut ids });
            if let Some(e) = w.done.take() {
                toc.push(e);
            }
        }
        p.finish(&mut Anchors { e: Emit { w: &mut w, out: &mut out }, ids: &mut ids });
        w.finish(&mut out);
        if let Some(e) = w.done.take() {
            toc.push(e);
        }
        (show(&out), toc, ids, out)
    }

    pub const CHAPTER: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Ignored</title><style>p { color: red }</style>
<script>if (a < b) { x(); }</script></head>
<body>
<section epub:type="chapter" id="ch1">
<h1 id="h">Chapter&nbsp;I<br/>Loomings</h1>
<p>Call me <em>Ishmael</em>. Some years ago&mdash;never mind <b>how long</b>&#160;precisely&#8212;having
little or no money in my purse,</p>
<blockquote><p>a quoted <i>line</i></p>and after</blockquote>
<ul><li>one</li><li>two<ol start="3"><li>three</li><li>four</li></ol></li></ul>
<pre>let  x = 1;
let y = 2;</pre>
<p>Text<sup>1</sup> with <a href="n.xhtml#n1" id="r1">a note</a>.<img src="x.png" alt="pic"/></p>
<hr/>
<div>A div<div>inside</div>tail</div>
<p><ruby>漢<rt>kan</rt>字<rt>ji</rt></ruby></p>
<svg><text>svg text</text></svg>
<table><tr><td>cell 1</td><td>cell 2</td></tr></table>
<center>The End</center>
</section>
</body></html>"#;

    #[test]
    fn structure() {
        let (s, toc, ids, out) = convert(CHAPTER);
        assert_eq!(
            s,
            "¶1Chapter\u{a0}I¶1Loomings\
             ¶0Call me {i}Ishmael{}. Some years ago—never mind {b}how long{}\u{a0}precisely—having little or no money in my purse,\
             ¶5a quoted {i}line¶5and after\
             ¶8• one¶8• two¶83. three¶84. four\
             ¶7let x = 1;¶7let y = 2;\
             ¶0Text1 with a note.¶10\
             ¶0A div¶0inside¶0tail¶0漢字¶0cell 1¶0cell 2¶4The End"
        );
        let titles: Vec<(u8, String)> = toc.iter().map(|e| (e.level, e.title().to_string())).collect();
        assert_eq!(titles, [(1, "Chapter\u{a0}I. Loomings".to_string())]);
        // Anchors: the section and heading at the heading's paragraph, the
        // note reference at its paragraph.
        let h = crate::xml::hash32;
        let at = |id: &str| ids.iter().find(|(i, _)| *i == h(id.as_bytes())).unwrap().1 as usize;
        assert_eq!(at("ch1"), 0);
        assert_eq!(at("h"), 0);
        assert_eq!(out[at("r1")], crate::norm::PARA);
        assert!(show(&out[at("r1")..]).starts_with("¶0Text1"));
    }

    #[test]
    fn html_soup_and_depth() {
        // Unclosed paragraphs and list items, uppercase tags, no body.
        let (s, ..) = convert("<P>one<P>two<UL><LI>a<LI>b</UL>three");
        assert_eq!(s, "¶0one¶0two¶8• a¶8• b¶0three");
        // A head never closed.
        assert_eq!(convert("<html><head><title>T</title><body><p>x</p>").0, "¶0x");
        // Very deep nesting: text kept, nothing breaks.
        let deep = std::format!("{}deep{}<p>after</p>", "<div><i>".repeat(500), "</i></div>".repeat(500));
        let (s, ..) = convert(&deep);
        assert!(s.contains("deep") && s.ends_with("after"), "{s}");
        // Stray end tags and markup in text.
        assert_eq!(convert("</p></div><p>a < b & c</p></b>").0, "¶0a < b & c");
    }

    #[test]
    fn meta_charset() {
        let mut p = Xhtml::new();
        let mut out = Vec::new();
        let mut w = Writer::new();
        for c in
            "<head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1251\"/>".chars()
        {
            p.char(c, &mut Emit { w: &mut w, out: &mut out });
        }
        assert_eq!(p.take_charset(), Some(Encoding::Cp1251));
        assert_eq!(p.take_charset(), None);
        for c in "<meta charset='koi8-r'>".chars() {
            p.char(c, &mut Emit { w: &mut w, out: &mut out });
        }
        assert_eq!(p.take_charset(), Some(Encoding::Koi8r));
    }
}
