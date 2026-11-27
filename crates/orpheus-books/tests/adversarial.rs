//! Adversarial books (VER-3, 07 §3.7), beyond `epub.rs` (deflate bombs,
//! refused archives, deep markup) and the fuzz bodies: the EPUB samples
//! cut at every byte of their head and tail and at spread points; every
//! zip header field (local, central, end record) at its edge values;
//! hostile entry names and package paths; markup soup from tokens
//! (proptest) through the EPUB job and through the TXT/Markdown/FB2
//! converter. The oracles are the bodies' (`fuzz/src/lib.rs`): no panic,
//! no stall, bounded work per step, a well-formed text stream, contents
//! inside the text.

#[path = "../../../tests/adversarial.rs"]
mod adversarial;
#[allow(dead_code)]
#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "common/epubkit.rs"]
mod kit;

use adversarial::{config, edges, hostile_names, put_le};
use bodies::{check_stream, check_toc, convert_epub};
use kit::{Part, zip};
use orpheus_books::inflate::SCRATCH;
use proptest::prelude::*;

/// The EPUB job's oracles on any bytes.
fn epub(b: &[u8]) {
    if let Ok(c) = convert_epub(b, SCRATCH) {
        check_stream(&c.text);
        check_toc(&c.text, &c.toc);
    }
}

#[test]
fn samples_cut_everywhere() {
    for book in [kit::epub3(), kit::epub2()] {
        let n = book.len();
        let mut cuts: Vec<usize> = (0..n.min(400)).collect();
        cuts.extend(n.saturating_sub(400)..n);
        cuts.extend((0..n).step_by((n / 150).max(1)));
        for cut in cuts {
            epub(&book[..cut]);
        }
    }
}

/// Offsets of every local header, central entry and the end record.
fn records(z: &[u8]) -> (Vec<usize>, Vec<usize>, usize) {
    let find = |sig: &[u8; 4]| -> Vec<usize> {
        z.windows(4).enumerate().filter(|(_, w)| w == sig).map(|(i, _)| i).collect()
    };
    let eocd = *find(b"PK\x05\x06").last().unwrap();
    (find(b"PK\x03\x04"), find(b"PK\x01\x02"), eocd)
}

fn rd(z: &[u8], o: usize, n: usize) -> u64 {
    z[o..o + n].iter().rev().fold(0, |a, &b| a << 8 | u64::from(b))
}

#[test]
fn zip_fields_at_their_edges() {
    let book = kit::epub3();
    let (locals, centrals, eocd) = records(&book);
    let mut variants = 0;
    let mut each = |o: usize, n: usize| {
        for v in edges(8 * n as u32, rd(&book, o, n)).into_iter().chain([book.len() as u64, o as u64]) {
            if n < 8 && v >> (8 * n) != 0 {
                continue;
            }
            let mut b = book.clone();
            put_le(&mut b, o, n, v);
            epub(&b);
            variants += 1;
        }
    };
    // Local headers: flags, method, CRC, sizes, name and extra lengths.
    for &l in locals.iter().take(4) {
        for (f, n) in [(6, 2), (8, 2), (14, 4), (18, 4), (22, 4), (26, 2), (28, 2)] {
            each(l + f, n);
        }
    }
    // Central entries: method, CRC, sizes, name/extra/comment lengths,
    // disk, attributes, local offset.
    for &c in &centrals {
        for (f, n) in [(10, 2), (16, 4), (20, 4), (24, 4), (28, 2), (30, 2), (32, 2), (34, 2), (42, 4)] {
            each(c + f, n);
        }
    }
    // End record: disks, entry counts, directory size and offset, comment.
    for (f, n) in [(4, 2), (6, 2), (8, 2), (10, 2), (12, 4), (16, 4), (20, 2)] {
        each(eocd + f, n);
    }
    assert!(variants > 300);
}

#[test]
fn central_directory_lies() {
    let parts = kit::epub3_parts();
    let book = zip(&parts, b"");
    let (_, centrals, eocd) = records(&book);
    // Every central entry pointing at the first local header, at the end
    // record, at itself; the directory offset at every entry.
    for target in [0u64, eocd as u64, centrals[0] as u64, 1, book.len() as u64 - 1] {
        let mut b = book.clone();
        for &c in &centrals {
            put_le(&mut b, c + 42, 4, target);
        }
        epub(&b);
    }
    for &c in &centrals {
        let mut b = book.clone();
        put_le(&mut b, eocd + 16, 4, c as u64);
        epub(&b);
    }
    // The same entry many times (the directory repeated): a duplicate
    // package, duplicate chapters.
    let mut twice = kit::epub3_parts();
    twice.extend(kit::epub3_parts());
    epub(&zip(&twice, b""));
    // Thousands of empty entries.
    let mut many = kit::epub3_parts();
    for i in 0..3000 {
        many.push(Part::new(&format!("x/{i}"), b"", false));
    }
    epub(&zip(&many, b""));
}

