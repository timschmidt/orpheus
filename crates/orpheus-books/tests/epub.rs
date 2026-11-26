//! EPUB books through the whole job (BOOK-2) over an in-memory card
//! (`fuzz/src/lib.rs`: `convert_epub`, which also checks that every step
//! reads at most 512 bytes of the book once and writes a bounded number
//! of records): EPUB 3 and EPUB 2 samples built from parts
//! (`tests/common/epubkit.rs`), every paragraph and nothing else, the
//! contents and their positions, the library's metadata, the scratch's
//! rules, and damaged, hostile and odd archives.

#[allow(dead_code)]
#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "common/epubkit.rs"]
mod kit;

use bodies::{Converted, MemStore, block_on, check_stream, check_toc, convert_epub};
use kit::{Part, paragraphs, zip};
use orpheus_books::epub::{Epub, Fail, Step};
use orpheus_books::inflate::{SCRATCH, TABLES};
use orpheus_books::norm::Sink;

fn conv(book: &[u8]) -> Converted {
    let c = convert_epub(book, SCRATCH).expect("converted");
    check_stream(&c.text);
    check_toc(&c.text, &c.toc);
    c
}

/// Contents as (level, title, the first paragraph's text at its offset).
fn contents(c: &Converted) -> Vec<(u8, String, String)> {
    c.toc
        .iter()
        .map(|e| {
            let at = paragraphs(&c.text[e.offset as usize..]);
            (e.level, e.title().to_string(), at.first().map(|p| p.1.clone()).unwrap_or_default())
        })
        .collect()
}

#[test]
fn epub3_text_contents_and_metadata() {
    let book = kit::epub3();
    let c = conv(&book);
    // Every paragraph, in order, nothing else: no text lost.
    assert_eq!(paragraphs(&c.text), kit::epub3_paras());
    assert_eq!(c.damaged, 0);
    assert_eq!(c.meta.title.as_str(), "The Lighthouse Keeper");
    assert_eq!(c.meta.author.as_str(), "Orpheus Tests");
    assert_eq!(c.meta.language.as_str(), "en");
    // The navigation document's entries, each at its heading (or, for the
    // anchor inside a paragraph, at that paragraph).
    let t = contents(&c);
    let want = [
        (1, "Chapter 1", "Chapter 1"),
        (2, "The Lamp", "The Lamp"),
        (2, "Before the mark", "Before the mark, “quoted” text and after the mark…"),
        (2, "The Storm", "The Storm"),
        (1, "Chapter 2", "Chapter 2"),
        (1, "Глава 3", "Глава 3"),
        (1, "A note at the end", "A note at the end."),
    ];
    assert_eq!(t.len(), want.len(), "{t:?}");
    for ((l, title, at), (wl, wt, wat)) in t.iter().zip(want) {
        assert_eq!((*l, title.as_str(), at.as_str()), (wl, wt, wat));
    }
    assert!(c.toc.windows(2).all(|w| w[0].offset < w[1].offset), "in reading order");
    // Bounded steps: at most one 512-byte read of the book; the sink never
    // gets more than its 1 KiB.
    assert_eq!(c.max_reads, 1);
    assert!(c.max_text <= 1024);
    eprintln!("EPUB 3 sample: {} bytes, {} steps, {} bytes of text", book.len(), c.steps, c.text.len());
}

#[test]
fn epub2_ncx_contents() {
    let c = conv(&kit::epub2());
    assert_eq!(paragraphs(&c.text), kit::epub2_paras());
    assert_eq!(c.meta.title.as_str(), "Winter Harbour");
    assert_eq!(c.meta.language.as_str(), "en-GB");
    assert_eq!(
        contents(&c),
        [
            (1, "I. The Ice".to_string(), "I. The Ice".to_string()),
            (2, "The Thaw".to_string(), "The Thaw".to_string()),
            (1, "II. The Boats".to_string(), "II. The Boats".to_string()),
        ]
    );
}

