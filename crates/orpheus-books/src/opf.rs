//! EPUB's package documents (05 §7.15, BOOK-2), each read once as a
//! stream over the shared XML tokenizer, telling what it finds as
//! [`Out`] records (the EPUB job writes them to its scratch tables):
//!
//! * `META-INF/container.xml`: the package document's path (the first
//!   `rootfile` of type `application/oebps-package+xml`, or of no type).
//! * The package document (OPF): `dc:title`, the first `dc:creator`,
//!   `dc:language`; the manifest (id, path, whether a content document,
//!   the navigation document — `properties="nav"` — and the NCX); the
//!   spine in order (`linear="no"` items too: no text is lost).
//! * EPUB 3's navigation document: the `<nav epub:type="toc">`, its
//!   nested `ol`s as levels and every `a href` as an entry (`span`
//!   headings without a link are left out; their entries' links are
//!   kept).
//! * EPUB 2's NCX: `navMap`'s nested `navPoint`s, each with its
//!   `navLabel` and `content src` (in either order).
//!
//! Every `href` is resolved against its document's folder
//! ([`crate::zip::resolve`]).

use crate::enc::Decoder;
use crate::fb2::MetaText;
use crate::norm::{TITLE_MAX, TocEntry};
use crate::xml::{Handler, Xml};
use crate::zip::{Name, Target, resolve};

/// A manifest item's kind.
pub const CONTENT: u8 = 1;
pub const NCX: u8 = 2;

/// A record the job writes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Out {
    /// A manifest item: its id's hash, its path's hash, its kind.
    Item { id: u32, path: u64, kind: u8 },
    /// The next spine item: its `idref`'s hash.
    Spine { idref: u32 },
    /// A contents entry (`offset` not known yet) and where it points.
    Toc { entry: TocEntry, target: Target },
}

/// The book's title, first author and language.
#[derive(Clone, Copy, Debug, Default)]
pub struct Metadata {
    pub title: MetaText<64>,
    pub author: MetaText<48>,
    pub language: MetaText<16>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Meta {
    None,
    Title,
    Creator,
    Language,
}

/// What is being read.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Container,
    Package,
    Nav,
    Ncx,
}

/// The paths a document is read with: its own (hrefs are relative to its
/// folder) and where the ones it names go (container: the package
/// document; package: the navigation document and the NCX). The job's
/// buffers, lent for each feed.
pub struct Paths<'a> {
    pub base: &'a [u8],
    pub path: &'a mut Name,
    pub ncx: &'a mut Name,
}

/// One package document being read.
#[derive(Clone, Copy, Debug)]
pub struct Doc {
    xml: Xml<256>,
    dec: Decoder,
    head: bool,
    /// A library read: stop at the end of the metadata.
    pub stop_at_meta: bool,
    h: Doc0,
}