#[test]
fn hostile_entry_names_and_package_paths() {
    let mut names = hostile_names();
    names.extend(
        [
            "../../../etc/passwd",
            "/OEBPS/content.opf",
            "OEBPS/../OEBPS/content.opf",
            "OEBPS//content.opf",
            "OEBPS\\content.opf",
            "%2e%2e/x",
            "OEBPS/./Text/ch1.xhtml",
            "META-INF/container.xml/",
        ]
        .map(String::from),
    );
    for n in &names {
        // A chapter under the hostile name, referenced by it.
        let mut parts = kit::epub3_parts();
        parts.push(Part::new(&format!("OEBPS/{n}"), b"<html><body><p>Hostile</p></body></html>", true));
        let href: String = n.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
        let opf = kit::OPF3
            .replace(
                "</manifest>",
                &format!("<item id=\"h\" href=\"{href}\" media-type=\"application/xhtml+xml\"/></manifest>"),
            )
            .replace("</spine>", "<itemref idref=\"h\"/></spine>");
        parts[2] = Part::new("OEBPS/content.opf", opf.as_bytes(), true);
        epub(&zip(&parts, b""));
        // The container naming it as the package.
        let mut parts = kit::epub3_parts();
        parts[1] = Part::new(
            "META-INF/container.xml",
            kit::CONTAINER.replace("OEBPS/content.opf", &href).as_bytes(),
            true,
        );
        epub(&zip(&parts, b""));
        // The package itself under the name.
        let mut parts = kit::epub3_parts();
        parts[2].name = n.clone();
        epub(&zip(&parts, b""));
    }
}

/// Markup tokens a hostile or broken book is made of.
fn token() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("<p>".to_string()),
        Just("</p>".to_string()),
        Just("<div>".to_string()),
        Just("</div>".to_string()),
        Just("<h1>".to_string()),
        Just("</h2>".to_string()),
        Just("<br/>".to_string()),
        Just("<b><i><u>".to_string()),
        Just("</i></b>".to_string()),
        Just("<a href=\"#x\" id=\"x\">".to_string()),
        Just("</a>".to_string()),
        Just("<!--".to_string()),
        Just("-->".to_string()),
        Just("<![CDATA[".to_string()),
        Just("]]>".to_string()),
        Just("<?xml version=\"1.0\" encoding=\"KOI8-R\"?>".to_string()),
        Just("<!DOCTYPE html [<!ENTITY a \"&a;&a;\">]>".to_string()),
        Just("&amp;&lt;&#x10FFFF;&#0;&#xD800;&#99999999999;&nbsp;&bogus;&".to_string()),
        Just("<script>if (a < b) {}</script>".to_string()),
        Just("<style>p{}</style>".to_string()),
        Just("<img src=\"../../x.png\"/>".to_string()),
        Just("<table><tr><td>".to_string()),
        Just("<ul><li>".to_string()),
        Just("<".to_string()),
        Just(">".to_string()),
        Just("\"".to_string()),
        Just("=".to_string()),
        Just(" ".repeat(70)),
        Just("\u{0}\u{1}\u{7}\u{1b}".to_string()),
        Just("\u{FEFF}\u{202E}\u{200B}".to_string()),
        "[a-zA-Z ]{1,40}",
        "\\PC{1,8}",
    ]
}

/// The FB2 and Markdown flavour of hostile tokens.
fn plain_token() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("# ".to_string()),
        Just("\n\n".to_string()),
        Just("\n".to_string()),
        Just("**".to_string()),
        Just("_".to_string()),
        Just("`".to_string()),
        Just("> ".to_string()),
        Just("- ".to_string()),
        Just("<section><title><p>".to_string()),
        Just("</p></title></section>".to_string()),
        Just("<emphasis>".to_string()),
        Just("<FictionBook>".to_string()),
        Just("<binary content-type=\"image/png\">AAAA</binary>".to_string()),
        Just("\r\n".to_string()),
        Just("\t".to_string()),
        Just("\u{FEFF}".to_string()),
        "[a-z ]{1,30}",
        "\\PC{1,6}",
    ]
}

proptest! {
    #![proptest_config(config("books-markup-soup", 96))]

    /// A chapter of markup soup in a real EPUB, stored or deflated.
    #[test]
    fn epub_markup_soup(tokens in prop::collection::vec(token(), 0..200), deflate in any::<bool>(), depth in 0usize..2000) {
        let mut x: String = "<div>".repeat(depth);
        x.push_str(&tokens.concat());
        let mut parts = kit::epub3_parts();
        parts[5] = Part::new("OEBPS/Text/ch1.xhtml", x.as_bytes(), deflate);
        epub(&zip(&parts, b""));
    }

    /// The package document as soup.
    #[test]
    fn opf_soup(tokens in prop::collection::vec(prop_oneof![
        token(),
        Just("<item id=\"a\" href=\"Text/ch1.xhtml\" media-type=\"application/xhtml+xml\"/>".to_string()),
        Just("<itemref idref=\"a\"/>".to_string()),
        Just("<spine>".to_string()),
        Just("</spine>".to_string()),
        Just("<manifest>".to_string()),
        Just("<dc:title>".to_string()),
        Just("</dc:title>".to_string()),
    ], 0..120)) {
        let mut parts = kit::epub3_parts();
        let opf = format!("<?xml version=\"1.0\"?><package>{}</package>", tokens.concat());
        parts[2] = Part::new("OEBPS/content.opf", opf.as_bytes(), true);
        epub(&zip(&parts, b""));
    }

    /// TXT, Markdown and FB2 soup through the converter body.
    #[test]
    fn plain_soup(format in 0u8..3, piece in any::<u8>(), tokens in prop::collection::vec(prop_oneof![plain_token(), token()], 0..300)) {
        let mut d = vec![format, piece];
        d.extend_from_slice(tokens.concat().as_bytes());
        bodies::books_convert(&d);
    }
}
