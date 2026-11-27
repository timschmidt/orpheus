//! The Books model over a real FAT32 / exFAT image (formatted with the
//! Linux tools, held in a RAM disk; skipped if they are missing): the
//! library search, opening (conversion, contents, layout in steps),
//! reading every page through the reader's own path, jumps, bookmarks
//! and position kept across a reopen, a re-layout keeping the position,
//! card work bounded per step, writes only under `/.orpheus`, and a clean
//! file system after. BOOK-2: the same for EPUB books, with the scratch
//! lent or not.

use std::path::PathBuf;
use std::process::Command;

#[path = "common/epubkit.rs"]
mod kit;

use orpheus_books::files::{self, CACHE_DIR, LibRecord, NORM_HEADER, PROGRESS_NEW};
use orpheus_books::layout::{self, Line, Mark, Metrics, Params, Skip};
use orpheus_books::norm::{BOLD, Item, item_at};
use orpheus_books::shelf::{Books, LibPhase, Phase, Req, Scope, Shelf};
use orpheus_fs::host::{RamDisk, block_on};
use orpheus_fs::{BlockDevice, CacheSlot, MountOptions, OpenMode, Sector, Volume};

/// A RAM disk that counts the sectors read and written.
struct Counting {
    disk: RamDisk,
    reads: u64,
    writes: u64,
}

impl BlockDevice for Counting {
    type Error = <RamDisk as BlockDevice>::Error;
    async fn read(&mut self, lba: u32, buf: &mut [Sector]) -> Result<(), Self::Error> {
        self.reads += buf.len() as u64;
        self.disk.read(lba, buf).await
    }
    async fn write(&mut self, lba: u32, buf: &[Sector]) -> Result<(), Self::Error> {
        self.writes += buf.len() as u64;
        self.disk.write(lba, buf).await
    }
    fn sector_count(&self) -> u32 {
        self.disk.sector_count()
    }
}

type Vol<'c> = Volume<'c, Counting>;

fn have(tool: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {tool}")).output().is_ok_and(|o| o.status.success())
}

/// The running test's name (its image files are its own: the tests run
/// in parallel).
fn test_name() -> String {
    std::thread::current().name().unwrap_or("test").replace("::", "-")
}

fn dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("orpheus-books-shelf");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn image(exfat: bool) -> Option<RamDisk> {
    let tool = if exfat { "mkfs.exfat" } else { "mkfs.fat" };
    if !have(tool) {
        eprintln!("{tool} missing: skipped");
        return None;
    }
    let p = dir().join(format!("{}-{}", test_name(), if exfat { "exfat.img" } else { "fat32.img" }));
    let f = std::fs::File::create(&p).unwrap();
    f.set_len(64 << 20).unwrap();
    drop(f);
    let mut c = Command::new(tool);
    if exfat {
        c.args(["-q", "-c", "4096"]);
    } else {
        c.args(["-F", "32", "-s", "1"]);
    }
    assert!(c.arg(&p).output().unwrap().status.success());
    Some(RamDisk::from_bytes(std::fs::read(&p).unwrap()))
}

