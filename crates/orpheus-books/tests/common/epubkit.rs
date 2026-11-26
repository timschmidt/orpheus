//! EPUB books built from parts for the tests (BOOK-2): a zip writer
//! (stored or deflated by flate2, entries changed on purpose for the
//! robustness tests) and two sample books of our own text, EPUB 2 (NCX)
//! and EPUB 3 (navigation document), with the paragraphs each must give.

#![allow(dead_code)]

use std::io::Write;

use orpheus_books::zip::crc32;

/// One entry as written.
pub struct Part {
    pub name: String,
    pub data: Vec<u8>,
    pub deflate: bool,
    /// Changes for the robustness tests.
    pub flags: u16,
    pub method: Option<u16>,
    pub crc: Option<u32>,
    pub usize: Option<u32>,
    /// Written compressed bytes instead of compressing `data`.
    pub raw: Option<Vec<u8>>,
}

impl Part {
    pub fn new(name: &str, data: &[u8], deflate: bool) -> Part {
        Part {
            name: name.into(),
            data: data.into(),
            deflate,
            flags: 0,
            method: None,
            crc: None,
            usize: None,
            raw: None,
        }
    }
}

pub fn deflate(data: &[u8], level: u32) -> Vec<u8> {
    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(level));
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// A zip of `parts` in order, with `comment`.
pub fn zip(parts: &[Part], comment: &[u8]) -> Vec<u8> {
    let mut z = Vec::new();
    let mut cd = Vec::new();
    for p in parts {
        let body = match &p.raw {
            Some(r) => r.clone(),
            None if p.deflate => deflate(&p.data, 9),
            None => p.data.clone(),
        };
        let method = p.method.unwrap_or(if p.deflate { 8 } else { 0 });
        let crc = p.crc.unwrap_or_else(|| crc32(0, &p.data));
        let usize = p.usize.unwrap_or(p.data.len() as u32);
        let local = z.len() as u32;
        let mut h = Vec::new();
        h.extend_from_slice(b"PK\x03\x04");
        h.extend_from_slice(&20u16.to_le_bytes());
        h.extend_from_slice(&p.flags.to_le_bytes());
        h.extend_from_slice(&method.to_le_bytes());
        h.extend_from_slice(&[0, 0, 0x21, 0x5B]);
        h.extend_from_slice(&crc.to_le_bytes());
        h.extend_from_slice(&(body.len() as u32).to_le_bytes());
        h.extend_from_slice(&usize.to_le_bytes());
        h.extend_from_slice(&(p.name.len() as u16).to_le_bytes());
        h.extend_from_slice(&4u16.to_le_bytes());
        z.extend_from_slice(&h);
        z.extend_from_slice(p.name.as_bytes());
        z.extend_from_slice(b"\xFE\xCA\0\0");
        z.extend_from_slice(&body);
        cd.extend_from_slice(b"PK\x01\x02");
        cd.extend_from_slice(&[0x14, 0x03]);
        cd.extend_from_slice(&h[4..28]);
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u32.to_le_bytes());
        cd.extend_from_slice(&local.to_le_bytes());
        cd.extend_from_slice(p.name.as_bytes());
    }
    let at = z.len() as u32;
    z.extend_from_slice(&cd);
    z.extend_from_slice(b"PK\x05\x06\0\0\0\0");
    z.extend_from_slice(&(parts.len() as u16).to_le_bytes());
    z.extend_from_slice(&(parts.len() as u16).to_le_bytes());
    z.extend_from_slice(&(cd.len() as u32).to_le_bytes());
    z.extend_from_slice(&at.to_le_bytes());
    z.extend_from_slice(&(comment.len() as u16).to_le_bytes());
    z.extend_from_slice(comment);
    z
}

pub const CONTAINER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

/// A chapter as XHTML and the paragraphs it must give: (kind, text) with
/// spaces collapsed, styles dropped.
pub struct Chapter {
    pub xhtml: String,
    pub paras: Vec<(u8, String)>,
}

const WORDS: [&str; 24] = [
    "lamp", "keeper", "salt", "wind", "harbour", "night", "gull", "rope", "tide", "stone", "light", "ship",
    "fog", "bell", "shore", "watch", "oil", "glass", "storm", "wave", "rock", "star", "north", "sail",
];