#[derive(Clone, Copy, Debug)]
struct Doc0 {
    kind: Kind,
    tmp: Name,
    /// The start tag being read: its name and the attributes kept.
    el: El,
    href: Option<Target>,
    id: u32,
    media: u8,
    nav_prop: bool,
    type_toc: bool,
    pkg: bool,
    full_path: bool,
    /// Records for the job (two: one character can finish one tag after a
    /// replacement character's text).
    out: [Option<Out>; 2],
    pub meta: Metadata,
    meta_at: Meta,
    /// Seen: the end of `<metadata>` (a library read stops there).
    pub meta_done: bool,
    /// Container: the package document's path was found (in
    /// [`Paths::path`]). Package: the navigation document's and the
    /// NCX's.
    found: bool,
    has_ncx: bool,
    /// Navigation: inside the contents `nav`, `ol` depth, an `a`'s title
    /// and target being read.
    in_toc: u8,
    navs: u8,
    ol: u8,
    title: MetaText<TITLE_MAX>,
    collecting: bool,
    /// NCX: inside `navMap`, `navPoint` depth, the label being read, an
    /// entry waiting for its label or its children.
    in_map: bool,
    depth: u8,
    in_label: bool,
    pending: Option<(Target, u8)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum El {
    Other,
    Rootfile,
    Metadata,
    Title,
    Creator,
    Language,
    Item,
    Itemref,
    Nav,
    Ol,
    A,
    NavMap,
    NavPoint,
    NavLabel,
    Content,
}

fn element(kind: Kind, name: &[u8]) -> El {
    match (kind, name) {
        (Kind::Container, b"rootfile") => El::Rootfile,
        (Kind::Package, b"metadata") => El::Metadata,
        (Kind::Package, b"title") => El::Title,
        (Kind::Package, b"creator") => El::Creator,
        (Kind::Package, b"language") => El::Language,
        (Kind::Package, b"item") => El::Item,
        (Kind::Package, b"itemref") => El::Itemref,
        (Kind::Nav, b"nav") => El::Nav,
        (Kind::Nav, b"ol") => El::Ol,
        (Kind::Nav, b"a") => El::A,
        (Kind::Ncx, b"navmap") => El::NavMap,
        (Kind::Ncx, b"navpoint") => El::NavPoint,
        (Kind::Ncx, b"navlabel") => El::NavLabel,
        (Kind::Ncx, b"content") => El::Content,
        _ => El::Other,
    }
}

/// `v` has the space-separated token `t`.
fn token(v: &[u8], t: &[u8]) -> bool {
    v.split(|c| c.is_ascii_whitespace()).any(|w| w == t)
}

impl Doc {
    /// Read a document of `kind`.
    #[must_use]
    pub fn new(kind: Kind) -> Doc {
        Doc {
            xml: Xml::new(),
            dec: Decoder::new(crate::enc::Encoding::Utf8),
            head: false,
            stop_at_meta: false,
            h: Doc0 {
                kind,
                tmp: Name::new(),
                el: El::Other,
                href: None,
                id: 0,
                media: 0,
                nav_prop: false,
                type_toc: false,
                pkg: false,
                full_path: false,
                out: [None; 2],
                meta: Metadata::default(),
                meta_at: Meta::None,
                meta_done: false,
                found: false,
                has_ncx: false,
                in_toc: 0,
                navs: 0,
                ol: 0,
                title: MetaText::default(),
                collecting: false,
                in_map: false,
                depth: 0,
                in_label: false,
                pending: None,
            },
        }
    }

    #[must_use]
    pub fn kind(&self) -> Kind {
        self.h.kind
    }

    /// Feed bytes until a record waits (returns the bytes taken).
    pub fn feed(&mut self, b: &[u8], p: &mut Paths<'_>) -> usize {
        if !self.head && !b.is_empty() {
            // The encoding from the first bytes (UTF-8 or UTF-16).
            self.head = true;
            let (enc, _) = crate::enc::detect(b, false);
            self.dec = Decoder::new(enc);
        }
        let mut used = 0;
        for &c in b {
            if self.h.out[0].is_some() || (self.h.meta_done && self.stop_at_meta) {
                break;
            }
            used += 1;
            let xml = &mut self.xml;
            let mut on = On { d: &mut self.h, p: &mut *p };
            self.dec.feed(&[c], &mut |ch| {
                if ch != '\u{FEFF}' {
                    xml.char(ch, &mut on);
                }
            });
        }
        used
    }

    /// The document ends.
    pub fn finish(&mut self, p: &mut Paths<'_>) {
        let xml = &mut self.xml;
        let mut on = On { d: &mut self.h, p };
        self.dec.finish(&mut |ch| xml.char(ch, &mut on));
        self.xml.finish(&mut on);
        self.h.flush_pending();
    }

    /// A record, if one waits.
    pub fn take(&mut self) -> Option<Out> {
        let o = self.h.out[0].take();
        self.h.out[0] = self.h.out[1].take();
        o
    }

    #[must_use]
    pub fn meta(&self) -> &Metadata {
        &self.h.meta
    }

    /// The end of the metadata was read.
    #[must_use]
    pub fn meta_done(&self) -> bool {
        self.h.meta_done
    }

    /// Container: the package document's path was put in
    /// [`Paths::path`]; package: the navigation document's.
    #[must_use]
    pub fn found(&self) -> bool {
        self.h.found
    }

    /// Package: the NCX's path was put in [`Paths::ncx`].
    #[must_use]
    pub fn has_ncx(&self) -> bool {
        self.h.has_ncx
    }
}

impl Doc0 {
    fn emit(&mut self, o: Out) {
        if self.out[0].is_none() {
            self.out[0] = Some(o);
        } else if self.out[1].is_none() {
            self.out[1] = Some(o);
        }
    }