/// Without a navigation document or an NCX, the headings are the
/// contents (as for the other formats).
#[test]
fn headings_without_navigation() {
    let mut parts = kit::epub3_parts();
    parts[2] = Part::new("OEBPS/content.opf", kit::OPF3.replace(" properties=\"nav\"", "").as_bytes(), true);
    let c = conv(&zip(&parts, b""));
    let t: Vec<_> = contents(&c).into_iter().map(|(l, t, _)| (l, t)).collect();
    assert_eq!(
        t,
        [
            (1, "Chapter 1".to_string()),
            (2, "The Lamp".to_string()),
            (2, "The Storm".to_string()),
            (1, "Chapter 2".to_string()),
            (1, "Глава 3".to_string())
        ]
    );
    assert_eq!(paragraphs(&c.text), kit::epub3_paras());
}

/// Contents listed out of reading order still land on their chapters.
#[test]
fn contents_out_of_order() {
    let mut parts = kit::epub3_parts();
    let nav = kit::NAV3.replace("<li><a href=\"Text/ch3.xhtml\">Глава 3</a></li>", "");
    let nav = nav.replace(
        "<ol>\n  <li><a href=\"Text/ch1.xhtml\">",
        "<ol>\n  <li><a href=\"Text/ch3.xhtml\">Глава 3</a></li><li><a href=\"Text/ch1.xhtml\">",
    );
    parts[3] = Part::new("OEBPS/nav.xhtml", nav.as_bytes(), true);
    let c = conv(&zip(&parts, b""));
    let t = contents(&c);
    assert_eq!((t[0].1.as_str(), t[0].2.as_str()), ("Глава 3", "Глава 3"));
    assert_eq!((t[1].1.as_str(), t[1].2.as_str()), ("Chapter 1", "Chapter 1"));
}

/// The library reads the title, author and language only: the end
/// record, the directory, the container and the package's head.
#[test]
fn library_metadata() {
    for (book, title, author) in [
        (kit::epub3(), "The Lighthouse Keeper", "Orpheus Tests"),
        (kit::epub2(), "Winter Harbour", "Orpheus Tests"),
    ] {
        let mut st = MemStore { book: book.clone(), ..MemStore::default() };
        let mut mem = vec![0u8; SCRATCH];
        let mut e = Epub::library(book.len() as u32);
        let mut sink: Vec<u8> = Vec::new();
        let mut steps = 0;
        loop {
            let r = block_on(e.step(&mut st, Some(&mut mem), &mut Out(&mut sink)));
            steps += 1;
            if r == Step::Done {
                break;
            }
            assert_eq!(r, Step::Busy);
            assert!(steps < 50);
        }
        assert_eq!(e.metadata().title.as_str(), title);
        assert_eq!(e.metadata().author.as_str(), author);
        assert!(sink.is_empty() && st.puts == 0, "nothing written");
        assert!(steps <= 12, "{steps} steps");
    }
}

struct Out<'a>(&'a mut Vec<u8>);
impl Sink for Out<'_> {
    fn put(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    fn room(&self) -> usize {
        1024usize.saturating_sub(self.0.len())
    }
}

/// Stored parts need no scratch; a deflated one waits for it; a scratch
/// taken away in the middle of an entry gives the conversion up (the
/// caller starts again); a scratch under 32 KiB is not enough.
#[test]
fn the_scratch() {
    let stored: Vec<Part> = kit::epub3_parts()
        .into_iter()
        .map(|mut p| {
            p.deflate = false;
            p
        })
        .collect();
    let c = convert_epub(&zip(&stored, b""), 0).expect("stored: no scratch needed");
    assert_eq!(paragraphs(&c.text), kit::epub3_paras());

    let book = kit::epub3();
    let mut st = MemStore { book: book.clone(), ..MemStore::default() };
    let mut sink = Vec::new();
    let mut e = Epub::new(book.len() as u32);
    let mut small = vec![0u8; TABLES + 16 * 1024];
    let mut waited = false;
    for _ in 0..100 {
        match block_on(e.step(&mut st, Some(&mut small), &mut Out(&mut sink))) {
            Step::NeedScratch => {
                waited = true;
                break;
            }
            r => assert_eq!(r, Step::Busy),
        }
    }
    assert!(waited && e.needs_scratch(), "a deflated entry waits for a full scratch");
    let mut mem = vec![0u8; SCRATCH];
    assert_eq!(block_on(e.step(&mut st, Some(&mut mem), &mut Out(&mut sink))), Step::Busy);
    assert_eq!(block_on(e.step(&mut st, None, &mut Out(&mut sink))), Step::Lost);
}

fn damaged_parts(f: impl Fn(&mut Part)) -> Converted {
    let mut parts = kit::epub3_parts();
    f(&mut parts[5]);
    conv(&zip(&parts, b""))
}