fn fsck(disk: &RamDisk, exfat: bool) {
    let tool = if exfat { "fsck.exfat" } else { "fsck.fat" };
    if !have(tool) {
        return;
    }
    let p =
        dir().join(format!("{}-{}", test_name(), if exfat { "check-exfat.img" } else { "check-fat32.img" }));
    std::fs::write(&p, &disk.data).unwrap();
    let o = Command::new(tool).arg("-n").arg(&p).output().unwrap();
    assert!(
        o.status.success(),
        "{tool}: {}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}

/// Every character 6 px (bold 7), CJK 12, space 4: the reader's fonts
/// stand-in.
struct Mono;
impl Metrics for Mono {
    fn advance(&self, c: char, style: u8) -> u16 {
        let base = if layout::breaks_around(c) {
            12
        } else if c == ' ' {
            4
        } else {
            6
        };
        base + u16::from(style & BOLD != 0)
    }
}

const P: Params =
    Params { width: 110, height: 100, line: 16, gap: 3, indent: 8, inset: 8, justify: false, font: 0 };

fn cp1251(s: &str) -> Vec<u8> {
    // Russian letters only (and ASCII): enough for the sample.
    s.chars()
        .map(|c| match c {
            'А'..='я' => (u32::from(c) - 0x410 + 0xC0) as u8,
            'Ё' => 0xA8,
            'ё' => 0xB8,
            '—' => 0x97,
            '«' => 0xAB,
            '»' => 0xBB,
            c if c.is_ascii() => c as u8,
            _ => b'?',
        })
        .collect()
}

const TALE: &str = "The Project Gutenberg eBook\r\n\r\nTitle: A Tale of Two Cities\r\nAuthor: Charles Dickens\r\n\r\nBook the First--Recalled to Life\r\n\r\nCHAPTER I.\r\n\r\nThe Period\r\n\r\n";
const PARA: &str = "It was the best of times, it was the worst of times, it was the age of wisdom,\r\nit was the age of foolishness, it was the epoch of belief, it was the epoch\r\nof incredulity, it was the season of Light, it was the season of Darkness.\r\n\r\n";

fn tale() -> Vec<u8> {
    let mut s = String::from(TALE);
    for ch in 1..=6 {
        for _ in 0..12 {
            s.push_str(PARA);
        }
        s.push_str(&format!("CHAPTER {}.\r\n\r\n", ["II", "III", "IV", "V", "VI", "VII"][ch - 1]));
    }
    // Latin-1: one accented word, so it is not UTF-8.
    s.push_str("The end, caf\u{e9}.\r\n");
    s.chars().map(|c| c as u32 as u8).collect()
}

fn war() -> Vec<u8> {
    let mut body = String::new();
    for ch in 1..=5 {
        body.push_str(&format!("<section><title><p>Глава {ch}</p></title>"));
        for _ in 0..8 {
            body.push_str("<p>— Ну, что, князь, Генуя и Лукка стали не больше, как поместья, <emphasis>поместья</emphasis> фамилии Буонапарте.</p>");
        }
        body.push_str("</section>");
    }
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"windows-1251\"?>\n<FictionBook><description><title-info><author><first-name>Лев</first-name><last-name>Толстой</last-name></author><book-title>Война и мир</book-title></title-info></description><body><title><p>Война и мир</p></title>{body}</body></FictionBook>"
    );
    cp1251(&xml)
}

fn notes() -> Vec<u8> {
    let mut s = String::from("---\ntitle: Field notes\nauthor: Orpheus\n---\n# Field notes\n\n");
    for i in 1..=4 {
        s.push_str(&format!("## Day {i}\n\nWe walked *far* today and saw **many** birds; the path was `steep`.\n\n- one\n- two\n\n"));
    }
    s.into_bytes()
}

fn utf16() -> Vec<u8> {
    let mut b = vec![0xFF, 0xFE];
    b.extend(
        "夜に駆ける\r\n\r\n沈むように溶けてゆくように、二人だけの空が広がる夜に。\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes),
    );
    b
}

fn put(vol: &mut Vol<'_>, path: &str, data: &[u8]) {
    let mut f = block_on(vol.open(path, OpenMode::CreateNew)).unwrap();
    block_on(vol.write(&mut f, data)).unwrap();
    block_on(vol.close(f)).unwrap();
}

fn fill(vol: &mut Vol<'_>) {
    for d in ["/Books", "/Books/Russian", "/Books/.hidden", "/Music"] {
        block_on(vol.create_dir(d)).unwrap();
    }
    put(vol, "/Books/tale.txt", &tale());
    put(vol, "/Books/Russian/war.fb2", &war());
    put(vol, "/Books/notes.md", &notes());
    put(vol, "/Books/yoru.txt", &utf16());
    put(vol, "/Books/cover.jpg", b"\xFF\xD8");
    put(vol, "/Books/.hidden/secret.txt", b"no");
    put(vol, "/README.TXT", b"Title: Read me\nA card.\n");
    put(vol, "/Music/info.md", b"# Album notes\n");
    block_on(vol.flush()).unwrap();
}

/// Steps until `done`; the most sectors any one step read and wrote.
fn run(shelf: &mut Books, vol: &mut Vol<'_>, mut done: impl FnMut(&Shelf) -> bool) -> (u64, u64, u32) {
    let (mut r, mut w, mut n) = (0, 0, 0);
    for _ in 0..200_000 {
        if done(&shelf.shelf) {
            return (r, w, n);
        }
        let (r0, w0) = (vol.device().reads, vol.device().writes);
        block_on(shelf.step(vol, &Mono));
        r = r.max(vol.device().reads - r0);
        w = w.max(vol.device().writes - w0);
        n += 1;
    }
    panic!("never done");
}

fn idle(shelf: &mut Books, vol: &mut Vol<'_>) {
    for _ in 0..200_000 {
        if !block_on(shelf.step(vol, &Mono)) {
            return;
        }
    }
    panic!("never idle");
}

fn titles(shelf: &mut Books, vol: &mut Vol<'_>) -> Vec<(String, String)> {
    shelf.request(Req::Rows(0));
    idle(shelf, vol);
    (0..shelf.lib.count)
        .map(|i| {
            let r = shelf.lib.row(i).unwrap();
            (r.title.as_str().to_string(), r.author.as_str().to_string())
        })
        .collect()
}

fn row_of(shelf: &Shelf, title: &str) -> u16 {
    (0..shelf.lib.count).find(|&i| shelf.lib.row(i).unwrap().title.as_str() == title).unwrap()
}

/// The text of the lines of the view (as the reader draws it).
fn view_lines(shelf: &Shelf) -> (Vec<String>, layout::PageInfo) {
    let b = &shelf.book;
    let (base, w, at_end) = b.window().expect("window");
    let mut lines = Vec::new();
    let info = layout::page(w, base, at_end, b.top, b.skip, P, &Mono, &mut |l: &Line, _| {
        let mut s = String::new();
        let mut i = (l.mark.offset - base) as usize;
        while i < (l.end - base) as usize {
            let (it, n) = item_at(w, i).unwrap();
            if let Item::Char(c) = it {
                s.push(c);
            }
            i += n;
        }
        lines.push(s);
    });
    (lines, info)
}

fn read_file(vol: &mut Vol<'_>, path: &str) -> Vec<u8> {
    let mut f = block_on(vol.open(path, OpenMode::Read)).unwrap();
    let mut b = vec![0u8; f.size() as usize];
    let n = block_on(vol.read_exact_or_eof(&mut f, &mut b)).unwrap();
    b.truncate(n);
    b
}

fn words(text: &[u8]) -> String {
    let mut s = String::new();
    let mut i = 0;
    while let Some((it, n)) = item_at(text, i) {
        if let Item::Char(c) = it
            && c != ' '
        {
            s.push(c);
        }
        i += n;
    }
    s
}

fn book(exfat: bool) {
    let Some(disk) = image(exfat) else { return };
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let dev = Counting { disk, reads: 0, writes: 0 };
    let mut vol: Vol<'_> = block_on(Volume::mount(dev, &mut cache, MountOptions::default())).unwrap();
    fill(&mut vol);
    let before: Vec<Vec<u8>> = ["/Books/tale.txt", "/Books/Russian/war.fb2", "/README.TXT"]
        .iter()
        .map(|p| read_file(&mut vol, p))
        .collect();
    // The player's rule: writes only under /.orpheus.
    vol.set_write_prefix(Some("/.orpheus"));

    // The library: /Books only, sorted by title, titles from the files.
    let mut shelf = Books::new();
    shelf.request(Req::Library(Scope::BooksFolder));
    let (r, w, _) = run(&mut shelf, &mut vol, |s| s.lib.phase == LibPhase::Ready);
    // Making a folder zeroes its cluster (4 KiB here: 8 sectors and the
    // entry), once; every other step writes at most a few sectors.
    assert!(r <= 16 && w <= 12, "library search step: {r} read, {w} written");
    let t = titles(&mut shelf, &mut vol);
    assert_eq!(
        t,
        [
            ("A Tale of Two Cities".into(), "Charles Dickens".into()),
            ("Field notes".into(), "Orpheus".into()),
            ("yoru".into(), String::new()),
            ("Война и мир".into(), "Лев Толстой".into()),
        ]
    );
    assert!((0..4).all(|i| shelf.lib.row(i).unwrap().progress == PROGRESS_NEW));

    // Open the FB2: converted (Windows-1251), contents, laid out.
    shelf.request(Req::Params(P));
    let war_row = row_of(&shelf, "Война и мир");
    shelf.request(Req::Open(war_row));
    let (r, w, n) = run(&mut shelf, &mut vol, |s| s.book.phase == Phase::Ready && s.book.complete);
    assert!(r <= 16 && w <= 12, "open step: {r} read, {w} written ({n} steps)");
    let b = &shelf.book;
    assert_eq!(b.toc_count, 6, "the book's title and five chapters");
    assert!(b.pages > 10, "{} pages", b.pages);
    let text = read_file(&mut vol, &{
        let mut p = files::PathBuf::new();
        p.file(CACHE_DIR, b.id, "txt").to_string()
    });
    let text = &text[NORM_HEADER..];
    assert_eq!(text.len() as u32, b.text_len);
    assert!(words(text).contains("Глава3"));

    // Every page through the reader's path (window, page, next): all the
    // words once, in order.
    let mut all = String::new();
    let mut pages = 0;
    loop {
        idle(&mut shelf, &mut vol);
        let (lines, info) = view_lines(&shelf);
        assert!(!info.short, "the window holds the page");
        assert_eq!(shelf.book.page_no, Some(pages), "page number");
        for l in &lines {
            all.extend(l.chars().filter(|c| *c != ' '));
        }
        pages += 1;
        match info.next {
            Some(m) => shelf.request(Req::Show(m)),
            None => break,
        }
    }
    assert_eq!(pages, shelf.book.pages);
    assert_eq!(all, words(text), "no word lost or duplicated");

    // Back a page, a line up, to 50 %, the next and previous chapter.
    shelf.request(Req::Permille(500));
    idle(&mut shelf, &mut vol);
    let half = shelf.book.top;
    let k = shelf.book.page_no.unwrap();
    assert!(k > 2 && k < pages - 2);
    assert!(half.offset <= text.len() as u32 / 2);
    shelf.request(Req::PrevPage);
    idle(&mut shelf, &mut vol);
    assert_eq!(shelf.book.page_no, Some(k - 1));
    let (_, prev) = view_lines(&shelf);
    assert_eq!(prev.next, Some(half), "the page before ends where it was");
    shelf.request(Req::Show(half));
    shelf.request(Req::LineUp);
    idle(&mut shelf, &mut vol);
    let (up, upi) = view_lines(&shelf);
    assert!(upi.first.unwrap().offset < half.offset && upi.skip > 0, "{up:?}");
    shelf.request(Req::Chapter(true));
    idle(&mut shelf, &mut vol);
    let ch = shelf.book.top;
    assert!(ch.offset > half.offset && ch.kind.level().is_some());
    shelf.request(Req::Chapter(false));
    idle(&mut shelf, &mut vol);
    assert!(shelf.book.top.offset < ch.offset && shelf.book.top.kind.level().is_some());
    // Contents rows.
    shelf.request(Req::TocRows(0));
    idle(&mut shelf, &mut vol);
    assert_eq!(shelf.book.toc_row(2).unwrap().title(), "Глава 2");
    shelf.request(Req::Toc(3));
    idle(&mut shelf, &mut vol);
    let (l3, _) = view_lines(&shelf);
    assert_eq!(l3[0], "Глава 3");

    // A bookmark here, then reading on; close; the position and the
    // bookmark come back with the book, the library shows progress.
    let first = view_lines(&shelf).1.first.unwrap();
    shelf.request(Req::AddBookmark(first));
    shelf.request(Req::Permille(800));
    idle(&mut shelf, &mut vol);
    let at = view_lines(&shelf).1.first.unwrap();
    shelf.request(Req::Seen(at));
    shelf.request(Req::Close);
    idle(&mut shelf, &mut vol);
    assert_eq!(shelf.book.phase, Phase::Closed);
    let progress = shelf.lib.row(war_row).unwrap().progress;
    assert!((700..=800).contains(&progress), "{progress}");

    let mut again = Books::new();
    again.request(Req::Library(Scope::BooksFolder));
    idle(&mut again, &mut vol);
    again.request(Req::Params(P));
    titles(&mut again, &mut vol);
    assert_eq!(again.lib.row(war_row).unwrap().progress, progress, "progress in the library file");
    again.request(Req::Open(war_row));
    let steps_before = again.steps;
    run(&mut again, &mut vol, |s| s.book.phase == Phase::Ready && s.book.window().is_some());
    // (The open itself is five steps of one file operation each since
    // BOOK-1/2b.)
    assert!(again.steps - steps_before < 14, "cached: no conversion ({} steps)", again.steps - steps_before);
    assert_eq!(again.book.top, at, "position restored");
    let bm = again.book.state.bookmarks();
    assert_eq!(bm.len(), 1);
    assert_eq!(bm[0].mark, first);
    assert!(bm[0].snippet.as_str().starts_with("Глава 3"), "{}", bm[0].snippet.as_str());
    idle(&mut again, &mut vol);
    assert!(again.book.complete, "the index from the cache");

    // Another layout keeps the position: the new index's page holds it.
    let narrow = Params { width: 80, line: 18, ..P };
    again.request(Req::Params(narrow));
    run(&mut again, &mut vol, |s| s.book.complete);
    idle(&mut again, &mut vol);
    assert_eq!(again.book.top, at);
    assert!(again.book.page_no.is_some());
    again.request(Req::Close);
    idle(&mut again, &mut vol);

    // The other books open and lay out too (Latin-1 text with a Gutenberg
    // header, Markdown, UTF-16 with CJK).
    for title in ["A Tale of Two Cities", "Field notes", "yoru"] {
        let row = row_of(&again, title);
        again.request(Req::Open(row));
        run(&mut again, &mut vol, |s| s.book.phase == Phase::Ready && s.book.complete);
        let (lines, _) = view_lines(&again);
        assert!(!lines.is_empty(), "{title}");
        match title {
            "A Tale of Two Cities" => {
                assert_eq!(again.book.toc_count, 8, "book + 7 chapters");
                assert!(lines.iter().any(|l| l.contains("Gutenberg")));
            }
            "Field notes" => assert_eq!(again.book.toc_count, 5),
            _ => assert_eq!(lines[0], "夜に駆ける"),
        }
        again.request(Req::Close);
        idle(&mut again, &mut vol);
    }

    // The whole card: the read-me and the album notes too; hidden folders
    // and other files never.
    again.request(Req::Rescan(Scope::WholeCard));
    run(&mut again, &mut vol, |s| s.lib.phase == LibPhase::Ready);
    let all = titles(&mut again, &mut vol);
    assert_eq!(all.len(), 6, "{all:?}");
    assert!(all.iter().any(|(t, _)| t == "Read me") && all.iter().any(|(t, _)| t == "Album notes"));

    // Nothing outside /.orpheus changed; the file system is clean.
    vol.set_write_prefix(None);
    let after: Vec<Vec<u8>> = ["/Books/tale.txt", "/Books/Russian/war.fb2", "/README.TXT"]
        .iter()
        .map(|p| read_file(&mut vol, p))
        .collect();
    assert_eq!(before, after);
    let mut lib = vec![0u8; files::LIB_RECORD];
    let mut f = block_on(vol.open(files::LIBRARY_PATH, OpenMode::Read)).unwrap();
    f.set_position(files::lib_record_at(0));
    block_on(vol.read_exact_or_eof(&mut f, &mut lib)).unwrap();
    assert!(!LibRecord::decode(&lib).path().is_empty());
    block_on(vol.flush()).unwrap();
    let (dev, _) = block_on(vol.unmount());
    fsck(&dev.disk, exfat);
}

#[test]
fn books_on_fat32() {
    book(false);
}

#[test]
fn books_on_exfat() {
    book(true);
}

/// A book changed on the card after its first open is converted again
/// (the cache keeps the size and time it was made from).
#[test]
fn changed_book_is_converted_again() {
    let Some(disk) = image(false) else { return };
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let dev = Counting { disk, reads: 0, writes: 0 };
    let mut vol: Vol<'_> = block_on(Volume::mount(dev, &mut cache, MountOptions::default())).unwrap();
    block_on(vol.create_dir("/Books")).unwrap();
    put(&mut vol, "/Books/a.txt", b"First version.\n");
    let mut shelf = Books::new();
    shelf.request(Req::Library(Scope::BooksFolder));
    shelf.request(Req::Params(P));
    idle(&mut shelf, &mut vol);
    shelf.request(Req::Open(0));
    run(&mut shelf, &mut vol, |s| s.book.phase == Phase::Ready && s.book.window().is_some());
    assert_eq!(view_lines(&shelf).0, ["First version."]);
    shelf.request(Req::Close);
    idle(&mut shelf, &mut vol);
    let mut f = block_on(vol.open("/Books/a.txt", OpenMode::Truncate)).unwrap();
    block_on(vol.write(&mut f, b"Second, longer version.\n")).unwrap();
    block_on(vol.close(f)).unwrap();
    shelf.request(Req::Open(0));
    run(&mut shelf, &mut vol, |s| s.book.phase == Phase::Ready && s.book.window().is_some());
    assert_eq!(view_lines(&shelf).0, ["Second, longer", "version."]);
    let _ = Mark::START;
    let _ = Skip::None;
}

/// Steps with the scratch lent (or not) until `done` (one step at least:
/// a request takes effect in a step); the most sectors any one step read
/// and wrote, and the steps.
fn run_lent(
    shelf: &mut Books,
    vol: &mut Vol<'_>,
    mut scratch: Option<&mut [u8]>,
    mut done: impl FnMut(&Shelf) -> bool,
) -> (u64, u64, u32) {
    let (mut r, mut w, mut n) = (0, 0, 0);
    for _ in 0..400_000 {
        if n > 0 && done(&shelf.shelf) {
            return (r, w, n);
        }
        let (r0, w0) = (vol.device().reads, vol.device().writes);
        block_on(shelf.step_with(vol, &Mono, scratch.as_deref_mut()));
        r = r.max(vol.device().reads - r0);
        w = w.max(vol.device().writes - w0);
        n += 1;
    }
    panic!("never done");
}

fn idle_lent(shelf: &mut Books, vol: &mut Vol<'_>, mut scratch: Option<&mut [u8]>) {
    for _ in 0..400_000 {
        if !block_on(shelf.step_with(vol, &Mono, scratch.as_deref_mut())) {
            return;
        }
    }
    panic!("never idle");
}

fn epub_books(exfat: bool) {
    let Some(disk) = image(exfat) else { return };
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let dev = Counting { disk, reads: 0, writes: 0 };
    let mut vol: Vol<'_> = block_on(Volume::mount(dev, &mut cache, MountOptions::default())).unwrap();
    block_on(vol.create_dir("/Books")).unwrap();
    let (e3, e2) = (kit::epub3(), kit::epub2());
    put(&mut vol, "/Books/lighthouse.epub", &e3);
    put(&mut vol, "/Books/winter.epub", &e2);
    put(&mut vol, "/Books/broken.epub", b"Not an archive at all.");
    block_on(vol.flush()).unwrap();
    vol.set_write_prefix(Some("/.orpheus"));
    let mut scratch = vec![0u8; orpheus_books::inflate::SCRATCH];

    // The search without the scratch: EPUB titles are the file names.
    let mut shelf = Books::new();
    {
        // The step futures (card task frames on the player; host sizes).
        let a = core::mem::size_of_val(&shelf.step(&mut vol, &Mono));
        let b = core::mem::size_of_val(&shelf.step_with(&mut vol, &Mono, None));
        eprintln!("Books::step future {a} B, step_with {b} B");
        // A ratchet (BOOK-1/2 wiring, 2026-11-26): 5,464 -> 2,600 B with
        // the steps' buffers, the open's directory entry and the
        // library's record and reader in the desk; the rest is mostly the
        // file system's own futures (a path walk's open: 1,608 B here).
        assert!(b <= 2_816, "step_with future {b} B");
    }
    shelf.request(Req::Library(Scope::BooksFolder));
    run_lent(&mut shelf, &mut vol, None, |s| s.lib.phase == LibPhase::Ready);
    shelf.request(Req::Rows(0));
    idle_lent(&mut shelf, &mut vol, None);
    let names: Vec<String> = (0..3).map(|i| shelf.lib.row(i).unwrap().title.as_str().to_string()).collect();
    assert_eq!(names, ["broken", "lighthouse", "winter"]);

    // With it: from the package documents.
    shelf.request(Req::Rescan(Scope::BooksFolder));
    let (r, w, _) = run_lent(&mut shelf, &mut vol, Some(&mut scratch), |s| {
        s.lib.phase == LibPhase::Ready && s.lib.count == 3
    });
    assert!(r <= 16 && w <= 12, "library search step: {r} read, {w} written");
    let t = titles(&mut shelf, &mut vol);
    assert_eq!(
        t,
        [
            ("broken".into(), String::new()),
            ("The Lighthouse Keeper".into(), "Orpheus Tests".into()),
            ("Winter Harbour".into(), "Orpheus Tests".into()),
        ]
    );

    // Opening waits for the scratch, then converts in bounded steps.
    shelf.request(Req::Params(P));
    let row = row_of(&shelf, "The Lighthouse Keeper");
    shelf.request(Req::Open(row));
    idle_lent(&mut shelf, &mut vol, None);
    assert_eq!(shelf.book.phase, Phase::Converting);
    assert!(shelf.book.waiting && shelf.needs_scratch());
    let (r, w, n) = run_lent(&mut shelf, &mut vol, Some(&mut scratch), |s| {
        s.book.phase == Phase::Ready && s.book.complete
    });
    assert!(r <= 16 && w <= 12, "EPUB open step: {r} read, {w} written ({n} steps)");
    assert!(!shelf.needs_scratch());
    let b = &shelf.book;
    assert_eq!(b.toc_count, 7);
    assert_eq!(&b.lang[..2], b"en");
    let text = read_file(&mut vol, &{
        let mut p = files::PathBuf::new();
        p.file(CACHE_DIR, b.id, "txt").to_string()
    });
    let text = &text[NORM_HEADER..];
    assert_eq!(kit::paragraphs(text), kit::epub3_paras(), "no text lost");

    // Every page through the reader: every word once, in order.
    let mut all = String::new();
    let mut pages = 0;
    loop {
        idle_lent(&mut shelf, &mut vol, None);
        let (lines, info) = view_lines(&shelf);
        assert_eq!(shelf.book.page_no, Some(pages));
        for l in &lines {
            all.extend(l.chars().filter(|c| *c != ' '));
        }
        pages += 1;
        match info.next {
            Some(m) => shelf.request(Req::Show(m)),
            None => break,
        }
    }
    assert_eq!(pages, shelf.book.pages);
    assert_eq!(all, words(text));

    // The contents: rows, jumps to each entry, the next chapter.
    shelf.request(Req::TocRows(0));
    idle_lent(&mut shelf, &mut vol, None);
    assert_eq!(shelf.book.toc_row(1).unwrap().title(), "The Lamp");
    for (i, first) in [(1, "The Lamp"), (2, "Before the mark, “quoted”"), (5, "Глава 3")] {
        shelf.request(Req::Toc(i));
        idle_lent(&mut shelf, &mut vol, None);
        let (l, _) = view_lines(&shelf);
        assert!(l[0].starts_with(first) || first.starts_with(&l[0]), "entry {i}: {:?}", l[0]);
    }
    shelf.request(Req::Toc(0));
    idle_lent(&mut shelf, &mut vol, None);
    shelf.request(Req::Chapter(true));
    idle_lent(&mut shelf, &mut vol, None);
    assert_eq!(view_lines(&shelf).0[0], "The Lamp");
    shelf.request(Req::Close);
    idle_lent(&mut shelf, &mut vol, None);

    // Opened again: from the cache, no scratch wanted.
    shelf.request(Req::Open(row));
    let before = shelf.steps;
    run_lent(&mut shelf, &mut vol, None, |s| s.book.phase == Phase::Ready && s.book.window().is_some());
    assert!(shelf.steps - before < 10, "cached");
    shelf.request(Req::Close);
    idle_lent(&mut shelf, &mut vol, None);

    // The EPUB 2 book; the broken one fails.
    let row = row_of(&shelf, "Winter Harbour");
    shelf.request(Req::Open(row));
    run_lent(&mut shelf, &mut vol, Some(&mut scratch), |s| s.book.phase == Phase::Ready && s.book.complete);
    assert_eq!(shelf.book.toc_count, 3);
    assert_eq!(view_lines(&shelf).0[0], "I. The Ice");
    shelf.request(Req::Close);
    idle_lent(&mut shelf, &mut vol, None);
    let row = row_of(&shelf, "broken");
    shelf.request(Req::Open(row));
    run_lent(&mut shelf, &mut vol, Some(&mut scratch), |s| s.book.phase == Phase::Failed);
    // (Its files' entries are written in the steps after.)
    idle_lent(&mut shelf, &mut vol, None);

    // A book first listed by its file name gets its title on opening.
    let mut other = Books::new();
    other.request(Req::Rescan(Scope::BooksFolder));
    run_lent(&mut other, &mut vol, None, |s| s.lib.phase == LibPhase::Ready && s.lib.count == 3);
    other.request(Req::Params(P));
    titles(&mut other, &mut vol);
    // Its cache gone (converted again on opening).
    let row = row_of(&other, "lighthouse");
    let id = files::book_id("/Books/lighthouse.epub");
    block_on(vol.remove(files::PathBuf::new().file(CACHE_DIR, id, "txt"))).unwrap();
    other.request(Req::Open(row));
    run_lent(&mut other, &mut vol, Some(&mut scratch), |s| s.book.phase == Phase::Ready);
    idle_lent(&mut other, &mut vol, Some(&mut scratch));
    assert_eq!(other.lib.row(row).unwrap().title.as_str(), "The Lighthouse Keeper");
    assert_eq!(other.book.title.as_str(), "The Lighthouse Keeper");

    // The books are as they were; the file system is clean.
    vol.set_write_prefix(None);
    assert_eq!(read_file(&mut vol, "/Books/lighthouse.epub"), e3);
    assert_eq!(read_file(&mut vol, "/Books/winter.epub"), e2);
    block_on(vol.flush()).unwrap();
    let (dev, _) = block_on(vol.unmount());
    fsck(&dev.disk, exfat);
}

#[test]
fn epub_on_fat32() {
    epub_books(false);
}

#[test]
fn epub_on_exfat() {
    epub_books(true);
}

/// BOOK-2, 2026-11-25: a conversion or a layout given up (another book
/// opened during it, the layout changed while it ran) dropped its files
/// without writing their directory entries: the clusters it had written
/// were lost for good (fsck.fat: "Reclaimed N unused clusters"), on every
/// such change. The EPUB job's scratch tables were never written back at
/// all.
#[test]
fn regress_abandoned_job_loses_no_clusters() {
    let Some(disk) = image(false) else { return };
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let dev = Counting { disk, reads: 0, writes: 0 };
    let mut vol: Vol<'_> = block_on(Volume::mount(dev, &mut cache, MountOptions::default())).unwrap();
    block_on(vol.create_dir("/Books")).unwrap();
    put(&mut vol, "/Books/a.txt", &tale());
    put(&mut vol, "/Books/b.txt", &tale());
    put(&mut vol, "/Books/c.epub", &kit::epub3());
    let mut scratch = vec![0u8; orpheus_books::inflate::SCRATCH];
    let mut shelf = Books::new();
    shelf.request(Req::Library(Scope::BooksFolder));
    shelf.request(Req::Params(P));
    idle(&mut shelf, &mut vol);
    // A conversion given up half way.
    shelf.request(Req::Open(0));
    for _ in 0..30 {
        block_on(shelf.step(&mut vol, &Mono));
    }
    assert_eq!(shelf.book.phase, Phase::Converting);
    // An EPUB's conversion given up half way.
    shelf.request(Req::Open(2));
    for _ in 0..60 {
        block_on(shelf.step_with(&mut vol, &Mono, Some(&mut scratch)));
    }
    assert_eq!(shelf.book.phase, Phase::Converting);
    // A layout given up half way, by another layout.
    shelf.request(Req::Open(1));
    run(&mut shelf, &mut vol, |s| s.book.phase == Phase::Ready && s.book.pages > 3);
    assert!(!shelf.book.complete);
    shelf.request(Req::Params(Params { width: 90, ..P }));
    run(&mut shelf, &mut vol, |s| s.book.complete);
    shelf.request(Req::Close);
    idle(&mut shelf, &mut vol);
    block_on(vol.flush()).unwrap();
    let (dev, _) = block_on(vol.unmount());
    fsck(&dev.disk, false);
}

/// One step of the split as the firmware runs it: the model (`Shelf`) and
/// the card side (`Desk`) apart. Before the step, `Desk::next_kind`
/// predicts; nothing predicted means nothing done. The sectors written.
fn split_step(sh: &mut Shelf, desk: &mut orpheus_books::shelf::Desk, vol: &mut Vol<'_>) -> (bool, u64) {
    let kind = desk.next_kind(sh);
    let w0 = vol.device().writes;
    let changed = block_on(desk.step_with(sh, vol, &Mono, None));
    if kind.is_none() {
        assert!(!changed, "a step did work no kind predicted");
    }
    (changed, vol.device().writes - w0)
}

/// BOOK-1/2 wiring (2026-11-26): the model and the card side apart, as on
/// the player. While music plays (`mkdir` off) a missing folder is not
/// made: the work waits (`wants_mkdir`, the step reports nothing done,
/// nothing written, no folder made); once the folders are there the
/// library search, a book's conversion, its layout and every page go on
/// with no step writing more than BOOK-1's 12 sectors (measured here: 11
/// on FAT32 and 8 on exFAT, the search's last step and a conversion's
/// write: cluster allocations and the sector cache's write-backs on these
/// 512-byte-cluster images; a folder made zeroes a cluster: 256 sectors on
/// the owner's card). The card hand-over (`suspend`) writes the reading
/// position, leaves the volume clean and reopens the book there.
fn split(exfat: bool) {
    use orpheus_books::shelf::Desk;
    let Some(disk) = image(exfat) else { return };
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let dev = Counting { disk, reads: 0, writes: 0 };
    let mut vol: Vol<'_> = block_on(Volume::mount(dev, &mut cache, MountOptions::default())).unwrap();
    block_on(vol.create_dir("/Books")).unwrap();
    put(&mut vol, "/Books/tale.txt", &tale());
    block_on(vol.flush()).unwrap();
    vol.set_write_prefix(Some("/.orpheus"));
    let (mut sh, mut desk) = (Shelf::new(), Desk::new());

    // Music plays: no folder made, the work waits for the idle card task.
    desk.mkdir = false;
    sh.request(Req::Library(Scope::BooksFolder));
    sh.request(Req::Params(P));
    let w0 = vol.device().writes;
    for _ in 0..4 {
        if !split_step(&mut sh, &mut desk, &mut vol).0 {
            break;
        }
    }
    assert!(desk.wants_mkdir(), "a folder is missing");
    assert_eq!(vol.device().writes, w0, "nothing written while the folder waits");
    assert!(!split_step(&mut sh, &mut desk, &mut vol).0, "the step does nothing");
    // The music stops: the folders are made (all four, one per step,
    // before the search writes its file).
    desk.mkdir = true;
    for _ in 0..16 {
        split_step(&mut sh, &mut desk, &mut vol);
        if sh.lib.phase == LibPhase::Searching && block_on(vol.open_dir(files::CACHE_DIR)).is_ok() {
            break;
        }
    }
    assert!(!desk.wants_mkdir());
    // The music plays again: everything else, in small writes.
    desk.mkdir = false;
    let most = std::cell::Cell::new(0);
    let go = |sh: &mut Shelf, desk: &mut Desk, vol: &mut Vol<'_>, done: &dyn Fn(&Shelf) -> bool| {
        for _ in 0..200_000 {
            if done(sh) {
                return;
            }
            let (_, w) = split_step(sh, desk, vol);
            most.set(most.get().max(w));
        }
        panic!("never done");
    };
    go(&mut sh, &mut desk, &mut vol, &|s| s.lib.phase == LibPhase::Ready && s.lib.count == 1);
    sh.request(Req::Open(0));
    go(&mut sh, &mut desk, &mut vol, &|s| s.book.phase == Phase::Ready && s.book.complete);
    let pages = sh.book.pages;
    assert!(pages > 3, "{pages} pages");
    // Every page, as the reader turns them.
    for _ in 0..3 {
        go(&mut sh, &mut desk, &mut vol, &|s| s.book.window().is_some());
        let (_, info) = view_lines(&sh);
        let next = info.next.expect("a next page");
        sh.request(Req::Show(next));
        sh.request(Req::Seen(next));
    }
    go(&mut sh, &mut desk, &mut vol, &|s| s.book.window().is_some() && s.book.page_no.is_some());
    let at = sh.book.top;
    assert_eq!(sh.book.page_no, Some(3));
    assert!(most.get() <= 12, "a step wrote {} sectors while music played", most.get());
    for d in files::DIRS {
        assert!(block_on(vol.open_dir(d)).is_ok(), "{d} made before the music played again");
    }
    // The card goes to USB and comes back: the position written, the
    // volume clean, the book open again where it was.
    block_on(desk.suspend(&mut sh, &mut vol, 64));
    assert_eq!(vol.dirty_sectors(), 0);
    assert_eq!(sh.book.phase, Phase::Opening);
    go(&mut sh, &mut desk, &mut vol, &|s| s.book.phase == Phase::Ready && s.book.window().is_some());
    assert_eq!(sh.book.top, at, "reopened at the saved position");
    sh.request(Req::Close);
    for _ in 0..1000 {
        if !split_step(&mut sh, &mut desk, &mut vol).0 {
            break;
        }
    }
    block_on(vol.flush()).unwrap();
    let (dev, _) = block_on(vol.unmount());
    fsck(&dev.disk, exfat);
}