/// A paragraph of `n` words (deterministic from `seed`).
pub fn sentence(seed: usize, n: usize) -> String {
    let mut s = String::new();
    for i in 0..n {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(WORDS[(seed * 7 + i * 5 + i * i) % WORDS.len()]);
    }
    let mut c = s.chars();
    let first = c.next().unwrap().to_uppercase().collect::<String>();
    format!("{first}{}.", c.as_str())
}

fn page(head: &str, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n<head><title>{head}</title><link rel=\"stylesheet\" href=\"../style.css\"/><style>h1 {{ text-align: center }}</style></head>\n<body>\n{body}\n</body></html>"
    )
}

/// EPUB 3 chapter 1: a heading, sections with ids, an anchor in a
/// paragraph, emphasis, entities.
pub fn ch1() -> Chapter {
    let mut b = String::from("<section epub:type=\"chapter\"><h1>Chapter 1</h1>\n");
    let mut paras = vec![(1, "Chapter 1".to_string())];
    for i in 0..6 {
        let s = sentence(i, 30);
        b.push_str(&format!("<p>{}</p>\n", s.replacen(' ', " <em>", 1).replacen(' ', "</em> ", 1)));
        paras.push((0, s));
    }
    b.push_str("<h2 id=\"lamp\">The Lamp</h2>\n");
    paras.push((2, "The Lamp".into()));
    for i in 6..10 {
        let s = sentence(i, 40);
        b.push_str(&format!("<p>{s}</p>\n"));
        paras.push((0, s));
    }
    b.push_str(
        "<p>Before the mark, &ldquo;quoted&rdquo; text<a id=\"mid\"/> and after the mark&#8230;</p>\n",
    );
    paras.push((0, "Before the mark, “quoted” text and after the mark…".into()));
    b.push_str("<h2 id=\"storm\">The Storm</h2>\n");
    paras.push((2, "The Storm".into()));
    for i in 10..14 {
        let s = sentence(i, 25);
        b.push_str(&format!("<p>{s}</p>\n"));
        paras.push((0, s));
    }
    b.push_str("<p><img src=\"../Images/lamp.png\" alt=\"A lamp\"/></p></section>");
    Chapter { xhtml: page("Chapter 1", &b), paras }
}

/// EPUB 3 chapter 2: long (over 32 KiB, so the window wraps), lists, a
/// quote, a poem with line breaks, a scene break.
pub fn ch2() -> Chapter {
    let mut b = String::from("<h1 id=\"c2\">Chapter 2</h1>\n");
    let mut paras = vec![(1, "Chapter 2".to_string())];
    for i in 0..220 {
        let s = sentence(100 + i, 20 + i % 13);
        b.push_str(&format!("<p class=\"body\">{s}</p>\n"));
        paras.push((0, s));
        if i == 50 {
            b.push_str("<ul><li>First item</li><li>Second <b>item</b></li></ul>\n");
            paras.push((8, "• First item".into()));
            paras.push((8, "• Second item".into()));
            b.push_str("<blockquote><p>A quoted line.</p></blockquote>\n");
            paras.push((5, "A quoted line.".into()));
        }
        if i == 120 {
            b.push_str("<p class=\"verse\">Line one,<br/>line two,<br/>line three.</p>\n<hr/>\n");
            paras.push((0, "Line one,".into()));
            paras.push((0, "line two,".into()));
            paras.push((0, "line three.".into()));
            paras.push((10, String::new()));
        }
    }
    Chapter { xhtml: page("Chapter 2", &b), paras }
}

/// Windows-1251 bytes of `s` (Russian letters and ASCII).
pub fn cp1251(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            'А'..='я' => (u32::from(c) - 0x410 + 0xC0) as u8,
            'Ё' => 0xA8,
            'ё' => 0xB8,
            c if c.is_ascii() => c as u8,
            _ => b'?',
        })
        .collect()
}

