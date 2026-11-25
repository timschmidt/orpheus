//! The Books model's RAM (BOOK-1): every buffer is fixed; these are the
//! figures docs/STATUS.md reports (host, 64-bit; the player's 32-bit
//! layout is the same or smaller: no pointers in the large parts).
//! `cargo test -p orpheus-books --test ram -- --nocapture` prints them.

use core::mem::size_of;

use orpheus_books::convert::{Converter, MetaReader};
use orpheus_books::files::{BookState, LibRecord};
use orpheus_books::layout::{Liner, Pager, Paginator};
use orpheus_books::shelf::{Book, Library, Shelf, WINDOW};

#[test]
fn ram_figures() {
    let rows = [
        ("Shelf (all of it)", size_of::<Shelf>()),
        ("  Book (text window, contents window, index cache, state)", size_of::<Book>()),
        ("  Library (sort order, row window)", size_of::<Library>()),
        ("  BookState (position, 12 bookmarks)", size_of::<BookState>()),
        ("Converter (decoder, parser, writer)", size_of::<Converter>()),
        ("MetaReader (library search)", size_of::<MetaReader>()),
        ("Paginator", size_of::<Paginator>()),
        ("Liner", size_of::<Liner>()),
        ("Pager", size_of::<Pager>()),
        ("LibRecord", size_of::<LibRecord>()),
    ];
    for (name, n) in rows {
        eprintln!("{name:60} {n:6} B");
    }
    assert!(size_of::<Book>() >= WINDOW);
    // A ratchet (STATUS, BOOK-1): measured 9,360 B for the whole model
    // with its card work (the search, conversion and layout share one
    // union); the layout's line state small enough for the UI task's draw
    // path.
    assert!(size_of::<Shelf>() <= 9_472, "{} B", size_of::<Shelf>());
    assert!(size_of::<Liner>() + size_of::<Pager>() <= 160, "{} B", size_of::<Liner>() + size_of::<Pager>());
}
