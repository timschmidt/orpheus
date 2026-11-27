//! The Books model's RAM (BOOK-1): every buffer is fixed; these are the
//! figures docs/STATUS.md reports (host, 64-bit; the player's 32-bit
//! layout is the same or smaller: no pointers in the large parts).
//! `cargo test -p orpheus-books --test ram -- --nocapture` prints them.

use core::mem::size_of;

use orpheus_books::convert::{Converter, MetaReader};
use orpheus_books::epub::Epub;
use orpheus_books::files::{BookState, LibRecord};
use orpheus_books::inflate::{Inflate, SCRATCH};
use orpheus_books::layout::{Liner, Pager, Paginator};
use orpheus_books::opf::Doc;
use orpheus_books::shelf::{Book, Books, Desk, Library, Shelf, WINDOW};
use orpheus_books::xhtml::Xhtml;
use orpheus_books::zip::{CdWalk, Name};

#[test]
fn ram_figures() {
    let rows = [
        ("Books (model and card side)", size_of::<Books>()),
        ("Shelf (the model: the UI side)", size_of::<Shelf>()),
        ("  Book (text window, contents window, index cache, state)", size_of::<Book>()),
        ("  Library (sort order, row window)", size_of::<Library>()),
        ("  BookState (position, 12 bookmarks)", size_of::<BookState>()),
        ("Desk (the card side: the work, files, I/O buffer)", size_of::<Desk>()),
        ("Converter (decoder, parser, writer)", size_of::<Converter>()),
        ("MetaReader (library search)", size_of::<MetaReader>()),
        ("Paginator", size_of::<Paginator>()),
        ("Liner", size_of::<Liner>()),
        ("Pager", size_of::<Pager>()),
        ("LibRecord", size_of::<LibRecord>()),
        ("Epub (the EPUB job, without its scratch)", size_of::<Epub>()),
        ("  Doc (a package document being read)", size_of::<Doc>()),
        ("  Xhtml (content document parser)", size_of::<Xhtml>()),
        ("  Inflate (decoder state)", size_of::<Inflate>()),
        ("  CdWalk (central directory reader)", size_of::<CdWalk>()),
        ("  Name (a path)", size_of::<Name>()),
        ("Scratch lent for EPUB (window + Huffman tables)", SCRATCH),
    ];
    for (name, n) in rows {
        eprintln!("{name:60} {n:6} B");
    }
    assert!(size_of::<Book>() >= WINDOW);
    // A ratchet (STATUS, BOOK-1): measured 9,360 B for the whole model
    // with its card work (the search, conversion and layout share one
    // union); the layout's line state small enough for the UI task's draw
    // path. BOOK-2 (2026-11-25): 12,160 B. The EPUB job (2,880 B: its
    // 512-byte read buffer, three paths of 257 B, a package document's
    // reader of 952 B or the converter, the anchor watch, the inflate
    // state; the window and tables are the lent scratch, 35,072 B) is in
    // the union twice over: converting, and in the library search reading
    // a title (in place of the 540-byte head reader), the larger
    // (+2,360 B); the files a job leaves, kept until their entries are
    // written (`regress_abandoned_job_loses_no_clusters`), and a library
    // title to rewrite (+440 B).
    // The firmware wiring (2026-11-26) split it: the model the screens
    // read (5,476 B: the book with its 2 KiB window, the library rows)
    // and the card side (7,712 B: the work, the files, a 1 KiB I/O
    // buffer and a directory entry, which were the step future's: 5,464
    // -> 2,600 B, `tests/shelf.rs`); the search's directory entry is the
    // desk's (-840 B in all).
    assert!(size_of::<Books>() <= 13_248, "{} B", size_of::<Books>());
    assert!(size_of::<Shelf>() <= 5_504, "{} B", size_of::<Shelf>());
    assert!(size_of::<Desk>() <= 7_744, "{} B", size_of::<Desk>());
    assert!(size_of::<Epub>() <= 2_944, "{} B", size_of::<Epub>());
    assert!(size_of::<Liner>() + size_of::<Pager>() <= 160, "{} B", size_of::<Liner>() + size_of::<Pager>());
}