#[test]
fn split_on_fat32() {
    split(false);
}

#[test]
fn split_on_exfat() {
    split(true);
}

/// Regression (BOOK-1/2b, 2026-11-27): the on-device suite's `books`
/// journey failed on the player because the test books carried reading
/// progress ("The Lighthou… 17 %", "The Test Card 24 %" instead of "New")
/// left by earlier runs, and the suite's library replaced the owner's
/// (`/.orpheus/books/library.bin`), so the owner's next visit searched
/// the card again and every book's progress showed "New". Now the suite's
/// library ([`Scope::Test`]) has a file of its own and its books' state
/// is kept in RAM only: nothing of it reaches the card, and every run
/// starts at the first page.
#[test]
fn regress_suite_books_left_their_reading_state() {
    let Some(disk) = image(false) else { return };
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let dev = Counting { disk, reads: 0, writes: 0 };
    let mut vol: Vol<'_> = block_on(Volume::mount(dev, &mut cache, MountOptions::default())).unwrap();
    block_on(vol.create_dir("/Books")).unwrap();
    put(&mut vol, "/Books/owner.txt", &tale());
    for d in ["/.orpheus", "/.orpheus/test", "/.orpheus/test/suite", "/.orpheus/test/suite/books"] {
        block_on(vol.create_dir(d)).unwrap();
    }
    put(&mut vol, "/.orpheus/test/suite/books/card.txt", &tale());
    vol.set_write_prefix(Some("/.orpheus"));

    // The owner reads a third of their book.
    let mut owner = Books::new();
    owner.request(Req::Library(Scope::BooksFolder));
    owner.request(Req::Params(P));
    idle(&mut owner, &mut vol);
    owner.request(Req::Open(0));
    run(&mut owner, &mut vol, |s| s.book.phase == Phase::Ready && s.book.complete);
    owner.request(Req::Permille(330));
    idle(&mut owner, &mut vol);
    let at = view_lines(&owner).1.first.unwrap();
    owner.request(Req::Seen(at));
    owner.request(Req::Close);
    idle(&mut owner, &mut vol);
    let progress = owner.lib.row(0).unwrap().progress;
    assert!((250..=400).contains(&progress), "{progress}");
    let owner_lib = read_file(&mut vol, files::LIBRARY_PATH);

    // Two suite runs: each opens the suite's book, reads on, bookmarks,
    // closes.
    for run_no in 0..2 {
        let mut t = Books::new();
        t.request(Req::Library(Scope::Test));
        t.request(Req::Params(P));
        idle(&mut t, &mut vol);
        assert_eq!(t.lib.count, 1);
        titles(&mut t, &mut vol);
        assert_eq!(t.lib.row(0).unwrap().progress, PROGRESS_NEW, "run {run_no}: the book is new");
        t.request(Req::Open(0));
        run(&mut t, &mut vol, |s| s.book.phase == Phase::Ready && s.book.complete);
        idle(&mut t, &mut vol);
        assert_eq!(t.book.top.offset, 0, "run {run_no}: the first page");
        assert_eq!(t.book.state.n, 0, "run {run_no}: no bookmarks");
        assert!(t.book.ram_only);
        let first = view_lines(&t).1.first.unwrap();
        t.request(Req::AddBookmark(first));
        t.request(Req::Permille(600));
        idle(&mut t, &mut vol);
        let at = view_lines(&t).1.first.unwrap();
        for _ in 0..20 {
            // More position changes than make a save while reading.
            t.request(Req::Seen(at));
            idle(&mut t, &mut vol);
            t.request(Req::Seen(first));
            idle(&mut t, &mut vol);
        }
        t.request(Req::Seen(at));
        t.request(Req::Close);
        idle(&mut t, &mut vol);
        // Opened again in the same session: still from the start (its
        // state was never written).
        t.request(Req::Open(0));
        run(&mut t, &mut vol, |s| s.book.phase == Phase::Ready && s.book.window().is_some());
        assert_eq!(t.book.top.offset, 0);
        t.request(Req::Close);
        idle(&mut t, &mut vol);
    }
    // No state file for the suite's book; the owner's library untouched.
    let id = files::book_id("/.orpheus/test/suite/books/card.txt");
    let mut p = files::PathBuf::new();
    assert!(
        !block_on(vol.exists(p.file(files::BOOKS_DIR, id, "pos"))).unwrap(),
        "no .pos for the suite's book"
    );
    assert_eq!(read_file(&mut vol, files::LIBRARY_PATH), owner_lib, "the owner's library file");
    assert!(block_on(vol.exists(files::TEST_LIBRARY_PATH)).unwrap());

    // The owner comes back: their library as it was (no search), the
    // progress and the position kept.
    let mut back = Books::new();
    back.request(Req::Library(Scope::BooksFolder));
    back.request(Req::Params(P));
    let n = run(&mut back, &mut vol, |s| s.lib.phase == LibPhase::Ready).2;
    assert!(n <= 2, "read, not searched ({n} steps)");
    titles(&mut back, &mut vol);
    assert_eq!(back.lib.row(0).unwrap().progress, progress);
    back.request(Req::Open(0));
    run(&mut back, &mut vol, |s| s.book.phase == Phase::Ready && s.book.window().is_some());
    assert_eq!(back.book.top, at);
    block_on(vol.flush()).unwrap();
    let (dev, _) = block_on(vol.unmount());
    fsck(&dev.disk, false);
}

