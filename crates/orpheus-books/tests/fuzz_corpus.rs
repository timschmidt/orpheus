//! The fuzz targets' committed corpus and regression inputs, replayed on
//! stable through the bodies the cargo-fuzz targets run (VER-4); and the
//! seed inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-books --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/books_*/seed-*`).

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "common/epubkit.rs"]
mod kit;
#[path = "../../../tests/replay.rs"]
mod replay;

#[test]
fn books_convert_corpus() {
    let n = replay::replay("books_convert", bodies::books_convert);
    assert!(n >= 8, "{n} inputs");
}

#[test]
fn books_layout_corpus() {
    let n = replay::replay("books_layout", bodies::books_layout);
    assert!(n >= 4, "{n} inputs");
}

#[test]
fn books_zip_corpus() {
    let n = replay::replay("books_zip", bodies::books_zip);
    assert!(n >= 6, "{n} inputs");
}

#[test]
fn books_epub_corpus() {
    let n = replay::replay("books_epub", bodies::books_epub);
    assert!(n >= 8, "{n} inputs");
}

/// BOOK-2 seeds: the sample EPUBs whole, damaged and odd; zips and
/// DEFLATE streams (`ORPHEUS_WRITE_SEEDS=1` rewrites them).
#[test]
fn epub_seeds() {
    use kit::{Part, zip};
    let with = |f: &dyn Fn(&mut Vec<Part>)| {
        let mut p = kit::epub3_parts();
        f(&mut p);
        zip(&p, b"")
    };
    let mut z64 = kit::epub3();
    let n = z64.len();
    z64[n - 12..n - 8].copy_from_slice(&u32::MAX.to_le_bytes());
    let deep = format!(
        "<html><body>{}<p>deep</p>{}<P>unclosed<P>&bogus; &#x41;<div><span>x</div></span><p>end",
        "<div><blockquote><i>".repeat(300),
        "</i></blockquote></div>".repeat(299)
    );
    let no_nav = kit::OPF3.replace(" properties=\"nav\"", "");
    let epub: Vec<(String, Vec<u8>)> = vec![
        ("epub3".into(), kit::epub3()),
        ("epub2".into(), kit::epub2()),
        (
            "epub3-stored".into(),
            with(&|p| {
                p.truncate(6);
                for x in p.iter_mut() {
                    x.deflate = false;
                }
            }),
        ),
        ("epub3-no-container".into(), with(&|p| drop(p.remove(1)))),
        ("epub3-encrypted".into(), with(&|p| p[5].flags = 1)),
        ("epub3-bad-crc".into(), with(&|p| p[6].crc = Some(7))),
        (
            "epub3-truncated-part".into(),
            with(&|p| {
                let z = kit::deflate(&p[5].data, 9);
                p[5].raw = Some(z[..z.len() / 3].to_vec());
            }),
        ),
        (
            "epub3-bomb".into(),
            with(&|p| {
                p[5].raw = Some(kit::deflate(&vec![b'<'; 3 << 20], 9));
                p[5].usize = Some(5000);
            }),
        ),
        ("epub3-no-nav".into(), with(&|p| p[2] = Part::new("OEBPS/content.opf", no_nav.as_bytes(), true))),
        ("epub3-deep".into(), with(&|p| p[5] = Part::new("OEBPS/Text/ch1.xhtml", deep.as_bytes(), true))),
        ("epub3-comment".into(), zip(&kit::epub3_parts(), &[b'#'; 2000])),
        ("zip64".into(), z64),
        ("truncated".into(), kit::epub2()[..2000].to_vec()),
    ];
    let text = kit::ch1().xhtml.into_bytes();
    let piece = |p: u8, z: Vec<u8>| {
        let mut v = vec![p];
        v.extend(z);
        v
    };
    let deflated = kit::deflate(&text, 9);
    let zips: Vec<(String, Vec<u8>)> = vec![
        ("epub2".into(), piece(7, kit::epub2())),
        ("deflate9".into(), piece(0, deflated.clone())),
        ("deflate1".into(), piece(31, kit::deflate(&text, 1))),
        ("stored".into(), piece(100, kit::deflate(&text[..3000], 0))),
        ("truncated".into(), piece(255, deflated[..deflated.len() / 2].to_vec())),
        ("garbage".into(), piece(3, (0..600u32).map(|i| (i * 167 % 251) as u8).collect())),
        ("far".into(), piece(5, kit::deflate(&[text.clone(), text.clone()].concat(), 9))),
        ("fixed".into(), piece(1, vec![0x4B, 0x4C, 0x4C, 0x04, 0x02, 0x00])),
    ];
    replay::write_seeds("books_epub", &epub);
    replay::write_seeds("books_zip", &zips);
    for (n, v) in &epub {
        eprintln!("seed books_epub/{n}");
        bodies::books_epub(v);
    }
    for (n, v) in &zips {
        eprintln!("seed books_zip/{n}");
        bodies::books_zip(v);
    }
}

