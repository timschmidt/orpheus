//! Shared constants and wire formats.
//!
//! One definition of every address, magic word and byte layout that more than
//! one of these sides must agree on:
//!
//! * the firmware (`fw/firmware`) and the bring-up images,
//! * the boot stages (`fw/boot`),
//! * the host tools (`xtask`, `orpheus-host`).
//!
//! Plain integers and byte-slice codecs only, so the crate builds for the
//! device and the host alike and needs no `unsafe`. Firmware writes these
//! layouts with volatile accesses at the offsets given here.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod boot;
pub mod devchan;
pub mod fault;
pub mod hash;
pub mod layout;
pub mod logring;
pub mod mailbox;
pub mod mem;
pub mod prof;
pub mod result;
pub mod slot;

/// Read a little-endian `u32` at `off`; `None` if out of range.
#[must_use]
pub fn rd32(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Write a little-endian `u32` at `off`; returns `false` if out of range.
pub fn wr32(b: &mut [u8], off: usize, v: u32) -> bool {
    match off.checked_add(4).and_then(|end| b.get_mut(off..end)) {
        Some(s) => {
            s.copy_from_slice(&v.to_le_bytes());
            true
        }
        None => false,
    }
}

/// Four ASCII bytes as the little-endian word the firmware stores
/// (`magic(b"YRES")` reads back as the bytes `Y R E S` in a memory dump).
#[must_use]
pub const fn magic(m: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rw32_bounds() {
        let mut b = [0u8; 8];
        assert!(wr32(&mut b, 4, 0x1122_3344));
        assert!(!wr32(&mut b, 5, 1));
        assert_eq!(rd32(&b, 4), Some(0x1122_3344));
        assert_eq!(rd32(&b, 5), None);
        assert_eq!(rd32(&b, usize::MAX - 1), None);
    }

    #[test]
    fn magic_is_memory_order() {
        assert_eq!(magic(b"ADFU"), 0x5546_4441);
        assert_eq!(magic(b"YTST"), 0x5453_5459);
    }
}