/// Parts that cannot be read become a note; the rest of the book is
/// read.
#[test]
fn unreadable_parts() {
    let ch1 = kit::ch1().paras.len();
    for (why, change) in [
        ("[encrypted]", Box::new(|p: &mut Part| p.flags = 1) as Box<dyn Fn(&mut Part)>),
        ("[unsupported]", Box::new(|p: &mut Part| p.method = Some(12))),
        ("[too large]", Box::new(|p: &mut Part| p.usize = Some(17 << 20))),
    ] {
        let c = damaged_parts(change);
        let p = paragraphs(&c.text);
        assert_eq!(p[0], (0, why.to_string()), "{why}");
        assert_eq!(&p[1..], &kit::epub3_paras()[ch1..], "{why}: the rest");
        assert_eq!(c.damaged, 1);
        // Its contents entries point at its start (the note).
        assert_eq!(c.toc[0].offset, 0);
    }
    // A wrong CRC: the text, then the note.
    let c = damaged_parts(|p| p.crc = Some(1));
    let p = paragraphs(&c.text);
    assert_eq!(&p[..ch1], &kit::epub3_paras()[..ch1]);
    assert_eq!(p[ch1], (0, "[damaged]".to_string()));
    // A truncated stream: some text, the note, the rest of the book.
    let c = damaged_parts(|p| {
        let z = kit::deflate(&p.data, 9);
        p.raw = Some(z[..z.len() / 2].to_vec());
    });
    let p = paragraphs(&c.text);
    let at = p.iter().position(|x| x.1 == "[damaged]").expect("noted");
    assert!(at > 2 && at < ch1, "{at}");
    assert_eq!(&p[at + 1..], &kit::epub3_paras()[ch1..]);
    // Garbage in place of DEFLATE.
    let c = damaged_parts(|p| p.raw = Some(vec![0xFF; 300]));
    assert!(paragraphs(&c.text).contains(&(0, "[damaged]".to_string())));
}

/// A deflate bomb stops at the size the entry declares (a few steps), or
/// is not read at all when it declares more than a part may hold.
#[test]
fn deflate_bombs() {
    let zeros = vec![b'a'; 24 << 20];
    let bomb = kit::deflate(&zeros, 9);
    eprintln!("bomb: {} bytes inflate to {}", bomb.len(), zeros.len());
    for (declared, steps_max) in [(1000u32, 2_000), (24 << 20, 2_000)] {
        let c = damaged_parts(|p| {
            p.raw = Some(bomb.clone());
            p.usize = Some(declared);
        });
        assert!(c.steps < steps_max, "{} steps", c.steps);
        assert_eq!(c.damaged, 1);
        assert!(c.text.len() < 100_000 + 2_000, "{} bytes of text", c.text.len());
    }
}

/// Archives that are not EPUBs, or not ones we read, are refused.
#[test]
fn refused_archives() {
    assert!(matches!(convert_epub(b"", SCRATCH), Err(Fail::NotZip)));
    assert!(matches!(convert_epub(b"Just some text, not an archive.", SCRATCH), Err(Fail::NotZip)));
    let mut z64 = kit::epub3();
    let n = z64.len();
    z64[n - 12..n - 8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(convert_epub(&z64, SCRATCH), Err(Fail::Zip64)));
    let mut multi = kit::epub3();
    multi[n - 18] = 1;
    assert!(matches!(convert_epub(&multi, SCRATCH), Err(Fail::MultiDisk)));
    // No package document at all.
    let parts: Vec<Part> = kit::epub3_parts().into_iter().filter(|p| !p.name.ends_with(".opf")).collect();
    assert!(matches!(convert_epub(&zip(&parts, b""), SCRATCH), Err(Fail::NoPackage)));
    // A package without a spine.
    let mut parts = kit::epub3_parts();
    let opf = kit::OPF3.split("<spine>").next().unwrap().to_string() + "</package>";
    parts[2] = Part::new("OEBPS/content.opf", opf.as_bytes(), true);
    assert!(matches!(convert_epub(&zip(&parts, b""), SCRATCH), Err(Fail::Empty)));
    // A truncated archive: no end record.
    let b = kit::epub3();
    assert!(matches!(convert_epub(&b[..b.len() - 30], SCRATCH), Err(Fail::NotZip)));
}

