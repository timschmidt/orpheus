//! Which slot an update writes, what boot stage 2 will start, and the
//! recovery switch between the two full-layout slots.

use orpheus_proto::hash::sum32;
use orpheus_proto::layout::{SLOTS, full, full_writable};
use orpheus_proto::rd32;
use orpheus_proto::slot::{self, HEADER_SIZE, SlotHeader};

use crate::Error;
use crate::ofw::{IMAGE_MAX, LOAD_ADDR, RESIDENT_MAX, RESIDENT_OFF_MAX};

/// Value the superseded slot's magic word is programmed to (only bits are
/// cleared, no erase): anything but `"YFW1"` makes stage 2 skip the slot;
/// the rest of the header stays, so [`switch`] can restore it.
pub const SUPERSEDED: u32 = 0;

/// The slot an update writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    /// Index in [`SLOTS`] (0 full A, 1 full B).
    pub index: u8,
    pub base: u32,
    /// Bytes the installer may write from `base`.
    pub size: u32,
    /// The running slot, when it comes before the target in stage 2's order
    /// and so must be superseded for the new image to start.
    pub supersede: Option<u32>,
}

/// The inactive slot of the full layout. `booted` is stage 2's record of
/// the slot it started (boot-info low byte): A → write B and supersede A;
/// B or the old dual slot → write A (first in stage 2's order already).
pub fn choose(layout_full: bool, booted: Option<u8>) -> Result<Target, Error> {
    if !layout_full {
        return Err(Error::NotFull);
    }
    let a = Target { index: 0, base: full::SLOT_A, size: IMAGE_MAX, supersede: None };
    match booted {
        Some(0) => {
            Ok(Target { index: 1, base: full::SLOT_B, size: IMAGE_MAX, supersede: Some(full::SLOT_A) })
        }
        Some(1 | 2) => Ok(a),
        _ => Err(Error::NoSlot),
    }
}

impl Target {
    /// Whether `[addr, addr + len)` may be written for this update: inside
    /// the target slot, or exactly the superseded slot's magic word; and
    /// always inside the full layout's writable range (never sector 0,
    /// never `0x7E0000` and above).
    #[must_use]
    pub fn allows(&self, addr: u32, len: u32) -> bool {
        let in_slot = addr >= self.base
            && addr.checked_add(len).is_some_and(|e| e <= self.base.saturating_add(self.size));
        let magic_word = self.supersede == Some(addr) && len == 4;
        (in_slot || magic_word) && full_writable(addr, len) && is_slot_base_or_inside(self.base)
    }
}

/// A target must be one of the two full-layout slots.
fn is_slot_base_or_inside(base: u32) -> bool {
    base == full::SLOT_A || base == full::SLOT_B
}

/// Stage 2's check of one slot (`fw/boot/stage2`: magic, version, bounds,
/// resident sum), over a NOR read function. The firmware additionally
/// needs the header CRC; [`boots`] checks both.
pub fn stage2_accepts(base: u32, read: &mut dyn FnMut(u32, &mut [u8]) -> bool) -> bool {
    let mut h = [0u8; HEADER_SIZE];
    if !read(base, &mut h) {
        return false;
    }
    let w = |o| rd32(&h, o).unwrap_or(0);
    if w(0) != slot::MAGIC || w(4) != slot::VERSION {
        return false;
    }
    let (off, len, load, entry) = (w(0x10), w(0x14), w(0x18), w(0x1C));
    if load != LOAD_ADDR || len == 0 || len > RESIDENT_MAX || off >= RESIDENT_OFF_MAX || entry & 1 != 1 {
        return false;
    }
    let mut sum = 0u32;
    let mut buf = [0u8; 256];
    let mut at = 0;
    while at < len {
        let n = (len - at).min(256);
        if !read(base + off + at, &mut buf[..n as usize]) {
            return false;
        }
        sum = sum.wrapping_add(sum32(&buf[..n as usize]));
        at += n;
    }
    sum == w(0x20)
}

/// The slot stage 2 starts (index in [`SLOTS`]), `None` = USB recovery.
pub fn stage2_pick(read: &mut dyn FnMut(u32, &mut [u8]) -> bool) -> Option<usize> {
    SLOTS.iter().position(|&b| stage2_accepts(b, read))
}

/// Whether stage 2 starts slot `index` *and* the firmware accepts its
/// header (CRC) — i.e. the player boots it with paging on.
pub fn boots(index: usize, read: &mut dyn FnMut(u32, &mut [u8]) -> bool) -> bool {
    let Some(&base) = SLOTS.get(index) else { return false };
    let mut h = [0u8; HEADER_SIZE];
    stage2_pick(read) == Some(index) && read(base, &mut h) && SlotHeader::from_bytes(&h).is_ok()
}

/// What a slot's header says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotState {
    /// A valid `"YFW1"` header (build id).
    Valid([u8; 16]),
    /// Superseded by an update: magic cleared, the rest intact (build id).
    Superseded([u8; 16]),
    /// Erased or anything else.
    Empty,
}

