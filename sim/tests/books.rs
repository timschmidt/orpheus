//! M15 Books (BOOK-1, BOOK-2): the Books app over the simulator's card
//! (`sim/src/books.rs`, the books of `sim/src/card.rs`): the library,
//! reading every page through the reader's keys, position and bookmarks
//! across a restart, a re-layout keeping the position, the card work in
//! steps with the reader answering keys meanwhile, writes only under
//! `/.orpheus`, and the differential tests of every reading setting
//! (07 §5.12; `tests/settings/effects.toml`).

use orpheus_apps::books::BooksService;
use orpheus_books::files::PROGRESS_NEW;
use orpheus_books::shelf::{LibPhase, Phase};
use orpheus_sim::AppSim;
use orpheus_sim::books::SimBooks;
use orpheus_sim::games::read;

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

fn sim(script: &str) -> AppSim {
    let mut s = AppSim::new();
    run(&mut s, script);
    s
}

const LIB: &str = "set:books=on goto:Books wait:50 expect-screen:Books";

/// Open the book of library row `row` (by title: Alice, A Tale of Two
/// Cities, Moby-Dick, The Lighthouse Keeper (EPUB), Война и мир, Пиковая
/// дама, 枕草子).
fn open(row: usize) -> String {
    format!("{LIB} down*{row} ok wait:50 expect-screen:Reader")
}

fn titles(s: &AppSim) -> Vec<String> {
    let lib = &s.sys.books().lib;
    (0..lib.count).map(|i| lib.row(i).map_or("?".into(), |r| r.title.as_str().to_string())).collect()
}

#[test]
fn library_lists_the_books_folder_by_title() {
    let s = sim(LIB);
    let lib = &s.sys.books().lib;
    assert_eq!(lib.phase, LibPhase::Ready);
    assert_eq!(
        titles(&s),
        [
            "Alice's Adventures in Wonderland",
            "A Tale of Two Cities",
            "Moby-Dick; or, The Whale",
            "The Lighthouse Keeper",
            "Война и мир",
            "Пиковая дама",
            "枕草子"
        ]
    );
    assert_eq!(lib.row(0).unwrap().author.as_str(), "Lewis Carroll", "Markdown front matter");
    assert_eq!(lib.row(2).unwrap().author.as_str(), "Herman Melville", "UTF-16 Gutenberg header");
    assert_eq!(lib.row(3).unwrap().author.as_str(), "Orpheus Project", "EPUB package document");
    assert_eq!(lib.row(5).unwrap().author.as_str(), "Александр Пушкин", "FB2 in Windows-1251");
    assert!((0..7).all(|i| lib.row(i).unwrap().progress == PROGRESS_NEW));
    // Lyrics.lrc, pictures and the rest of the card are not books.
}

/// Every page of a book through the reader's own keys: the page number
/// counts up by one per Next to the last page, then "End of the book";
/// Prev walks back to the first.
#[test]
fn every_page_by_next_and_back_by_prev() {
    let mut s = sim(&open(5));
    let total = s.sys.books().book.pages;
    assert!(s.sys.books().book.complete && total > 3, "{total} pages");
    for k in 0..total {
        assert_eq!(s.sys.books().book.page_no, Some(k));
        run(&mut s, "next wait:20");
    }
    assert_eq!(s.sys.books().book.page_no, Some(total - 1), "stays on the last page");
    for k in (0..total - 1).rev() {
        run(&mut s, "prev wait:20");
        assert_eq!(s.sys.books().book.page_no, Some(k));
    }
}

/// The position and the bookmarks are kept on the card: the book opens
/// where it was left after the player starts again (a new model over the
/// same card), the library shows the progress.
#[test]
fn position_and_bookmarks_survive_a_restart() {
    let mut s = sim(&format!("{} next next next wait:20 menu ok wait:20 next next wait:20", open(1)));
    let top = s.sys.books().book.top;
    assert_eq!(s.sys.books().book.state.n, 1);
    run(&mut s, "back wait:50 expect-screen:Books");
    let progress = s.sys.books().lib.row(1).unwrap().progress;
    assert!(progress > 0 && progress != PROGRESS_NEW, "{progress}");
    // The player starts again: the model is new, the card the same.
    s.sys.books = SimBooks::default();
    run(&mut s, "back goto:Books wait:50 down ok wait:50 expect-screen:Reader");
    assert_eq!(s.sys.books().lib.row(1).unwrap().progress, progress);
    let b = &s.sys.books().book;
    assert_eq!(b.top, top, "opens where it was left");
    assert_eq!(b.state.n, 1, "the bookmark");
    let id = b.id;
    let pos = read(&mut s.sys.media, &format!("/.orpheus/books/{id:08x}.pos"));
    assert!(pos.is_some_and(|p| p.starts_with(b"OBP1")));
}