const FB2: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook><description><title-info><author><first-name>Lewis</first-name><last-name>Carroll</last-name></author><book-title>Alice</book-title></title-info></description>
<body><title><p>Alice's Adventures</p></title><epigraph><p>All in the golden afternoon</p><text-author>L. C.</text-author></epigraph>
<section><title><p>Chapter I</p><p>Down the Rabbit-Hole</p></title><p>Alice was beginning to get <emphasis>very</emphasis> tired of sitting by her sister on the bank, and of having <strong>nothing</strong> to do&#8230;</p><empty-line/><poem><stanza><v>How doth the little crocodile</v><v>Improve his shining tail</v></stanza></poem></section>
<section><title><p>Chapter II</p></title><p>&laquo;Curiouser and curiouser!&raquo; cried Alice.<a l:href="#n1" type="note">[1]</a></p><p><![CDATA[a <raw> bit]]></p></section></body>
<body name="notes"><title><p>Notes</p></title><section id="n1"><title><p>1</p></title><p>A note.</p></section></body>
<binary id="x" content-type="image/png">iVBORw0KGgo=</binary></FictionBook>"##;

const MD: &str = "---\ntitle: Notes\nauthor: Me\n---\n# Title\n\nSome *emphasis*, **bold**, `code_x`, a [link](http://x/y_(z)) and ![an image](a.png).\n\n> A quote\n> over two lines\n\n- one\n- two\n3. three\n\n---\n## Part\n```\nlet  x = 1;\n```\n#hashtag and snake_case_words\n";

const TXT: &str = "Title: A Tale\r\nAuthor: Someone\r\n\r\nCHAPTER I.\r\n\r\nIt was the best of times,\r\nit was the worst of times.\r\n\r\n   An indented paragraph\r\nwrapped.\r\n\r\n* * *\r\n\r\nXIV.\r\n\r\nThe end.\r\n";

const RU: &str = "Глава 1\nТак говорила в июле 1805 года известная Анна Павловна Шерер, фрейлина и приближённая императрицы Марии Феодоровны, встречая важного и чиновного князя Василия, первого приехавшего на её вечер.\nЧасть вторая\n— Ну, что, князь?\n";

fn cp1251(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            'А'..='я' => (u32::from(c) - 0x410 + 0xC0) as u8,
            'ё' => 0xB8,
            '—' => 0x97,
            c if c.is_ascii() => c as u8,
            _ => b'?',
        })
        .collect()
}

fn koi8(s: &str) -> Vec<u8> {
    // KOI8-R's letters in table order (0xC0–0xFF).
    let table = "юабцдефгхийклмнопярстужвьызшэщчъЮАБЦДЕФГХИЙКЛМНОПЯРСТУЖВЬЫЗШЭЩЧЪ";
    s.chars()
        .map(|c| match table.chars().position(|t| t == c) {
            Some(i) => 0xC0 + i as u8,
            None if c == 'ё' => 0xA3,
            None if c.is_ascii() => c as u8,
            None => b'?',
        })
        .collect()
}

fn seed(format: u8, piece: u8, file: &[u8]) -> Vec<u8> {
    let mut v = vec![format, piece];
    v.extend_from_slice(file);
    v
}

