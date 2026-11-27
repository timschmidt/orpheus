//! The Books app on the player (05 §7.15; BOOK-1/2 wiring): the model the
//! screens read (`orpheus_books::shelf::Shelf`, the UI side) and its card
//! work (`Desk`, the card side), each in a static of its own.
//!
//! **Who holds the model.** The desk's steps change the model (the window
//! read, a jump resolved, rows loaded), so the two tasks take turns: the
//! UI takes it at its first use in a loop ([`ui_shelf`]) and gives it back
//! at the loop's end ([`ui_release`]); the card task takes it for one step
//! at a time ([`chore`], [`run_idle`], [`run_paused`]). The card task runs
//! in the RT executor and preempts the UI: it only *tries* to take the
//! model inside a chore (a UI loop in progress: the next chore), and
//! awaits the UI's release elsewhere; the UI, which cannot preempt the
//! card task, waits for a step in progress by spinning (at most one step:
//! the card task gives the model back after each).
//!
//! **Card work** (`Desk::step_with`, one bounded piece per step: ≤ 16
//! sectors read and ≤ 12 written, measured on FAT32 and exFAT images in
//! `orpheus-books`): while a track plays, as chores of the card task with
//! their measured needs (the [`Kind`]s are chore kinds `job:bwrite` …
//! `job:blay`), each piece stopping at the chore's budget
//! (`audio::piece_left` before each step), with no folder made (a folder's
//! cluster is zeroed: 256 sectors on the owner's 128 KiB-cluster card; a
//! missing folder waits for the music to stop) and no scratch (an EPUB
//! waits: "Stop the music to prepare this book"); paused, steps between
//! the paused loop's requests (folders made, no scratch: the decoder is
//! loaded); with no track loaded, in the idle loop, with the decoder
//! arena's first 35,072 bytes lent as the EPUB's inflate window while
//! `Desk::needs_scratch` (taken back whenever the idle loop gives the arena
//! to anything else: [`arena_taken`], and the next lent step starts the
//! entry again). Writes only under `/.orpheus/books/` and
//! `/.orpheus/cache/books/` (CLAUDE.md rule 5; the volume's write prefix
//! is `/.orpheus` as well).
//!
//! **Test hooks** (developer channel, read-only for the card): `books
//! status` (the model as the card task last left it), `books test on|off`
//! (the library is `/.orpheus/test/suite/books`, the suite's books).
//!
//! **Not in the shipping image yet** (firmware feature `books`, off): with
//! it the paged code needs 139 of the code window's 128 pages (Books adds
//! about 112 KB of code: `orpheus-books` 92 KB, its screens 12 KB, this
//! module 8 KB) and `.bss` grows 13,240 B (docs/STATUS.md, the BOOK-1/2
//! wiring). Without the feature the functions below do nothing (no card
//! work is ever asked for), so the card task and the UI call them
//! unconditionally.

#[cfg(feature = "books")]
mod on;
#[cfg(feature = "books")]
pub use on::*;

#[cfg(not(feature = "books"))]
mod off {
    use orpheus_fs::Volume;
    use yoton_bsp::card::Card;

    pub fn init() {}
    pub fn ui_release() {}
    #[must_use]
    pub fn take_changed() -> bool {
        false
    }
    pub async fn save_for_shutdown(_max_ms: u64) {}
    #[must_use]
    pub fn wants() -> bool {
        false
    }
    #[must_use]
    pub fn wants_idle() -> bool {
        false
    }
    #[must_use]
    pub fn kind() -> usize {
        0
    }
    pub fn arena_taken() {}
    pub async fn chore(_vol: &mut Volume<'static, Card>) {}
    pub async fn run_paused(_vol: &mut Volume<'static, Card>) {}
    pub async fn run_idle(_vol: &mut Volume<'static, Card>, _arena: &mut [u8]) {}
    pub async fn card_leaving(_vol: &mut Volume<'static, Card>) {}
    /// `books …`: not in this build.
    pub fn test_command(_rest: &[u8]) -> bool {
        crate::audio::reply(|w| {
            w.s(b"books: not in this build");
        });
        true
    }
}
#[cfg(not(feature = "books"))]
pub use off::*;
