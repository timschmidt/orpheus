//! FictionBook 2 (05 §7.15): an XML tokenizer over `char`s and the
//! elements that matter for reading. Sections nest into heading levels;
//! `<title>` paragraphs become headings (one contents entry per title),
//! `<subtitle>`, `<epigraph>`/`<cite>`/`<annotation>`, `<v>` (verse),
//! `<text-author>`, `<empty-line/>` their paragraph kinds; `<emphasis>` and
//! `<strong>` the inline styles. Text outside `<body>` is not shown; the
//! title and authors of `<title-info>` are collected for the library.
//! A `<body name="notes">` keeps its title as a heading, its sections'
//! titles (footnote numbers) as subtitles, out of the contents. Images
//! and `<binary>` data are skipped without being kept.

use crate::norm::{BOLD, Events, ITALIC, Kind};

/// Elements nested deeper than this are counted but not told apart.
const DEPTH: usize = 24;
const NAME: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
enum El {
    Other,
    Description,
    TitleInfo,
    BookTitle,
    Author,
    FirstName,
    MiddleName,
    LastName,
    Body,
    Section,
    Title,
    Subtitle,
    P,
    V,
    Epigraph,
    Cite,
    Annotation,
    TextAuthor,
    Emphasis,
    Strong,
    EmptyLine,
    Td,
}

