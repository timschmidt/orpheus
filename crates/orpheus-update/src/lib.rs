//! Firmware update (UPD-1, docs/plan/02-architecture.md §2.5).
//!
//! * [`ofw`]: the update file `orpheus-<version>.ofw` — a 384-byte header
//!   (version, build id, SHA-256, target layout, a short change list)
//!   followed by the `"YFW1"` slot image, validated before anything is
//!   written.
//! * [`slots`]: which slot an update writes (always the one the player did
//!   not boot from), the model of boot stage 2's slot choice the tests check
//!   every intermediate NOR state against, and the recovery switch.
//! * [`install`]: the installer, generic over a [`install::Flash`]: the
//!   firmware drives it with the guarded HAL NOR driver from the card or
//!   the developer channel, the tests with a simulated NOR that loses power
//!   between any two operations.
//! * [`wire`]: the developer channel's `UPDATE` request (06 §2.5).
//!
//! # Safety argument (why a power cut never leaves an unbootable player)
//!
//! Stage 2 boots the first slot of full A, full B, dual whose header has
//! the magic `"YFW1"`, version 1, sane bounds and a matching resident sum
//! (`fw/boot/stage2`). The installer
//!
//! 1. erases the target slot's header sector first, so the target is
//!    invalid while anything else in it changes;
//! 2. writes and verifies the body (read back, SHA-256 of the whole image);
//! 3. programs the header without its magic word, verifies it, then the
//!    magic word alone: a torn 4-byte program leaves bits set, so the word
//!    is `"YFW1"` only once complete;
//! 4. only when the target comes *after* the running slot in stage 2's
//!    order (running A, target B), clears the running slot's magic word
//!    (bits can be cleared without an erase; a torn clear is not `"YFW1"`).
//!
//! At every point stage 2 therefore finds either the old image intact and
//! first, or the new image complete and verified first. `install::tests`
//! cuts the power before every operation and checks exactly that.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(any(test, feature = "std"))]
extern crate std;

pub mod install;
pub mod ofw;
#[cfg(any(test, feature = "std"))]
pub mod sim;
pub mod slots;
pub mod wire;

/// The SHA-256 the format uses (for callers that hash a file first).
pub use sha2::{Digest, Sha256};

/// Why an update was refused or stopped. The numbers are the developer
/// channel's error codes ([`wire::Status::error`]); never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Error {
    /// Fewer bytes than a header.
    Short = 1,
    /// Not an `"OFW1"` file.
    Magic = 2,
    /// A header version this firmware does not know.
    Version = 3,
    /// Header checksum wrong (damaged file).
    Crc = 4,
    /// Built for another NOR layout than the player's.
    Layout = 5,
    /// Image larger than a slot, or too small to hold a firmware.
    Size = 6,
    /// The slot header inside the image is damaged.
    SlotHeader = 7,
    /// The file's build id differs from its image's.
    BuildId = 8,
    /// Resident image or page store outside what stage 2 / the pager accept.
    Bounds = 9,
    /// The player runs the dual-boot layout: no second slot to write.
    NotFull = 10,
    /// The firmware does not know which slot it runs from (started from RAM).
    NoSlot = 11,
    /// Another update is in progress.
    Busy = 12,
    /// Data out of order (a chunk skipped or overlapping differently).
    Order = 13,
    /// The image's SHA-256 differs from the header's (damaged file).
    Hash = 14,
    /// The resident image's sum differs from the slot header's.
    Sum = 15,
    /// The header sector of the image is not header + erased bytes.
    Padding = 16,
    /// A write outside the target slot (never happens; the guard says no).
    Refused = 17,
    /// The NOR driver reported a failure.
    Flash = 18,
    /// Read-back differs from what was written.
    Verify = 19,
    /// Request not valid in this state.
    State = 20,
    /// The version text is not printable UTF-8.
    Text = 21,
    /// Battery too low for an update without the cable.
    Battery = 22,
    /// Cancelled.
    Aborted = 23,
    /// The file could not be read from the card.
    Io = 24,
}

impl Error {
    /// The error for a code (0 = none).
    #[must_use]
    pub fn from_code(c: u8) -> Option<Error> {
        use Error::*;
        const ALL: [Error; 24] = [
            Short, Magic, Version, Crc, Layout, Size, SlotHeader, BuildId, Bounds, NotFull, NoSlot, Busy,
            Order, Hash, Sum, Padding, Refused, Flash, Verify, State, Text, Battery, Aborted, Io,
        ];
        ALL.iter().copied().find(|e| *e as u8 == c)
    }

    /// Short English reason (host tools, logs; the UI has its own strings).
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Error::Short => "file too short",
            Error::Magic => "not an Orpheus update file",
            Error::Version => "update file of an unknown version",
            Error::Crc => "update file header damaged",
            Error::Layout => "built for another flash layout",
            Error::Size => "image does not fit a slot",
            Error::SlotHeader => "slot header in the image damaged",
            Error::BuildId => "build id mismatch",
            Error::Bounds => "image outside the boot stage's limits",
            Error::NotFull => "dual-boot layout: update from the computer",
            Error::NoSlot => "running slot unknown",
            Error::Busy => "another update is running",
            Error::Order => "data out of order",
            Error::Hash => "SHA-256 mismatch (damaged file)",
            Error::Sum => "resident checksum mismatch",
            Error::Padding => "header sector not erased padding",
            Error::Refused => "write outside the target slot refused",
            Error::Flash => "flash error",
            Error::Verify => "read-back differs",
            Error::State => "not possible now",
            Error::Text => "version text invalid",
            Error::Battery => "battery too low",
            Error::Aborted => "cancelled",
            Error::Io => "could not read the file",
        }
    }
}
