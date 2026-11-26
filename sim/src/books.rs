//! The simulator's Books back end (M15, 05 §7.15, BOOK-1): the
//! `orpheus_books::shelf::Shelf` the screens read, its card work run in
//! steps over the simulated card through `orpheus-fs` (as the firmware's
//! card task would run it as a chore), its files under `/.orpheus/books/`
//! and `/.orpheus/cache/books/`.
//!
//! Script hooks (`set:`): `books=on|off` puts Books on Home (off by
//! default: not on the player yet); `booksteps=N` runs at most `N` card
//! steps per simulator step (0, the default: until the work is done), so
//! the reader's "Preparing" and the layout in progress can be looked at.

use orpheus_apps::books::Face;
use orpheus_books::shelf::Shelf;
use orpheus_fs::host::block_on;

use crate::media::Media;

#[derive(Default)]
pub struct SimBooks {
    pub shelf: Shelf,
    /// `set:booksteps=N`: card steps per simulator step (0: until idle).
    pub per_step: u32,
    /// Card steps run (tests).
    pub steps: u64,
}

/// Steps one simulator step runs at most when not limited.
const UNTIL_IDLE: u32 = 200_000;

impl SimBooks {
    /// Run the card work; true if the model changed.
    pub fn pump(&mut self, media: &mut Media, face: &Face) -> bool {
        let vol = media.volume();
        let limit = if self.per_step == 0 { UNTIL_IDLE } else { self.per_step };
        let mut changed = false;
        for _ in 0..limit {
            if !block_on(self.shelf.step(vol, face)) {
                break;
            }
            self.steps += 1;
            changed = true;
        }
        changed
    }
}