/// Another font size lays the book out again and keeps the position: the
/// view's top is the same text, and its page in the new index holds it.
#[test]
fn relayout_keeps_the_position() {
    let mut s = sim(&format!("{} next*6 wait:20 down down wait:20", open(4)));
    let top = s.sys.books().book.top;
    let pages = s.sys.books().book.pages;
    for size in 1..=3 {
        run(&mut s, &format!("set:0x0f01={size} wait:50"));
        let b = &s.sys.books().book;
        assert_eq!(b.top.offset, top.offset, "size {size}");
        assert!(b.complete && b.pages != pages, "laid out again ({} pages)", b.pages);
        assert!(b.page_no.is_some());
    }
}

/// The card work runs in steps (one bounded piece per step: at most a
/// few sector reads and writes) and the reader answers keys while the
/// book is still being laid out.
#[test]
fn layout_in_steps_keeps_the_reader_answering() {
    let mut s = sim(&format!("{LIB} set:booksteps=2 down*4 ok"));
    for _ in 0..500 {
        let b = &s.sys.books().book;
        if b.phase == Phase::Ready && b.window().is_some() {
            break;
        }
        run(&mut s, "wait:10");
    }
    let b = &s.sys.books().book;
    assert!(b.window().is_some() && !b.complete, "readable while still laying out");
    run(&mut s, "next wait:20 next wait:20");
    let b = &s.sys.books().book;
    assert_eq!(b.phase, Phase::Ready);
    assert!(b.top.offset > 0, "pages turned before the index is complete");
    let laid = b.laid;
    run(&mut s, "wait:2000");
    assert!(s.sys.books().book.complete || s.sys.books().book.laid > laid);
    run(&mut s, "set:booksteps=0 wait:20");
    assert!(s.sys.books().book.complete);
}

/// Only `/.orpheus/` is written (CLAUDE.md rule 5): the books are as they
/// were after opening, reading, bookmarking and closing all of them.
#[test]
fn books_are_never_written() {
    let names = [
        "/Books/Alice in Wonderland.md",
        "/Books/A Tale of Two Cities.txt",
        "/Books/Moby Dick.txt",
        "/Books/Война и мир.txt",
        "/Books/Пиковая дама.fb2",
        "/Books/枕草子.txt",
        orpheus_sim::card::EPUB_PATH,
    ];
    let mut s = sim(LIB);
    let before: Vec<_> = names.iter().map(|n| read(&mut s.sys.media, n).unwrap()).collect();
    for row in 0..7 {
        run(&mut s, &format!("down*{row} ok wait:50 next next menu ok wait:20 back wait:50 up*6"));
    }
    let after: Vec<_> = names.iter().map(|n| read(&mut s.sys.media, n).unwrap()).collect();
    assert_eq!(before, after);
    assert!(read(&mut s.sys.media, "/.orpheus/books/library.bin").is_some());
}

/// Library › Menu › Find books in › Whole card: the setting, and the
/// library searched again with the card's other text files.
#[test]
fn find_books_in_from_the_menu() {
    let s = sim(&format!("{LIB} menu down ok down ok back back wait:50"));
    assert_eq!(s.sys.settings.get(orpheus_apps::books::id::FIND_IN), 1);
    assert_eq!(titles(&s).len(), 9);
}

/// BOOK-2: the EPUB on the card. Its contents are the navigation
/// document's (nested), each entry opens at its heading or anchor; every
/// page by Next; the text of all three chapters, the image skipped.
#[test]
fn epub_contents_and_pages() {
    let mut s = sim(&open(3));
    let b = &s.sys.books().book;
    assert_eq!(b.format, orpheus_books::convert::Format::Epub);
    assert_eq!(b.toc_count, 5);
    assert!(b.complete && b.pages > 3, "{} pages", b.pages);
    let total = b.pages;
    // Contents › the second entry (The Log, an anchor in chapter I).
    run(&mut s, "ok wait:50 ok wait:50 expect-screen:BookContents down ok wait:50 expect-screen:Reader");
    let b = &s.sys.books().book;
    let (base, w, _) = b.window().unwrap();
    let at = (b.top.offset - base) as usize;
    let text: String = String::from_utf8_lossy(&w[at..at + 12]).chars().filter(|c| !c.is_control()).collect();
    assert!(text.starts_with("The Log"), "{text:?}");
    // Every page from the start.
    let mut s = sim(&open(3));
    for k in 0..total {
        assert_eq!(s.sys.books().book.page_no, Some(k));
        run(&mut s, "next wait:20");
    }
    let id = s.sys.books().book.id;
    let cache = read(&mut s.sys.media, &format!("/.orpheus/cache/books/{id:08x}.txt")).unwrap();
    let text = String::from_utf8_lossy(&cache);
    for words in ["hundred and twelve steps", "Then the glass fell.", "All well.", "showing no flag"] {
        assert!(text.contains(words), "{words}");
    }
}