/// Regression (BOOK-1/2b, 2026-11-27): opening a book while music played
/// cost about two underruns every time on the player: the open was one
/// step (the library record, the book's directory entry, its saved
/// state, the cache's header and the contents file: five path walks),
/// 49–77 ms and once 256 ms on the owner's card, against chore budgets
/// of 26–30 ms. Now every step of an open does one file operation: here
/// (a book four folders deep, the cache and state folders holding many
/// files, the player's 8-slot sector cache and walk cache) no step of an
/// open reads more than a few sectors (one step read 25 before, 5 now).
#[test]
fn regress_book_open_was_one_long_step() {
    use orpheus_books::shelf::Desk;
    let Some(disk) = image(false) else { return };
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let dev = Counting { disk, reads: 0, writes: 0 };
    let mut vol: Vol<'_> = block_on(Volume::mount(dev, &mut cache, MountOptions::default())).unwrap();
    let mut walks = orpheus_fs::WalkCache::EMPTY;
    vol.set_walk_cache(&mut walks);
    for d in ["/Music", "/.orpheus", "/.orpheus/test", "/.orpheus/test/suite", "/.orpheus/test/suite/books"] {
        block_on(vol.create_dir(d)).unwrap();
    }
    for i in 0..40 {
        put(&mut vol, &format!("/Music/a long name for track number {i:03}.mp3"), b"x");
    }
    put(&mut vol, "/.orpheus/test/suite/books/The Test Card.txt", &tale());
    put(&mut vol, "/.orpheus/test/suite/books/Another book.txt", &notes());
    let (mut sh, mut desk) = (Shelf::new(), Desk::new());
    sh.request(Req::Library(Scope::Test));
    sh.request(Req::Params(P));
    while block_on(desk.step_with(&mut sh, &mut vol, &Mono, None)) {}
    // Both opened once (cached, laid out), and many other files beside
    // their caches.
    for row in 0..2 {
        sh.request(Req::Open(row));
        while block_on(desk.step_with(&mut sh, &mut vol, &Mono, None)) {}
        assert_eq!(sh.book.phase, Phase::Ready);
        sh.request(Req::Close);
        while block_on(desk.step_with(&mut sh, &mut vol, &Mono, None)) {}
    }
    for i in 0..60 {
        put(&mut vol, &format!("/.orpheus/cache/books/other cached file {i:03}.txt"), b"x");
        put(&mut vol, &format!("/.orpheus/books/other state file {i:03}.pos"), b"x");
    }
    block_on(vol.flush()).unwrap();
    // Each opened again, as while music plays (no folder made): the most
    // sectors a step read.
    desk.mkdir = false;
    let mut most = 0;
    for row in [1, 0, 1] {
        sh.request(Req::Open(row));
        for _ in 0..1000 {
            if sh.book.phase == Phase::Ready && sh.book.window().is_some() && sh.book.page_no.is_some() {
                break;
            }
            let r0 = vol.device().reads;
            block_on(desk.step_with(&mut sh, &mut vol, &Mono, None));
            most = most.max(vol.device().reads - r0);
        }
        assert_eq!(sh.book.phase, Phase::Ready);
        sh.request(Req::Close);
        while block_on(desk.step_with(&mut sh, &mut vol, &Mono, None)) {}
    }
    assert!(most <= 6, "a step of an open read {most} sectors");
}