fn element(name: &[u8]) -> El {
    // A namespace prefix ("fb:p") is dropped.
    let name = match name.iter().rposition(|&b| b == b':') {
        Some(i) => name.get(i + 1..).unwrap_or(&[]),
        None => name,
    };
    match name {
        b"description" => El::Description,
        b"title-info" => El::TitleInfo,
        b"book-title" => El::BookTitle,
        b"author" => El::Author,
        b"first-name" => El::FirstName,
        b"middle-name" => El::MiddleName,
        b"last-name" => El::LastName,
        b"body" => El::Body,
        b"section" => El::Section,
        b"title" => El::Title,
        b"subtitle" => El::Subtitle,
        b"p" => El::P,
        b"v" => El::V,
        b"epigraph" => El::Epigraph,
        b"cite" => El::Cite,
        b"annotation" => El::Annotation,
        b"text-author" => El::TextAuthor,
        b"emphasis" => El::Emphasis,
        b"strong" => El::Strong,
        b"empty-line" => El::EmptyLine,
        b"td" | b"th" => El::Td,
        _ => El::Other,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum St {
    Text,
    /// After `<`.
    Open,
    /// In a tag's name (`close`: `</name`).
    Name {
        close: bool,
    },
    /// In a tag after its name; `q` the open quote of a value; `slash`: a
    /// `/` was the last thing (self-closing).
    Attrs {
        close: bool,
        q: char,
        slash: bool,
    },
    /// `<!` and what follows: comment, CDATA or a declaration.
    Bang,
    Comment,
    Cdata,
    Decl,
    Pi,
    Entity,
}

/// Short text in a fixed buffer (titles, authors, snippets).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MetaText<const N: usize> {
    pub buf: [u8; N],
    pub len: u8,
}

impl<const N: usize> Default for MetaText<N> {
    fn default() -> Self {
        MetaText { buf: [0; N], len: 0 }
    }
}

impl<const N: usize> MetaText<N> {
    pub fn push(&mut self, c: char) {
        let c = if c.is_whitespace() { ' ' } else { c };
        if c == ' ' && (self.len == 0 || self.buf.get(usize::from(self.len) - 1) == Some(&b' ')) {
            return;
        }
        let mut tmp = [0u8; 4];
        let s = c.encode_utf8(&mut tmp).as_bytes();
        let at = usize::from(self.len);
        if at + s.len() <= N {
            self.buf[at..at + s.len()].copy_from_slice(s);
            self.len += s.len() as u8;
        }
    }

    pub fn push_str(&mut self, s: &str) {
        for c in s.chars() {
            self.push(c);
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        let b = self.buf.get(..usize::from(self.len)).unwrap_or(&[]);
        let b = match core::str::from_utf8(b) {
            Ok(s) => return s.trim_end(),
            Err(e) => b.get(..e.valid_up_to()).unwrap_or(&[]),
        };
        core::str::from_utf8(b).unwrap_or("").trim_end()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.as_str().is_empty()
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

/// The FB2 parser: one character at a time.
#[derive(Clone, Copy, Debug)]
pub struct Fb2 {
    st: St,
    name: [u8; NAME],
    name_n: u8,
    /// Attribute name and value being read (`body name="notes"`).
    attr: [u8; 8],
    attr_n: u8,
    val: [u8; 8],
    val_n: u8,
    notes_attr: bool,
    stack: [El; DEPTH],
    depth: u8,
    /// Text is shown (inside a body).
    body: u8,
    sections: u8,
    titles: u8,
    in_notes: bool,
    bold: u8,
    italic: u8,
    /// `<!`-construct and comment end matching.
    mark: u8,
    ent: [u8; 10],
    ent_n: u8,
    /// Collect the title and the first author (description).
    pub title: MetaText<64>,
    pub author: MetaText<48>,
    authors: u8,
    last_name: MetaText<24>,
}

impl Default for Fb2 {
    fn default() -> Self {
        Fb2::new()
    }
}

impl Fb2 {
    #[must_use]
    pub fn new() -> Fb2 {
        Fb2 {
            st: St::Text,
            name: [0; NAME],
            name_n: 0,
            attr: [0; 8],
            attr_n: 0,
            val: [0; 8],
            val_n: 0,
            notes_attr: false,
            stack: [El::Other; DEPTH],
            depth: 0,
            body: 0,
            sections: 0,
            titles: 0,
            in_notes: false,
            bold: 0,
            italic: 0,
            mark: 0,
            ent: [0; 10],
            ent_n: 0,
            title: MetaText::default(),
            author: MetaText::default(),
            authors: 0,
            last_name: MetaText::default(),
        }
    }

    fn inside(&self, el: El) -> bool {
        let d = usize::from(self.depth).min(DEPTH);
        self.stack[..d].contains(&el)
    }

    fn top(&self) -> El {
        let d = usize::from(self.depth).min(DEPTH);
        if d == 0 { El::Other } else { self.stack[d - 1] }
    }

    fn flags(&self) -> u8 {
        (if self.bold > 0 { BOLD } else { 0 }) | (if self.italic > 0 { ITALIC } else { 0 })
    }

    /// The kind of a paragraph opened here.
    fn para_kind(&self, el: El) -> Kind {
        if self.inside(El::Title) {
            if self.in_notes && self.sections > 0 {
                return Kind::Subtitle;
            }
            return Kind::heading(self.sections.max(1));
        }
        match el {
            El::Subtitle => Kind::Subtitle,
            El::V => Kind::Verse,
            El::TextAuthor => Kind::Author,
            _ if self.inside(El::Epigraph) || self.inside(El::Cite) || self.inside(El::Annotation) => {
                Kind::Quote
            }
            _ => Kind::Body,
        }
    }

    fn open(&mut self, el: El, self_closing: bool, ev: &mut impl Events) {
        let showing = self.body > 0;
        match el {
            El::Body => {
                self.body += 1;
                self.in_notes = self.notes_attr;
            }
            El::Section if showing => {
                ev.end_para();
                self.sections = self.sections.saturating_add(1);
            }
            El::Title if showing => {
                let notes_section = self.in_notes && self.sections > 0;
                if !notes_section {
                    ev.title(true);
                }
                self.titles += 1;
                ev.para(self.para_kind(El::P));
            }
            El::P | El::V | El::Subtitle | El::TextAuthor | El::Td if showing => {
                let k = self.para_kind(el);
                ev.para(k);
                ev.style(self.flags());
            }
            El::EmptyLine if showing => ev.para(Kind::Break),
            El::Emphasis => {
                self.italic = self.italic.saturating_add(1);
                ev.style(self.flags());
            }
            El::Strong => {
                self.bold = self.bold.saturating_add(1);
                ev.style(self.flags());
            }
            El::Author if self.inside(El::TitleInfo) => {
                self.authors = self.authors.saturating_add(1);
                if self.authors == 2 {
                    self.author.push_str(", …");
                }
            }
            _ => {}
        }
        if self_closing {
            self.close(el, ev);
        } else {
            let d = usize::from(self.depth);
            if let Some(slot) = self.stack.get_mut(d) {
                *slot = el;
            }
            self.depth = self.depth.saturating_add(1);
        }
    }

    fn close(&mut self, el: El, ev: &mut impl Events) {
        let showing = self.body > 0;
        match el {
            El::Body => {
                ev.end_para();
                self.body = self.body.saturating_sub(1);
                self.in_notes = false;
            }
            El::Section if showing => {
                ev.end_para();
                self.sections = self.sections.saturating_sub(1);
            }
            El::Title if showing => {
                self.titles = self.titles.saturating_sub(1);
                let notes_section = self.in_notes && self.sections > 0;
                if notes_section {
                    ev.end_para();
                } else {
                    ev.title(false);
                }
            }
            El::P | El::V | El::Subtitle | El::TextAuthor | El::Td if showing => {
                ev.end_para();
                if self.inside(El::Title) {
                    // Text between title paragraphs stays a heading.
                    ev.para(self.para_kind(El::P));
                }
            }
            El::Emphasis => {
                self.italic = self.italic.saturating_sub(1);
                ev.style(self.flags());
            }
            El::Strong => {
                self.bold = self.bold.saturating_sub(1);
                ev.style(self.flags());
            }
            El::FirstName | El::MiddleName if self.inside(El::TitleInfo) && self.authors == 1 => {
                self.author.push(' ');
            }
            El::Author if self.inside(El::TitleInfo) && self.authors == 1 && !self.last_name.is_empty() => {
                if !self.author.is_empty() {
                    self.author.push(' ');
                }
                let mut ln = MetaText::<24>::default();
                core::mem::swap(&mut ln, &mut self.last_name);
                self.author.push_str(ln.as_str());
            }
            _ => {}
        }
    }

    /// The end tag `el`: pop to its open tag (tolerating unclosed tags
    /// inside it, ignoring a stray end tag).
    fn end_tag(&mut self, el: El, ev: &mut impl Events) {
        let d = usize::from(self.depth).min(DEPTH);
        let Some(at) = self.stack[..d].iter().rposition(|&e| e == el) else {
            if usize::from(self.depth) > DEPTH {
                self.depth -= 1;
            }
            return;
        };
        while usize::from(self.depth) > at {
            let top = self.top();
            self.depth -= 1;
            self.close(top, ev);
        }
    }

    fn text(&mut self, c: char, ev: &mut impl Events) {
        if self.body > 0 {
            ev.text(c);
            return;
        }
        if self.inside(El::TitleInfo) {
            match self.top() {
                El::BookTitle => self.title.push(c),
                El::FirstName | El::MiddleName if self.authors == 1 => self.author.push(c),
                El::LastName if self.authors == 1 => self.last_name.push(c),
                _ => {}
            }
        }
    }

    fn entity(&mut self, semi: bool, ev: &mut impl Events) {
        let n = usize::from(self.ent_n);
        let name = &self.ent[..n];
        let c = match name {
            b"amp" => Some('&'),
            b"lt" => Some('<'),
            b"gt" => Some('>'),
            b"quot" => Some('"'),
            b"apos" => Some('\''),
            b"nbsp" => Some('\u{A0}'),
            b"mdash" => Some('—'),
            b"ndash" => Some('–'),
            b"hellip" => Some('…'),
            b"laquo" => Some('«'),
            b"raquo" => Some('»'),
            b"copy" => Some('©'),
            [b'#', b'x' | b'X', hex @ ..] => core::str::from_utf8(hex)
                .ok()
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .and_then(char::from_u32),
            [b'#', dec @ ..] => {
                core::str::from_utf8(dec).ok().and_then(|d| d.parse::<u32>().ok()).and_then(char::from_u32)
            }
            _ => None,
        };
        match c {
            Some(c) => self.text(c, ev),
            None => {
                // Not an entity: the text as it was.
                self.text('&', ev);
                let ent = self.ent;
                for &b in &ent[..n] {
                    self.text(char::from(b), ev);
                }
                if semi {
                    self.text(';', ev);
                }
            }
        }
        self.ent_n = 0;
    }

    fn tag_done(&mut self, close: bool, slash: bool, ev: &mut impl Events) {
        let el = element(&self.name[..usize::from(self.name_n)]);
        if close {
            self.end_tag(el, ev);
        } else {
            self.open(el, slash, ev);
        }
        self.notes_attr = false;
        self.st = St::Text;
    }

    fn attr_value_done(&mut self) {
        if &self.attr[..usize::from(self.attr_n)] == b"name" {
            let v = &self.val[..usize::from(self.val_n)];
            self.notes_attr = v == b"notes" || v == b"comments" || v == b"footnot";
        }
        self.attr_n = 0;
        self.val_n = 0;
    }

    pub fn char(&mut self, c: char, ev: &mut impl Events) {
        match self.st {
            St::Text => match c {
                '<' => self.st = St::Open,
                '&' => {
                    self.ent_n = 0;
                    self.st = St::Entity;
                }
                c => self.text(c, ev),
            },
            St::Entity => {
                if c == ';' {
                    self.st = St::Text;
                    self.entity(true, ev);
                } else if c.is_ascii_alphanumeric() || c == '#' {
                    if let Some(slot) = self.ent.get_mut(usize::from(self.ent_n)) {
                        *slot = c as u8;
                        self.ent_n += 1;
                    } else {
                        self.st = St::Text;
                        self.entity(false, ev);
                        self.char(c, ev);
                    }
                } else {
                    self.st = St::Text;
                    self.entity(false, ev);
                    self.char(c, ev);
                }
            }
            St::Open => match c {
                '/' => {
                    self.name_n = 0;
                    self.st = St::Name { close: true };
                }
                '!' => {
                    self.mark = 0;
                    self.st = St::Bang;
                }
                '?' => {
                    self.mark = 0;
                    self.st = St::Pi;
                }
                c if c.is_alphabetic() || c == '_' => {
                    self.name_n = 0;
                    self.st = St::Name { close: false };
                    self.char(c, ev);
                }
                c => {
                    // A stray `<`.
                    self.st = St::Text;
                    self.text('<', ev);
                    self.char(c, ev);
                }
            },
            St::Name { close } => match c {
                '>' => self.tag_done(close, false, ev),
                '/' => self.st = St::Attrs { close, q: '\0', slash: true },
                c if c.is_whitespace() => {
                    self.attr_n = 0;
                    self.val_n = 0;
                    self.st = St::Attrs { close, q: '\0', slash: false };
                }
                c => {
                    if let Some(slot) = self.name.get_mut(usize::from(self.name_n)) {
                        *slot = if c.is_ascii() { c.to_ascii_lowercase() as u8 } else { b'?' };
                        self.name_n += 1;
                    }
                }
            },
            St::Attrs { close, q, slash } => {
                if q != '\0' {
                    if c == q {
                        self.attr_value_done();
                        self.st = St::Attrs { close, q: '\0', slash: false };
                    } else if let Some(slot) = self.val.get_mut(usize::from(self.val_n)) {
                        *slot = if c.is_ascii() { c as u8 } else { b'?' };
                        self.val_n += 1;
                    }
                    return;
                }
                match c {
                    '>' => self.tag_done(close, slash, ev),
                    '/' => self.st = St::Attrs { close, q, slash: true },
                    '"' | '\'' => {
                        self.val_n = 0;
                        self.st = St::Attrs { close, q: c, slash: false };
                    }
                    '=' => {}
                    c if c.is_whitespace() => {
                        if self.val_n == 0 && self.attr_n > 0 && slash {
                            self.st = St::Attrs { close, q, slash: false };
                        }
                    }
                    c => {
                        if self.attr_n > 0 && self.val_n > 0 {
                            self.attr_n = 0;
                        }
                        if let Some(slot) = self.attr.get_mut(usize::from(self.attr_n)) {
                            *slot = if c.is_ascii() { c.to_ascii_lowercase() as u8 } else { b'?' };
                            self.attr_n += 1;
                        }
                        self.st = St::Attrs { close, q, slash: false };
                    }
                }
            }
            St::Bang => {
                // `<!--`, `<![CDATA[`, else a declaration.
                self.mark += 1;
                match (self.mark, c) {
                    (1, '-') => {}
                    (2, '-') => {
                        self.mark = 0;
                        self.st = St::Comment;
                    }
                    (1, '[') => {}
                    (2..=7, c) if "CDATA[".chars().nth(usize::from(self.mark) - 2) == Some(c) => {
                        if self.mark == 7 {
                            self.mark = 0;
                            self.st = St::Cdata;
                        }
                    }
                    (_, '>') => self.st = St::Text,
                    _ => self.st = St::Decl,
                }
            }
            St::Comment => {
                // Ends at `-->`.
                self.mark = match (self.mark, c) {
                    (0, '-') => 1,
                    (1, '-') | (2, '-') => 2,
                    (2, '>') => {
                        self.st = St::Text;
                        0
                    }
                    _ => 0,
                };
            }
            St::Cdata => {
                // Ends at `]]>`; its text is text.
                match (self.mark, c) {
                    (0, ']') => self.mark = 1,
                    (1, ']') => self.mark = 2,
                    (2, ']') => self.text(']', ev),
                    (2, '>') => {
                        self.mark = 0;
                        self.st = St::Text;
                    }
                    (m, c) => {
                        for _ in 0..m {
                            self.text(']', ev);
                        }
                        self.mark = 0;
                        self.text(c, ev);
                    }
                }
            }
            St::Decl => {
                if c == '>' {
                    self.st = St::Text;
                }
            }
            St::Pi => {
                self.mark = match (self.mark, c) {
                    (_, '?') => 1,
                    (1, '>') => {
                        self.st = St::Text;
                        0
                    }
                    _ => 0,
                };
            }
        }
    }

    pub fn finish(&mut self, ev: &mut impl Events) {
        if self.st == St::Entity {
            self.st = St::Text;
            self.entity(false, ev);
        }
        while self.depth > 0 {
            let top = self.top();
            self.depth -= 1;
            self.close(top, ev);
        }
        ev.end_para();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use crate::norm::tests::show;
    use crate::norm::{Emit, TocEntry, Writer};
    use std::string::{String, ToString};
    use std::vec::Vec;

    pub fn convert(text: &str) -> (String, Vec<TocEntry>, Fb2) {
        let mut out = Vec::new();
        let mut w = Writer::new();
        let mut p = Fb2::new();
        let mut toc = Vec::new();
        for c in text.chars() {
            p.char(c, &mut Emit { w: &mut w, out: &mut out });
            if let Some(e) = w.done.take() {
                toc.push(e);
            }
        }
        p.finish(&mut Emit { w: &mut w, out: &mut out });
        w.finish(&mut out);
        if let Some(e) = w.done.take() {
            toc.push(e);
        }
        (show(&out), toc, p)
    }

    pub const SAMPLE: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
<description><title-info><genre>prose</genre>
<author><first-name>Lev</first-name><middle-name>N.</middle-name><last-name>Tolstoy</last-name></author>
<book-title>War &amp; Peace</book-title><lang>en</lang></title-info></description>
<body>
<title><p>War and Peace</p></title>
<epigraph><p>An epigraph.</p><text-author>Someone</text-author></epigraph>
<section><title><p>Book One</p><p>1805</p></title>
<section><title><p>Chapter I</p></title>
<p>"Well, Prince, so <emphasis>Genoa</emphasis> and <strong>Lucca</strong> are now&#160;just family estates.<a l:href="#n1" type="note">[1]</a>"</p>
<empty-line/>
<poem><stanza><v>Line one,</v><v>line two.</v></stanza></poem>
<!-- a comment with <p>tags</p> -->
<p><![CDATA[a <raw> bit]]> &unknown; &#x416;</p>
<image l:href="#pic"/>
</section></section>
</body>
<body name="notes"><title><p>Notes</p></title>
<section id="n1"><title><p>1</p></title><p>A footnote.</p></section>
</body>
<binary id="pic" content-type="image/png">iVBORw0KGgo=</binary>
</FictionBook>"##;

    #[test]
    fn structure() {
        let (s, toc, p) = convert(SAMPLE);
        assert_eq!(
            s,
            "¶1War and Peace¶5An epigraph.¶9Someone¶1Book One¶11805¶2Chapter I\
             ¶0\"Well, Prince, so {i}Genoa{} and {b}Lucca{} are now\u{A0}just family estates.[1]\"\
             ¶10¶6Line one,¶6line two.¶0a <raw> bit &unknown; Ж¶1Notes¶4¶0A footnote."
                .replace("¶4¶0", "¶41¶0")
        );
        let titles: Vec<(u8, String)> = toc.iter().map(|e| (e.level, e.title().to_string())).collect();
        assert_eq!(
            titles,
            [
                (1, "War and Peace".to_string()),
                (1, "Book One. 1805".to_string()),
                (2, "Chapter I".to_string()),
                (1, "Notes".to_string())
            ]
        );
        assert_eq!(p.title.as_str(), "War & Peace");
        assert_eq!(p.author.as_str(), "Lev N. Tolstoy");
    }

    #[test]
    fn malformed_does_not_stall() {
        for t in [
            "<body><p>unclosed <emphasis>em",
            "<body></p></section><p>stray ends</p></body>",
            "<body><p>a < b && c > d</p>",
            "<body><p>&amp</p>",
            "<<<>>>",
            "<body><section><section><section><section><title><p>Deep</p></title></section></section></section></section></body>",
        ] {
            let (s, _, _) = convert(t);
            assert!(!s.is_empty() || t == "<<<>>>", "{t}: {s}");
        }
        let (s, toc, _) = convert(
            "<body><section><section><section><section><title><p>Deep</p></title></section></section></section></section></body>",
        );
        assert_eq!((s.as_str(), toc[0].level), ("¶3Deep", 3));
        assert_eq!(convert("<body><p>a < b && c > d</p>").0, "¶0a < b && c > d");
    }
}