/// BOOK-2: without the scratch (music playing on the player) an EPUB
/// waits in "Preparing"; lent, it converts.
#[test]
fn epub_waits_for_the_scratch() {
    // (Off before the library is read, its title would be the file name.)
    let mut s = sim(&format!("{LIB} set:bookscratch=off down*3 ok wait:200"));
    let b = &s.sys.books().book;
    assert_eq!(b.phase, Phase::Converting);
    assert!(b.waiting);
    run(&mut s, "set:bookscratch=on wait:50 expect-screen:Reader");
    assert_eq!(s.sys.books().book.phase, Phase::Ready);
}

/// Auto page turn: with 10 s, the page turns after 10 s without a key.
#[test]
fn auto_page_turn() {
    let mut s = sim(&format!("set:0x0f06=1 {}", open(1)));
    run(&mut s, "wait:9000");
    assert_eq!(s.sys.books().book.page_no, Some(0));
    run(&mut s, "wait:1500");
    assert_eq!(s.sys.books().book.page_no, Some(1));
}

mod settings_effects {
    use super::*;

    /// The frame after `script` with `setting` set first.
    fn frame(setting: &str, script: &str) -> Vec<u16> {
        sim(&format!("{setting} {script}")).fb.clone()
    }

    /// Screens the reading settings must not change.
    fn others() -> Vec<String> {
        vec![
            "set:books=on wait:10".into(),
            "goto:Files wait:100".into(),
            // The text viewer (Files) on the same book.
            "next ok ok wait:100 ok wait:200 expect-screen:TextViewer".into(),
            "goto:Settings wait:50".into(),
        ]
    }

    /// `key=v` changes the frame of every context in `own` against
    /// `key=reference`, and not the frames of the others (and of `not`).
    fn shows_only_in(key: u16, v: i16, reference: i16, own: &[String], not: &[String]) {
        let set = |x: i16| format!("set:{key:#06x}={x}");
        for o in own {
            assert_ne!(frame(&set(v), o), frame(&set(reference), o), "{key:#06x}={v} shows in `{o}`");
        }
        for o in others().iter().chain(not) {
            assert_eq!(frame(&set(v), o), frame(&set(reference), o), "{key:#06x} leaks into `{o}`");
        }
    }

    fn reader() -> Vec<String> {
        vec![open(1), open(5), format!("{} ok wait:50 expect-screen:BookMenu down*8", open(1))]
    }

    fn library() -> Vec<String> {
        vec![LIB.into()]
    }

    #[test]
    fn font_size() {
        for v in 1..=3 {
            shows_only_in(0x0F01, v, 0, &reader(), &library());
        }
    }

    #[test]
    fn margins() {
        shows_only_in(0x0F02, 0, 1, &reader(), &library());
        shows_only_in(0x0F02, 2, 1, &reader(), &library());
    }

    #[test]
    fn line_spacing() {
        shows_only_in(0x0F03, 0, 1, &reader(), &library());
        shows_only_in(0x0F03, 2, 1, &reader(), &library());
    }

    #[test]
    fn justify() {
        // Justification moves words only on lines that are not a
        // paragraph's last: the Tale's page has such lines.
        shows_only_in(0x0F04, 1, 0, &reader()[..1], &library());
        // It does not move breaks: the same pages.
        let a = sim(&format!("set:0x0f04=1 {}", open(1)));
        let b = sim(&format!("set:0x0f04=0 {}", open(1)));
        assert_eq!(a.sys.books().book.pages, b.sys.books().book.pages);
    }

    #[test]
    fn palette() {
        for v in 1..=3 {
            shows_only_in(0x0F05, v, 0, &reader()[..2], &library());
        }
    }

    #[test]
    fn auto_page_turn() {
        // The reader after 12 s without a key: the next page, or not.
        let later = [format!("{} wait:12000", open(1))];
        for v in 1..=4 {
            let own: &[String] = if v == 1 { &later } else { &[] };
            shows_only_in(0x0F06, v, 0, own, &library());
        }
        let turned = sim(&format!("set:0x0f06=1 {}", later[0]));
        assert_eq!(turned.sys.books().book.page_no, Some(1));
        // The menu shows the value.
        let menu = [format!("{} ok wait:50 down*9", open(1))];
        shows_only_in(0x0F06, 4, 0, &menu, &[]);
    }

    #[test]
    fn find_books_in() {
        // The library: the whole card adds README.TXT and otpauth.txt.
        // (The reader on a book whose row is the same either way.)
        shows_only_in(0x0F07, 1, 0, &library(), &reader()[..1]);
        let card = sim(&format!("set:0x0f07=1 {LIB}"));
        let t = titles(&card);
        assert_eq!(t.len(), 9, "{t:?}");
        assert!(t.contains(&"README".to_string()));
    }
}