/// Missing or misleading pieces are worked around: no container (the
/// archive's package document is used), a container naming a missing
/// package, a long archive comment, a spine item missing from the
/// archive or the manifest, an image in the spine.
#[test]
fn missing_and_odd_pieces() {
    let all = kit::epub3_paras();
    let parts: Vec<Part> =
        kit::epub3_parts().into_iter().filter(|p| !p.name.starts_with("META-INF")).collect();
    assert_eq!(paragraphs(&conv(&zip(&parts, b"")).text), all, "no container");
    let mut parts = kit::epub3_parts();
    parts[1] = Part::new(
        "META-INF/container.xml",
        kit::CONTAINER.replace("content.opf", "gone.opf").as_bytes(),
        true,
    );
    assert_eq!(paragraphs(&conv(&zip(&parts, b"")).text), all, "a container naming a missing package");
    assert_eq!(paragraphs(&conv(&zip(&kit::epub3_parts(), &[b'#'; 60_000])).text), all, "a long comment");
    // ch2 missing from the archive, an unknown idref, an image item read
    // as a spine item.
    let mut parts = kit::epub3_parts();
    parts.remove(6);
    let opf = kit::OPF3.replace(
        "<itemref idref=\"ch3\"/>",
        "<itemref idref=\"nothing\"/><itemref idref=\"img\"/><itemref idref=\"ch3\"/>",
    );
    parts[2] = Part::new("OEBPS/content.opf", opf.as_bytes(), true);
    let c = conv(&zip(&parts, b""));
    let mut want = kit::ch1().paras;
    want.extend(kit::ch3().1);
    want.extend(kit::notes().paras);
    assert_eq!(paragraphs(&c.text), want);
    // Chapter 2's entry has no text to point at: the next one's place.
    let t = contents(&c);
    assert!(t.iter().any(|(_, title, _)| title == "Chapter 2"));
}

/// Encodings: the XML declaration (KOI8-R), a byte-order mark (UTF-16),
/// and `<meta http-equiv>` over a head that reads as Latin-1 (chapter 3
/// of the sample).
#[test]
fn encodings() {
    let koi8 = |s: &str| -> Vec<u8> {
        let table = "юабцдефгхийклмнопярстужвьызшэщчъЮАБЦДЕФГХИЙКЛМНОПЯРСТУЖВЬЫЗШЭЩЧЪ";
        s.chars()
            .map(|c| match table.chars().position(|t| t == c) {
                Some(i) => 0xC0 + i as u8,
                None => c as u8,
            })
            .collect()
    };
    let mut parts = kit::epub3_parts();
    parts[5] = Part::new(
        "OEBPS/Text/ch1.xhtml",
        &koi8("<?xml version=\"1.0\" encoding=\"KOI8-R\"?><html><body><p>Мороз и солнце</p></body></html>"),
        true,
    );
    let mut utf16 = vec![0xFF, 0xFE];
    utf16.extend("<html><body><p>День чудесный</p></body></html>".encode_utf16().flat_map(u16::to_le_bytes));
    parts[6] = Part::new("OEBPS/Text/chapter two.xhtml", &utf16, true);
    let c = conv(&zip(&parts, b""));
    let p = paragraphs(&c.text);
    assert_eq!(p[0].1, "Мороз и солнце");
    assert_eq!(p[1].1, "День чудесный");
    assert_eq!(p[2].1, "Глава 3", "meta http-equiv over a Latin-1 head");
    assert_eq!(p.last().unwrap().1, "A note at the end.");
}

/// Deep nesting and markup soup in a part.
#[test]
fn deep_and_messy_markup() {
    let mut parts = kit::epub3_parts();
    let deep = format!(
        "<html><body>{}<p>deep</p>{}<P>unclosed<P>also &bogus; &#x41;<div><span>x</div></span><p>end",
        "<div><blockquote><i>".repeat(2000),
        "</i></blockquote></div>".repeat(1999)
    );
    parts[5] = Part::new("OEBPS/Text/ch1.xhtml", deep.as_bytes(), true);
    let c = conv(&zip(&parts, b""));
    let p = paragraphs(&c.text);
    assert_eq!(p[0].1, "deep");
    assert!(p.iter().any(|x| x.1 == "also &bogus; A"));
    assert!(p.iter().any(|x| x.1 == "end"));
}