/// Classify a slot from its first 128 bytes.
#[must_use]
pub fn state_of(hdr: &[u8]) -> SlotState {
    if let Ok(h) = SlotHeader::from_bytes(hdr) {
        return SlotState::Valid(h.build_id);
    }
    match restored(hdr) {
        Some(b) => SlotState::Superseded(SlotHeader::from_bytes(&b).map(|h| h.build_id).unwrap_or([0; 16])),
        None => SlotState::Empty,
    }
}

/// A superseded header with its magic word put back, if that makes a
/// header with a valid CRC (so it really is the old header).
#[must_use]
pub fn restored(hdr: &[u8]) -> Option<[u8; HEADER_SIZE]> {
    let mut b: [u8; HEADER_SIZE] = hdr.get(..HEADER_SIZE)?.try_into().ok()?;
    if rd32(&b, 0)? != SUPERSEDED {
        return None;
    }
    b[..4].copy_from_slice(&slot::MAGIC.to_le_bytes());
    SlotHeader::from_bytes(&b).is_ok().then_some(b)
}

/// One write of the recovery switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchStep {
    /// Erase the header sector at `slot` and program `header` (128 bytes)
    /// into it: a superseded slot becomes valid again.
    Restore { slot: u32, header: [u8; HEADER_SIZE] },
    /// Program the magic word at `slot` to [`SUPERSEDED`].
    Supersede { slot: u32 },
}

impl SwitchStep {
    /// The sector this step writes; always a slot's header sector inside the
    /// full layout's writable range.
    #[must_use]
    pub const fn slot(&self) -> u32 {
        match self {
            SwitchStep::Restore { slot, .. } | SwitchStep::Supersede { slot } => *slot,
        }
    }
}

/// Recovery (WEB-3, `xtask dev switch-slot`): make stage 2 start the
/// *other* full-layout slot than it starts now, from the two header
/// sectors' first 128 bytes. At most two steps, done in order; after each
/// one the player still boots one complete image. `None` when there is no
/// other image to switch to.
#[must_use]
pub fn switch(a: &[u8], b: &[u8]) -> Option<[Option<SwitchStep>; 2]> {
    let (sa, sb) = (state_of(a), state_of(b));
    match (sa, sb) {
        // A starts now; B holds an image: supersede A.
        (SlotState::Valid(_), SlotState::Valid(_)) => {
            Some([Some(SwitchStep::Supersede { slot: full::SLOT_A }), None])
        }
        // A starts; B was superseded: restore B (A still first), then
        // supersede A.
        (SlotState::Valid(_), SlotState::Superseded(_)) => Some([
            Some(SwitchStep::Restore { slot: full::SLOT_B, header: restored(b)? }),
            Some(SwitchStep::Supersede { slot: full::SLOT_A }),
        ]),
        // B starts (or nothing); A was superseded: restore A, first again.
        (SlotState::Superseded(_), _) => {
            Some([Some(SwitchStep::Restore { slot: full::SLOT_A, header: restored(a)? }), None])
        }
        // Nothing starts but B was superseded: restore it.
        (SlotState::Empty, SlotState::Superseded(_)) => {
            Some([Some(SwitchStep::Restore { slot: full::SLOT_B, header: restored(b)? }), None])
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_writes_the_other_slot() {
        assert_eq!(choose(false, Some(0)), Err(Error::NotFull));
        assert_eq!(choose(true, None), Err(Error::NoSlot));
        assert_eq!(choose(true, Some(3)), Err(Error::NoSlot));
        let t = choose(true, Some(0)).unwrap();
        assert_eq!((t.base, t.supersede), (full::SLOT_B, Some(full::SLOT_A)));
        let t = choose(true, Some(1)).unwrap();
        assert_eq!((t.base, t.supersede), (full::SLOT_A, None));
        let t = choose(true, Some(2)).unwrap();
        assert_eq!((t.base, t.supersede), (full::SLOT_A, None));
    }

    #[test]
    fn target_guard_never_allows_sector0_or_vendor_data() {
        for booted in [0, 1, 2] {
            let t = choose(true, Some(booted)).unwrap();
            assert!(!t.allows(0, 4));
            assert!(!t.allows(0, 0x1000));
            assert!(!t.allows(0xFFC, 8));
            assert!(!t.allows(0x7E0000, 4));
            assert!(!t.allows(0x7DFFFC, 8));
            assert!(!t.allows(u32::MAX - 3, 4));
            assert!(!t.allows(t.base + t.size, 1));
            assert!(!t.allows(t.base - 4, 8));
            assert!(t.allows(t.base, 0x1000));
            assert!(t.allows(t.base + t.size - 256, 256));
            // The settings store and stage 2 are not part of any slot.
            assert!(!t.allows(0x7DA000, 4));
            assert!(!t.allows(0x7DF400, 4));
        }
        let t = choose(true, Some(0)).unwrap();
        assert!(t.allows(full::SLOT_A, 4));
        assert!(!t.allows(full::SLOT_A, 8));
        assert!(!t.allows(full::SLOT_A + 4, 4));
        // A forged target outside the slots allows nothing.
        let bad = Target { index: 0, base: 0, size: 0x2000, supersede: Some(0) };
        assert!(!bad.allows(0, 4) && !bad.allows(0x1000, 4));
        let bad = Target { index: 0, base: 0x7E0000, size: 0x10000, supersede: None };
        assert!(!bad.allows(0x7E0000, 4));
    }
}