    fn toc(&mut self, target: Target, level: u8) {
        let t = self.title.as_str();
        if t.is_empty() {
            return;
        }
        let mut e = TocEntry { offset: u32::MAX, level: level.clamp(1, 3), ..TocEntry::EMPTY };
        let b = t.as_bytes();
        e.title[..b.len()].copy_from_slice(b);
        e.len = b.len() as u8;
        self.emit(Out::Toc { entry: e, target });
    }

    fn flush_pending(&mut self) {
        if let Some((target, level)) = self.pending.take() {
            self.toc(target, level);
        }
        self.title.clear();
    }
}

/// The tokenizer's events with the document's state and the paths.
struct On<'a, 'p> {
    d: &'a mut Doc0,
    p: &'a mut Paths<'p>,
}

impl Handler for On<'_, '_> {
    fn start(&mut self, name: &[u8]) {
        let d = &mut *self.d;
        d.el = element(d.kind, name);
        d.href = None;
        d.id = 0;
        d.media = 0;
        d.nav_prop = false;
        d.type_toc = false;
        d.pkg = true;
        d.full_path = false;
    }

    fn attr(&mut self, name: &[u8], v: &[u8], hash: u32) {
        let (d, p) = (&mut *self.d, &mut *self.p);
        match (d.el, name) {
            (El::Rootfile, b"full-path") if !d.found => {
                p.path.set(v);
                d.full_path = true;
            }
            (El::Rootfile, b"media-type") => {
                d.pkg = v.trim_ascii().eq_ignore_ascii_case(b"application/oebps-package+xml");
            }
            (El::Item, b"id") | (El::Itemref, b"idref") => d.id = hash,
            (El::Item | El::A, b"href") | (El::Content, b"src") => {
                d.href = resolve(p.base, v.trim_ascii(), &mut d.tmp);
            }
            (El::Item, b"media-type") => {
                let v = v.trim_ascii();
                d.media = if v.eq_ignore_ascii_case(b"application/xhtml+xml")
                    || v.eq_ignore_ascii_case(b"text/html")
                    || v.eq_ignore_ascii_case(b"application/xml")
                {
                    CONTENT
                } else if v.eq_ignore_ascii_case(b"application/x-dtbncx+xml") {
                    NCX
                } else {
                    0
                };
            }
            (El::Item, b"properties") => d.nav_prop = token(v, b"nav"),
            (El::Nav, b"type") => d.type_toc = token(v, b"toc"),
            _ => {}
        }
    }

    fn start_end(&mut self, empty: bool) {
        let (d, p) = (&mut *self.d, &mut *self.p);
        d.start_end(empty, p);
    }

    fn end(&mut self, name: &[u8]) {
        self.d.end(name);
    }