/// EPUB 3 chapter 3: mostly English, one Russian heading and line, in
/// Windows-1251 named only by `<meta http-equiv>` (the head alone reads
/// as Latin-1).
pub fn ch3() -> (Vec<u8>, Vec<(u8, String)>) {
    let mut body = String::from("<h1>Глава 3</h1>\n");
    let mut paras = vec![(1, "Глава 3".to_string())];
    for i in 0..8 {
        let s = sentence(400 + i, 30);
        body.push_str(&format!("<p>{s}</p>\n"));
        paras.push((0, s));
    }
    body.push_str("<p>Так говорила Анна Павловна.</p>");
    paras.push((0, "Так говорила Анна Павловна.".into()));
    let x = format!(
        "<?xml version=\"1.0\"?>\n<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1251\"/><title>3</title></head><body>{body}</body></html>"
    );
    (cp1251(&x), paras)
}

pub fn notes() -> Chapter {
    let b = "<aside id=\"n1\" epub:type=\"footnote\"><p>A note at the end.</p></aside>";
    Chapter { xhtml: page("Notes", b), paras: vec![(0, "A note at the end.".into())] }
}

pub const OPF3: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="uid">urn:uuid:0000</dc:identifier>
    <dc:title>The Lighthouse Keeper</dc:title>
    <dc:creator>Orpheus Tests</dc:creator>
    <dc:language>en</dc:language>
    <meta property="dcterms:modified">2026-11-25T00:00:00Z</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="css" href="style.css" media-type="text/css"/>
    <item id="ch1" href="Text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch2" href="Text/chapter%20two.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch3" href="Text/ch3.xhtml" media-type="application/xhtml+xml"/>
    <item id="notes" href="Text/notes.xhtml" media-type="application/xhtml+xml"/>
    <item id="img" href="Images/lamp.png" media-type="image/png"/>
  </manifest>
  <spine>
    <itemref idref="ch1"/>
    <itemref idref="ch2"/>
    <itemref idref="ch3"/>
    <itemref idref="notes" linear="no"/>
  </spine>
</package>"#;

pub const NAV3: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Contents</title></head>
<body>
<nav epub:type="toc" id="toc"><h1>Contents</h1>
<ol>
  <li><a href="Text/ch1.xhtml">Chapter 1</a>
    <ol>
      <li><a href="Text/ch1.xhtml#lamp">The Lamp</a></li>
      <li><a href="Text/ch1.xhtml#mid">Before the mark</a></li>
      <li><a href="Text/ch1.xhtml#storm">The Storm</a></li>
    </ol>
  </li>
  <li><a href="Text/chapter%20two.xhtml#c2">Chapter 2</a></li>
  <li><a href="Text/ch3.xhtml">Глава 3</a></li>
  <li><a href="Text/notes.xhtml#n1">A note at the end</a></li>
</ol>
</nav>
<nav epub:type="landmarks" hidden=""><ol><li><a epub:type="bodymatter" href="Text/ch1.xhtml">Start</a></li></ol></nav>
</body></html>"#;

/// The EPUB 3 sample's parts, in archive order.
pub fn epub3_parts() -> Vec<Part> {
    let (c3, _) = ch3();
    vec![
        Part::new("mimetype", b"application/epub+zip", false),
        Part::new("META-INF/container.xml", CONTAINER.as_bytes(), true),
        Part::new("OEBPS/content.opf", OPF3.as_bytes(), true),
        Part::new("OEBPS/nav.xhtml", NAV3.as_bytes(), true),
        Part::new("OEBPS/style.css", b"p { text-indent: 1em }", true),
        Part::new("OEBPS/Text/ch1.xhtml", ch1().xhtml.as_bytes(), true),
        Part::new("OEBPS/Text/chapter two.xhtml", ch2().xhtml.as_bytes(), true),
        Part::new("OEBPS/Text/ch3.xhtml", &c3, false),
        Part::new("OEBPS/Text/notes.xhtml", notes().xhtml.as_bytes(), true),
        Part::new("OEBPS/Images/lamp.png", b"\x89PNG\r\n\x1a\n not really", false),
    ]
}

pub fn epub3() -> Vec<u8> {
    zip(&epub3_parts(), b"")
}

/// Every paragraph the EPUB 3 sample gives, in order.
pub fn epub3_paras() -> Vec<(u8, String)> {
    let mut v = ch1().paras;
    v.extend(ch2().paras);
    v.extend(ch3().1);
    v.extend(notes().paras);
    v
}

