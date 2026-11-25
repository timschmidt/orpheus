//! The developer channel's `UPDATE` request (`devchan::REQ_UPDATE`, 06
//! §2.5): the host writes the inactive slot through the running firmware,
//! without ADFU (`xtask dev update`, the browser installer's Update flow).
//!
//! | `wValue` | Dir | `wIndex` | Data |
//! |---|---|---|---|
//! | [`OP_BEGIN`] | OUT | — | `.ofw` header (384 bytes) + the image's first 128 bytes |
//! | [`OP_DATA`] | OUT | chunk number | image bytes from `chunk × CHUNK` (≤ [`CHUNK`]) |
//! | [`OP_COMMIT`] | OUT | — | — (read back, verify, switch slots; poll the status) |
//! | [`OP_ABORT`] | OUT | — | — |
//! | — | IN | — | [`Status`] (16 bytes) |
//!
//! The host reboots the player (`REBOOT`) once the status says
//! [`State::Done`].

use crate::Error;

pub const OP_BEGIN: u16 = 0;
pub const OP_DATA: u16 = 1;
pub const OP_COMMIT: u16 = 2;
pub const OP_ABORT: u16 = 3;
/// Image bytes per `DATA` request (the firmware's control buffer is 1 KiB).
pub const CHUNK: usize = 1024;
/// `BEGIN`'s data: the `.ofw` header, then the slot header.
pub const BEGIN_LEN: usize = crate::ofw::HEADER_SIZE + orpheus_proto::slot::HEADER_SIZE;

/// Where an update stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Idle = 0,
    /// Receiving the image.
    Writing = 1,
    /// Reading it back.
    Verifying = 2,
    /// Switching slots.
    Committing = 3,
    /// Done: reboot to start the new firmware.
    Done = 4,
    /// Stopped (see `error`); the running firmware stays.
    Failed = 5,
    /// The card task is installing from a file (the UI's update).
    Card = 6,
}

impl State {
    #[must_use]
    pub const fn from_code(c: u8) -> State {
        match c {
            1 => State::Writing,
            2 => State::Verifying,
            3 => State::Committing,
            4 => State::Done,
            5 => State::Failed,
            6 => State::Card,
            _ => State::Idle,
        }
    }
}

/// The status reply (16 bytes, little-endian).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// [`Error`] code, 0 = none.
    pub error: u8,
    /// Target slot index (0 A, 1 B; 0xFF none).
    pub slot: u8,
    pub received: u32,
    pub verified: u32,
    pub image_len: u32,
}

impl Status {
    pub const SIZE: usize = 16;

    #[must_use]
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.state as u8;
        b[1] = self.error;
        b[2] = self.slot;
        b[4..8].copy_from_slice(&self.received.to_le_bytes());
        b[8..12].copy_from_slice(&self.verified.to_le_bytes());
        b[12..16].copy_from_slice(&self.image_len.to_le_bytes());
        b
    }

    #[must_use]
    pub fn from_bytes(b: &[u8]) -> Option<Status> {
        let b: &[u8; Self::SIZE] = b.get(..Self::SIZE)?.try_into().ok()?;
        let w = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        Some(Status {
            state: State::from_code(b[0]),
            error: b[1],
            slot: b[2],
            received: w(4),
            verified: w(8),
            image_len: w(12),
        })
    }

    #[must_use]
    pub fn error(&self) -> Option<Error> {
        Error::from_code(self.error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_roundtrip() {
        let s = Status {
            state: State::Verifying,
            error: Error::Hash as u8,
            slot: 1,
            received: 0x12345,
            verified: 77,
            image_len: 0x100000,
        };
        assert_eq!(Status::from_bytes(&s.to_bytes()), Some(s));
        assert_eq!(s.error(), Some(Error::Hash));
        assert_eq!(Status::from_bytes(&[0; 15]), None);
        for c in 1..=24u8 {
            assert_eq!(Error::from_code(c).map(|e| e as u8), Some(c));
            assert!(!Error::from_code(c).unwrap().text().is_empty());
        }
        assert_eq!((Error::from_code(0), Error::from_code(25)), (None, None));
    }
}