/// The normalised stream of a seed file (for the layout seeds).
fn stream(format: u8, file: &[u8]) -> Vec<u8> {
    use orpheus_books::convert::{Converter, Format};
    struct All(Vec<u8>);
    impl orpheus_books::norm::Sink for All {
        fn put(&mut self, b: &[u8]) {
            self.0.extend_from_slice(b);
        }
    }
    let mut c = Converter::new(Format::from_u8(format), file, true);
    let mut out = All(Vec::new());
    let mut at = 0;
    while at < file.len() {
        at += c.feed(&file[at..], &mut out);
        let _ = c.take_toc();
    }
    c.finish(&mut out);
    out.0
}

#[test]
fn seeds() {
    let mut utf16 = vec![0xFF, 0xFE];
    utf16.extend(TXT.encode_utf16().flat_map(u16::to_le_bytes));
    let mut utf16be: Vec<u8> = Vec::new();
    utf16be.extend(RU.encode_utf16().flat_map(u16::to_be_bytes));
    let fb2_1251 = cp1251(&FB2.replace("UTF-8", "windows-1251").replace("Alice was", "Алиса была"));
    let convert: Vec<(String, Vec<u8>)> = vec![
        ("fb2".into(), seed(2, 63, FB2.as_bytes())),
        ("fb2-1251".into(), seed(2, 0, &fb2_1251)),
        (
            "fb2-broken".into(),
            seed(2, 5, b"<body><section><p>open <emphasis>em &amp &#xZZ; <</p></body></section><![CDATA[x"),
        ),
        ("md".into(), seed(1, 31, MD.as_bytes())),
        ("md-crlf".into(), seed(1, 2, MD.replace('\n', "\r\n").as_bytes())),
        ("txt".into(), seed(0, 255, TXT.as_bytes())),
        ("txt-utf16le-bom".into(), seed(0, 7, &utf16)),
        ("txt-utf16be".into(), seed(0, 9, &utf16be)),
        ("txt-cp1251-lines".into(), seed(0, 100, &cp1251(RU))),
        ("txt-koi8r".into(), seed(0, 3, &koi8(RU))),
        (
            "txt-latin1".into(),
            seed(
                0,
                17,
                b"Le c\xF4t\xE9 de chez Swann\r\n\r\nLongtemps, je me suis couch\xE9 de bonne heure.",
            ),
        ),
        ("txt-bad-utf8".into(), seed(0, 1, b"ok \xC3( \xE2\x82 \xF0\x9F\x98 \xFF\xFE end \xED\xA0\x80")),
        (
            "txt-cjk".into(),
            seed(
                0,
                4,
                "春は、あけぼの。やうやう白くなりゆく山ぎは、すこしあかりて。\n\n夏は、夜。".as_bytes(),
            ),
        ),
    ];
    let mut layout: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, w, l, format, file) in [
        ("fb2", 90u8, 14u8, 2u8, FB2.as_bytes().to_vec()),
        ("md-narrow", 20, 40, 1, MD.as_bytes().to_vec()),
        ("txt-wide-tall", 200, 200, 0, TXT.as_bytes().to_vec()),
        (
            "cjk",
            60,
            70,
            0,
            "春は、あけぼの。やうやう白くなりゆく山ぎは、すこしあかりて。".repeat(4).into_bytes(),
        ),
    ] {
        let mut v = vec![w, l];
        v.extend(stream(format, &file));
        layout.push((name.into(), v));
    }
    layout.push((
        "garbage".into(),
        vec![50, 9, 0x10, 0xFF, 0x08, b'a', 0x1F, 0x13, 0xE2, 0x82, b' ', b' ', b'x'],
    ));
    replay::write_seeds("books_convert", &convert);
    replay::write_seeds("books_layout", &layout);
    for (n, v) in &convert {
        eprintln!("seed books_convert/{n}");
        bodies::books_convert(v);
    }
    for (n, v) in &layout {
        eprintln!("seed books_layout/{n}");
        bodies::books_layout(v);
    }
}
