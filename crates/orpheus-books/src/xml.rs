//! An XML tokenizer over `char`s, shared by FB2 (BOOK-1) and EPUB's
//! container, package, navigation and XHTML documents (BOOK-2): start
//! tags with their attributes, end tags, text; comments, CDATA,
//! processing instructions and declarations; named and numeric character
//! references in text and attribute values. Lenient: a stray `<` is text,
//! an unknown or unterminated reference is kept as written, unquoted
//! attribute values (HTML) are read; it never stalls and never fails.
//!
//! Names are lower-cased ASCII with their namespace prefix dropped
//! (`dc:title` → `title`, `epub:type` → `type`); other characters read as
//! `?`; a name is cut at [`NAME`] bytes. Attribute values keep their first
//! `V` bytes of UTF-8 (a cut value is flagged) and the FNV-1a hash of the
//! whole value (ids are matched by it).

/// Bytes of an element or attribute name kept.
pub const NAME: usize = 16;

/// FNV-1a (32 bits): the start value.
pub const FNV32: u32 = 0x811C_9DC5;

/// FNV-1a (32 bits): one byte more.
#[must_use]
pub fn fnv32(h: u32, b: u8) -> u32 {
    (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
}

/// FNV-1a (32 bits) of `b`.
#[must_use]
pub fn hash32(b: &[u8]) -> u32 {
    b.iter().fold(FNV32, |h, &x| fnv32(h, x))
}

/// What the tokenizer finds.
pub trait Handler {
    /// A start tag's name.
    fn start(&mut self, name: &[u8]);
    /// One attribute of the start tag (value with references decoded; its
    /// first bytes, `hash` over all of them).
    fn attr(&mut self, _name: &[u8], _value: &[u8], _hash: u32) {}
    /// The start tag ends; `empty`: it closed itself (`<br/>`).
    fn start_end(&mut self, empty: bool);
    /// An end tag.
    fn end(&mut self, name: &[u8]);
    /// A character of text (CDATA and references included).
    fn text(&mut self, c: char);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum St {
    Text,
    /// After `<`.
    Open,
    /// In a tag's name.
    Name,
    /// In a tag between attributes.
    Tag,
    AttrName,
    /// After an attribute's name: `=`, or the next attribute.
    AfterName,
    /// After `=`.
    BeforeValue,
    /// In a value (`q`: its quote, or none: unquoted).
    Value,
    /// `<!` and what follows: comment, CDATA or a declaration.
    Bang,
    Comment,
    Cdata,
    Decl,
    Pi,
    /// A character reference (`ent_value`: inside an attribute value).
    Entity,
}

/// The named references known: HTML 4's Latin-1, symbol and special sets
/// (and `apos`), sorted for a binary search.
#[rustfmt::skip]
static ENTITIES: [(&[u8], u16); 204] = [
    (b"AElig", 0x00C6), (b"Aacute", 0x00C1), (b"Acirc", 0x00C2), (b"Agrave", 0x00C0), (b"Alpha", 0x0391),
    (b"Aring", 0x00C5), (b"Atilde", 0x00C3), (b"Auml", 0x00C4), (b"Beta", 0x0392), (b"Ccedil", 0x00C7),
    (b"Chi", 0x03A7), (b"Dagger", 0x2021), (b"Delta", 0x0394), (b"ETH", 0x00D0), (b"Eacute", 0x00C9),
    (b"Ecirc", 0x00CA), (b"Egrave", 0x00C8), (b"Epsilon", 0x0395), (b"Eta", 0x0397), (b"Euml", 0x00CB),
    (b"Gamma", 0x0393), (b"Iacute", 0x00CD), (b"Icirc", 0x00CE), (b"Igrave", 0x00CC), (b"Iota", 0x0399),
    (b"Iuml", 0x00CF), (b"Kappa", 0x039A), (b"Lambda", 0x039B), (b"Mu", 0x039C), (b"Ntilde", 0x00D1),
    (b"Nu", 0x039D), (b"OElig", 0x0152), (b"Oacute", 0x00D3), (b"Ocirc", 0x00D4), (b"Ograve", 0x00D2),
    (b"Omega", 0x03A9), (b"Omicron", 0x039F), (b"Oslash", 0x00D8), (b"Otilde", 0x00D5), (b"Ouml", 0x00D6),
    (b"Phi", 0x03A6), (b"Pi", 0x03A0), (b"Prime", 0x2033), (b"Psi", 0x03A8), (b"Rho", 0x03A1),
    (b"Scaron", 0x0160), (b"Sigma", 0x03A3), (b"THORN", 0x00DE), (b"Tau", 0x03A4), (b"Theta", 0x0398),
    (b"Uacute", 0x00DA), (b"Ucirc", 0x00DB), (b"Ugrave", 0x00D9), (b"Upsilon", 0x03A5), (b"Uuml", 0x00DC),
    (b"Xi", 0x039E), (b"Yacute", 0x00DD), (b"Yuml", 0x0178), (b"Zeta", 0x0396), (b"aacute", 0x00E1),
    (b"acirc", 0x00E2), (b"acute", 0x00B4), (b"aelig", 0x00E6), (b"agrave", 0x00E0), (b"alpha", 0x03B1),
    (b"amp", 0x0026), (b"apos", 0x0027), (b"aring", 0x00E5), (b"asymp", 0x2248), (b"atilde", 0x00E3),
    (b"auml", 0x00E4), (b"bdquo", 0x201E), (b"beta", 0x03B2), (b"brvbar", 0x00A6), (b"bull", 0x2022),
    (b"ccedil", 0x00E7), (b"cedil", 0x00B8), (b"cent", 0x00A2), (b"chi", 0x03C7), (b"circ", 0x02C6),
    (b"clubs", 0x2663), (b"copy", 0x00A9), (b"curren", 0x00A4), (b"dagger", 0x2020), (b"darr", 0x2193),
    (b"deg", 0x00B0), (b"delta", 0x03B4), (b"diams", 0x2666), (b"divide", 0x00F7), (b"eacute", 0x00E9),
    (b"ecirc", 0x00EA), (b"egrave", 0x00E8), (b"emsp", 0x2003), (b"ensp", 0x2002), (b"epsilon", 0x03B5),
    (b"eta", 0x03B7), (b"eth", 0x00F0), (b"euml", 0x00EB), (b"euro", 0x20AC), (b"fnof", 0x0192),
    (b"frac12", 0x00BD), (b"frac14", 0x00BC), (b"frac34", 0x00BE), (b"frasl", 0x2044), (b"gamma", 0x03B3),
    (b"ge", 0x2265), (b"gt", 0x003E), (b"harr", 0x2194), (b"hearts", 0x2665), (b"hellip", 0x2026),
    (b"iacute", 0x00ED), (b"icirc", 0x00EE), (b"iexcl", 0x00A1), (b"igrave", 0x00EC), (b"infin", 0x221E),
    (b"iota", 0x03B9), (b"iquest", 0x00BF), (b"iuml", 0x00EF), (b"kappa", 0x03BA), (b"lambda", 0x03BB),
    (b"laquo", 0x00AB), (b"larr", 0x2190), (b"ldquo", 0x201C), (b"le", 0x2264), (b"loz", 0x25CA),
    (b"lrm", 0x200E), (b"lsaquo", 0x2039), (b"lsquo", 0x2018), (b"lt", 0x003C), (b"macr", 0x00AF),
    (b"mdash", 0x2014), (b"micro", 0x00B5), (b"middot", 0x00B7), (b"minus", 0x2212), (b"mu", 0x03BC),
    (b"nbsp", 0x00A0), (b"ndash", 0x2013), (b"ne", 0x2260), (b"not", 0x00AC), (b"ntilde", 0x00F1),
    (b"nu", 0x03BD), (b"oacute", 0x00F3), (b"ocirc", 0x00F4), (b"oelig", 0x0153), (b"ograve", 0x00F2),
    (b"oline", 0x203E), (b"omega", 0x03C9), (b"omicron", 0x03BF), (b"ordf", 0x00AA), (b"ordm", 0x00BA),
    (b"oslash", 0x00F8), (b"otilde", 0x00F5), (b"ouml", 0x00F6), (b"para", 0x00B6), (b"permil", 0x2030),
    (b"phi", 0x03C6), (b"pi", 0x03C0), (b"plusmn", 0x00B1), (b"pound", 0x00A3), (b"prime", 0x2032),
    (b"psi", 0x03C8), (b"quot", 0x0022), (b"radic", 0x221A), (b"raquo", 0x00BB), (b"rarr", 0x2192),
    (b"rdquo", 0x201D), (b"reg", 0x00AE), (b"rho", 0x03C1), (b"rlm", 0x200F), (b"rsaquo", 0x203A),
    (b"rsquo", 0x2019), (b"sbquo", 0x201A), (b"scaron", 0x0161), (b"sect", 0x00A7), (b"shy", 0x00AD),
    (b"sigma", 0x03C3), (b"sigmaf", 0x03C2), (b"spades", 0x2660), (b"sum", 0x2211), (b"sup1", 0x00B9),
    (b"sup2", 0x00B2), (b"sup3", 0x00B3), (b"szlig", 0x00DF), (b"tau", 0x03C4), (b"theta", 0x03B8),
    (b"thinsp", 0x2009), (b"thorn", 0x00FE), (b"tilde", 0x02DC), (b"times", 0x00D7), (b"trade", 0x2122),
    (b"uacute", 0x00FA), (b"uarr", 0x2191), (b"ucirc", 0x00FB), (b"ugrave", 0x00F9), (b"uml", 0x00A8),
    (b"upsilon", 0x03C5), (b"uuml", 0x00FC), (b"xi", 0x03BE), (b"yacute", 0x00FD), (b"yen", 0x00A5),
    (b"yuml", 0x00FF), (b"zeta", 0x03B6), (b"zwj", 0x200D), (b"zwnj", 0x200C),
];

/// The character a reference names (`amp`, `#233`, `#xE9`), if known.
/// Numeric references to 0x80–0x9F mean Windows-1252 (as browsers read
/// them: `&#151;` is an em dash).
#[must_use]
pub fn entity(name: &[u8]) -> Option<char> {
    let num = |digits: &[u8], radix: u32| -> Option<char> {
        let v = u32::from_str_radix(core::str::from_utf8(digits).ok()?, radix).ok()?;
        if (0x80..0xA0).contains(&v) {
            let mut out = None;
            crate::enc::Decoder::new(crate::enc::Encoding::Latin1).feed(&[v as u8], &mut |c| out = Some(c));
            return out;
        }
        char::from_u32(v)
    };
    match name {
        [b'#', b'x' | b'X', hex @ ..] if !hex.is_empty() => num(hex, 16),
        [b'#', dec @ ..] if !dec.is_empty() => num(dec, 10),
        _ => {
            let i = ENTITIES.binary_search_by(|(n, _)| (*n).cmp(name)).ok()?;
            char::from_u32(u32::from(ENTITIES[i].1))
        }
    }
}

/// The tokenizer: one character at a time. `V`: bytes of an attribute
/// value kept (FB2 needs 8, EPUB's paths 256).
#[derive(Clone, Copy, Debug)]
pub struct Xml<const V: usize> {
    st: St,
    /// The tag is an end tag / a `/` was the last thing in it.
    close: bool,
    slash: bool,
    /// The value's quote (`'\0'`: unquoted).
    q: char,
    name: [u8; NAME],
    name_n: u8,
    attr: [u8; NAME],
    attr_n: u8,
    val: [u8; V],
    val_n: u16,
    hash: u32,
    /// `<!`-construct and comment end matching.
    mark: u8,
    ent: [u8; 10],
    ent_n: u8,
    ent_value: bool,
}

impl<const V: usize> Default for Xml<V> {
    fn default() -> Self {
        Xml::new()
    }
}

fn push_name(buf: &mut [u8; NAME], n: &mut u8, c: char) {
    if c == ':' {
        // A namespace prefix: the name starts again.
        *n = 0;
        return;
    }
    if let Some(slot) = buf.get_mut(usize::from(*n)) {
        *slot = if c.is_ascii() { c.to_ascii_lowercase() as u8 } else { b'?' };
        *n += 1;
    }
}

impl<const V: usize> Xml<V> {
    #[must_use]
    pub const fn new() -> Xml<V> {
        Xml {
            st: St::Text,
            close: false,
            slash: false,
            q: '\0',
            name: [0; NAME],
            name_n: 0,
            attr: [0; NAME],
            attr_n: 0,
            val: [0; V],
            val_n: 0,
            hash: FNV32,
            mark: 0,
            ent: [0; 10],
            ent_n: 0,
            ent_value: false,
        }
    }

    /// Inside markup (not text): a feed may stop anywhere, this only tells.
    #[must_use]
    pub fn in_text(&self) -> bool {
        self.st == St::Text
    }

    fn name(&self) -> &[u8] {
        &self.name[..usize::from(self.name_n)]
    }

    fn push_val(&mut self, c: char) {
        let mut tmp = [0u8; 4];
        let s = c.encode_utf8(&mut tmp).as_bytes();
        for &b in s {
            self.hash = fnv32(self.hash, b);
        }
        let at = usize::from(self.val_n);
        if let Some(dst) = self.val.get_mut(at..at + s.len()) {
            dst.copy_from_slice(s);
            self.val_n += s.len() as u16;
        } else {
            // Cut: nothing more is kept (not even a shorter character).
            self.val_n = V as u16;
        }
    }

    /// The attribute read so far is complete.
    fn attr_done(&mut self, h: &mut impl Handler) {
        if self.attr_n > 0 {
            let n = usize::from(self.val_n).min(V);
            h.attr(&self.attr[..usize::from(self.attr_n)], &self.val[..n], self.hash);
        }
        self.attr_n = 0;
        self.val_n = 0;
        self.hash = FNV32;
    }

    fn new_attr(&mut self, c: char) {
        self.attr_n = 0;
        self.val_n = 0;
        self.hash = FNV32;
        push_name(&mut self.attr, &mut self.attr_n, c);
        self.slash = false;
        self.st = St::AttrName;
    }

    /// The name ends: a start tag is told at once (its attributes follow).
    fn name_done(&mut self, h: &mut impl Handler) {
        if !self.close {
            let n = usize::from(self.name_n);
            h.start(&self.name[..n]);
        }
    }

    fn tag_done(&mut self, h: &mut impl Handler) {
        if self.close {
            h.end(self.name());
        } else {
            h.start_end(self.slash);
        }
        self.slash = false;
        self.st = St::Text;
    }

    fn put(&mut self, c: char, h: &mut impl Handler) {
        if self.ent_value { self.push_val(c) } else { h.text(c) }
    }

    fn entity_done(&mut self, semi: bool, h: &mut impl Handler) {
        let n = usize::from(self.ent_n);
        self.ent_n = 0;
        let ent = self.ent;
        match entity(&ent[..n]) {
            Some(c) => self.put(c, h),
            None => {
                // Not a reference: the text as it was.
                self.put('&', h);
                for &b in &ent[..n] {
                    self.put(char::from(b), h);
                }
                if semi {
                    self.put(';', h);
                }
            }
        }
    }

    pub fn char(&mut self, c: char, h: &mut impl Handler) {
        match self.st {
            St::Text => match c {
                '<' => self.st = St::Open,
                '&' => {
                    self.ent_n = 0;
                    self.ent_value = false;
                    self.st = St::Entity;
                }
                c => h.text(c),
            },
            St::Entity => {
                let back = if self.ent_value { St::Value } else { St::Text };
                if c == ';' {
                    self.st = back;
                    self.entity_done(true, h);
                } else if (c.is_ascii_alphanumeric() || c == '#')
                    && let Some(slot) = self.ent.get_mut(usize::from(self.ent_n))
                {
                    *slot = c as u8;
                    self.ent_n += 1;
                } else {
                    self.st = back;
                    self.entity_done(false, h);
                    self.char(c, h);
                }
            }
            St::Open => match c {
                '/' => {
                    self.name_n = 0;
                    self.close = true;
                    self.st = St::Name;
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
                    self.close = false;
                    self.slash = false;
                    self.st = St::Name;
                    push_name(&mut self.name, &mut self.name_n, c);
                }
                c => {
                    // A stray `<`.
                    self.st = St::Text;
                    h.text('<');
                    self.char(c, h);
                }
            },
            St::Name => match c {
                '>' => {
                    self.name_done(h);
                    self.tag_done(h);
                }
                '/' => {
                    self.name_done(h);
                    self.slash = true;
                    self.st = St::Tag;
                }
                c if c.is_whitespace() => {
                    self.name_done(h);
                    self.st = St::Tag;
                }
                c => push_name(&mut self.name, &mut self.name_n, c),
            },
            St::Tag => match c {
                '>' => self.tag_done(h),
                '/' => self.slash = true,
                c if c.is_whitespace() => {}
                '=' => {}
                '"' | '\'' => {
                    // A value without a name: read and dropped.
                    self.attr_n = 0;
                    self.val_n = 0;
                    self.q = c;
                    self.st = St::Value;
                }
                c => self.new_attr(c),
            },
            St::AttrName => match c {
                '=' => self.st = St::BeforeValue,
                '>' => {
                    self.attr_done(h);
                    self.tag_done(h);
                }
                '/' => {
                    self.attr_done(h);
                    self.slash = true;
                    self.st = St::Tag;
                }
                c if c.is_whitespace() => self.st = St::AfterName,
                c => push_name(&mut self.attr, &mut self.attr_n, c),
            },
            St::AfterName => match c {
                '=' => self.st = St::BeforeValue,
                c if c.is_whitespace() => {}
                '>' => {
                    self.attr_done(h);
                    self.tag_done(h);
                }
                '/' => {
                    self.attr_done(h);
                    self.slash = true;
                    self.st = St::Tag;
                }
                c => {
                    self.attr_done(h);
                    self.new_attr(c);
                }
            },
            St::BeforeValue => match c {
                c if c.is_whitespace() => {}
                '"' | '\'' => {
                    self.q = c;
                    self.st = St::Value;
                }
                '>' => {
                    self.attr_done(h);
                    self.tag_done(h);
                }
                c => {
                    self.q = '\0';
                    self.st = St::Value;
                    self.char(c, h);
                }
            },
            St::Value => {
                let end = if self.q == '\0' { c.is_whitespace() || c == '>' } else { c == self.q };
                if end {
                    self.attr_done(h);
                    self.st = St::Tag;
                    if c == '>' {
                        self.tag_done(h);
                    }
                } else if c == '&' {
                    self.ent_n = 0;
                    self.ent_value = true;
                    self.st = St::Entity;
                } else {
                    self.push_val(c);
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
                    (2, ']') => h.text(']'),
                    (2, '>') => {
                        self.mark = 0;
                        self.st = St::Text;
                    }
                    (m, c) => {
                        for _ in 0..m {
                            h.text(']');
                        }
                        self.mark = 0;
                        h.text(c);
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

    /// End of the document: an unfinished reference is told as text.
    pub fn finish(&mut self, h: &mut impl Handler) {
        if self.st == St::Entity && !self.ent_value {
            self.st = St::Text;
            self.entity_done(false, h);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use std::string::String;
    use std::vec::Vec;

    /// Every event as text: `<name a=v>`, `</name>`, `<name/>`.
    #[derive(Default)]
    pub struct Log(pub String);

    impl Handler for Log {
        fn start(&mut self, name: &[u8]) {
            self.0.push('<');
            self.0.push_str(core::str::from_utf8(name).unwrap());
        }
        fn attr(&mut self, name: &[u8], value: &[u8], hash: u32) {
            assert!(value.len() >= 8 || hash32(value) == hash, "the whole value");
            self.0.push(' ');
            self.0.push_str(core::str::from_utf8(name).unwrap());
            self.0.push('=');
            self.0.push_str(core::str::from_utf8(value).unwrap());
        }
        fn start_end(&mut self, empty: bool) {
            self.0.push_str(if empty { "/>" } else { ">" });
        }
        fn end(&mut self, name: &[u8]) {
            self.0.push_str("</");
            self.0.push_str(core::str::from_utf8(name).unwrap());
            self.0.push('>');
        }
        fn text(&mut self, c: char) {
            self.0.push(c);
        }
    }

    fn log<const V: usize>(s: &str) -> String {
        let mut x = Xml::<V>::new();
        let mut l = Log::default();
        for c in s.chars() {
            x.char(c, &mut l);
        }
        x.finish(&mut l);
        l.0
    }

    #[test]
    fn tags_attributes_references() {
        assert_eq!(
            log::<64>(
                r#"<?xml version="1.0"?><!DOCTYPE html><html:P Class='a b' id=x1 hidden><br/><a href="a&amp;b.xhtml#c" >&eacute;&#233;&#xE9;&#151;&bogus;&amp</a></html:P>"#
            ),
            "<p class=a b id=x1 hidden=><br/><a href=a&b.xhtml#c>éé\u{e9}—&bogus;&</a></p>"
        );
        assert_eq!(log::<64>("<x a = \"1\" b='2'/>"), "<x a=1 b=2/>");
        assert_eq!(log::<64>("a < b && c > d"), "a < b && c > d");
        assert_eq!(log::<64>("<!-- <p>x</p> --><![CDATA[<raw>]]]>"), "<raw>]");
        assert_eq!(log::<64>("<epub:switch xmlns:epub='u'>"), "<switch epub=u>");
    }

    #[test]
    fn long_values_are_cut_but_hashed() {
        let long = "x".repeat(100);
        let s = log::<8>(&std::format!("<a id=\"{long}\">"));
        assert_eq!(s, "<a id=xxxxxxxx>");
        let mut x = Xml::<8>::new();
        struct H(u32);
        impl Handler for H {
            fn start(&mut self, _: &[u8]) {}
            fn attr(&mut self, _: &[u8], _: &[u8], hash: u32) {
                self.0 = hash;
            }
            fn start_end(&mut self, _: bool) {}
            fn end(&mut self, _: &[u8]) {}
            fn text(&mut self, _: char) {}
        }
        let mut h = H(0);
        for c in std::format!("<a id=\"{long}\">").chars() {
            x.char(c, &mut h);
        }
        assert_eq!(h.0, hash32(long.as_bytes()));
    }

    #[test]
    fn entity_table() {
        let names: Vec<&[u8]> = ENTITIES.iter().map(|e| e.0).collect();
        assert!(names.windows(2).all(|w| w[0] < w[1]), "sorted");
        assert_eq!(entity(b"rsquo"), Some('’'));
        assert_eq!(entity(b"#x416"), Some('Ж'));
        assert_eq!(entity(b"#150"), Some('–'));
        assert_eq!(entity(b"#"), None);
        assert_eq!(entity(b"#xZZ"), None);
        assert_eq!(entity(b"#xD800"), None);
    }
}