/// Regression (BOOK-1/2b, 2026-11-27): on the player a book sometimes
/// never opened ("The book could not be read.", `books status`
/// `book=closed` or `opening` with its step count frozen until the
/// library was entered again), typically after another book was read and
/// the library shown again. The library screen asks for its rows when it
/// shows; with every row already loaded that step did nothing and said so
/// ("nothing to do"), and the firmware ends its card work at such a step
/// (it waits for the next request): the open asked for in the same
/// breath was never run. A step now goes on to the next work when the
/// rows are loaded already; here, as the firmware runs it, until the
/// first step that does nothing.
#[test]
fn regress_open_after_loaded_rows_never_ran() {
    use orpheus_books::shelf::Desk;
    let Some(disk) = image(false) else { return };
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let dev = Counting { disk, reads: 0, writes: 0 };
    let mut vol: Vol<'_> = block_on(Volume::mount(dev, &mut cache, MountOptions::default())).unwrap();
    block_on(vol.create_dir("/Books")).unwrap();
    put(&mut vol, "/Books/a.txt", &tale());
    put(&mut vol, "/Books/b.md", &notes());
    let (mut sh, mut desk) = (Shelf::new(), Desk::new());
    // As the firmware's card task: steps until one does nothing.
    let work = |sh: &mut Shelf, desk: &mut Desk, vol: &mut Vol<'_>| {
        let mut n = 0;
        while block_on(desk.step_with(sh, vol, &Mono, None)) {
            n += 1;
            assert!(n < 100_000);
        }
    };
    sh.request(Req::Library(Scope::BooksFolder));
    sh.request(Req::Params(P));
    sh.request(Req::Rows(0));
    work(&mut sh, &mut desk, &mut vol);
    assert_eq!(sh.lib.count, 2);
    for round in 0..2 {
        // Back in the library (its rows asked for again), then a book.
        sh.request(Req::Rows(0));
        sh.request(Req::Open(round));
        work(&mut sh, &mut desk, &mut vol);
        assert_eq!(sh.book.phase, Phase::Ready, "round {round}");
        assert!(sh.book.window().is_some() && sh.book.page_no.is_some(), "round {round}");
        sh.request(Req::Close);
        work(&mut sh, &mut desk, &mut vol);
    }
}