pub const OPF2: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="BookId">
 <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
  <dc:title>Winter Harbour</dc:title>
  <dc:creator opf:role="aut" opf:file-as="Tests, Orpheus">Orpheus Tests</dc:creator>
  <dc:language>en-GB</dc:language>
  <dc:identifier id="BookId">urn:x:2</dc:identifier>
 </metadata>
 <manifest>
  <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
  <item id="t1" href="text/one.html" media-type="application/xhtml+xml"/>
  <item id="t2" href="text/two.html" media-type="application/xhtml+xml"/>
 </manifest>
 <spine toc="ncx"><itemref idref="t1"/><itemref idref="t2"/></spine>
 <guide><reference type="text" href="text/one.html"/></guide>
</package>"#;

pub const NCX2: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE ncx PUBLIC "-//NISO//DTD ncx 2005-1//EN" "http://www.daisy.org/z3986/2005/ncx-2005-1.dtd">
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
<head><meta name="dtb:uid" content="urn:x:2"/></head>
<docTitle><text>Winter Harbour</text></docTitle>
<navMap>
<navPoint id="p1" playOrder="1"><navLabel><text>I. The Ice</text></navLabel><content src="text/one.html"/>
  <navPoint id="p2" playOrder="2"><navLabel><text>The Thaw</text></navLabel><content src="text/one.html#thaw"/></navPoint>
</navPoint>
<navPoint id="p3" playOrder="3"><navLabel><text>II. The Boats</text></navLabel><content src="text/two.html"/></navPoint>
</navMap>
</ncx>"#;

/// The EPUB 2 sample's two chapters (XHTML 1.1, `<div>` wrappers,
/// named entities) and their paragraphs.
pub fn epub2_chapters() -> Vec<Chapter> {
    let mut v = Vec::new();
    for (k, title) in ["I. The Ice", "II. The Boats"].iter().enumerate() {
        let mut b = format!("<div class=\"chapter\"><h2>{title}</h2>\n");
        let mut paras = vec![(2, title.to_string())];
        for i in 0..30 {
            if k == 0 && i == 15 {
                b.push_str("<h3 id=\"thaw\">The Thaw</h3>\n");
                paras.push((3, "The Thaw".into()));
            }
            let s = sentence(700 + k * 50 + i, 25);
            b.push_str(&format!("<p>{}&nbsp;&mdash;&nbsp;{i}</p>\n", s));
            paras.push((0, format!("{s}\u{a0}—\u{a0}{i}")));
        }
        b.push_str("</div>");
        let x = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.1//EN\" \"http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd\">\n<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>{title}</title></head><body>{b}</body></html>"
        );
        v.push(Chapter { xhtml: x, paras });
    }
    v
}

pub fn epub2() -> Vec<u8> {
    let ch = epub2_chapters();
    zip(
        &[
            Part::new("mimetype", b"application/epub+zip", false),
            Part::new(
                "META-INF/container.xml",
                CONTAINER.replace("OEBPS/content.opf", "content.opf").as_bytes(),
                true,
            ),
            Part::new("content.opf", OPF2.as_bytes(), true),
            Part::new("toc.ncx", NCX2.as_bytes(), true),
            Part::new("text/one.html", ch[0].xhtml.as_bytes(), true),
            Part::new("text/two.html", ch[1].xhtml.as_bytes(), true),
        ],
        b"",
    )
}

pub fn epub2_paras() -> Vec<(u8, String)> {
    epub2_chapters().into_iter().flat_map(|c| c.paras).collect()
}

/// The stream's paragraphs: (kind, text without style bytes).
pub fn paragraphs(s: &[u8]) -> Vec<(u8, String)> {
    use orpheus_books::norm::{Item, item_at};
    let mut v: Vec<(u8, String)> = Vec::new();
    let mut i = 0;
    while let Some((it, n)) = item_at(s, i) {
        match it {
            Item::Para(k) => v.push((k as u8, String::new())),
            Item::Char(c) => v.last_mut().expect("text in a paragraph").1.push(c),
            Item::Style(_) => {}
        }
        i += n;
    }
    v
}
