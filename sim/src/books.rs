//! The simulator's Books back end (M15, 05 §7.15, BOOK-1/2): the
//! `orpheus_books::shelf::Shelf` the screens read, its card work run in
//! steps over the simulated card through `orpheus-fs` (as the firmware's
//! card task would run it as a chore), its files under `/.orpheus/books/`
//! and `/.orpheus/cache/books/`.
//!
//! EPUB (BOOK-2) needs a scratch of `inflate::SCRATCH` bytes (the inflate
//! window and tables; on the player the decoder arena when no track is
//! loaded): the simulator lends its own while the shelf asks for it.
//!
//! Script hooks (`set:`): `books=on|off` puts Books on Home (off by
//! default: the goldens' Home; on the player it is there, and the
//! on-device suite's appearance turns it on); `bookstest=on` makes the on-device suite's
//! book folder the library (as the player's `books test on`, writing its
//! books to the card first); `booksteps=N` runs at most `N` card
//! steps per simulator step (0, the default: until the work is done), so
//! the reader's "Preparing" and the layout in progress can be looked at;
//! `bookscratch=off` lends no scratch (as on the player while music
//! plays): an EPUB waits in "Preparing"; `bookfail=on` marks the open
//! book unreadable (the reader's message).

use orpheus_apps::books::Face;
use orpheus_books::inflate::SCRATCH;
use orpheus_books::shelf::{Books, Req, Scope};
use orpheus_fs::host::block_on;

use crate::media::Media;

pub struct SimBooks {
    pub shelf: Books,
    /// `set:booksteps=N`: card steps per simulator step (0: until idle).
    pub per_step: u32,
    /// Card steps run (tests).
    pub steps: u64,
    /// `set:bookscratch=on|off`: the scratch is lent.
    pub lend: bool,
    /// `set:bookstest=on`: the library is the suite's book folder.
    pub test: bool,
    scratch: Vec<u8>,
}

impl Default for SimBooks {
    fn default() -> Self {
        SimBooks {
            shelf: Books::new(),
            per_step: 0,
            steps: 0,
            lend: true,
            test: false,
            scratch: vec![0; SCRATCH],
        }
    }
}

/// Steps one simulator step runs at most when not limited.
const UNTIL_IDLE: u32 = 200_000;

impl SimBooks {
    /// The request, its library scope the suite's folder under
    /// `set:bookstest=on` (as the firmware's `books test on`).
    #[must_use]
    pub fn test_scope(&self, r: Req) -> Req {
        match r {
            Req::Library(_) if self.test => Req::Library(Scope::Test),
            Req::Rescan(_) if self.test => Req::Rescan(Scope::Test),
            r => r,
        }
    }

    /// Run the card work; true if the model changed.
    pub fn pump(&mut self, media: &mut Media, face: &Face) -> bool {
        let vol = media.volume();
        let limit = if self.per_step == 0 { UNTIL_IDLE } else { self.per_step };
        let mut changed = false;
        for _ in 0..limit {
            let lent = (self.lend && self.shelf.needs_scratch()).then_some(&mut self.scratch[..]);
            if !block_on(self.shelf.step_with(vol, face, lent)) {
                break;
            }
            self.steps += 1;
            changed = true;
        }
        changed
    }
}