    fn text(&mut self, c: char) {
        self.d.text(c);
    }
}

impl Doc0 {
    fn start_end(&mut self, empty: bool, p: &mut Paths<'_>) {
        match self.el {
            El::Rootfile => {
                if self.full_path && self.pkg && !self.found {
                    self.found = true;
                } else if !self.found {
                    p.path.clear();
                }
            }
            El::Title | El::Creator | El::Language if !empty => {
                self.meta_at = match self.el {
                    El::Title if self.meta.title.is_empty() => Meta::Title,
                    El::Creator if self.meta.author.is_empty() => Meta::Creator,
                    El::Language if self.meta.language.is_empty() => Meta::Language,
                    _ => Meta::None,
                };
            }
            El::Item => {
                if let Some(t) = self.href {
                    if self.nav_prop && !self.found {
                        *p.path = self.tmp;
                        self.found = true;
                    } else if self.media == NCX && !self.has_ncx {
                        *p.ncx = self.tmp;
                        self.has_ncx = true;
                    }
                    let kind = if self.media == CONTENT || self.nav_prop { CONTENT } else { self.media };
                    self.emit(Out::Item { id: self.id, path: t.path, kind });
                }
            }
            // Non-linear items (`linear="no"`) are read in their place too.
            El::Itemref => self.emit(Out::Spine { idref: self.id }),
            El::Nav if !empty => {
                if self.in_toc > 0 {
                    self.navs += 1;
                } else if self.type_toc {
                    self.in_toc = 1;
                    self.navs = 1;
                    self.ol = 0;
                }
            }
            El::Ol if self.in_toc > 0 && !empty => self.ol = self.ol.saturating_add(1),
            El::A if self.in_toc > 0 && !empty => {
                self.title.clear();
                self.collecting = self.href.is_some();
                self.pending = self.href.map(|t| (t, self.ol.max(1)));
            }
            El::NavMap if !empty => self.in_map = true,
            El::NavPoint if self.in_map => {
                // A parent's entry comes before its children's.
                self.flush_pending();
                if !empty {
                    self.depth = self.depth.saturating_add(1);
                }
            }
            El::NavLabel if self.in_map && !empty => {
                self.title.clear();
                self.in_label = true;
            }
            El::Content if self.in_map => {
                if let Some(t) = self.href {
                    self.pending = Some((t, self.depth.max(1)));
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, name: &[u8]) {
        match element(self.kind, name) {
            El::Metadata => self.meta_done = true,
            El::Title | El::Creator | El::Language => self.meta_at = Meta::None,
            El::Nav if self.in_toc > 0 => {
                self.navs = self.navs.saturating_sub(1);
                if self.navs == 0 {
                    self.in_toc = 0;
                }
            }
            El::Ol if self.in_toc > 0 => self.ol = self.ol.saturating_sub(1),
            El::A if self.collecting => {
                self.collecting = false;
                self.flush_pending();
            }
            El::NavMap => {
                self.flush_pending();
                self.in_map = false;
            }
            El::NavPoint if self.in_map => {
                self.flush_pending();
                self.depth = self.depth.saturating_sub(1);
            }
            El::NavLabel => self.in_label = false,
            _ => {}
        }
    }

    fn text(&mut self, c: char) {
        match self.meta_at {
            Meta::Title => self.meta.title.push(c),
            Meta::Creator => self.meta.author.push(c),
            Meta::Language => self.meta.language.push(c),
            Meta::None => {}
        }
        if self.collecting || self.in_label {
            self.title.push(c);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use crate::xml::hash32;
    use crate::zip::name_hash;
    use std::string::{String, ToString};
    use std::vec::Vec;

    /// A document read in pieces: it, its records, and the paths it named
    /// (path, NCX).
    pub fn read(kind: Kind, path: &str, text: &str, piece: usize) -> (Doc, Vec<Out>, Name, Name) {
        let mut d = Doc::new(kind);
        let (mut named, mut ncx) = (Name::new(), Name::new());
        let mut paths = Paths { base: path.as_bytes(), path: &mut named, ncx: &mut ncx };
        let mut outs = Vec::new();
        for p in text.as_bytes().chunks(piece) {
            let mut at = 0;
            while at < p.len() {
                at += d.feed(&p[at..], &mut paths);
                while let Some(o) = d.take() {
                    outs.push(o);
                }
            }
        }
        d.finish(&mut paths);
        while let Some(o) = d.take() {
            outs.push(o);
        }
        (d, outs, named, ncx)
    }

    #[test]
    fn container() {
        let (d, _, path, _) = read(
            Kind::Container,
            "META-INF/container.xml",
            r#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="other.pdf" media-type="application/pdf"/>
<rootfile media-type="application/oebps-package+xml" full-path="OEBPS/content.opf"/></rootfiles></container>"#,
            7,
        );
        assert!(d.found());
        assert_eq!(path.as_bytes(), b"OEBPS/content.opf");
    }

    pub const OPF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:identifier id="uid">urn:x</dc:identifier>
<dc:title>Moby-Dick &amp; Other</dc:title><dc:title>Subtitle</dc:title>
<dc:creator opf:role="aut">Herman
 Melville</dc:creator><dc:creator>Someone Else</dc:creator>
<dc:language>en-US</dc:language>
</metadata>
<manifest>
<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
<item id="css" href="style.css" media-type="text/css"/>
<item id="c1" href="Text/ch%201.xhtml" media-type="application/xhtml+xml"/>
<item id="c2" href="Text/ch2.xhtml" media-type="application/xhtml+xml"/>
<item id="img" href="../Images/cover.jpg" media-type="image/jpeg"/>
</manifest>
<spine toc="ncx"><itemref idref="c1"/><itemref idref="nav" linear="no"/><itemref idref="c2"/></spine>
</package>"#;

    #[test]
    fn package() {
        for piece in [1, 13, 4096] {
            let (d, outs, nav, ncx) = read(Kind::Package, "OEBPS/content.opf", OPF, piece);
            let m = d.meta();
            assert_eq!(m.title.as_str(), "Moby-Dick & Other");
            assert_eq!(m.author.as_str(), "Herman Melville");
            assert_eq!(m.language.as_str(), "en-US");
            assert!(d.found() && d.has_ncx());
            assert_eq!(nav.as_bytes(), b"OEBPS/nav.xhtml");
            assert_eq!(ncx.as_bytes(), b"OEBPS/toc.ncx");
            let items: Vec<_> = outs
                .iter()
                .filter_map(|o| match o {
                    Out::Item { id, path, kind } => Some((*id, *path, *kind)),
                    _ => None,
                })
                .collect();
            assert_eq!(items.len(), 6);
            assert_eq!(items[3], (hash32(b"c1"), name_hash(b"OEBPS/Text/ch 1.xhtml"), CONTENT));
            assert_eq!(items[5], (hash32(b"img"), name_hash(b"Images/cover.jpg"), 0));
            assert_eq!(items[1].2, NCX);
            let spine: Vec<u32> = outs
                .iter()
                .filter_map(|o| match o {
                    Out::Spine { idref } => Some(*idref),
                    _ => None,
                })
                .collect();
            assert_eq!(spine, [hash32(b"c1"), hash32(b"nav"), hash32(b"c2")]);
        }
    }

    fn entries(outs: &[Out]) -> Vec<(u8, String, u64, Option<u32>)> {
        outs.iter()
            .filter_map(|o| match o {
                Out::Toc { entry, target } => {
                    Some((entry.level, entry.title().to_string(), target.path, target.frag))
                }
                _ => None,
            })
            .collect()
    }

    pub const NAV: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Contents</title></head>
<body><nav epub:type="landmarks"><ol><li><a href="Text/ch%201.xhtml">Start</a></li></ol></nav>
<nav epub:type="toc" id="toc"><h1>Contents</h1><ol>
<li><a href="Text/ch%201.xhtml">Chapter 1.
   Loomings</a>
  <ol><li><a href="Text/ch%201.xhtml#s2"><span>Part</span> two</a></li>
      <li><span>No link</span><ol><li><a href="Text/ch%201.xhtml#deep">Deep&#160;one</a></li></ol></li></ol></li>
<li><a href="Text/ch2.xhtml#top">Chapter 2</a></li>
<li><a href="http://example.com/">Elsewhere</a></li>
</ol></nav><nav epub:type="page-list"><ol><li><a href="Text/ch2.xhtml#p5">5</a></li></ol></nav></body></html>"#;

    #[test]
    fn navigation() {
        let (_, outs, ..) = read(Kind::Nav, "OEBPS/nav.xhtml", NAV, 5);
        let c1 = name_hash(b"OEBPS/Text/ch 1.xhtml");
        let h = |s: &str| Some(hash32(s.as_bytes()));
        assert_eq!(
            entries(&outs),
            [
                (1, "Chapter 1. Loomings".to_string(), c1, None),
                (2, "Part two".to_string(), c1, h("s2")),
                (3, "Deep one".to_string(), c1, h("deep")),
                (1, "Chapter 2".to_string(), name_hash(b"OEBPS/Text/ch2.xhtml"), h("top")),
            ]
        );
    }

    #[test]
    fn ncx() {
        let text = r#"<?xml version="1.0"?><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
<head><meta name="dtb:depth" content="2"/></head><docTitle><text>Ignored</text></docTitle>
<navMap>
<navPoint id="n1" playOrder="1"><navLabel><text>Chapter One</text></navLabel><content src="text/c1.html"/>
  <navPoint id="n2" playOrder="2"><content src="text/c1.html#sec"/><navLabel><text>A Section</text></navLabel></navPoint>
</navPoint>
<navPoint id="n3" playOrder="3"><navLabel><text>Chapter Two</text></navLabel><content src="text/c2.html"/></navPoint>
</navMap><pageList><pageTarget><navLabel><text>1</text></navLabel><content src="text/c1.html#p1"/></pageTarget></pageList></ncx>"#;
        let (_, outs, ..) = read(Kind::Ncx, "OPS/toc.ncx", text, 3);
        assert_eq!(
            entries(&outs),
            [
                (1, "Chapter One".to_string(), name_hash(b"OPS/text/c1.html"), None),
                (2, "A Section".to_string(), name_hash(b"OPS/text/c1.html"), Some(hash32(b"sec"))),
                (1, "Chapter Two".to_string(), name_hash(b"OPS/text/c2.html"), None),
            ]
        );
    }
}
