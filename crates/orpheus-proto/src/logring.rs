//! Persistent log ring: 2 KiB. The firmware and the bring-up images keep it
//! at [`ALT_BASE`] in SRAM0; the SRAM3 placement [`BASE`] was never used and
//! now holds the profiler block in the firmware ([`crate::prof`]).
//!
//! The firmware's `defmt` global logger appends rzCOBS-encoded frames (each
//! terminated by a `0x00` byte) to `data`; the host reads the whole area,
//! reorders it from `write` and decodes it with the ELF's defmt table.
//!
//! | Offset | Content |
//! |---|---|
//! | `+0x00` | magic `"YLOG"` |
//! | `+0x04` | `write`: total bytes ever written (position = `write % CAP`) |
//! | `+0x08` | build id hash (first 4 bytes of the build id) |
//! | `+0x0C` | `CAP` (data capacity in bytes) |
//! | `+0x10` | data |
//!
//! Until SRAM3 is proven to survive every reset path, bring-up images may use
//! [`ALT_BASE`] in SRAM0 instead; the header says which capacity applies.

use crate::{magic, rd32};

/// The first plan's SRAM3 placement; only `xtask dev hil` still looks there
/// (for the magic). The firmware's profiler block occupies it.
pub const BASE: u32 = 0x0030_7000;
pub const SIZE: usize = 2048;
/// Fallback placement inside the persistent area's spare space is too small;
/// RAM-loaded bring-up images put the ring just below the persistent area.
pub const ALT_BASE: u32 = 0x0013_7400;
pub const MAGIC: u32 = magic(b"YLOG");
pub const HEADER: usize = 16;
pub const CAP: usize = SIZE - HEADER;

pub const OFF_MAGIC: usize = 0;
pub const OFF_WRITE: usize = 4;
pub const OFF_BUILD: usize = 8;
pub const OFF_CAP: usize = 12;

/// The line a start appends when it keeps the ring.
pub const BOOT_MARK: &[u8] = b"--- boot ---";

/// Whether a start keeps the ring it finds (header intact), appending
/// [`BOOT_MARK`], instead of clearing it. Regression (2026-11-25, soak
/// finding): the ring was cleared at every start, so the lines before a
/// watchdog reset — the only evidence of what hung — were lost.
#[must_use]
pub fn keep(magic: u32, cap: u32, write: u32) -> bool {
    magic == MAGIC && cap as usize == CAP && write != u32::MAX
}

/// The lines before the last [`BOOT_MARK`] (the previous start's tail),
/// at most `n`, oldest first; empty without a mark.
#[must_use]
pub fn before_last_boot<'a>(lines: &'a [&'a [u8]], n: usize) -> &'a [&'a [u8]] {
    let Some(at) = lines.iter().rposition(|l| *l == BOOT_MARK) else { return &[] };
    &lines[at.saturating_sub(n)..at]
}

/// Return the ring's bytes in write order (oldest first) and whether data
/// was lost to wrapping. `None` if the header is absent or inconsistent.
#[must_use]
pub fn unroll(area: &[u8], out: &mut [u8]) -> Option<(usize, bool)> {
    if rd32(area, OFF_MAGIC)? != MAGIC {
        return None;
    }
    let write = rd32(area, OFF_WRITE)? as usize;
    let cap = rd32(area, OFF_CAP)? as usize;
    let data = area.get(HEADER..HEADER.checked_add(cap)?)?;
    if cap == 0 {
        return None;
    }
    let len = write.min(cap);
    let out = out.get_mut(..len)?;
    let start = if write > cap { write % cap } else { 0 };
    for (i, o) in out.iter_mut().enumerate() {
        *o = data[(start + i) % cap];
    }
    Some((len, write > cap))
}

#[cfg(test)]
mod tests {
    /// Regression: the log ring was cleared at every start (2026-11-25).
    #[test]
    fn regress_log_ring_survives_a_reset() {
        assert!(keep(MAGIC, CAP as u32, 1234));
        assert!(!keep(0, CAP as u32, 0));
        assert!(!keep(MAGIC, 64, 0));
        let a: &[u8] = b"pic: slow";
        let lines: [&[u8]; 5] = [b"x", a, BOOT_MARK, b"fw: start", b"y"];
        assert_eq!(before_last_boot(&lines, 1), &[a]);
        assert_eq!(before_last_boot(&lines, 8).len(), 2);
        assert!(before_last_boot(&lines[..2], 8).is_empty());
    }

    use super::*;
    use crate::wr32;

    fn area(write: u32, fill: impl Fn(usize) -> u8) -> [u8; SIZE] {
        let mut a = [0u8; SIZE];
        wr32(&mut a, OFF_MAGIC, MAGIC);
        wr32(&mut a, OFF_WRITE, write);
        wr32(&mut a, OFF_CAP, CAP as u32);
        for i in 0..CAP {
            a[HEADER + i] = fill(i);
        }
        a
    }

    #[test]
    fn not_wrapped() {
        let a = area(5, |i| i as u8);
        let mut out = [0u8; CAP];
        assert_eq!(unroll(&a, &mut out), Some((5, false)));
        assert_eq!(&out[..5], &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn wrapped() {
        // Writer has written CAP + 3 bytes: oldest byte is at index 3.
        let a = area(CAP as u32 + 3, |i| (i % 251) as u8);
        let mut out = [0u8; CAP];
        let (n, lost) = unroll(&a, &mut out).unwrap();
        assert!(lost);
        assert_eq!(n, CAP);
        assert_eq!(out[0], 3);
        assert_eq!(out[CAP - 1], 2);
    }

    #[test]
    fn missing() {
        let mut out = [0u8; 4];
        assert!(unroll(&[0u8; SIZE], &mut out).is_none());
    }
}
